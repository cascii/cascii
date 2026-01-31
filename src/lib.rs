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
//! - Progress reporting for integration with UI applications
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
//!
//! ## Progress Reporting
//!
//! For video conversions, you can receive detailed progress updates:
//!
//! ```no_run
//! use cascii::{AsciiConverter, ConversionOptions, VideoOptions, Progress, ProgressPhase};
//! use std::path::Path;
//!
//! let converter = AsciiConverter::new();
//! let video_opts = VideoOptions::default();
//! let conv_opts = ConversionOptions::default();
//!
//! converter.convert_video_with_detailed_progress(
//!     Path::new("video.mp4"),
//!     Path::new("output"),
//!     &video_opts,
//!     &conv_opts,
//!     false,
//!     |progress| {
//!         match progress.phase {
//!             ProgressPhase::ExtractingFrames => println!("Extracting frames..."),
//!             ProgressPhase::ExtractingAudio => println!("Extracting audio..."),
//!             ProgressPhase::ConvertingFrames => {
//!                 println!("Converting: {}/{} ({:.1}%)",
//!                     progress.completed, progress.total, progress.percentage);
//!             }
//!             ProgressPhase::Complete => println!("Done!"),
//!         }
//!     },
//! ).unwrap();
//! ```

use anyhow::{anyhow, Context, Result};
use image::DynamicImage;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Stdio};
use walkdir::WalkDir;

/// Configuration for ffmpeg/ffprobe binary paths
///
/// Use this to specify custom paths for ffmpeg and ffprobe binaries,
/// for example when bundling them with your application.
#[derive(Debug, Clone, Default)]
pub struct FfmpegConfig {
    /// Custom path to ffmpeg binary. If None, uses system PATH.
    pub ffmpeg_path: Option<PathBuf>,
    /// Custom path to ffprobe binary. If None, uses system PATH.
    pub ffprobe_path: Option<PathBuf>,
}

impl FfmpegConfig {
    /// Create a new FfmpegConfig with default settings (use system PATH)
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a config with custom ffmpeg path
    pub fn with_ffmpeg<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.ffmpeg_path = Some(path.into());
        self
    }

    /// Create a config with custom ffprobe path
    pub fn with_ffprobe<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.ffprobe_path = Some(path.into());
        self
    }

    /// Get the ffmpeg command name or path
    fn ffmpeg_cmd(&self) -> &OsStr {
        self.ffmpeg_path
            .as_ref()
            .map(|p| p.as_os_str())
            .unwrap_or_else(|| OsStr::new("ffmpeg"))
    }

    /// Get the ffprobe command name or path
    fn ffprobe_cmd(&self) -> &OsStr {
        self.ffprobe_path
            .as_ref()
            .map(|p| p.as_os_str())
            .unwrap_or_else(|| OsStr::new("ffprobe"))
    }
}

/// Represents the current phase of a conversion operation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProgressPhase {
    /// Extracting frames from video using ffmpeg
    ExtractingFrames,
    /// Extracting audio from video
    ExtractingAudio,
    /// Converting extracted frames to ASCII art
    ConvertingFrames,
    /// Conversion completed successfully
    Complete,
}

/// Progress information for conversion operations
///
/// This struct provides detailed progress information that can be used
/// to display progress in UI applications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    /// Current phase of the conversion
    pub phase: ProgressPhase,
    /// Number of items completed in the current phase
    pub completed: usize,
    /// Total number of items in the current phase (0 if unknown/indeterminate)
    pub total: usize,
    /// Percentage complete (0.0 to 100.0)
    pub percentage: f64,
    /// Human-readable message describing current status
    pub message: String,
}

impl Progress {
    /// Create a new progress update for extracting frames
    pub fn extracting_frames() -> Self {
        Self {
            phase: ProgressPhase::ExtractingFrames,
            completed: 0,
            total: 0,
            percentage: 0.0,
            message: "Extracting frames from video...".to_string(),
        }
    }

    /// Create a progress update for extracting frames with percentage
    pub fn extracting_frames_progress(current_time_us: u64, total_duration_us: u64) -> Self {
        let percentage = if total_duration_us > 0 {
            (current_time_us as f64 / total_duration_us as f64) * 100.0
        } else {
            0.0
        };
        Self {
            phase: ProgressPhase::ExtractingFrames,
            completed: current_time_us as usize,
            total: total_duration_us as usize,
            percentage,
            message: format!("Extracting frames: {:.1}%", percentage),
        }
    }

    /// Create a new progress update for extracting audio
    pub fn extracting_audio() -> Self {
        Self {
            phase: ProgressPhase::ExtractingAudio,
            completed: 0,
            total: 0,
            percentage: 0.0,
            message: "Extracting audio from video...".to_string(),
        }
    }

    /// Create a new progress update for frame conversion
    pub fn converting_frames(completed: usize, total: usize) -> Self {
        let percentage = if total > 0 {
            (completed as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        Self {
            phase: ProgressPhase::ConvertingFrames,
            completed,
            total,
            percentage,
            message: format!("Converting frame {} of {}", completed, total),
        }
    }

    /// Create a completion progress update
    pub fn complete(total_frames: usize) -> Self {
        Self {
            phase: ProgressPhase::Complete,
            completed: total_frames,
            total: total_frames,
            percentage: 100.0,
            message: format!("Conversion complete: {} frames", total_frames),
        }
    }
}

/// Result of a conversion operation, containing metadata about the conversion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionResult {
    /// Number of frames generated
    pub frame_count: usize,
    /// Target columns (width) used
    pub columns: u32,
    /// Font ratio used
    pub font_ratio: f32,
    /// Luminance threshold used
    pub luminance: u8,
    /// FPS used (for video conversions)
    pub fps: Option<u32>,
    /// Output mode used
    pub output_mode: String,
    /// Whether audio was extracted
    pub audio_extracted: bool,
    /// Path to the output directory
    pub output_dir: PathBuf,
}

impl ConversionResult {
    /// Write the conversion details to a details.md file in the output directory
    pub fn write_details_file(&self) -> Result<PathBuf> {
        let details_path = self.output_dir.join("details.md");

        let mut details = format!("Version: {}\nFrames: {}\nLuminance: {}\nFont Ratio: {}\nColumns: {}", env!("CARGO_PKG_VERSION"), self.frame_count, self.luminance, self.font_ratio, self.columns);

        if let Some(fps) = self.fps {
            details.push_str(&format!("\nFPS: {}", fps));
        }

        details.push_str(&format!("\nOutput: {}", self.output_mode));
        details.push_str(&format!("\nAudio: {}", self.audio_extracted));

        fs::write(&details_path, &details)
            .with_context(|| format!("writing details file to {}", details_path.display()))?;

        Ok(details_path)
    }

    /// Get the details as a string (without writing to file)
    pub fn to_details_string(&self) -> String {
        let mut details = format!("Version: {}\nFrames: {}\nLuminance: {}\nFont Ratio: {}\nColumns: {}", env!("CARGO_PKG_VERSION"), self.frame_count, self.luminance, self.font_ratio, self.columns);

        if let Some(fps) = self.fps {
            details.push_str(&format!("\nFPS: {}", fps));
        }

        details.push_str(&format!("\nOutput: {}", self.output_mode));
        details.push_str(&format!("\nAudio: {}", self.audio_extracted));

        details
    }
}

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

/// Controls what output files are generated
#[derive(Debug, Clone, PartialEq)]
pub enum OutputMode {
    /// Only generate .txt files (plain ASCII)
    TextOnly,
    /// Only generate .cframe files (combined text + color binary)
    ColorOnly,
    /// Generate both .txt and .cframe files
    TextAndColor,
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
    /// What output files to generate
    pub output_mode: OutputMode,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            columns: Some(400),
            font_ratio: 0.7,
            luminance: 20,
            ascii_chars: default_ascii_chars(),
            output_mode: OutputMode::TextOnly,
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

    /// Set the output mode
    pub fn with_output_mode(mut self, mode: OutputMode) -> Self {
        self.output_mode = mode;
        self
    }

    /// Create options from a preset
    pub fn from_preset(preset: &Preset, ascii_chars: String) -> Self {
        Self {
            columns: Some(preset.columns),
            font_ratio: preset.font_ratio,
            luminance: preset.luminance,
            ascii_chars,
            output_mode: OutputMode::TextOnly,
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
    /// Whether to extract audio from the video
    pub extract_audio: bool,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            fps: 30,
            start: None,
            end: None,
            columns: 400,
            extract_audio: false,
        }
    }
}

/// Main converter struct for ASCII art generation
pub struct AsciiConverter {
    config: AppConfig,
    ffmpeg_config: FfmpegConfig,
}

impl AsciiConverter {
    /// Create a new converter with default configuration
    pub fn new() -> Self {
        Self {
            config: AppConfig::default(),
            ffmpeg_config: FfmpegConfig::default(),
        }
    }

    /// Create a converter with custom configuration
    pub fn with_config(config: AppConfig) -> Result<Self> {
        // Validate ASCII characters
        if !config.ascii_chars.is_ascii() {
            return Err(anyhow!("Config contains non-ASCII characters in ascii_chars field. This will cause corrupted output. Please use only ASCII characters."));
        }
        Ok(Self { config, ffmpeg_config: FfmpegConfig::default() })
    }

    /// Set custom ffmpeg/ffprobe paths for this converter
    ///
    /// Use this when bundling ffmpeg with your application:
    /// ```no_run
    /// use cascii::{AsciiConverter, FfmpegConfig};
    ///
    /// let converter = AsciiConverter::new()
    ///     .with_ffmpeg_config(FfmpegConfig::new()
    ///         .with_ffmpeg("/path/to/ffmpeg")
    ///         .with_ffprobe("/path/to/ffprobe"));
    /// ```
    pub fn with_ffmpeg_config(mut self, ffmpeg_config: FfmpegConfig) -> Self {
        self.ffmpeg_config = ffmpeg_config;
        self
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

        Ok(Self { config, ffmpeg_config: FfmpegConfig::default() })
    }

    /// Get the current configuration
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// Get the current ffmpeg configuration
    pub fn ffmpeg_config(&self) -> &FfmpegConfig {
        &self.ffmpeg_config
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
        convert_image_to_ascii(input, output, options.font_ratio, options.luminance, options.columns, ascii_chars, &options.output_mode)
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
    pub fn convert_video(&self, input: &Path, output_dir: &Path, video_opts: &VideoOptions, conv_opts: &ConversionOptions, keep_images: bool) -> Result<ConversionResult> {
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
    /// let video_opts = VideoOptions { fps: 24, start: None, end: None, columns: 120, extract_audio: false };
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
    pub fn convert_video_with_progress<F>(&self, input: &Path, output_dir: &Path, video_opts: &VideoOptions, conv_opts: &ConversionOptions, keep_images: bool, progress_callback: Option<F>) -> Result<ConversionResult> where F: Fn(usize, usize) + Send + Sync {
        fs::create_dir_all(output_dir).context("creating output directory")?;

        // Extract frames with ffmpeg
        extract_video_frames(input, output_dir, video_opts.columns, video_opts.fps, video_opts.start.as_deref(), video_opts.end.as_deref(), &self.ffmpeg_config)?;

        // Extract audio if requested
        if video_opts.extract_audio {
            extract_audio(input, output_dir, video_opts.start.as_deref(), video_opts.end.as_deref(), &self.ffmpeg_config)?;
        }

        // Convert frames to ASCII with progress callback
        let ascii_chars = conv_opts.ascii_chars.as_bytes();
        let total_frames = convert_directory_parallel_with_progress(output_dir, output_dir, conv_opts.font_ratio, conv_opts.luminance, keep_images, ascii_chars, &conv_opts.output_mode, progress_callback)?;

        // Build result with conversion details
        let output_mode_str = match conv_opts.output_mode {
            OutputMode::TextOnly => "text-only",
            OutputMode::ColorOnly => "color-only",
            OutputMode::TextAndColor => "text+color",
        };

        let result = ConversionResult {
            frame_count: total_frames,
            columns: conv_opts.columns.unwrap_or(video_opts.columns),
            font_ratio: conv_opts.font_ratio,
            luminance: conv_opts.luminance,
            fps: Some(video_opts.fps),
            output_mode: output_mode_str.to_string(),
            audio_extracted: video_opts.extract_audio,
            output_dir: output_dir.to_path_buf(),
        };

        // Write the details.md file
        result.write_details_file()?;

        Ok(result)
    }

    /// Convert a video to ASCII animation frames with detailed progress reporting
    ///
    /// This method provides comprehensive progress updates through different phases
    /// of the conversion process, making it ideal for UI integration.
    ///
    /// # Arguments
    ///
    /// * `input` - Input video file path
    /// * `output_dir` - Directory to write ASCII frames
    /// * `video_opts` - Video extraction options (fps, start, end, columns)
    /// * `conv_opts` - ASCII conversion options
    /// * `keep_images` - Whether to keep extracted PNG frames
    /// * `progress_callback` - Callback called with detailed Progress information
    ///
    /// # Example
    ///
    /// ```no_run
    /// use cascii::{AsciiConverter, ConversionOptions, VideoOptions, Progress, ProgressPhase};
    /// use std::path::Path;
    ///
    /// let converter = AsciiConverter::new();
    /// let video_opts = VideoOptions::default();
    /// let conv_opts = ConversionOptions::default();
    ///
    /// converter.convert_video_with_detailed_progress(
    ///     Path::new("video.mp4"),
    ///     Path::new("output"),
    ///     &video_opts,
    ///     &conv_opts,
    ///     false,
    ///     |progress| {
    ///         match progress.phase {
    ///             ProgressPhase::ExtractingFrames => {
    ///                 println!("Extracting frames from video...");
    ///             }
    ///             ProgressPhase::ExtractingAudio => {
    ///                 println!("Extracting audio...");
    ///             }
    ///             ProgressPhase::ConvertingFrames => {
    ///                 println!("Converting: {}/{} ({:.1}%)",
    ///                     progress.completed, progress.total, progress.percentage);
    ///             }
    ///             ProgressPhase::Complete => {
    ///                 println!("Conversion complete!");
    ///             }
    ///         }
    ///     },
    /// ).unwrap();
    /// ```
    pub fn convert_video_with_detailed_progress<F>(&self, input: &Path, output_dir: &Path, video_opts: &VideoOptions, conv_opts: &ConversionOptions, keep_images: bool, progress_callback: F) -> Result<ConversionResult> where F: Fn(Progress) + Send + Sync {
        fs::create_dir_all(output_dir).context("creating output directory")?;

        // Phase 1: Extract frames from video with progress reporting
        extract_video_frames_with_progress(input, output_dir, video_opts, &self.ffmpeg_config, &progress_callback)?;

        // Phase 2: Extract audio if requested
        if video_opts.extract_audio {
            progress_callback(Progress::extracting_audio());
            extract_audio(input, output_dir, video_opts.start.as_deref(), video_opts.end.as_deref(), &self.ffmpeg_config)?;
        }

        // Phase 3: Convert frames to ASCII with progress
        let ascii_chars = conv_opts.ascii_chars.as_bytes();
        let total_frames = convert_directory_parallel_with_detailed_progress(output_dir, output_dir, conv_opts.font_ratio, conv_opts.luminance, keep_images, ascii_chars, &conv_opts.output_mode, &progress_callback)?;

        // Phase 4: Complete
        progress_callback(Progress::complete(total_frames));

        // Build result with conversion details
        let output_mode_str = match conv_opts.output_mode {
            OutputMode::TextOnly => "text-only",
            OutputMode::ColorOnly => "color-only",
            OutputMode::TextAndColor => "text+color",
        };

        let result = ConversionResult {
            frame_count: total_frames,
            columns: conv_opts.columns.unwrap_or(video_opts.columns),
            font_ratio: conv_opts.font_ratio,
            luminance: conv_opts.luminance,
            fps: Some(video_opts.fps),
            output_mode: output_mode_str.to_string(),
            audio_extracted: video_opts.extract_audio,
            output_dir: output_dir.to_path_buf(),
        };

        // Write the details.md file
        result.write_details_file()?;

        Ok(result)
    }

    /// Convert a directory of images to ASCII frames
    ///
    /// # Arguments
    ///
    /// * `input_dir` - Directory containing PNG images
    /// * `output_dir` - Directory to write ASCII files
    /// * `options` - Conversion options
    /// * `keep_images` - Whether to keep original images
    ///
    /// Returns the number of frames converted.
    pub fn convert_directory(&self, input_dir: &Path, output_dir: &Path, options: &ConversionOptions, keep_images: bool) -> Result<usize> {
        fs::create_dir_all(output_dir)?;
        let ascii_chars = options.ascii_chars.as_bytes();
        convert_directory_parallel(input_dir, output_dir, options.font_ratio, options.luminance, keep_images, ascii_chars, &options.output_mode)
    }

    /// Convert a directory of images to ASCII frames with detailed progress reporting
    ///
    /// # Arguments
    ///
    /// * `input_dir` - Directory containing PNG images
    /// * `output_dir` - Directory to write ASCII files
    /// * `options` - Conversion options
    /// * `keep_images` - Whether to keep original images
    /// * `progress_callback` - Callback called with detailed Progress information
    ///
    /// # Example
    ///
    /// ```no_run
    /// use cascii::{AsciiConverter, ConversionOptions, Progress};
    /// use std::path::Path;
    ///
    /// let converter = AsciiConverter::new();
    /// let options = ConversionOptions::default();
    ///
    /// converter.convert_directory_with_progress(
    ///     Path::new("input_frames"),
    ///     Path::new("output_ascii"),
    ///     &options,
    ///     false,
    ///     |progress| {
    ///         println!("Converting: {}/{} ({:.1}%)",
    ///             progress.completed, progress.total, progress.percentage);
    ///     },
    /// ).unwrap();
    /// ```
    pub fn convert_directory_with_progress<F>(&self, input_dir: &Path, output_dir: &Path, options: &ConversionOptions, keep_images: bool, progress_callback: F) -> Result<usize> where F: Fn(Progress) + Send + Sync {
        fs::create_dir_all(output_dir)?;
        let ascii_chars = options.ascii_chars.as_bytes();
        convert_directory_parallel_with_detailed_progress(input_dir, output_dir, options.font_ratio, options.luminance, keep_images, ascii_chars, &options.output_mode, &progress_callback)
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
        Ok(ConversionOptions::from_preset(preset, self.config.ascii_chars.clone()))
    }
}

impl Default for AsciiConverter {
    fn default() -> Self {
        Self::new()
    }
}

// Internal implementation functions
fn convert_image_to_ascii(img_path: &Path, out_txt: &Path, font_ratio: f32, threshold: u8, columns: Option<u32>, ascii_chars: &[u8], output_mode: &OutputMode) -> Result<()> {
    match output_mode {
        OutputMode::TextOnly => {
            let ascii_string = image_to_ascii_string(img_path, font_ratio, threshold, columns, ascii_chars)?;
            fs::write(out_txt, ascii_string).with_context(|| format!("writing {}", out_txt.display()))?;
        }
        OutputMode::ColorOnly => {
            let (ascii_string, width, height, rgb_data) =
                image_to_ascii_with_colors(img_path, font_ratio, threshold, columns, ascii_chars)?;
            let cframe_path = out_txt.with_extension("cframe");
            write_cframe_binary(width, height, &ascii_string, &rgb_data, &cframe_path)?;
        }
        OutputMode::TextAndColor => {
            let (ascii_string, width, height, rgb_data) = image_to_ascii_with_colors(img_path, font_ratio, threshold, columns, ascii_chars)?;
            fs::write(out_txt, &ascii_string).with_context(|| format!("writing {}", out_txt.display()))?;
            let cframe_path = out_txt.with_extension("cframe");
            write_cframe_binary(width, height, &ascii_string, &rgb_data, &cframe_path)?;
        }
    }
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

/// Returns (ascii_string, width, height, rgb_bytes)
/// rgb_bytes is a flat Vec<u8> with 3 bytes (R, G, B) per character, row-major order
fn image_to_ascii_with_colors(img_path: &Path, font_ratio: f32, threshold: u8, columns: Option<u32>, ascii_chars: &[u8]) -> Result<(String, u32, u32, Vec<u8>)> {
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
    let mut rgb_data: Vec<u8> = Vec::with_capacity((w as usize) * (h as usize) * 3);

    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x, y);
            let l = luminance(*px);
            out.push(char_for(l, threshold, ascii_chars));
            rgb_data.push(px[0]);
            rgb_data.push(px[1]);
            rgb_data.push(px[2]);
        }
        out.push('\n');
    }
    Ok((out, w, h, rgb_data))
}

/// Combined binary format (.cframe): text + color in one file.
/// Header (8 bytes): width (u32 LE) + height (u32 LE)
/// Body (width * height * 4 bytes): for each character position (row-major):
///   char (u8) + r (u8) + g (u8) + b (u8)
fn write_cframe_binary(width: u32, height: u32, ascii_content: &str, rgb_data: &[u8], path: &Path) -> Result<()> {
    use std::io::Write;
    let mut file = fs::File::create(path).with_context(|| format!("creating cframe file {}", path.display()))?;
    file.write_all(&width.to_le_bytes())?;
    file.write_all(&height.to_le_bytes())?;

    let mut char_idx = 0;
    for ch in ascii_content.chars() {
        if ch == '\n' { continue; }
        let rgb_offset = char_idx * 3;
        file.write_all(&[ch as u8, rgb_data[rgb_offset], rgb_data[rgb_offset + 1], rgb_data[rgb_offset + 2]])?;
        char_idx += 1;
    }
    Ok(())
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

fn extract_video_frames(input: &Path, out_dir: &Path, columns: u32, fps: u32, start: Option<&str>, end: Option<&str>, ffmpeg_config: &FfmpegConfig) -> Result<()> {
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

    let status = ProcCommand::new(ffmpeg_config.ffmpeg_cmd())
        .args(&ffmpeg_args)
        .status()
        .context("running ffmpeg")?;

    if !status.success() {
        return Err(anyhow!("ffmpeg failed"));
    }
    Ok(())
}

/// Get video duration in microseconds using ffprobe
fn get_video_duration_us(input: &Path, ffmpeg_config: &FfmpegConfig) -> Result<u64> {
    let output = ProcCommand::new(ffmpeg_config.ffprobe_cmd())
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            input.to_str().unwrap(),
        ])
        .output()
        .context("running ffprobe")?;

    if !output.status.success() {
        return Err(anyhow!("ffprobe failed to get duration"));
    }

    let duration_str = String::from_utf8_lossy(&output.stdout);
    let duration_secs: f64 = duration_str.trim().parse().unwrap_or(0.0);
    Ok((duration_secs * 1_000_000.0) as u64)
}

/// Extract video frames with progress reporting
fn extract_video_frames_with_progress<F>(input: &Path, out_dir: &Path, video_opts: &VideoOptions, ffmpeg_config: &FfmpegConfig, progress_callback: &F) -> Result<()> where F: Fn(Progress) + Send + Sync {
    let columns = video_opts.columns;
    let fps = video_opts.fps;
    let start = video_opts.start.as_deref();
    let end = video_opts.end.as_deref();

    let out_pattern = out_dir.join("frame_%04d.png");

    // Get video duration for progress calculation
    let _total_duration_us = get_video_duration_us(input, ffmpeg_config).unwrap_or(0);

    let mut ffmpeg_args: Vec<String> = vec!["-loglevel".into(), "error".into(), "-progress".into(), "pipe:1".into(), "-nostats".into()];

    if let Some(s) = start {
        if !s.is_empty() && s != "0" {
            ffmpeg_args.push("-ss".into());
            ffmpeg_args.push(s.to_string());
        }
    }

    ffmpeg_args.push("-i".into());
    ffmpeg_args.push(input.to_str().ok_or_else(|| anyhow!("input path is not valid UTF-8"))?.to_string());

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
    ffmpeg_args.push(out_pattern.to_str().ok_or_else(|| anyhow!("output path is not valid UTF-8"))?.to_string());
    progress_callback(Progress::extracting_frames());

    let mut child = ProcCommand::new(ffmpeg_config.ffmpeg_cmd())
        .args(&ffmpeg_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning ffmpeg")?;

    let status = child.wait().context("waiting for ffmpeg")?;
    if !status.success() {
        return Err(anyhow!("ffmpeg failed"));
    }

    Ok(())
}

fn extract_audio(input: &Path, out_dir: &Path, start: Option<&str>, end: Option<&str>, ffmpeg_config: &FfmpegConfig) -> Result<()> {
    let out_audio = out_dir.join("audio.mp3");
    let mut ffmpeg_args: Vec<String> = vec!["-loglevel".into(), "error".into(), "-y".into()];

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

    // Extract audio only, no video
    ffmpeg_args.push("-vn".into());
    ffmpeg_args.push("-acodec".into());
    ffmpeg_args.push("libmp3lame".into());
    ffmpeg_args.push("-q:a".into());
    ffmpeg_args.push("2".into());
    ffmpeg_args.push(out_audio.to_str().unwrap().to_string());

    let status = ProcCommand::new(ffmpeg_config.ffmpeg_cmd())
        .args(&ffmpeg_args)
        .status()
        .context("running ffmpeg for audio extraction")?;

    if !status.success() {
        return Err(anyhow!("ffmpeg audio extraction failed"));
    }
    Ok(())
}

fn parse_timestamp(s: &str) -> f64 {
    s.split(':').rev().enumerate().fold(0.0, |acc, (i, v)| {
        acc + v.parse::<f64>().unwrap_or(0.0) * 60f64.powi(i as i32)
    })
}

fn convert_directory_parallel(src_dir: &Path, dst_dir: &Path, font_ratio: f32, threshold: u8, keep_images: bool, ascii_chars: &[u8], output_mode: &OutputMode) -> Result<usize> {
    convert_directory_parallel_with_progress(src_dir, dst_dir, font_ratio, threshold, keep_images, ascii_chars, output_mode, None::<fn(usize, usize)>)
}

#[allow(clippy::too_many_arguments)]
fn convert_directory_parallel_with_progress<F>(src_dir: &Path, dst_dir: &Path, font_ratio: f32, threshold: u8, keep_images: bool, ascii_chars: &[u8], output_mode: &OutputMode, progress_callback: Option<F>) -> Result<usize> where F: Fn(usize, usize) + Send + Sync {
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
        convert_image_to_ascii(img_path, &out_txt, font_ratio, threshold, None, ascii_chars, output_mode)?;

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

    Ok(total)
}

/// Internal function for directory conversion with detailed Progress reporting
#[allow(clippy::too_many_arguments)]
fn convert_directory_parallel_with_detailed_progress<F>(src_dir: &Path, dst_dir: &Path, font_ratio: f32, threshold: u8, keep_images: bool, ascii_chars: &[u8], output_mode: &OutputMode, progress_callback: &F) -> Result<usize> where F: Fn(Progress) + Send + Sync {
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
    let last_reported_percent = Arc::new(AtomicUsize::new(0));

    // Report initial progress
    progress_callback(Progress::converting_frames(0, total));

    pngs.par_iter().try_for_each(|img_path| -> Result<()> {
        let file_stem = img_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("bad file name"))?;
        let out_txt = dst_dir.join(format!("{}.txt", file_stem));
        convert_image_to_ascii(img_path, &out_txt, font_ratio, threshold, None, ascii_chars, output_mode)?;

        // Update progress - throttle to only report every 1% change
        let current = completed.fetch_add(1, Ordering::SeqCst) + 1;
        let current_percent = if total > 0 { (current * 100) / total } else { 0 };
        let last_percent = last_reported_percent.load(Ordering::SeqCst);

        // Only report if percentage changed (throttle to ~100 updates max)
        if current_percent > last_percent || current == total {
            last_reported_percent.store(current_percent, Ordering::SeqCst);
            progress_callback(Progress::converting_frames(current, total));
        }

        Ok(())
    })?;

    if !keep_images {
        for img_path in &pngs {
            fs::remove_file(img_path)?;
        }
    }

    Ok(total)
}
