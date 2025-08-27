use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use dialoguer::{Confirm, FuzzySelect, Input, Select};
use indicatif::{ParallelProgressIterator, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;
use std::collections::{HashMap};
use walkdir::WalkDir;

/// Characters from darkest to lightest.
const ASCII_CHARS: &str = " .`'^,:;Il!i><~+_-?][}{1)(|/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$";

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
            None => return Err(anyhow!("Input directory must be provided when using --find-loop")),
        };
        if !input_path.is_dir() {
            return Err(anyhow!("--find-loop expects a directory containing frame_*.txt files"));
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

    // Quality defaults based on flags
    let (default_cols, default_fps, default_ratio) = if args.small {
        (80, 24, 0.44)
    } else if args.large {
        (800, 60, 0.7)
    } else if args.default {
        (200, 24, 0.5)
    } else {
        (800, 30, 0.7)
    };

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
                    .default(1u8)
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
                        .default("0".to_string())
                        .interact()?,
                );
            }
            if args.end.is_none() {
                args.end = Some(
                    Input::new()
                        .with_prompt("End time (e.g., 00:00:10) (optional)")
                        .interact()?,
                );
            }
        }
    }

    let columns = args.columns.unwrap_or(default_cols);
    let fps = args.fps.unwrap_or(default_fps);
    let font_ratio = args.font_ratio.unwrap_or(default_ratio);
    let luminance = args.luminance.unwrap_or(1);

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
                .map_or(false, |s| s.starts_with("frame_"))
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
                if name.starts_with("frame_") && (name.ends_with(".png") || name.ends_with(".txt")) {
                    fs::remove_file(path)?;
                }
            }
        }
    }

    if input_path.is_file() {
        if is_image_input {
            return process_single_image(
                &input_path,
                &output_path,
                columns,
                font_ratio,
                luminance,
                args.log_details,
            );
        }

        run_ffmpeg_extract(
            &input_path,
            &output_path,
            columns,
            fps,
            args.start.as_deref(),
            args.end.as_deref(),
        )?;
        convert_dir_pngs_parallel(
            &output_path,
            &output_path,
            font_ratio,
            luminance,
            args.keep_images,
        )?;
    } else if input_path.is_dir() {
        convert_dir_pngs_parallel(
            &input_path,
            &output_path,
            font_ratio,
            luminance,
            args.keep_images,
        )?;
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
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "txt"))
        .count();

    let mut details = format!(
        "Frames: {}\nLuminance: {}\nFont Ratio: {}\nColumns: {}",
        frame_count, luminance, font_ratio, columns
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

fn process_single_image(
    input_path: &Path,
    output_path: &Path,
    columns: u32,
    font_ratio: f32,
    luminance: u8,
    log_details: bool,
) -> Result<()> {
    let file_stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("ascii_art");
    let out_txt = output_path.join(format!("{}.txt", file_stem));

    println!("Converting image to ASCII...");
    convert_image_to_ascii(
        input_path,
        &out_txt,
        font_ratio,
        luminance,
        Some(columns),
    )?;

    println!("\nASCII generation complete in {}", output_path.display());

    let details = format!(
        "Luminance: {}\nFont Ratio: {}\nColumns: {}",
        luminance, font_ratio, columns
    );
    let details_path = output_path.join("details.md");
    fs::write(details_path, &details).context("writing details file")?;

    if log_details {
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
                && e.path()
                    .extension()
                    .map_or(false, |ext| matches!(ext.to_str(), Some("mp4" | "mkv" | "mov" | "avi" | "webm" | "png" | "jpg")))
        })
        .map(|e| e.path().to_str().unwrap_or("").to_string())
        .collect())
}

fn run_ffmpeg_extract(
    input: &Path,
    out_dir: &Path,
    columns: u32,
    fps: u32,
    start: Option<&str>,
    end: Option<&str>,
) -> Result<()> {
    println!("Extracting frames with ffmpeg...");
    let out_pattern = out_dir.join("frame_%04d.png");
    let mut ffmpeg_args: Vec<String> = vec![
        "-loglevel".into(),
        "error".into(),
    ];

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
            // If start time is also specified, calculate duration
            if let Some(s) = start {
                if !s.is_empty() && s != "0" {
                    // This is a simplistic duration calculation. Assumes HH:MM:SS or seconds format.
                    // For more robust parsing, a dedicated time parsing library would be better.
                    let start_secs = s.split(':').rev().enumerate().fold(0.0, |acc, (i, v)| {
                        acc + v.parse::<f64>().unwrap_or(0.0) * 60f64.powi(i as i32)
                    });
                    let end_secs = e.split(':').rev().enumerate().fold(0.0, |acc, (i, v)| {
                        acc + v.parse::<f64>().unwrap_or(0.0) * 60f64.powi(i as i32)
                    });
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

fn convert_dir_pngs_parallel(src_dir: &Path, dst_dir: &Path, font_ratio: f32, threshold: u8, keep_images: bool) -> Result<()> {
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

    println!("Converting {} images to ASCII...", pngs.len());
    let pb = ProgressBar::new(pngs.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("##-"),
    );

    pngs.par_iter()
        .progress_with(pb)
        .try_for_each(|img_path| -> Result<()> {
            let file_stem = img_path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow!("bad file name"))?;
            let out_txt = dst_dir.join(format!("{}.txt", file_stem));
            convert_image_to_ascii(img_path, &out_txt, font_ratio, threshold, None)
        })?;

    if !keep_images {
        for img_path in &pngs {
            fs::remove_file(img_path)?;
        }
    }

    Ok(())
}

fn convert_image_to_ascii(
    img_path: &Path,
    out_txt: &Path,
    font_ratio: f32,
    threshold: u8,
    columns: Option<u32>,
) -> Result<()> {
    let mut img = image::open(img_path)
        .with_context(|| format!("opening {}", img_path.display()))?
        .to_rgb8();

    if let Some(new_w) = columns {
        let (w, h) = img.dimensions();
        if new_w != w {
            let new_h = (h as f32 * (new_w as f32 / w as f32)).round() as u32;
            img = image::imageops::resize(&img, new_w, new_h, image::imageops::FilterType::Triangle);
        }
    }

    let (w, h) = img.dimensions();
    let new_h = ((h as f32) * font_ratio).max(1.0).round() as u32;
    if new_h != h {
        img = image::imageops::resize(&img, w, new_h, image::imageops::FilterType::Triangle);
    }

    let mut out = String::with_capacity((w as usize + 1) * (new_h as usize));
    for y in 0..new_h {
        for x in 0..w {
            let px = img.get_pixel(x, y);
            let l = luminance(*px);
            out.push(char_for(l, threshold));
        }
        out.push('\n');
    }
    fs::write(out_txt, out).with_context(|| format!("writing {}", out_txt.display()))?;
    Ok(())
}

fn luminance(rgb: image::Rgb<u8>) -> u8 {
    let r = rgb[0] as f32;
    let g = rgb[1] as f32;
    let b = rgb[2] as f32;
    (0.2126 * r + 0.7152 * g + 0.0722 * b).round() as u8
}

fn char_for(luma: u8, threshold: u8) -> char {
    if luma < threshold {
        return ' ';
    }
    let chars = ASCII_CHARS.as_bytes();
    let idx = (((luma.saturating_sub(threshold)) as f32 / (255u16.saturating_sub(threshold as u16) as f32))
        * ((chars.len() - 1) as f32))
        .clamp(0.0, (chars.len() - 1) as f32)
        as usize;
    chars[idx] as char
}

fn run_uninstall(is_interactive: bool) -> Result<()> {
    let bin_paths = vec!["/usr/local/bin/cascii", "/usr/local/bin/casci"]; // legacy symlink
    let app_support = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from(format!("{}/Library/Application Support", std::env::var("HOME").unwrap_or_default())))
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

fn run_trim(path: &Path, trim_left: usize, trim_right: usize, trim_top: usize, trim_bottom: usize) -> Result<()> {
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

fn trim_file(path: &Path, trim_left: usize, trim_right: usize, trim_top: usize, trim_bottom: usize) -> Result<()> {
    let content = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    if lines.is_empty() {
        return Err(anyhow!("Cannot trim empty file: {}", path.display()));
    }

    let height = lines.len();
    let width = lines[0].chars().count();

    // Validate rectangular and strip potential trailing \r
    for (idx, line) in lines.iter().enumerate() {
        if line.chars().count() != width {
            return Err(anyhow!("Non-rectangular frame at {} line {}", path.display(), idx + 1));
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

    for y in start_row..end_row_exclusive {
        let line = &lines[y];
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
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        if !name.starts_with("frame_") { continue; }
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
    use std::hash::{Hash, Hasher};
    let mut hash_to_indices: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut repeated_hashes: Vec<u64> = Vec::new();

    for (idx, (_, content)) in frames.iter().enumerate() {
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        let h = hasher.finish();
        let entry = hash_to_indices.entry(h).or_default();
        entry.push(idx);
        if entry.len() == 2 { // first time we see a repeat
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
                    if fn_end > fn_start + 1 { // exclude immediate neighbors
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
        println!("{}: frames {}..{} (inclusive start, exclusive end)", i + 1, frames[*s].0, frames[*e].0);
    }

    // Interactive menu
    loop {
        let choices = vec!["Export loop", "Repeat loop", "Quit"];
        let sel = Select::new().with_prompt("Choose an action").default(0).items(&choices).interact()?;
        match sel {
            0 => { // Export
                let labels: Vec<String> = loops.iter().map(|(s,e)| format!("{}..{}", frames[*s].0, frames[*e].0)).collect();
                let idx = Select::new().with_prompt("Select loop to export").default(0).items(&labels).interact()?;
                let (s, e) = loops[idx];
                export_loop(dir, &frames, s, e)?;
                println!("Exported loop {}..{}", frames[s].0, frames[e].0);
            }
            1 => { // Repeat
                let labels: Vec<String> = loops.iter().map(|(s,e)| format!("{}..{}", frames[*s].0, frames[*e].0)).collect();
                let idx = Select::new().with_prompt("Select loop to repeat").default(0).items(&labels).interact()?;
                let (s, e) = loops[idx];
                repeat_loop(dir, &frames, s, e)?;
                println!("Loop repeated");
            }
            _ => break,
        }
    }

    Ok(())
}

fn export_loop(dir: &Path, frames: &[(usize, String)], start_idx: usize, end_idx: usize) -> Result<()> {
    let start_frame = frames[start_idx].0;
    let end_frame = frames[end_idx].0;
    let out = dir.with_file_name(format!("{}_loop_{}_{}", dir.file_name().and_then(|s| s.to_str()).unwrap_or("frames"), start_frame, end_frame));
    fs::create_dir_all(&out)?;
    let mut counter: usize = 1;
    for i in start_idx..=end_idx { // inclusive both ends as per example ABCD A
        let filename = out.join(format!("frame_{:04}.txt", counter));
        fs::write(filename, &frames[i].1)?;
        counter += 1;
    }
    Ok(())
}

fn repeat_loop(dir: &Path, frames: &[(usize, String)], start_idx: usize, end_idx: usize) -> Result<()> {
    // Reinsert the selected loop immediately after the end index
    // We will renumber and rewrite all frames to the same directory
    let mut new_seq: Vec<String> = Vec::with_capacity(frames.len() + (end_idx - start_idx + 1));
    for (_, content) in frames.iter().take(end_idx + 1) { new_seq.push(content.clone()); }
    for i in start_idx..=end_idx { new_seq.push(frames[i].1.clone()); }
    for (_, content) in frames.iter().skip(end_idx + 1) { new_seq.push(content.clone()); }

    // Write back with new numbering
    // First, remove existing frame_*.txt
    for entry in WalkDir::new(dir).min_depth(1).max_depth(1).into_iter().filter_map(|e| e.ok()) {
        let p = entry.path().to_path_buf();
        if p.is_file() {
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if name.starts_with("frame_") && name.ends_with(".txt") { let _ = fs::remove_file(p); }
            }
        }
    }
    for (i, content) in new_seq.iter().enumerate() {
        let filename = dir.join(format!("frame_{:04}.txt", i + 1));
        fs::write(filename, content)?;
    }
    Ok(())
}
