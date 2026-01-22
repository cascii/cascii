use anyhow::{anyhow, Context, Result};
use cascii::{AppConfig, AsciiConverter, ConversionOptions, VideoOptions};
use clap::{Parser, Subcommand};
use dialoguer::{Confirm, FuzzySelect, Input, Select};
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
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

    /// Extract colors to CSV files alongside ASCII output
    #[arg(long, default_value_t = false)]
    colors: bool,

    /// Start time for video conversion (e.g., 00:01:23.456 or 83.456)
    #[arg(long)]
    start: Option<String>,

    /// End time for video conversion (e.g., 00:01:23.456 or 83.456)
    #[arg(long)]
    end: Option<String>,

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
        run_trim(&input_path, trim_left, trim_right, trim_top, trim_bottom)?;
        println!(
            "Trim completed: left={}, right={}, top={}, bottom={}",
            trim_left, trim_right, trim_top, trim_bottom
        );
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

    let mut output_path = args.out.unwrap_or_else(|| PathBuf::from("."));

    // If input is a file, create a directory for the output
    if input_path.is_file() {
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
            args.columns = Some(
                Input::new()
                    .with_prompt("Columns (width)")
                    .default(default_cols)
                    .interact()?,
            );
        }

        if args.font_ratio.is_none() {
            args.font_ratio = Some(
                Input::new()
                    .with_prompt("Font Ratio")
                    .default(default_ratio)
                    .interact()?,
            );
        }

        if args.luminance.is_none() {
            args.luminance = Some(
                Input::new()
                    .with_prompt("Luminance threshold")
                    .default(20u8)
                    .interact()?,
            );
        }

        if !is_image_input {
            // Video-specific prompts
            if args.fps.is_none() {
                args.fps = Some(
                    Input::new()
                        .with_prompt("Frames per second (FPS)")
                        .default(default_fps)
                        .interact()?,
                );
            }
            if args.start.is_none() {
                args.start = Some(
                    Input::new()
                        .with_prompt("Start time (e.g., 00:00:05)")
                        .default(cfg.default_start.clone())
                        .interact()?,
                );
            }
            if args.end.is_none() {
                args.end = Some(
                    Input::new()
                        .with_prompt("End time (e.g., 00:00:10) (optional)")
                        .default(cfg.default_end.clone())
                        .interact()?,
                );
            }
        }
    }

    let columns = args.columns.unwrap_or(default_cols);
    let fps = args.fps.unwrap_or(default_fps);
    let font_ratio = args.font_ratio.unwrap_or(default_ratio);
    let luminance = args.luminance.unwrap_or(active.luminance);

    // --- Execution ---
    fs::create_dir_all(&output_path).context("creating output dir")?;

    // Check if output directory already contains frames.
    let has_frames = WalkDir::new(&output_path)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(Result::ok)
        .any(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|s| s.starts_with("frame_"))
        });

    if has_frames {
        if is_interactive
            && !Confirm::new()
                .with_prompt(format!(
                    "Output directory {} already contains frames. Overwrite?",
                    output_path.display()
                ))
                .default(false)
                .interact()?
        {
            println!("Operation cancelled.");
            return Ok(());
        }

        // Clean up existing frames
        for entry in fs::read_dir(&output_path)? {
            let entry = entry?;
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if name.starts_with("frame_") && (name.ends_with(".png") || name.ends_with(".txt"))
                {
                    fs::remove_file(path)?;
                }
            }
        }
    }

    // Create conversion options
    let conv_opts = ConversionOptions {
        columns: Some(columns),
        font_ratio,
        luminance,
        ascii_chars: cfg.ascii_chars.clone(),
        extract_colors: args.colors,
    };

    if input_path.is_file() {
        if is_image_input {
            println!("Converting image to ASCII...");
            converter.convert_image(
                input_path,
                &output_path.join(format!(
                    "{}.txt",
                    input_path.file_stem().unwrap().to_str().unwrap()
                )),
                &conv_opts,
            )?;
        } else {
            println!("Extracting video frames...");
            let video_opts = VideoOptions {
                fps,
                start: args.start.clone(),
                end: args.end.clone(),
                columns,
            };

            // Create progress bar (will be initialized once we know total frames)
            let progress_bar: Arc<Mutex<Option<ProgressBar>>> = Arc::new(Mutex::new(None));
            let pb_clone = Arc::clone(&progress_bar);

            converter.convert_video_with_progress(
                input_path,
                &output_path,
                &video_opts,
                &conv_opts,
                args.keep_images,
                Some(move |completed: usize, total: usize| {
                    let mut pb_guard = pb_clone.lock().unwrap();
                    if pb_guard.is_none() {
                        // Initialize progress bar on first callback
                        let pb = ProgressBar::new(total as u64);
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
                        pb.set_position(completed as u64);
                    }
                }),
            )?;

            // Finish the progress bar
            let pb_opt = progress_bar.lock().unwrap().take();
            if let Some(pb) = pb_opt {
                pb.finish_with_message("Done");
            }
        }
    } else if input_path.is_dir() {
        println!("Converting directory of images...");
        converter.convert_directory(input_path, &output_path, &conv_opts, args.keep_images)?;
    } else {
        return Err(anyhow!("Input path does not exist"));
    }

    println!("\nASCII generation complete in {}", output_path.display());

    // --- Create details.txt ---
    let frame_count = WalkDir::new(&output_path)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "txt"))
        .count();

    let mut details = format!(
        "Version: {}\nFrames: {}\nLuminance: {}\nFont Ratio: {}\nColumns: {}",
        env!("CARGO_PKG_VERSION"),
        frame_count,
        luminance,
        font_ratio,
        columns
    );

    if input_path.is_file() && !is_image_input {
        details.push_str(&format!("\nFPS: {}", fps));
    }

    let details_path = output_path.join("details.md");
    fs::write(details_path, &details).context("writing details file")?;

    if args.log_details {
        println!("\n--- Generation Details ---");
        println!("{}", details);
    }

    Ok(())
}

fn find_media_files() -> Result<Vec<String>> {
    Ok(WalkDir::new(".")
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().is_file()
                && e.path().extension().is_some_and(|ext| {
                    matches!(
                        ext.to_str(),
                        Some("mp4" | "mkv" | "mov" | "avi" | "webm" | "png" | "jpg")
                    )
                })
        })
        .map(|e| e.path().to_str().unwrap_or("").to_string())
        .collect())
}

fn run_uninstall(is_interactive: bool) -> Result<()> {
    let bin_paths = vec!["/usr/local/bin/cascii", "/usr/local/bin/casci"]; // legacy symlink
    let app_support = dirs::data_dir()
        .unwrap_or_else(|| {
            PathBuf::from(format!(
                "{}/Library/Application Support",
                std::env::var("HOME").unwrap_or_default()
            ))
        })
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
            eprintln!(
                "Warning: failed to remove app support directory {}: {}",
                app_support.display(),
                e
            );
        }
    }

    Ok(())
}

fn run_trim(
    path: &Path,
    trim_left: usize,
    trim_right: usize,
    trim_top: usize,
    trim_bottom: usize,
) -> Result<()> {
    if path.is_file() {
        trim_file(path, trim_left, trim_right, trim_top, trim_bottom)?;
    } else if path.is_dir() {
        // Find all frame_*.txt recursively and process them
        for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
            let p = entry.path();
            if p.is_file() {
                if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                    if name.starts_with("frame_") && name.ends_with(".txt") {
                        trim_file(p, trim_left, trim_right, trim_top, trim_bottom)?;
                    }
                }
            }
        }
    } else {
        return Err(anyhow!("Path does not exist: {}", path.display()));
    }
    Ok(())
}

fn trim_file(
    path: &Path,
    trim_left: usize,
    trim_right: usize,
    trim_top: usize,
    trim_bottom: usize,
) -> Result<()> {
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    if lines.is_empty() {
        return Err(anyhow!("Cannot trim empty file: {}", path.display()));
    }

    let height = lines.len();
    let width = lines[0].chars().count();

    // Validate rectangular and strip potential trailing \r
    for (idx, line) in lines.iter().enumerate() {
        if line.chars().count() != width {
            return Err(anyhow!(
                "Non-rectangular frame at {} line {}",
                path.display(),
                idx + 1
            ));
        }
    }

    if trim_top + trim_bottom >= height {
        return Err(anyhow!(
            "Trim rows exceed or equal file height ({} >= {}) for {}",
            trim_top + trim_bottom,
            height,
            path.display()
        ));
    }
    if trim_left + trim_right >= width {
        return Err(anyhow!(
            "Trim columns exceed or equal file width ({} >= {}) for {}",
            trim_left + trim_right,
            width,
            path.display()
        ));
    }

    // Apply vertical trims
    let start_row = trim_top;
    let end_row_exclusive = height - trim_bottom;
    let mut trimmed: Vec<String> = Vec::with_capacity(end_row_exclusive - start_row);

    for line in lines.iter().take(end_row_exclusive).skip(start_row) {
        // Apply horizontal trims using char indices (to handle unicode safely)
        let left = trim_left;
        let right = trim_right;
        let take_len = width - left - right;
        let slice: String = line.chars().skip(left).take(take_len).collect();
        trimmed.push(slice);
    }

    let new_content = trimmed.join("\n") + "\n";
    fs::write(path, new_content).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn run_find_loop(dir: &Path) -> Result<()> {
    // Load frames in order
    let mut frames: Vec<(usize, String)> = Vec::new();
    let mut entries: Vec<PathBuf> = WalkDir::new(dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|p| p.extension().map(|e| e == "txt").unwrap_or(false))
        .collect();
    entries.sort();

    for p in entries {
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if !name.starts_with("frame_") {
            continue;
        }
        // parse frame number
        let num = name
            .trim_start_matches("frame_")
            .trim_end_matches(".txt")
            .parse::<usize>()
            .unwrap_or(frames.len());
        let content = fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        frames.push((num, content));
    }
    if frames.is_empty() {
        return Err(anyhow!("No frame_*.txt files found in {}", dir.display()));
    }
    frames.sort_by_key(|(n, _)| *n);

    // Hash frames and map to indices
    use std::collections::hash_map::DefaultHasher;
    let mut hash_to_indices: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut repeated_hashes: Vec<u64> = Vec::new();

    for (idx, (_, content)) in frames.iter().enumerate() {
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        let h = hasher.finish();
        let entry = hash_to_indices.entry(h).or_default();
        entry.push(idx);
        if entry.len() == 2 {
            // first time we see a repeat
            repeated_hashes.push(h);
        }
    }

    if repeated_hashes.is_empty() {
        println!("No repeated frames detected.");
        return Ok(());
    }

    // Build candidate loops: for each repeated hash, all non-adjacent pairs between occurrences
    // Ignore immediate number neighbors (e.g., frame N and frame N+1)
    let mut loops: Vec<(usize, usize)> = Vec::new();
    for h in &repeated_hashes {
        if let Some(indices) = hash_to_indices.get(h) {
            let n = indices.len();
            for a in 0..n.saturating_sub(1) {
                for b in (a + 1)..n {
                    let s = indices[a];
                    let e = indices[b];
                    let fn_start = frames[s].0;
                    let fn_end = frames[e].0;
                    if fn_end > fn_start + 1 {
                        // exclude immediate neighbors
                        loops.push((s, e));
                    }
                }
            }
        }
    }
    // Deduplicate loops
    loops.sort();
    loops.dedup();

    if loops.is_empty() {
        println!("No loopable segments detected.");
        return Ok(());
    }

    println!("Found loops:");
    for (i, (s, e)) in loops.iter().enumerate() {
        println!(
            "{}: frames {}..{} (inclusive start, exclusive end)",
            i + 1,
            frames[*s].0,
            frames[*e].0
        );
    }

    // Interactive menu
    loop {
        let choices = vec!["Export loop", "Repeat loop", "Quit"];
        let sel = Select::new()
            .with_prompt("Choose an action")
            .default(0)
            .items(&choices)
            .interact()?;
        match sel {
            0 => {
                // Export
                let labels: Vec<String> = loops
                    .iter()
                    .map(|(s, e)| format!("{}..{}", frames[*s].0, frames[*e].0))
                    .collect();
                let idx = Select::new()
                    .with_prompt("Select loop to export")
                    .default(0)
                    .items(&labels)
                    .interact()?;
                let (s, e) = loops[idx];
                export_loop(dir, &frames, s, e)?;
                println!("Exported loop {}..{}", frames[s].0, frames[e].0);
            }
            1 => {
                // Repeat
                let labels: Vec<String> = loops
                    .iter()
                    .map(|(s, e)| format!("{}..{}", frames[*s].0, frames[*e].0))
                    .collect();
                let idx = Select::new()
                    .with_prompt("Select loop to repeat")
                    .default(0)
                    .items(&labels)
                    .interact()?;
                let (s, e) = loops[idx];
                repeat_loop(dir, &frames, s, e)?;
                println!("Loop repeated");
            }
            _ => break,
        }
    }

    Ok(())
}

fn export_loop(
    dir: &Path,
    frames: &[(usize, String)],
    start_idx: usize,
    end_idx: usize,
) -> Result<()> {
    let start_frame = frames[start_idx].0;
    let end_frame = frames[end_idx].0;
    let out = dir.with_file_name(format!(
        "{}_loop_{}_{}",
        dir.file_name().and_then(|s| s.to_str()).unwrap_or("frames"),
        start_frame,
        end_frame
    ));
    fs::create_dir_all(&out)?;
    let mut counter: usize = 1;
    for frame in frames.iter().take(end_idx + 1).skip(start_idx) {
        // inclusive both ends as per example ABCD A
        let filename = out.join(format!("frame_{:04}.txt", counter));
        fs::write(filename, &frame.1)?;
        counter += 1;
    }
    Ok(())
}

fn repeat_loop(
    dir: &Path,
    frames: &[(usize, String)],
    start_idx: usize,
    end_idx: usize,
) -> Result<()> {
    // Reinsert the selected loop immediately after the end index
    // We will renumber and rewrite all frames to the same directory
    let mut new_seq: Vec<String> = Vec::with_capacity(frames.len() + (end_idx - start_idx + 1));
    for (_, content) in frames.iter().take(end_idx + 1) {
        new_seq.push(content.clone());
    }
    for frame in frames.iter().take(end_idx + 1).skip(start_idx) {
        new_seq.push(frame.1.clone());
    }
    for (_, content) in frames.iter().skip(end_idx + 1) {
        new_seq.push(content.clone());
    }

    // Write back with new numbering
    // First, remove existing frame_*.txt
    for entry in WalkDir::new(dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = entry.path().to_path_buf();
        if p.is_file() {
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if name.starts_with("frame_") && name.ends_with(".txt") {
                    let _ = fs::remove_file(p);
                }
            }
        }
    }
    for (i, content) in new_seq.iter().enumerate() {
        let filename = dir.join(format!("frame_{:04}.txt", i + 1));
        fs::write(filename, content)?;
    }
    Ok(())
}
