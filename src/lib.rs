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
//! converter.convert_image(Path::new("input.png"), Path::new("output.txt"), &options)?;
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
//!             ProgressPhase::RenderingVideo => println!("Rendering video..."),
//!             ProgressPhase::Complete => println!("Done!"),
//!         }
//!     },
//! ).unwrap();
//! ```

use anyhow::{anyhow, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

mod background_fit_optimized;
pub mod color_shift;
pub mod convert;
pub mod crop;
pub mod loop_detect;
pub mod preprocessing;
pub mod render;
pub mod video;

/// A cheap, clonable cancellation flag shared between a running conversion and
/// the code that wants to stop it.
///
/// Cloning shares the same underlying flag, so a clone handed to another thread
/// (or stored in a registry) can cancel work running elsewhere. Conversions
/// check the token cooperatively at frame boundaries and while waiting on
/// `ffmpeg`, so cancellation is "stop soon" rather than instantaneous.
#[derive(Clone, Default)]
pub struct CancelToken(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    /// Create a fresh, not-yet-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent and cheap to call from any thread.
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Returns `true` once [`cancel`](Self::cancel) has been called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Error returned by conversion functions when a [`CancelToken`] was triggered
/// mid-flight.
///
/// Callers can downcast the returned `anyhow::Error` to tell a user-requested
/// cancellation apart from a genuine failure:
///
/// ```no_run
/// # let err: anyhow::Error = anyhow::anyhow!("x");
/// if err.downcast_ref::<cascii::Cancelled>().is_some() {
///     // cancelled by the user — not a real error
/// }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "operation cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Returns `true` if `err` represents a [`Cancelled`] cancellation.
pub fn is_cancelled_error(err: &anyhow::Error) -> bool {
    err.downcast_ref::<Cancelled>().is_some()
}

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
    pub(crate) fn ffmpeg_cmd(&self) -> &OsStr {
        self.ffmpeg_path.as_ref().map(|p| p.as_os_str()).unwrap_or(OsStr::new("ffmpeg"))
    }

    /// Get the ffprobe command name or path
    pub(crate) fn ffprobe_cmd(&self) -> &OsStr {
        self.ffprobe_path.as_ref().map(|p| p.as_os_str()).unwrap_or(OsStr::new("ffprobe"))
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
    /// Rendering ASCII frames to video and encoding with ffmpeg
    RenderingVideo,
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
        Self {phase: ProgressPhase::ExtractingFrames, completed: 0, total: 0, percentage: 0.0, message: "Extracting frames from video...".to_string()}
    }

    /// Create a progress update for extracting frames with percentage
    pub fn extracting_frames_progress(current_time_us: u64, total_duration_us: u64) -> Self {
        let percentage = if total_duration_us > 0 {(current_time_us as f64 / total_duration_us as f64) * 100.0} else {0.0};
        Self {phase: ProgressPhase::ExtractingFrames, completed: current_time_us as usize, total: total_duration_us as usize, percentage, message: format!("Extracting frames: {:.1}%", percentage)}
    }

    /// Create a new progress update for extracting audio
    pub fn extracting_audio() -> Self {
        Self {phase: ProgressPhase::ExtractingAudio, completed: 0, total: 0, percentage: 0.0, message: "Extracting audio from video...".to_string()}
    }

    /// Create a new progress update for frame conversion
    pub fn converting_frames(completed: usize, total: usize) -> Self {
        let percentage = if total > 0 {(completed as f64 / total as f64) * 100.0} else {0.0};
        Self {phase: ProgressPhase::ConvertingFrames, completed, total, percentage, message: format!("Converting frame {} of {}", completed, total)}
    }

    /// Create a progress update for rendering video frames
    pub fn rendering_video(completed: usize, total: usize) -> Self {
        let percentage = if total > 0 {(completed as f64 / total as f64) * 100.0} else {0.0};
        Self {phase: ProgressPhase::RenderingVideo, completed, total, percentage, message: format!("Rendering frame {} of {}", completed, total)}
    }

    /// Create a completion progress update
    pub fn complete(total_frames: usize) -> Self {
        Self {phase: ProgressPhase::Complete, completed: total_frames, total: total_frames, percentage: 100.0, message: format!("Conversion complete: {} frames", total_frames)}
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
    /// Background color name
    pub background_color: String,
    /// Foreground color name
    pub color: String,
    /// Whether per-cell background fitting was enabled
    pub fit_cell_backgrounds: bool,
    /// Background fitting implementation: "off", "legacy", or "optimized"
    #[serde(default = "default_cell_background_mode")]
    pub cell_background_mode: String,
    /// Resolved background luminance threshold actually used by the bg-fit pass. Equal to `luminance` unless an explicit override was set via `ConversionOptions::with_bg_luminance`.
    pub bg_luminance: u8,
    /// Character ramp used for glyph selection, from darkest to lightest.
    pub ascii_chars: String,
}

fn default_cell_background_mode() -> String {
    "off".to_string()
}

/// Serializable details written to `details.toml`
#[derive(Debug, Serialize)]
struct Details {
    version: String,
    frames: usize,
    luminance: u8,
    font_ratio: f32,
    columns: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    fps: Option<u32>,
    output: String,
    audio: bool,
    background_color: String,
    color: String,
    fit_cell_backgrounds: bool,
    cell_background_mode: String,
    bg_luminance: u8,
    ascii_chars: String,
}

impl ConversionResult {
    fn to_details(&self) -> Details {
        Details {version: env!("CARGO_PKG_VERSION").to_string(), frames: self.frame_count, luminance: self.luminance, font_ratio: self.font_ratio, columns: self.columns, fps: self.fps, output: self.output_mode.clone(), audio: self.audio_extracted, background_color: self.background_color.clone(), color: self.color.clone(), fit_cell_backgrounds: self.fit_cell_backgrounds, cell_background_mode: self.cell_background_mode.clone(), bg_luminance: self.bg_luminance, ascii_chars: self.ascii_chars.clone()}
    }

    /// Write the conversion details to a details.toml file in the output directory
    pub fn write_details_file(&self) -> Result<PathBuf> {
        let details_path = self.output_dir.join("details.toml");
        let toml_string = toml::to_string_pretty(&self.to_details()).context("serializing details to TOML")?;
        fs::write(&details_path, &toml_string).with_context(|| format!("writing details file to {}", details_path.display()))?;

        Ok(details_path)
    }

    /// Get the details as a TOML string (without writing to file)
    pub fn to_details_string(&self) -> String {
        toml::to_string_pretty(&self.to_details()).expect("failed to serialize details to TOML")
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

/// Controls how per-cell colors are modeled during conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellColorMode {
    /// Current behavior: a single foreground color per character cell.
    ForegroundOnly,
    /// Experimental option C: fit both foreground and background colors per cell.
    FitForegroundBackground,
    /// Optimized foreground/background fitter with a small atlas and candidate shortlist.
    FitForegroundBackgroundOptimized,
}

impl CellColorMode {
    pub fn fits_cell_backgrounds(self) -> bool {
        self != Self::ForegroundOnly
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ForegroundOnly => "off",
            Self::FitForegroundBackground => "legacy",
            Self::FitForegroundBackgroundOptimized => "optimized",
        }
    }
}

/// Options for ASCII conversion
#[derive(Debug, Clone)]
pub struct ConversionOptions {
    /// Target width in characters (columns)
    pub columns: Option<u32>,
    /// Font aspect ratio (width/height of character)
    pub font_ratio: f32,
    /// Luminance threshold (0-255) for the foreground glyph pass.
    /// Cells whose average luminance falls below this value emit a space
    /// instead of a glyph.
    pub luminance: u8,
    /// Optional override for the background pass's luminance threshold.
    ///
    /// `None` means "track the foreground value", which preserves the
    /// historical single-threshold behaviour. `Some(n)` lets callers decide
    /// independently whether to emit a per-cell background.
    pub bg_luminance: Option<u8>,
    /// ASCII character set to use (from darkest to lightest)
    pub ascii_chars: String,
    /// What output files to generate
    pub output_mode: OutputMode,
    /// How per-cell colors should be modeled during conversion
    pub cell_color_mode: CellColorMode,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {columns: Some(400), font_ratio: 0.7, luminance: 20, bg_luminance: None, ascii_chars: default_ascii_chars(), output_mode: OutputMode::TextOnly, cell_color_mode: CellColorMode::ForegroundOnly}
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

    /// Set an explicit background luminance threshold for the bg-fit pass.
    ///
    /// Without this override the bg pass uses the same threshold as the
    /// foreground glyph pass.
    pub fn with_bg_luminance(mut self, bg_luminance: u8) -> Self {
        self.bg_luminance = Some(bg_luminance);
        self
    }

    /// Set the background luminance override from an `Option`.
    ///
    /// `Some(n)` overrides the bg threshold; `None` clears the override and
    /// makes the bg pass track the foreground luminance again.
    pub fn with_bg_luminance_opt(mut self, bg_luminance: Option<u8>) -> Self {
        self.bg_luminance = bg_luminance;
        self
    }

    /// Resolve the effective background luminance threshold actually used by
    /// the bg-fit pass: the override when set, otherwise the foreground value.
    pub fn resolve_bg_threshold(&self) -> u8 {
        self.bg_luminance.unwrap_or(self.luminance)
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

    /// Set the per-cell color mode
    pub fn with_cell_color_mode(mut self, mode: CellColorMode) -> Self {
        self.cell_color_mode = mode;
        self
    }

    /// Create options from a preset
    pub fn from_preset(preset: &Preset, ascii_chars: String) -> Self {
        Self {columns: Some(preset.columns), font_ratio: preset.font_ratio, luminance: preset.luminance, bg_luminance: None, ascii_chars, output_mode: OutputMode::TextOnly, cell_color_mode: CellColorMode::ForegroundOnly}
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
    /// Optional ffmpeg filtergraph applied before cascii's scale/fps extraction
    ///
    /// Example: `"format=gray,edgedetect=mode=colormix:high=0.2:low=0.05"`
    pub preprocess_filter: Option<String>,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {fps: 30, start: None, end: None, columns: 400, extract_audio: false, preprocess_filter: None}
    }
}

/// Options for rendering ASCII frames to a video file
#[derive(Debug, Clone)]
pub struct ToVideoOptions {
    /// Output video file path (e.g., "output.mp4")
    pub output_path: PathBuf,
    /// Font size in pixels for rendering characters (determines output resolution)
    pub font_size: f32,
    /// CRF quality for H.264 encoding (0-51, lower is better quality, 18 is visually lossless)
    pub crf: u8,
    /// Whether to mux audio from the source video into the output
    pub mux_audio: bool,
    /// Override color rendering: `Some(true)` forces per-character colors from .cframe data,
    /// `Some(false)` forces monochrome white-on-black, `None` auto-detects from file types.
    pub use_colors: Option<bool>,
}

impl Default for ToVideoOptions {
    fn default() -> Self {
        Self {output_path: PathBuf::from("output.mp4"), font_size: 14.0, crf: 18, mux_audio: false, use_colors: None}
    }
}

/// Main converter struct for ASCII art generation
pub struct AsciiConverter {
    config: AppConfig,
    ffmpeg_config: FfmpegConfig,
    cancel_token: Option<CancelToken>,
}

impl AsciiConverter {
    /// Create a new converter with default configuration
    pub fn new() -> Self {
        Self {config: AppConfig::default(), ffmpeg_config: FfmpegConfig::default(), cancel_token: None}
    }

    /// Create a converter with custom configuration
    pub fn with_config(config: AppConfig) -> Result<Self> {
        // Validate ASCII characters
        if !config.ascii_chars.is_ascii() {
            return Err(anyhow!("Config contains non-ASCII characters in ascii_chars field. This will cause corrupted output. Please use only ASCII characters."));
        }
        Ok(Self {config, ffmpeg_config: FfmpegConfig::default(), cancel_token: None})
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

    /// Attach a [`CancelToken`] so an in-flight conversion can be interrupted.
    ///
    /// Hold a clone of the token elsewhere (e.g. a job registry) and call
    /// [`CancelToken::cancel`] to stop the work. Without a token, conversions
    /// run to completion as before.
    ///
    /// ```no_run
    /// use cascii::{AsciiConverter, CancelToken};
    ///
    /// let token = CancelToken::new();
    /// let converter = AsciiConverter::new().with_cancel_token(token.clone());
    /// // hand `token` to another thread; calling `token.cancel()` stops the run.
    /// ```
    pub fn with_cancel_token(mut self, token: CancelToken) -> Self {
        self.cancel_token = Some(token);
        self
    }

    /// Load configuration from a file
    pub fn from_config_file(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).with_context(|| format!("reading config {}", path.display()))?;
        let config: AppConfig = serde_json::from_str(&text).context("parsing config json")?;

        if !config.ascii_chars.is_ascii() {
            return Err(anyhow!("Config file {} contains non-ASCII characters in ascii_chars field. This will cause corrupted output. Please use only ASCII characters.", path.display()));
        }

        Ok(Self {config, ffmpeg_config: FfmpegConfig::default(), cancel_token: None})
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
    /// converter.convert_image(Path::new("image.png"), Path::new("output.txt"), &options)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn convert_image(&self, input: &Path, output: &Path, options: &ConversionOptions) -> Result<()> {
        let ascii_chars = options.ascii_chars.as_bytes();
        convert::convert_image_to_ascii(input, output, options.font_ratio, options.luminance, options.resolve_bg_threshold(), options.columns, ascii_chars, &options.output_mode, options.cell_color_mode)
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
        convert::image_to_ascii_string(input, options.font_ratio, options.luminance, options.columns, ascii_chars)
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
    /// converter.convert_video(Path::new("video.mp4"), Path::new("output_dir"), &video_opts, &conv_opts, false)?;
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
    /// let video_opts = VideoOptions {fps: 24, start: None, end: None, columns: 120, extract_audio: false, preprocess_filter: None};
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
    pub fn convert_video_with_progress<F: Fn(usize, usize) + Send + Sync>(&self, input: &Path, output_dir: &Path, video_opts: &VideoOptions, conv_opts: &ConversionOptions, keep_images: bool, progress_callback: Option<F>) -> Result<ConversionResult> {
        fs::create_dir_all(output_dir).context("creating output directory")?;

        // Extract frames with ffmpeg
        let ascii_chars = conv_opts.ascii_chars.as_bytes();
        video::extract_video_frames(input, output_dir, video_opts.columns, video_opts.fps, video_opts.start.as_deref(), video_opts.end.as_deref(), video_opts.preprocess_filter.as_deref(), &self.ffmpeg_config, self.cancel_token.as_ref())?;

        // Extract audio if requested
        if video_opts.extract_audio {
            video::extract_audio(input, output_dir, video_opts.start.as_deref(), video_opts.end.as_deref(), &self.ffmpeg_config, self.cancel_token.as_ref())?;
        }

        // Convert frames to ASCII with progress callback
        let total_frames = if conv_opts.cell_color_mode == CellColorMode::FitForegroundBackgroundOptimized {convert::convert_directory_parallel_optimized_with_progress(output_dir, output_dir, conv_opts.font_ratio, conv_opts.luminance, conv_opts.resolve_bg_threshold(), conv_opts.columns.unwrap_or(video_opts.columns), keep_images, ascii_chars, &conv_opts.output_mode, progress_callback, self.cancel_token.as_ref())?} else {convert::convert_directory_parallel_with_progress(output_dir, output_dir, conv_opts.font_ratio, conv_opts.luminance, conv_opts.resolve_bg_threshold(), keep_images, ascii_chars, &conv_opts.output_mode, conv_opts.cell_color_mode, progress_callback, self.cancel_token.as_ref())?};

        // Build result with conversion details
        let output_mode_str = match conv_opts.output_mode {
            OutputMode::TextOnly => "text-only",
            OutputMode::ColorOnly => "color-only",
            OutputMode::TextAndColor => "text+color",
        };

        let result = ConversionResult {frame_count: total_frames, columns: conv_opts.columns.unwrap_or(video_opts.columns), font_ratio: conv_opts.font_ratio, luminance: conv_opts.luminance, fps: Some(video_opts.fps), output_mode: output_mode_str.to_string(), audio_extracted: video_opts.extract_audio, output_dir: output_dir.to_path_buf(), background_color: "black".to_string(), color: "white".to_string(), fit_cell_backgrounds: conv_opts.cell_color_mode.fits_cell_backgrounds(), cell_background_mode: conv_opts.cell_color_mode.as_str().to_string(), bg_luminance: conv_opts.resolve_bg_threshold(), ascii_chars: conv_opts.ascii_chars.clone()};

        // Write the details.toml file
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
    ///             ProgressPhase::RenderingVideo => {
    ///                 println!("Rendering video...");
    ///             }
    ///             ProgressPhase::Complete => {
    ///                 println!("Conversion complete!");
    ///             }
    ///         }
    ///     },
    /// ).unwrap();
    /// ```
    pub fn convert_video_with_detailed_progress<F: Fn(Progress) + Send + Sync>(&self, input: &Path, output_dir: &Path, video_opts: &VideoOptions, conv_opts: &ConversionOptions, keep_images: bool, progress_callback: F) -> Result<ConversionResult> {
        fs::create_dir_all(output_dir).context("creating output directory")?;

        // Phase 1: Extract frames from video with progress reporting
        let ascii_chars = conv_opts.ascii_chars.as_bytes();
        video::extract_video_frames_with_progress(input, output_dir, video_opts, &self.ffmpeg_config, &progress_callback, self.cancel_token.as_ref())?;

        // Phase 2: Extract audio if requested
        if video_opts.extract_audio {
            progress_callback(Progress::extracting_audio());
            video::extract_audio(input, output_dir, video_opts.start.as_deref(), video_opts.end.as_deref(), &self.ffmpeg_config, self.cancel_token.as_ref())?;
        }

        // Phase 3: Convert frames to ASCII with progress
        let total_frames = if conv_opts.cell_color_mode == CellColorMode::FitForegroundBackgroundOptimized {convert::convert_directory_parallel_optimized_with_detailed_progress(output_dir, output_dir, conv_opts.font_ratio, conv_opts.luminance, conv_opts.resolve_bg_threshold(), conv_opts.columns.unwrap_or(video_opts.columns), keep_images, ascii_chars, &conv_opts.output_mode, &progress_callback, self.cancel_token.as_ref())?} else {convert::convert_directory_parallel_with_detailed_progress(output_dir, output_dir, conv_opts.font_ratio, conv_opts.luminance, conv_opts.resolve_bg_threshold(), keep_images, ascii_chars, &conv_opts.output_mode, conv_opts.cell_color_mode, &progress_callback, self.cancel_token.as_ref())?};

        // Phase 4: Complete
        progress_callback(Progress::complete(total_frames));

        // Build result with conversion details
        let output_mode_str = match conv_opts.output_mode {
            OutputMode::TextOnly => "text-only",
            OutputMode::ColorOnly => "color-only",
            OutputMode::TextAndColor => "text+color",
        };

        let result = ConversionResult {frame_count: total_frames, columns: conv_opts.columns.unwrap_or(video_opts.columns), font_ratio: conv_opts.font_ratio, luminance: conv_opts.luminance, fps: Some(video_opts.fps), output_mode: output_mode_str.to_string(), audio_extracted: video_opts.extract_audio, output_dir: output_dir.to_path_buf(), background_color: "black".to_string(), color: "white".to_string(), fit_cell_backgrounds: conv_opts.cell_color_mode.fits_cell_backgrounds(), cell_background_mode: conv_opts.cell_color_mode.as_str().to_string(), bg_luminance: conv_opts.resolve_bg_threshold(), ascii_chars: conv_opts.ascii_chars.clone()};

        // Write the details.toml file
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
        if options.cell_color_mode == CellColorMode::FitForegroundBackgroundOptimized {
            convert::convert_directory_parallel_optimized_with_progress(input_dir, output_dir, options.font_ratio, options.luminance, options.resolve_bg_threshold(), options.columns.unwrap_or(400), keep_images, ascii_chars, &options.output_mode, None::<fn(usize, usize)>, self.cancel_token.as_ref())
        } else {
            convert::convert_directory_parallel(input_dir, output_dir, options.font_ratio, options.luminance, options.resolve_bg_threshold(), keep_images, ascii_chars, &options.output_mode, options.cell_color_mode, self.cancel_token.as_ref())
        }
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
    pub fn convert_directory_with_progress<F: Fn(Progress) + Send + Sync>(&self, input_dir: &Path, output_dir: &Path, options: &ConversionOptions, keep_images: bool, progress_callback: F) -> Result<usize> {
        fs::create_dir_all(output_dir)?;
        let ascii_chars = options.ascii_chars.as_bytes();
        convert::convert_directory_parallel_with_detailed_progress(input_dir, output_dir, options.font_ratio, options.luminance, options.resolve_bg_threshold(), keep_images, ascii_chars, &options.output_mode, options.cell_color_mode, &progress_callback, self.cancel_token.as_ref())
    }

    /// Get a preset by name
    pub fn get_preset(&self, name: &str) -> Option<&Preset> {
        self.config.presets.get(name)
    }

    /// Get conversion options from a preset name
    pub fn options_from_preset(&self, preset_name: &str) -> Result<ConversionOptions> {
        let preset = self.get_preset(preset_name).ok_or_else(|| anyhow!("Preset '{}' not found", preset_name))?;
        Ok(ConversionOptions::from_preset(preset, self.config.ascii_chars.clone()))
    }

    /// Convert a video to an ASCII-art video file
    ///
    /// Extracts frames from the input video, converts each to ASCII art,
    /// renders the ASCII characters to pixel buffers, and pipes them to
    /// ffmpeg to produce an output MP4 video.
    pub fn convert_video_to_video<F: Fn(Progress) + Send + Sync>(&self, input: &Path, video_opts: &VideoOptions, conv_opts: &ConversionOptions, to_video_opts: &ToVideoOptions, progress_callback: F) -> Result<ConversionResult> {
        // Create temp directory for intermediate PNG frames
        let temp_dir = std::env::temp_dir().join(format!("cascii_tovideo_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).context("creating temp directory")?;

        // Ensure cleanup on exit (both success and error paths)
        let result = self.convert_video_to_video_inner(input, video_opts, conv_opts, to_video_opts, &temp_dir, &progress_callback);

        // Clean up temp directory
        let _ = fs::remove_dir_all(&temp_dir);

        result
    }

    fn convert_video_to_video_inner<F: Fn(Progress) + Send + Sync>(&self, input: &Path, video_opts: &VideoOptions, conv_opts: &ConversionOptions, to_video_opts: &ToVideoOptions, temp_dir: &Path, progress_callback: &F) -> Result<ConversionResult> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc::sync_channel;
        use std::sync::Arc;
        use std::thread;

        // Phase 1: Extract frames from video
        let ascii_chars = conv_opts.ascii_chars.as_bytes();
        video::extract_video_frames_with_progress(input, temp_dir, video_opts, &self.ffmpeg_config, progress_callback, self.cancel_token.as_ref())?;

        // Phase 2: Extract audio if requested
        let audio_path = if to_video_opts.mux_audio {
            progress_callback(Progress::extracting_audio());
            video::extract_audio(input, temp_dir, video_opts.start.as_deref(), video_opts.end.as_deref(), &self.ffmpeg_config, self.cancel_token.as_ref())?;
            Some(temp_dir.join("audio.mp3"))
        } else {
            None
        };

        // Collect and sort PNG frame paths
        let mut png_paths: Vec<PathBuf> = WalkDir::new(temp_dir).min_depth(1).max_depth(1).into_iter().filter_map(|e| e.ok()).map(|e| e.into_path()).filter(|p| p.extension().map(|e| e == "png").unwrap_or(false)).collect();
        png_paths.sort();

        let total_frames = png_paths.len();
        if total_frames == 0 {
            return Err(anyhow!("No frames extracted from video"));
        }

        // Phase 3: Build glyph atlas
        let atlas = render::build_glyph_atlas(to_video_opts.font_size)?;

        // Phase 4: Convert first frame to determine output resolution
        let background_analysis = convert::background_analysis_for_mode(ascii_chars, conv_opts.cell_color_mode)?;
        let bg_threshold = conv_opts.resolve_bg_threshold();
        let first_frame = convert::image_to_ascii_frame_data_with_analysis(&png_paths[0], conv_opts.font_ratio, conv_opts.luminance, bg_threshold, conv_opts.columns, ascii_chars, conv_opts.cell_color_mode, background_analysis.as_ref())?;
        let mut pixel_w = first_frame.width_chars * atlas.cell_width;
        let mut pixel_h = first_frame.height_chars * atlas.cell_height;
        // H.264 requires even dimensions
        if pixel_w % 2 != 0 {
            pixel_w += 1;
        }
        if pixel_h % 2 != 0 {
            pixel_h += 1;
        }

        // Phase 5: Spawn ffmpeg encoder
        let mut child = Some(render::spawn_ffmpeg_encoder(pixel_w, pixel_h, video_opts.fps, to_video_opts.crf, audio_path.as_deref(), &to_video_opts.output_path, &self.ffmpeg_config)?);
        let mut stdin = Some(child.as_mut().and_then(|child| child.stdin.take()).ok_or_else(|| anyhow!("failed to open ffmpeg stdin pipe"))?);
        let use_colors = conv_opts.output_mode != OutputMode::TextOnly;

        // Phase 6: Process frames in batches
        let batch_size = 100;
        let completed = Arc::new(AtomicUsize::new(0));

        progress_callback(Progress::rendering_video(0, total_frames));

        thread::scope(|scope| -> Result<()> {
            let (sender, receiver) = sync_channel::<Result<Vec<convert::AsciiFrameData>>>(2);
            let worker = scope.spawn(move || {
                for batch_start in (0..total_frames).step_by(batch_size) {
                    let batch_end = (batch_start + batch_size).min(total_frames);
                    let batch = &png_paths[batch_start..batch_end];
                    let frame_data: Result<Vec<convert::AsciiFrameData>> = batch.par_iter().map(|path| convert::image_to_ascii_frame_data_with_analysis(path, conv_opts.font_ratio, conv_opts.luminance, bg_threshold, conv_opts.columns, ascii_chars, conv_opts.cell_color_mode, background_analysis.as_ref())).collect();
                    if sender.send(frame_data).is_err() {
                        return;
                    }
                }
            });

            for frame_data in receiver {
                let frame_data = frame_data?;

                // Render and pipe sequentially (preserves frame order)
                for frame in &frame_data {
                    if self.cancel_token.as_ref().is_some_and(|c| c.is_cancelled()) {
                        drop(stdin.take());
                        if let Some(mut encoder) = child.take() {
                            let _ = encoder.kill();
                            let _ = encoder.wait();
                        }
                        return Err(Cancelled.into());
                    }
                    let rgb_buf = render::render_ascii_frame_to_rgb(frame, &atlas, use_colors);
                    if let Err(e) = stdin.as_mut().unwrap().write_all(&rgb_buf) {
                        drop(stdin.take());
                        let output = child.take().unwrap().wait_with_output().context("waiting for ffmpeg")?;
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        return Err(anyhow!("ffmpeg encoding failed: {} (stderr: {})", e, stderr));
                    }

                    let current = completed.fetch_add(1, Ordering::SeqCst) + 1;
                    let current_percent = current.checked_mul(100).and_then(|value| value.checked_div(total_frames)).unwrap_or(0);
                    let last_percent = if current > 1 {((current - 1) * 100) / total_frames} else {0};

                    if current_percent > last_percent || current == total_frames {
                        progress_callback(Progress::rendering_video(current, total_frames));
                    }
                }
            }

            worker.join().map_err(|_| anyhow!("frame conversion worker panicked"))?;
            Ok(())
        })?;

        // Close stdin to signal end of input
        drop(stdin.take());

        // Wait for ffmpeg to finish
        let output = child.take().unwrap().wait_with_output().context("waiting for ffmpeg")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("ffmpeg encoding failed: {}", stderr));
        }

        // Phase 7: Complete
        progress_callback(Progress::complete(total_frames));
        let output_mode_str = match conv_opts.output_mode {
            OutputMode::TextOnly => "text-only",
            OutputMode::ColorOnly => "color-only",
            OutputMode::TextAndColor => "text+color",
        };

        Ok(ConversionResult {frame_count: total_frames, columns: conv_opts.columns.unwrap_or(video_opts.columns), font_ratio: conv_opts.font_ratio, luminance: conv_opts.luminance, fps: Some(video_opts.fps), output_mode: output_mode_str.to_string(), audio_extracted: to_video_opts.mux_audio, output_dir: to_video_opts.output_path.parent().unwrap_or(Path::new(".")).to_path_buf(), background_color: "black".to_string(), color: "white".to_string(), fit_cell_backgrounds: conv_opts.cell_color_mode.fits_cell_backgrounds(), cell_background_mode: conv_opts.cell_color_mode.as_str().to_string(), bg_luminance: conv_opts.resolve_bg_threshold(), ascii_chars: conv_opts.ascii_chars.clone()})
    }

    /// Render existing ASCII frame files (.cframe or .txt) from a directory to a video file
    ///
    /// Scans the directory for .cframe files first; if none found, falls back to .txt files.
    /// Renders each frame using the glyph atlas and pipes to ffmpeg.
    pub fn render_frames_to_video<F: Fn(Progress) + Send + Sync>(&self, input_dir: &Path, fps: u32, to_video_opts: &ToVideoOptions, progress_callback: F) -> Result<ConversionResult> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        // Scan for .cframe files first, then fall back to .txt
        let mut frame_paths: Vec<PathBuf> = WalkDir::new(input_dir).min_depth(1).max_depth(1).into_iter().filter_map(|e| e.ok()).map(|e| e.into_path()).filter(|p| p.extension().map(|e| e == "cframe").unwrap_or(false)).collect();

        let use_cframes = !frame_paths.is_empty();

        if !use_cframes {
            frame_paths = WalkDir::new(input_dir).min_depth(1).max_depth(1).into_iter().filter_map(|e| e.ok()).map(|e| e.into_path()).filter(|p| p.extension().map(|e| e == "txt").unwrap_or(false) && p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with("frame_")).unwrap_or(false)).collect();
        }

        frame_paths.sort();

        let total_frames = frame_paths.len();
        if total_frames == 0 {
            return Err(anyhow!("No .cframe or .txt frame files found in {}", input_dir.display()));
        }

        // Build glyph atlas
        let atlas = render::build_glyph_atlas(to_video_opts.font_size)?;

        // Read first frame to determine pixel dimensions
        let first_frame = if use_cframes {
            convert::read_cframe_to_frame_data(&frame_paths[0])?
        } else {
            convert::read_txt_to_frame_data(&frame_paths[0])?
        };

        let mut pixel_w = first_frame.width_chars * atlas.cell_width;
        let mut pixel_h = first_frame.height_chars * atlas.cell_height;
        if !pixel_w.is_multiple_of(2) {
            pixel_w += 1;
        }
        if !pixel_h.is_multiple_of(2) {
            pixel_h += 1;
        }

        // Check for audio.mp3 in the directory
        let audio_path = if to_video_opts.mux_audio {
            let ap = input_dir.join("audio.mp3");
            if ap.exists() {
                Some(ap)
            } else {
                None
            }
        } else {
            None
        };

        // Spawn ffmpeg encoder
        let mut child = render::spawn_ffmpeg_encoder(pixel_w, pixel_h, fps, to_video_opts.crf, audio_path.as_deref(), &to_video_opts.output_path, &self.ffmpeg_config)?;
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("failed to open ffmpeg stdin pipe"))?;

        // Process frames in batches
        let batch_size = 100;
        let completed = Arc::new(AtomicUsize::new(0));
        let render_with_colors = to_video_opts.use_colors.unwrap_or(use_cframes);
        progress_callback(Progress::rendering_video(0, total_frames));

        for batch_start in (0..total_frames).step_by(batch_size) {
            let batch_end = (batch_start + batch_size).min(total_frames);
            let batch = &frame_paths[batch_start..batch_end];

            // Read batch in parallel
            let frame_data: Vec<convert::AsciiFrameData> = batch.par_iter().map(|path| if use_cframes {convert::read_cframe_to_frame_data(path)} else {convert::read_txt_to_frame_data(path)}).collect::<Result<Vec<_>>>()?;

            // Render and pipe sequentially
            for frame in &frame_data {
                let rgb_buf = render::render_ascii_frame_to_rgb(frame, &atlas, render_with_colors);
                if let Err(e) = stdin.write_all(&rgb_buf) {
                    drop(stdin);
                    let output = child.wait_with_output().context("waiting for ffmpeg")?;
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(anyhow!("ffmpeg encoding failed: {} (stderr: {})", e, stderr));
                }

                let current = completed.fetch_add(1, Ordering::SeqCst) + 1;
                let current_percent = current.checked_mul(100).and_then(|value| value.checked_div(total_frames)).unwrap_or(0);
                let last_percent = if current > 1 {((current - 1) * 100) / total_frames} else {0};

                if current_percent > last_percent || current == total_frames {
                    progress_callback(Progress::rendering_video(current, total_frames));
                }
            }
        }

        drop(stdin);

        let output = child.wait_with_output().context("waiting for ffmpeg")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("ffmpeg encoding failed: {}", stderr));
        }

        progress_callback(Progress::complete(total_frames));

        let mode_str = if use_cframes {"color"} else {"text-only"};

        let fit_cell_backgrounds = first_frame.bg_rgb_colors.len() == (first_frame.width_chars * first_frame.height_chars * 3) as usize;
        Ok(ConversionResult {frame_count: total_frames, columns: first_frame.width_chars, font_ratio: 0.0, luminance: 0, fps: Some(fps), output_mode: mode_str.to_string(), audio_extracted: audio_path.is_some(), output_dir: to_video_opts.output_path.parent().unwrap_or(Path::new(".")).to_path_buf(), background_color: "black".to_string(), color: "white".to_string(), fit_cell_backgrounds, cell_background_mode: if fit_cell_backgrounds {"legacy"} else {"off"}.to_string(), bg_luminance: 0, ascii_chars: default_ascii_chars()})
    }
}

impl Default for AsciiConverter {
    fn default() -> Self {
        Self::new()
    }
}

// Re-export crop API
pub use crop::{crop_frames, run_trim, CropResult};
