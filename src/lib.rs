//! # cascii - ASCII Art Generator Library
//!
//! `cascii` is a high-performance library for converting images and videos into ASCII art.
//!
//! ## Features
//!
//! - Convert single images to ASCII art
//! - Extract and convert video frames to ASCII
//! - Configurable character sets and quality presets
//! - Parallel processing for high performance
//!
//! ## Example
//!
//! ```no_run
//! use cascii::{AsciiConverter, ConversionOptions};
//! use std::path::Path;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Convert a single image
//! let converter = AsciiConverter::new();
//! let options = ConversionOptions::default().with_columns(400);
//! converter.convert_image(
//!     Path::new("input.png"),
//!     Path::new("output.txt"),
//!     &options
//! )?;
//! # Ok(())
//! # }
//! ```

use anyhow::{anyhow, Context, Result};
use image::DynamicImage;
use rayon::prelude::*;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;
use walkdir::WalkDir;

/// Configuration preset defining quality settings
#[derive(Debug, Deserialize, Clone)]
pub struct Preset {
    pub columns: u32,
    pub fps: u32,
    pub font_ratio: f32,
    pub luminance: u8,
}

fn default_ascii_chars() -> String {
    " .'`^,:;Il!i><~+_-?][}{1)(|/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$".to_string()
}

fn default_start_str() -> String {
    "0".to_string()
}
fn default_end_str() -> String {
    String::new()
}

/// Application configuration with presets and ASCII character set
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub presets: std::collections::HashMap<String, Preset>,
    pub default_preset: String,
    #[serde(default = "default_ascii_chars")]
    pub ascii_chars: String,
    #[serde(default = "default_start_str")]
    pub default_start: String,
    #[serde(default = "default_end_str")]
    pub default_end: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        let default_json = r#"{
            "presets": {
                "default": {"columns": 400, "fps": 30, "font_ratio": 0.7, "luminance": 20},
                "small":   {"columns": 80,  "fps": 24, "font_ratio": 0.44, "luminance": 20},
                "large":   {"columns": 800, "fps": 60, "font_ratio": 0.7, "luminance": 20}
            },
            "default_preset": "default",
            "ascii_chars": " .'`^,:;Il!i><~+_-?][}{1)(|/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$",
            "default_start": "0",
            "default_end": ""
        }"#;
        serde_json::from_str(default_json).unwrap()
    }
}

/// Options for ASCII conversion
#[derive(Debug, Clone)]
pub struct ConversionOptions {
    /// Target width in characters (columns)
    pub columns: Option<u32>,
    /// Font aspect ratio (width/height of character)
    pub font_ratio: f32,
    /// Luminance threshold (0-255) for transparency
    pub luminance: u8,
    /// ASCII character set to use (from darkest to lightest)
    pub ascii_chars: String,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            columns: Some(400),
            font_ratio: 0.7,
            luminance: 20,
            ascii_chars: default_ascii_chars(),
        }
    }
}

impl ConversionOptions {
    /// Create options with a specific width
    pub fn with_columns(mut self, columns: u32) -> Self {
        self.columns = Some(columns);
        self
    }

    /// Create options with a specific font ratio
    pub fn with_font_ratio(mut self, font_ratio: f32) -> Self {
        self.font_ratio = font_ratio;
        self
    }

    /// Create options with a specific luminance threshold
    pub fn with_luminance(mut self, luminance: u8) -> Self {
        self.luminance = luminance;
        self
    }

    /// Create options with custom ASCII character set
    pub fn with_ascii_chars(mut self, ascii_chars: String) -> Self {
        self.ascii_chars = ascii_chars;
        self
    }

    /// Create options from a preset
    pub fn from_preset(preset: &Preset, ascii_chars: String) -> Self {
        Self {
            columns: Some(preset.columns),
            font_ratio: preset.font_ratio,
            luminance: preset.luminance,
            ascii_chars,
        }
    }
}

/// Options for video conversion
#[derive(Debug, Clone)]
pub struct VideoOptions {
    /// Frames per second to extract
    pub fps: u32,
    /// Start time (e.g., "00:01:23.456" or "83.456")
    pub start: Option<String>,
    /// End time (e.g., "00:01:23.456" or "83.456")
    pub end: Option<String>,
    /// Target width in characters
    pub columns: u32,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            fps: 30,
            start: None,
            end: None,
            columns: 400,
        }
    }
}

/// Main converter struct for ASCII art generation
pub struct AsciiConverter {
    config: AppConfig,
}

impl AsciiConverter {
    /// Create a new converter with default configuration
    pub fn new() -> Self {
        Self {
            config: AppConfig::default(),
        }
    }

    /// Create a converter with custom configuration
    pub fn with_config(config: AppConfig) -> Result<Self> {
        // Validate ASCII characters
        if !config.ascii_chars.is_ascii() {
            return Err(anyhow!("Config contains non-ASCII characters in ascii_chars field. This will cause corrupted output. Please use only ASCII characters."));
        }
        Ok(Self { config })
    }

    /// Load configuration from a file
    pub fn from_config_file(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let config: AppConfig = serde_json::from_str(&text).context("parsing config json")?;

        if !config.ascii_chars.is_ascii() {
            return Err(anyhow!(
                "Config file {} contains non-ASCII characters in ascii_chars field. \
                This will cause corrupted output. Please use only ASCII characters.",
                path.display()
            ));
        }

        Ok(Self { config })
    }

    /// Get the current configuration
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// Convert a single image to ASCII art
    ///
    /// # Arguments
    ///
    /// * `input` - Path to input image (PNG, JPG)
    /// * `output` - Path to output text file
    /// * `options` - Conversion options
    ///
    /// # Example
    ///
    /// ```no_run
    /// use cascii::{AsciiConverter, ConversionOptions};
    /// use std::path::Path;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let converter = AsciiConverter::new();
    /// let options = ConversionOptions::default().with_columns(200);
    /// converter.convert_image(
    ///     Path::new("image.png"),
    ///     Path::new("output.txt"),
    ///     &options
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn convert_image(&self, input: &Path, output: &Path, options: &ConversionOptions) -> Result<()> {
        let ascii_chars = options.ascii_chars.as_bytes();
        convert_image_to_ascii(input, output,options.font_ratio, options.luminance, options.columns, ascii_chars)
    }

    /// Convert image to ASCII string (without writing to file)
    ///
    /// # Example
    ///
    /// ```no_run
    /// use cascii::{AsciiConverter, ConversionOptions};
    /// use std::path::Path;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let converter = AsciiConverter::new();
    /// let options = ConversionOptions::default();
    /// let ascii_art = converter.image_to_string(Path::new("image.png"), &options)?;
    /// println!("{}", ascii_art);
    /// # Ok(())
    /// # }
    /// ```
    pub fn image_to_string(&self, input: &Path, options: &ConversionOptions) -> Result<String> {
        let ascii_chars = options.ascii_chars.as_bytes();
        image_to_ascii_string(input, options.font_ratio, options.luminance, options.columns, ascii_chars)
    }

    /// Extract frames from video and convert to ASCII
    ///
    /// # Arguments
    ///
    /// * `input` - Path to input video file
    /// * `output_dir` - Directory to write ASCII frames
    /// * `video_opts` - Video extraction options
    /// * `conv_opts` - ASCII conversion options
    /// * `keep_images` - Whether to keep intermediate PNG frames
    ///
    /// # Example
    ///
    /// ```no_run
    /// use cascii::{AsciiConverter, VideoOptions, ConversionOptions};
    /// use std::path::Path;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let converter = AsciiConverter::new();
    /// let video_opts = VideoOptions::default();
    /// let conv_opts = ConversionOptions::default();
    /// converter.convert_video(
    ///     Path::new("video.mp4"),
    ///     Path::new("output_dir"),
    ///     &video_opts,
    ///     &conv_opts,
    ///     false
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn convert_video(&self, input: &Path, output_dir: &Path, video_opts: &VideoOptions, conv_opts: &ConversionOptions, keep_images: bool) -> Result<()> {
        self.convert_video_with_progress(input, output_dir, video_opts, conv_opts, keep_images, None::<fn(usize, usize)>)
    }

    /// Convert a video to ASCII animation frames with progress callback
    ///
    /// # Arguments
    ///
    /// * `input` - Input video file path
    /// * `output_dir` - Directory to write ASCII frames
    /// * `video_opts` - Video extraction options (fps, start, end, columns)
    /// * `conv_opts` - ASCII conversion options
    /// * `keep_images` - Whether to keep extracted PNG frames
    /// * `progress_callback` - Optional callback called with (completed, total) for each frame
    ///
    /// # Example
    ///
    /// ```no_run
    /// use cascii::{AsciiConverter, ConversionOptions, VideoOptions};
    /// use std::path::Path;
    ///
    /// let converter = AsciiConverter::new();
    /// let video_opts = VideoOptions { fps: 24, start: None, end: None, columns: 120 };
    /// let conv_opts = ConversionOptions::default();
    ///
    /// converter.convert_video_with_progress(
    ///     Path::new("video.mp4"),
    ///     Path::new("output"),
    ///     &video_opts,
    ///     &conv_opts,
    ///     false,
    ///     Some(|completed, total| {
    ///         println!("Progress: {}/{} ({:.1}%)", completed, total, (completed as f64 / total as f64) * 100.0);
    ///     }),
    /// ).unwrap();
    /// ```
    pub fn convert_video_with_progress<F>(&self, input: &Path, output_dir: &Path, video_opts: &VideoOptions, conv_opts: &ConversionOptions, keep_images: bool, progress_callback: Option<F>) -> Result<()> where F: Fn(usize, usize) + Send + Sync {
        fs::create_dir_all(output_dir).context("creating output directory")?;

        // Extract frames with ffmpeg
        extract_video_frames(input, output_dir, video_opts.columns, video_opts.fps, video_opts.start.as_deref(), video_opts.end.as_deref())?;

        // Convert frames to ASCII with progress callback
        let ascii_chars = conv_opts.ascii_chars.as_bytes();
        convert_directory_parallel_with_progress(output_dir, output_dir, conv_opts.font_ratio, conv_opts.luminance, keep_images, ascii_chars, progress_callback)?;

        Ok(())
    }

    /// Convert a directory of images to ASCII frames
    ///
    /// # Arguments
    ///
    /// * `input_dir` - Directory containing PNG images
    /// * `output_dir` - Directory to write ASCII files
    /// * `options` - Conversion options
    /// * `keep_images` - Whether to keep original images
    pub fn convert_directory(&self, input_dir: &Path, output_dir: &Path, options: &ConversionOptions, keep_images: bool) -> Result<()> {
        fs::create_dir_all(output_dir)?;
        let ascii_chars = options.ascii_chars.as_bytes();
        convert_directory_parallel(input_dir, output_dir, options.font_ratio, options.luminance, keep_images, ascii_chars)
    }

    /// Get a preset by name
    pub fn get_preset(&self, name: &str) -> Option<&Preset> {
        self.config.presets.get(name)
    }

    /// Get conversion options from a preset name
    pub fn options_from_preset(&self, preset_name: &str) -> Result<ConversionOptions> {
        let preset = self
            .get_preset(preset_name)
            .ok_or_else(|| anyhow!("Preset '{}' not found", preset_name))?;
        Ok(ConversionOptions::from_preset(
            preset,
            self.config.ascii_chars.clone(),
        ))
    }
}

impl Default for AsciiConverter {
    fn default() -> Self {
        Self::new()
    }
}

// Internal implementation functions
fn convert_image_to_ascii(img_path: &Path, out_txt: &Path, font_ratio: f32, threshold: u8, columns: Option<u32>, ascii_chars: &[u8]) -> Result<()> {
    let ascii_string =
        image_to_ascii_string(img_path, font_ratio, threshold, columns, ascii_chars)?;
    fs::write(out_txt, ascii_string).with_context(|| format!("writing {}", out_txt.display()))?;
    Ok(())
}

fn image_to_ascii_string(img_path: &Path, font_ratio: f32, threshold: u8, columns: Option<u32>, ascii_chars: &[u8]) -> Result<String> {
    let mut img = image::open(img_path)
        .with_context(|| format!("opening {}", img_path.display()))?
        .to_rgb8();

    let (orig_w, orig_h) = img.dimensions();
    let (target_w, target_h) = if let Some(cols) = columns {
        let w = cols;
        let h = (orig_h as f32 / orig_w as f32 * cols as f32 * font_ratio).round() as u32;
        (w, h.max(1))
    } else {
        let w = orig_w;
        let h = (orig_h as f32 * font_ratio).round() as u32;
        (w, h.max(1))
    };

    if target_w != orig_w || target_h != orig_h {
        let dyn_img = DynamicImage::ImageRgb8(img);
        img = dyn_img
            .resize_exact(target_w, target_h, image::imageops::FilterType::Lanczos3)
            .to_rgb8();
    }

    let (w, h) = img.dimensions();
    let mut out = String::with_capacity((w as usize + 1) * (h as usize));
    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x, y);
            let l = luminance(*px);
            out.push(char_for(l, threshold, ascii_chars));
        }
        out.push('\n');
    }
    Ok(out)
}

fn luminance(rgb: image::Rgb<u8>) -> u8 {
    let r = rgb[0] as f64;
    let g = rgb[1] as f64;
    let b = rgb[2] as f64;
    (0.2126 * r + 0.7152 * g + 0.0722 * b) as u8
}

fn char_for(luma: u8, threshold: u8, ascii_chars: &[u8]) -> char {
    if luma < threshold {
        return ' ';
    }

    let effective_luma = (luma as u32).saturating_sub(threshold as u32);
    let range = (255u32).saturating_sub(threshold as u32).max(1);
    let num_chars_minus_1 = (ascii_chars.len() as u32).saturating_sub(1);

    let idx = (effective_luma * num_chars_minus_1) / range;
    let idx = idx.min(num_chars_minus_1) as usize;

    ascii_chars[idx] as char
}

fn extract_video_frames(input: &Path, out_dir: &Path, columns: u32, fps: u32, start: Option<&str>, end: Option<&str>) -> Result<()> {
    let out_pattern = out_dir.join("frame_%04d.png");
    let mut ffmpeg_args: Vec<String> = vec!["-loglevel".into(), "error".into()];

    if let Some(s) = start {
        if !s.is_empty() && s != "0" {
            ffmpeg_args.push("-ss".into());
            ffmpeg_args.push(s.to_string());
        }
    }

    ffmpeg_args.push("-i".into());
    ffmpeg_args.push(input.to_str().unwrap().to_string());

    if let Some(e) = end {
        if !e.is_empty() {
            if let Some(s) = start {
                if !s.is_empty() && s != "0" {
                    let start_secs = parse_timestamp(s);
                    let end_secs = parse_timestamp(e);
                    let duration = end_secs - start_secs;
                    if duration > 0.0 {
                        ffmpeg_args.push("-t".into());
                        ffmpeg_args.push(duration.to_string());
                    }
                } else {
                    ffmpeg_args.push("-t".into());
                    ffmpeg_args.push(e.to_string());
                }
            } else {
                ffmpeg_args.push("-t".into());
                ffmpeg_args.push(e.to_string());
            }
        }
    }

    let vf_option = format!("scale={}:-2,fps={}", columns, fps);
    ffmpeg_args.push("-vf".into());
    ffmpeg_args.push(vf_option);
    ffmpeg_args.push(out_pattern.to_str().unwrap().to_string());

    let status = ProcCommand::new("ffmpeg")
        .args(&ffmpeg_args)
        .status()
        .context("running ffmpeg")?;

    if !status.success() {
        return Err(anyhow!("ffmpeg failed"));
    }
    Ok(())
}

fn parse_timestamp(s: &str) -> f64 {
    s.split(':').rev().enumerate().fold(0.0, |acc, (i, v)| {
        acc + v.parse::<f64>().unwrap_or(0.0) * 60f64.powi(i as i32)
    })
}

fn convert_directory_parallel(src_dir: &Path, dst_dir: &Path, font_ratio: f32, threshold: u8, keep_images: bool, ascii_chars: &[u8]) -> Result<()> {
    convert_directory_parallel_with_progress(src_dir, dst_dir, font_ratio, threshold, keep_images, ascii_chars, None::<fn(usize, usize)>)
}

fn convert_directory_parallel_with_progress<F>(src_dir: &Path, dst_dir: &Path, font_ratio: f32, threshold: u8, keep_images: bool, ascii_chars: &[u8], progress_callback: Option<F>,) -> Result<()> where F: Fn(usize, usize) + Send + Sync {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fs::create_dir_all(dst_dir)?;
    let mut pngs: Vec<PathBuf> = WalkDir::new(src_dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|p| p.extension().map(|e| e == "png").unwrap_or(false))
        .collect();
    pngs.sort();

    let total = pngs.len();
    let completed = Arc::new(AtomicUsize::new(0));

    pngs.par_iter().try_for_each(|img_path| -> Result<()> {
        let file_stem = img_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("bad file name"))?;
        let out_txt = dst_dir.join(format!("{}.txt", file_stem));
        convert_image_to_ascii(img_path, &out_txt, font_ratio, threshold, None, ascii_chars)?;

        // Update progress
        let current = completed.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(ref callback) = progress_callback {
            callback(current, total);
        }

        Ok(())
    })?;

    if !keep_images {
        for img_path in &pngs {
            fs::remove_file(img_path)?;
        }
    }

    Ok(())
}
