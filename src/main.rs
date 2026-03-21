use anyhow::{anyhow, Context, Result};
use cascii::loop_detect::run_find_loop;
use cascii::preprocessing::{preprocess_directory, preprocess_image_to_temp, resolve_preprocess_filter, PREPROCESS_PRESETS};
use cascii::{
    crop_frames, run_trim, AppConfig, AsciiConverter, ConversionOptions, OutputMode, Progress,
    ProgressPhase, ToVideoOptions, VideoOptions,
};
use clap::{Parser, Subcommand};
use dialoguer::{Confirm, FuzzySelect, Input};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use walkdir::WalkDir;

fn load_config() -> Result<AppConfig> {
    // Look for cascii.json in app support, current dir fallback, then built-in default
    let mut tried: Vec<PathBuf> = Vec::new();
    if let Some(mut d) = dirs::data_dir() {
        d.push("cascii");
        d.push("cascii.json");
        tried.push(d);
    }
    tried.push(PathBuf::from("cascii.json"));

    for p in &tried {
        if p.exists() {
            let text =
                fs::read_to_string(p).with_context(|| format!("reading config {}", p.display()))?;
            let cfg: AppConfig = serde_json::from_str(&text).context("parsing config json")?;

            // Validate that ascii_chars contains only ASCII characters
            if !cfg.ascii_chars.is_ascii() {
                return Err(anyhow!(
                    "Config file {} contains non-ASCII characters in ascii_chars field. \
                    This will cause corrupted output. Please use only ASCII characters.",
                    p.display()
                ));
            }

            return Ok(cfg);
        }
    }

    // Built-in defaults
    Ok(AppConfig::default())
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Uninstall cascii and remove associated data
    Uninstall,
}

#[derive(Parser, Debug)]
#[command(version, about = "Interactive video/image to ASCII frame generator.")]
struct Args {
    /// Optional subcommands
    #[command(subcommand)]
    cmd: Option<Command>,
    /// Input video file or directory of images
    input: Option<PathBuf>,

    /// Output directory for the generated files
    out: Option<PathBuf>,

    /// Target columns for scaling (width)
    #[arg(long)]
    columns: Option<u32>,

    /// Frames per second when extracting from video
    #[arg(long)]
    fps: Option<u32>,

    /// Font aspect ratio (character width:height)
    #[arg(long)]
    font_ratio: Option<f32>,

    /// Use default quality preset
    #[arg(long, default_value_t = false, conflicts_with_all = &["small", "large"])]
    default: bool,

    /// Use smaller default values for quality settings
    #[arg(long, short, default_value_t = false, conflicts_with_all = &["default", "large"])]
    small: bool,

    /// Use larger default values for quality settings
    #[arg(long, short, default_value_t = false, conflicts_with_all = &["default", "small"])]
    large: bool,

    /// Luminance threshold (0-255) for what is considered transparent
    #[arg(long)]
    luminance: Option<u8>,

    /// Log details to standard output
    #[arg(long, default_value_t = false)]
    log_details: bool,

    /// Keep intermediate image files
    #[arg(long, default_value_t = false)]
    keep_images: bool,

    /// Generate both .txt and .cframe (color) files
    #[arg(long, default_value_t = false, conflicts_with = "color_only")]
    colors: bool,

    /// Generate only .cframe (color) files, no .txt
    #[arg(long, default_value_t = false, conflicts_with = "colors")]
    color_only: bool,

    /// Render ASCII frames into a video file (mp4) instead of frame files
    #[arg(long, default_value_t = false)]
    to_video: bool,

    /// Font size in pixels for --to-video rendering (determines output resolution)
    #[arg(long, default_value_t = 14.0)]
    video_font_size: f32,

    /// CRF quality for --to-video encoding (0-51, lower = better, 18 = visually lossless)
    #[arg(long, default_value_t = 18)]
    crf: u8,

    /// Extract audio from video to audio.mp3
    #[arg(long, default_value_t = false)]
    audio: bool,

    /// Start time for video conversion (e.g., 00:01:23.456 or 83.456)
    #[arg(long)]
    start: Option<String>,

    /// End time for video conversion (e.g., 00:01:23.456 or 83.456)
    #[arg(long)]
    end: Option<String>,

    /// ffmpeg -vf filtergraph applied before ASCII conversion (video + single image inputs)
    #[arg(long, alias = "preprocessing", conflicts_with = "preprocess_preset")]
    preprocess: Option<String>,

    /// Built-in preprocessing preset name (see --list-preprocess-presets)
    #[arg(long, alias = "preprocessing-preset", conflicts_with = "preprocess")]
    preprocess_preset: Option<String>,

    /// Output directory for preprocessing: preprocess images here instead of using a temp file
    #[arg(long)]
    preprocess_output: Option<PathBuf>,

    /// List available preprocessing presets and exit
    #[arg(long, default_value_t = false)]
    list_preprocess_presets: bool,

    /// Find repeated loops in a frames directory (frame_*.txt)
    #[arg(long, default_value_t = false)]
    find_loop: bool,

    /// Trim equally from all sides (overridden by directional trims)
    #[arg(long)]
    trim: Option<usize>,

    /// Trim columns from the left side
    #[arg(long)]
    trim_left: Option<usize>,

    /// Trim columns from the right side
    #[arg(long)]
    trim_right: Option<usize>,

    /// Trim rows from the top
    #[arg(long)]
    trim_top: Option<usize>,

    /// Trim rows from the bottom
    #[arg(long)]
    trim_bottom: Option<usize>,

    /// Output directory for trim: copy frames here before cropping instead of trimming in-place
    #[arg(long)]
    trim_output: Option<PathBuf>,
}

fn print_preprocess_presets() {
    println!("Available preprocessing presets:");
    for preset in PREPROCESS_PRESETS {
        println!("  {:<16} {}", preset.name, preset.description);
        println!("      {}", preset.filter);
    }
}

fn main() -> Result<()> {
    let mut args = Args::parse();
    let is_interactive = !(args.default || args.small || args.large);

    // Handle subcommands early
    if let Some(Command::Uninstall) = &args.cmd {
        run_uninstall(is_interactive)?;
        println!("cascii uninstalled.");
        return Ok(());
    }

    if args.list_preprocess_presets {
        print_preprocess_presets();
        return Ok(());
    }

    let preprocess_filter = resolve_preprocess_filter(args.preprocess.as_deref(), args.preprocess_preset.as_deref())?;

    // Handle trimming early and exit
    let any_trim = args.trim.unwrap_or(0) > 0
        || args.trim_left.unwrap_or(0) > 0
        || args.trim_right.unwrap_or(0) > 0
        || args.trim_top.unwrap_or(0) > 0
        || args.trim_bottom.unwrap_or(0) > 0;
    if any_trim {
        let input_path = match &args.input {
            Some(p) => p.clone(),
            None => return Err(anyhow!("Input path must be provided when using --trim")),
        };
        let base = args.trim.unwrap_or(0);
        let trim_left = args.trim_left.unwrap_or(base);
        let trim_right = args.trim_right.unwrap_or(base);
        let trim_top = args.trim_top.unwrap_or(base);
        let trim_bottom = args.trim_bottom.unwrap_or(base);

        if let Some(output_dir) = &args.trim_output {
            if !input_path.is_dir() {
                return Err(anyhow!("--trim-output requires the input to be a directory"));
            }
            let result = crop_frames(&input_path, trim_top, trim_bottom, trim_left, trim_right, output_dir)?;
            println!(
                "Trim completed: left={}, right={}, top={}, bottom={} → {} frames written to {} ({}×{})",
                trim_left, trim_right, trim_top, trim_bottom,
                result.frame_count,
                output_dir.display(),
                result.new_width,
                result.new_height,
            );
        } else {
            run_trim(&input_path, trim_left, trim_right, trim_top, trim_bottom)?;
            println!(
                "Trim completed: left={}, right={}, top={}, bottom={}",
                trim_left, trim_right, trim_top, trim_bottom
            );
        }
        return Ok(());
    }

    // Handle loop finding early
    if args.find_loop {
        let input_path = match &args.input {
            Some(p) => p.clone(),
            None => {
                return Err(anyhow!(
                    "Input directory must be provided when using --find-loop"
                ))
            }
        };
        if !input_path.is_dir() {
            return Err(anyhow!(
                "--find-loop expects a directory containing frame_*.txt files"
            ));
        }
        run_find_loop(&input_path)?;
        return Ok(());
    }

    // --- Interactive Prompts ---
    if args.input.is_none() {
        if !is_interactive {
            return Err(anyhow!("Input file must be provided when using a preset."));
        }
        let files = find_media_files()?;
        if files.is_empty() {
            return Err(anyhow!("No media files found in current directory."));
        }
        let selection = FuzzySelect::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt("Choose an input file")
            .default(0)
            .items(&files)
            .interact()?;
        args.input = Some(PathBuf::from(&files[selection]));
    }

    let input_path = args.input.as_ref().unwrap();

    let is_image_input = input_path.is_file()
        && matches!(
            input_path.extension().and_then(|s| s.to_str()),
            Some("png" | "jpg" | "jpeg")
        );

    if preprocess_filter.is_some() && input_path.is_dir() && args.preprocess_output.is_none() {
        return Err(anyhow!(
            "Preprocessing a directory requires --preprocess-output to specify where preprocessed images are written"
        ));
    }

    // Handle directory preprocessing early and exit
    if let Some(ref filter) = preprocess_filter {
        if input_path.is_dir() {
            let output_dir = args.preprocess_output.as_ref().unwrap();
            let converter = AsciiConverter::with_config(load_config()?)?;
            let count = preprocess_directory(input_path, filter, output_dir, converter.ffmpeg_config())?;
            println!(
                "Preprocessing completed: {} images written to {}",
                count,
                output_dir.display(),
            );
            return Ok(());
        }
    }

    // Compute output path for --to-video (video file) or normal mode (directory)
    let video_output_path = if args.to_video {
        if let Some(ref out) = args.out {
            let mut p = out.clone();
            // Ensure it has .mp4 extension
            if p.extension().map(|e| e != "mp4").unwrap_or(true) {
                p.set_extension("mp4");
            }
            p
        } else {
            let stem = input_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("output");
            PathBuf::from(format!("{}_ascii.mp4", stem))
        }
    } else {
        PathBuf::new() // unused in non-to-video mode
    };

    let mut output_path = args.out.clone().unwrap_or_else(|| PathBuf::from("."));

    // If input is a file and not --to-video mode, create a directory for the output
    if input_path.is_file() && !args.to_video {
        let file_stem = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("cascii_output");
        output_path.push(file_stem);
    }

    // Load config and decide preset
    let cfg = load_config()?;
    let converter = AsciiConverter::with_config(cfg.clone())?;

    let active_preset_name = if args.small {
        "small"
    } else if args.large {
        "large"
    } else if args.default {
        cfg.default_preset.as_str()
    } else {
        // interactive default uses the configured default preset
        cfg.default_preset.as_str()
    };

    let active = cfg
        .presets
        .get(active_preset_name)
        .ok_or_else(|| anyhow!(format!("Missing preset '{}' in config", active_preset_name)))?;
    let default_cols = active.columns;
    let default_fps = active.fps;
    let default_ratio = active.font_ratio;

    if is_interactive {
        if args.columns.is_none() {
            args.columns = Some(Input::new().with_prompt("Columns (width)").default(default_cols).interact()?);
        }

        if args.font_ratio.is_none() {
            args.font_ratio = Some(Input::new().with_prompt("Font Ratio").default(default_ratio).interact()?);
        }

        if args.luminance.is_none() {
            args.luminance = Some(Input::new().with_prompt("Luminance threshold").default(20u8).interact()?);
        }

        if !is_image_input {
            // Video-specific prompts
            if args.fps.is_none() {
                args.fps = Some(Input::new().with_prompt("Frames per second (FPS)").default(default_fps).interact()?);
            }
            if args.start.is_none() {
                args.start = Some(Input::new().with_prompt("Start time (e.g., 00:00:05)").default(cfg.default_start.clone()).interact()?);
            }
            if args.end.is_none() {
                args.end = Some(Input::new().with_prompt("End time (e.g., 00:00:10) (optional)").default(cfg.default_end.clone()).interact()?);
            }
        }
    }

    let columns = args.columns.unwrap_or(default_cols);
    let fps = args.fps.unwrap_or(default_fps);
    let font_ratio = args.font_ratio.unwrap_or(default_ratio);
    let luminance = args.luminance.unwrap_or(active.luminance);

    // --- Execution ---
    if !args.to_video {
        fs::create_dir_all(&output_path).context("creating output dir")?;

        // Check if output directory already contains frames.
        let has_frames = WalkDir::new(&output_path)
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(Result::ok)
            .any(|e| {e.file_name().to_str().is_some_and(|s| s.starts_with("frame_"))});

        if has_frames {
            if is_interactive && !Confirm::new().with_prompt(format!("Output directory {} already contains frames. Overwrite?", output_path.display())).default(false).interact()? {
                println!("Operation cancelled.");
                return Ok(());
            }

            // Clean up existing frames
            for entry in fs::read_dir(&output_path)? {
                let entry = entry?;
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    if name.starts_with("frame_") && (name.ends_with(".png") || name.ends_with(".txt") || name.ends_with(".cframe") || name.ends_with(".colors")) {
                        fs::remove_file(path)?;
                    }
                }
            }
        }
    }

    // Determine output mode
    let output_mode = if args.color_only {
        OutputMode::ColorOnly
    } else if args.colors {
        OutputMode::TextAndColor
    } else {
        OutputMode::TextOnly
    };

    // Create conversion options
    let conv_opts = ConversionOptions {columns: Some(columns), font_ratio, luminance, ascii_chars: cfg.ascii_chars.clone(), output_mode: output_mode.clone()};

    if input_path.is_file() {
        if is_image_input {
            println!("Converting image to ASCII...");
            let preprocessed_image = if let Some(filter) = preprocess_filter.as_deref() {
                println!("Applying preprocessing filter before ASCII conversion...");
                Some(preprocess_image_to_temp(input_path, filter, converter.ffmpeg_config())?)
            } else {
                None
            };
            let image_input = preprocessed_image.as_ref().map_or(input_path.as_path(), |f| f.path());
            converter.convert_image(image_input, &output_path.join(format!("{}.txt", input_path.file_stem().unwrap().to_str().unwrap())), &conv_opts)?;
        } else if args.to_video {
            let video_opts = VideoOptions {fps, start: args.start.clone(), end: args.end.clone(), columns, extract_audio: args.audio, preprocess_filter: preprocess_filter.clone()};
            let to_video_opts = ToVideoOptions {output_path: video_output_path.clone(), font_size: args.video_font_size, crf: args.crf, mux_audio: args.audio, use_colors: None};

            // Create progress bar for multi-phase progress
            let progress_bar: Arc<Mutex<Option<ProgressBar>>> = Arc::new(Mutex::new(None));
            let spinner: Arc<Mutex<Option<ProgressBar>>> = Arc::new(Mutex::new(None));
            let pb_clone = Arc::clone(&progress_bar);
            let spinner_clone = Arc::clone(&spinner);

            converter.convert_video_to_video(
                input_path,
                &video_opts,
                &conv_opts,
                &to_video_opts,
                move |progress: Progress| {
                    match progress.phase {
                        ProgressPhase::ExtractingFrames => {
                            let mut sp_guard = spinner_clone.lock().unwrap();
                            if sp_guard.is_none() {
                                let sp = ProgressBar::new_spinner();
                                sp.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg}").unwrap());
                                sp.set_message("Extracting frames from video...");
                                sp.enable_steady_tick(std::time::Duration::from_millis(100));
                                *sp_guard = Some(sp);
                            }
                        }
                        ProgressPhase::ExtractingAudio => {
                            let mut sp_guard = spinner_clone.lock().unwrap();
                            if let Some(sp) = sp_guard.take() {
                                sp.finish_with_message("Frames extracted");
                            }
                            let sp = ProgressBar::new_spinner();
                            sp.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg}").unwrap());
                            sp.set_message("Extracting audio...");
                            sp.enable_steady_tick(std::time::Duration::from_millis(100));
                            *sp_guard = Some(sp);
                        }
                        ProgressPhase::RenderingVideo => {
                            // Finish spinner, switch to progress bar
                            let mut sp_guard = spinner_clone.lock().unwrap();
                            if let Some(sp) = sp_guard.take() {
                                sp.finish_with_message("Extraction complete");
                            }
                            drop(sp_guard);

                            let mut pb_guard = pb_clone.lock().unwrap();
                            if pb_guard.is_none() && progress.total > 0 {
                                let pb = ProgressBar::new(progress.total as u64);
                                pb.set_style(ProgressStyle::default_bar().template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({percent}%)").unwrap().progress_chars("#>-"));
                                pb.set_message("Rendering video");
                                *pb_guard = Some(pb);
                            }
                            if let Some(ref pb) = *pb_guard {
                                pb.set_position(progress.completed as u64);
                            }
                        }
                        ProgressPhase::ConvertingFrames | ProgressPhase::Complete => {}
                    }
                },
            )?;

            let pb_opt = progress_bar.lock().unwrap().take();
            if let Some(pb) = pb_opt {
                pb.finish_with_message("Done");
            }

            println!("\nASCII video saved to {}", video_output_path.display());
            return Ok(());
        } else {
            let video_opts = VideoOptions {fps, start: args.start.clone(), end: args.end.clone(), columns, extract_audio: args.audio, preprocess_filter: preprocess_filter.clone()};
            // Create progress bar for multi-phase progress
            let progress_bar: Arc<Mutex<Option<ProgressBar>>> = Arc::new(Mutex::new(None));
            let spinner: Arc<Mutex<Option<ProgressBar>>> = Arc::new(Mutex::new(None));
            let pb_clone = Arc::clone(&progress_bar);
            let spinner_clone = Arc::clone(&spinner);

            converter.convert_video_with_detailed_progress(
                input_path,
                &output_path,
                &video_opts,
                &conv_opts,
                args.keep_images,
                move |progress: Progress| {
                    match progress.phase {
                        ProgressPhase::ExtractingFrames => {
                            // Show spinner for indeterminate extraction phase
                            let mut sp_guard = spinner_clone.lock().unwrap();
                            if sp_guard.is_none() {
                                let sp = ProgressBar::new_spinner();
                                sp.set_style(
                                    ProgressStyle::default_spinner()
                                        .template("{spinner:.green} {msg}")
                                        .unwrap()
                                );
                                sp.set_message("Extracting frames from video...");
                                sp.enable_steady_tick(std::time::Duration::from_millis(100));
                                *sp_guard = Some(sp);
                            }
                        }
                        ProgressPhase::ExtractingAudio => {
                            // Finish spinner if running, show audio extraction
                            let mut sp_guard = spinner_clone.lock().unwrap();
                            if let Some(sp) = sp_guard.take() {
                                sp.finish_with_message("Frames extracted");
                            }
                            let sp = ProgressBar::new_spinner();
                            sp.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg}").unwrap());
                            sp.set_message("Extracting audio...");
                            sp.enable_steady_tick(std::time::Duration::from_millis(100));
                            *sp_guard = Some(sp);
                        }
                        ProgressPhase::ConvertingFrames => {
                            // Finish spinner, switch to progress bar
                            let mut sp_guard = spinner_clone.lock().unwrap();
                            if let Some(sp) = sp_guard.take() {
                                sp.finish_with_message("Extraction complete");
                            }
                            drop(sp_guard);

                            let mut pb_guard = pb_clone.lock().unwrap();
                            if pb_guard.is_none() && progress.total > 0 {
                                // Initialize progress bar on first conversion callback
                                let pb = ProgressBar::new(progress.total as u64);
                                pb.set_style(
                                    ProgressStyle::default_bar()
                                        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({percent}%)")
                                        .unwrap()
                                        .progress_chars("#>-"),
                                );
                                pb.set_message("Converting frames");
                                *pb_guard = Some(pb);
                            }
                            if let Some(ref pb) = *pb_guard {
                                pb.set_position(progress.completed as u64);
                            }
                        }
                        ProgressPhase::RenderingVideo | ProgressPhase::Complete => {
                            // Not used in non-to-video mode
                        }
                    }
                },
            )?;

            // Finish the progress bar
            let pb_opt = progress_bar.lock().unwrap().take();
            if let Some(pb) = pb_opt {
                pb.finish_with_message("Done");
            }
        }
    } else if input_path.is_dir() {
        if args.to_video {
            let to_video_opts = ToVideoOptions {output_path: video_output_path.clone(), font_size: args.video_font_size, crf: args.crf, mux_audio: args.audio, use_colors: None};
            let progress_bar: Arc<Mutex<Option<ProgressBar>>> = Arc::new(Mutex::new(None));
            let pb_clone = Arc::clone(&progress_bar);

            converter.render_frames_to_video(
                input_path,
                fps,
                &to_video_opts,
                move |progress: Progress| {
                    if progress.phase == ProgressPhase::RenderingVideo {
                        let mut pb_guard = pb_clone.lock().unwrap();
                        if pb_guard.is_none() && progress.total > 0 {
                            let pb = ProgressBar::new(progress.total as u64);
                            pb.set_style(ProgressStyle::default_bar().template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({percent}%)").unwrap().progress_chars("#>-"));
                            pb.set_message("Rendering video");
                            *pb_guard = Some(pb);
                        }
                        if let Some(ref pb) = *pb_guard {
                            pb.set_position(progress.completed as u64);
                        }
                    }
                },
            )?;

            let pb_opt = progress_bar.lock().unwrap().take();
            if let Some(pb) = pb_opt {
                pb.finish_with_message("Done");
            }

            println!("\nASCII video saved to {}", video_output_path.display());
            return Ok(());
        } else {
            println!("Converting directory of images...");
            converter.convert_directory(input_path, &output_path, &conv_opts, args.keep_images)?;

            // For directory conversion, create details.toml manually since it doesn't go through video conversion
            let frame_ext = if output_mode == OutputMode::ColorOnly {
                "cframe"
            } else {
                "txt"
            };
            let frame_count = WalkDir::new(&output_path)
                .min_depth(1)
                .max_depth(1)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == frame_ext))
                .count();

            let mode_str = match output_mode {
                OutputMode::TextOnly        => "text-only",
                OutputMode::ColorOnly       => "color-only",
                OutputMode::TextAndColor    => "text+color",
            };

            let result = cascii::ConversionResult {
                frame_count,
                columns,
                font_ratio,
                luminance,
                fps: None,
                output_mode: mode_str.to_string(),
                audio_extracted: false,
                output_dir: output_path.clone(),
                background_color: "black".to_string(),
                color: "white".to_string(),
            };

            result
                .write_details_file()
                .context("writing details file")?;
            let details = result.to_details_string();

            if args.log_details {
                println!("\n--- Generation Details ---");
                println!("{}", details);
            }
        }
    } else {
        return Err(anyhow!("Input path does not exist"));
    }

    println!("\nASCII generation complete in {}", output_path.display());

    Ok(())
}

fn find_media_files() -> Result<Vec<String>> {
    Ok(WalkDir::new(".")
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {e.path().is_file() && e.path().extension().is_some_and(|ext| {matches!(ext.to_str(), Some("mp4" | "mkv" | "mov" | "avi" | "webm" | "png" | "jpg"))})})
        .map(|e| e.path().to_str().unwrap_or("").to_string())
        .collect())
}

fn run_uninstall(is_interactive: bool) -> Result<()> {
    let bin_paths = vec!["/usr/local/bin/cascii", "/usr/local/bin/casci"]; // legacy symlink
    let app_support = dirs::data_dir()
        .unwrap_or_else(|| {PathBuf::from(format!("{}/Library/Application Support", std::env::var("HOME").unwrap_or_default()))})
        .join("cascii");

    if is_interactive {
        let confirmed = Confirm::new()
            .with_prompt("This will remove cascii and its app support directory. Continue?")
            .default(false)
            .interact()?;
        if !confirmed {
            println!("Uninstall cancelled.");
            return Ok(());
        }
    }

    for p in bin_paths {
        let path = Path::new(p);
        if path.exists() {
            if let Err(e) = fs::remove_file(path) {
                eprintln!("Warning: failed to remove {}: {}", p, e);
            }
        }
    }

    if app_support.exists() {
        if let Err(e) = fs::remove_dir_all(&app_support) {
            eprintln!("Warning: failed to remove app support directory {}: {}", app_support.display(), e);
        }
    }

    Ok(())
}
