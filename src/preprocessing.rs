use anyhow::{anyhow, Context, Result};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::video::parse_timestamp;
use crate::FfmpegConfig;

#[derive(Debug, Clone, Copy)]
pub struct PreprocessPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub filter: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreprocessInputKind {
    Image,
    Video,
    Directory,
}

pub const PREPROCESS_PRESETS: &[PreprocessPreset] = &[PreprocessPreset {name: "contours", description: "Grayscale edge-detection with strong contrast (good for outlines).", filter: "format=gray,edgedetect=mode=colormix:high=0.2:low=0.05,eq=contrast=2.5:brightness=-0.1"}, PreprocessPreset {name: "contours-soft", description: "Softer contour extraction with less aggressive edges.", filter: "format=gray,edgedetect=mode=colormix:high=0.12:low=0.03,eq=contrast=2.0:brightness=-0.05"}, PreprocessPreset {name: "contours-strong", description: "Very sharp contour extraction for bold linework.", filter: "format=gray,edgedetect=mode=colormix:high=0.35:low=0.08,eq=contrast=3.2:brightness=-0.12"}, PreprocessPreset {name: "bw-contrast", description: "Simple grayscale + contrast boost for clean monochrome ASCII.", filter: "format=gray,eq=contrast=2.2:brightness=-0.08"}, PreprocessPreset {name: "noir-detail", description: "Grayscale sharpened look that emphasizes texture.", filter: "format=gray,unsharp=5:5:1.0:5:5:0.0,eq=contrast=1.8:brightness=-0.04"}, PreprocessPreset {name: "vivid", description: "Boost color saturation/contrast and sharpen for colorful ASCII.", filter: "eq=saturation=1.8:contrast=1.2:brightness=0.02,unsharp=5:5:0.8:5:5:0.0"}, PreprocessPreset {name: "warm-pop", description: "Warmer color balance with moderate saturation boost.", filter: "colorbalance=rs=0.06:gs=0.02:bs=-0.04,eq=saturation=1.35:contrast=1.12"}, PreprocessPreset {name: "cool-pop", description: "Cooler color balance with moderate saturation boost.", filter: "colorbalance=rs=-0.04:gs=0.02:bs=0.07,eq=saturation=1.28:contrast=1.10"}, PreprocessPreset {name: "soft-glow", description: "Gentle blur and color lift for smoother gradients.", filter: "gblur=sigma=1.0,eq=saturation=1.15:contrast=1.08:brightness=0.02"}, PreprocessPreset {name: "bg-white", description: "Key near-white backgrounds out before conversion (best on flat white backdrops).", filter: "format=rgba,colorkey=0xFFFFFF:0.12:0.03"}, PreprocessPreset {name: "bg-black", description: "Key near-black backgrounds out before conversion (best on flat black backdrops).", filter: "format=rgba,colorkey=0x000000:0.12:0.03"}];

pub fn find_preprocess_preset(name: &str) -> Option<&'static PreprocessPreset> {
    PREPROCESS_PRESETS.iter().find(|preset| preset.name.eq_ignore_ascii_case(name))
}

pub fn resolve_preprocess_filter(preprocess: Option<&str>, preprocess_preset: Option<&str>) -> Result<Option<String>> {
    if let Some(filter) = preprocess {
        let filter = filter.trim();
        if filter.is_empty() {
            return Err(anyhow!("--preprocess cannot be empty"));
        }
        return Ok(Some(filter.to_string()));
    }

    if let Some(name) = preprocess_preset {
        let preset = find_preprocess_preset(name.trim()).ok_or_else(|| {
            let available = PREPROCESS_PRESETS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ");
            anyhow!("Unknown preprocessing preset '{}'. Available presets: {}", name, available)
        })?;
        return Ok(Some(preset.filter.to_string()));
    }

    Ok(None)
}

pub(crate) fn build_frame_extraction_vf(columns: u32, fps: u32, preprocess_filter: Option<&str>) -> String {
    let base = format!("scale={}:-2,fps={}", columns, fps);
    let preprocess = preprocess_filter.and_then(normalize_filter);
    match preprocess {
        Some(filter) => format!("{},{}", filter, base),
        None => base,
    }
}

fn normalize_filter(filter: &str) -> Option<&str> {
    let filter = filter.trim();
    let filter = filter.trim_end_matches(',');
    if filter.is_empty() {
        None
    } else {
        Some(filter)
    }
}

pub fn detect_preprocess_input_kind(input: &Path) -> Result<PreprocessInputKind> {
    if input.is_dir() {
        return Ok(PreprocessInputKind::Directory);
    }

    if !input.is_file() {
        return Err(anyhow!("Input path does not exist: {}", input.display()));
    }

    let ext = input.extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_ascii_lowercase()).unwrap_or_default();

    if matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
        Ok(PreprocessInputKind::Image)
    } else {
        Ok(PreprocessInputKind::Video)
    }
}

pub fn resolve_preprocess_output_path(input: &Path, output_target: &Path, kind: PreprocessInputKind) -> Result<PathBuf> {
    if kind == PreprocessInputKind::Directory {
        return Ok(output_target.to_path_buf());
    }

    let target_is_dir = output_target.is_dir() || output_target.extension().is_none();
    if !target_is_dir {
        return Ok(output_target.to_path_buf());
    }

    let stem = input.file_stem().and_then(|s| s.to_str()).filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("Could not derive an output filename from {}", input.display()))?;

    let ext = match kind {
        PreprocessInputKind::Image => "png",
        PreprocessInputKind::Video => "mp4",
        PreprocessInputKind::Directory => unreachable!(),
    };

    Ok(output_target.join(format!("{stem}_preprocessed.{ext}")))
}

fn ensure_output_parent(output: &Path) -> Result<()> {
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| format!("creating output directory {}", parent.display()))?;
        }
    }
    Ok(())
}

fn build_standalone_filter_complex(filter: &str, final_format: &str) -> Result<String> {
    let filter = normalize_filter(filter).ok_or_else(|| anyhow!("preprocess filter cannot be empty"))?;
    Ok(format!("[0:v]{filter},format=rgba[fg];color=c=black:s=16x16,format=rgba[bg0];[bg0][fg]scale2ref[bg][fg1];[bg][fg1]overlay=shortest=1:format=auto,format={final_format}[v]"))
}

fn apply_optional_time_range(command: &mut ProcCommand, start: Option<&str>, end: Option<&str>) {
    if let Some(start) = start.filter(|s| !s.is_empty() && *s != "0") {
        command.arg("-ss").arg(start);
    }

    if let Some(end) = end.filter(|s| !s.is_empty()) {
        let duration = match start.filter(|s| !s.is_empty() && *s != "0") {
            Some(start) => {
                let duration = parse_timestamp(end) - parse_timestamp(start);
                if duration > 0.0 {
                    Some(duration.to_string())
                } else {
                    None
                }
            }
            None => Some(end.to_string()),
        };

        if let Some(duration) = duration {
            command.arg("-t").arg(duration);
        }
    }
}

pub fn preprocess_image_to_file(input: &Path, filter: &str, output: &Path, ffmpeg_config: &FfmpegConfig) -> Result<()> {
    ensure_output_parent(output)?;
    let filter_complex = build_standalone_filter_complex(filter, "rgb24")?;

    let status = ProcCommand::new(ffmpeg_config.ffmpeg_cmd()).arg("-loglevel").arg("error").arg("-y").arg("-i").arg(input).arg("-filter_complex").arg(&filter_complex).arg("-map").arg("[v]").arg("-frames:v").arg("1").arg(output).status().with_context(|| format!("running ffmpeg preprocessing on {}", input.display()))?;

    if !status.success() {
        return Err(anyhow!("ffmpeg preprocessing failed for {}", input.display()));
    }

    Ok(())
}

pub fn preprocess_video_to_file(input: &Path, filter: &str, output: &Path, start: Option<&str>, end: Option<&str>, ffmpeg_config: &FfmpegConfig) -> Result<()> {
    ensure_output_parent(output)?;

    let ext = output.extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_ascii_lowercase()).unwrap_or_default();
    let filter_complex = build_standalone_filter_complex(filter, "yuv420p")?;

    let mut command = ProcCommand::new(ffmpeg_config.ffmpeg_cmd());
    command.arg("-loglevel").arg("error").arg("-y");
    apply_optional_time_range(&mut command, start, end);
    command.arg("-i").arg(input);
    command.arg("-filter_complex").arg(&filter_complex).arg("-map").arg("[v]").arg("-map").arg("0:a?");

    match ext.as_str() {
        "" | "mp4" | "m4v" | "mov" => {
            command.arg("-c:v").arg("libx264").arg("-crf").arg("18").arg("-preset").arg("medium").arg("-pix_fmt").arg("yuv420p").arg("-c:a").arg("aac").arg("-movflags").arg("+faststart");
        }
        "mkv" => {
            command.arg("-c:v").arg("libx264").arg("-crf").arg("18").arg("-preset").arg("medium").arg("-pix_fmt").arg("yuv420p").arg("-c:a").arg("aac");
        }
        "webm" => {
            command.arg("-c:v").arg("libvpx-vp9").arg("-crf").arg("30").arg("-b:v").arg("0").arg("-pix_fmt").arg("yuv420p").arg("-c:a").arg("libopus");
        }
        _ => {
            return Err(anyhow!("Unsupported preprocess video output format '{}'. Use .mp4, .mov, .m4v, .mkv, or .webm.", output.display()));
        }
    }

    let status = command.arg(output).status().with_context(|| format!("running ffmpeg preprocessing on {}", input.display()))?;

    if !status.success() {
        return Err(anyhow!("ffmpeg preprocessing failed for {}", input.display()));
    }

    Ok(())
}

pub struct TempFileGuard {
    path: PathBuf,
}

impl TempFileGuard {
    pub fn new(path: PathBuf) -> Self {
        Self {path}
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Preprocess all images in a directory, writing results to an output directory.
///
/// Each image file (`.png`, `.jpg`, `.jpeg`) is processed through the given
/// ffmpeg filter and written to `output_dir` as a `.png` file, preserving the
/// original file stem.
pub fn preprocess_directory(source_dir: &Path, filter: &str, output_dir: &Path, ffmpeg_config: &FfmpegConfig) -> Result<usize> {
    if !source_dir.exists() {
        return Err(anyhow!("Source directory does not exist: {}", source_dir.display()));
    }

    fs::create_dir_all(output_dir).with_context(|| format!("creating output directory {}", output_dir.display()))?;

    let mut images: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(source_dir).with_context(|| format!("reading directory {}", source_dir.display()))?.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "png" | "jpg" | "jpeg") {
                    images.push(path);
                }
            }
        }
    }
    images.sort();

    if images.is_empty() {
        return Err(anyhow!("No image files found in {}", source_dir.display()));
    }

    images.par_iter().try_for_each(|img_path| {
        let stem = img_path.file_stem().and_then(|s| s.to_str()).unwrap_or("frame");
        let out_path = output_dir.join(format!("{}.png", stem));
        preprocess_image_to_file(img_path, filter, &out_path, ffmpeg_config)
    })?;

    Ok(images.len())
}

pub fn preprocess_image_to_temp(input: &Path, filter: &str, ffmpeg_config: &FfmpegConfig) -> Result<TempFileGuard> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let out_path = std::env::temp_dir().join(format!("cascii_preprocessed_{}_{}.png", std::process::id(), stamp));

    let status = ProcCommand::new(ffmpeg_config.ffmpeg_cmd()).arg("-loglevel").arg("error").arg("-y").arg("-i").arg(input).arg("-vf").arg(filter).arg("-frames:v").arg("1").arg(&out_path).status().context("running ffmpeg preprocessing for image input")?;

    if !status.success() {
        return Err(anyhow!("ffmpeg image preprocessing failed"));
    }

    Ok(TempFileGuard::new(out_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{env, fs};

    fn ffmpeg_available() -> bool {
        Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|status| status.success()).unwrap_or(false)
    }

    fn temp_test_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let dir = env::temp_dir().join(format!("cascii_preprocess_{label}_{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolves_preprocess_output_path_for_directory_target() -> Result<()> {
        let input = Path::new("/tmp/example.png");
        let output = resolve_preprocess_output_path(input, Path::new("/tmp/preprocessed"), PreprocessInputKind::Image)?;
        assert_eq!(output, PathBuf::from("/tmp/preprocessed/example_preprocessed.png"));
        Ok(())
    }

    #[test]
    fn standalone_filter_complex_wraps_filter_on_black_background() -> Result<()> {
        let filter_complex = build_standalone_filter_complex("colorkey=0xFFFFFF:0.1:0.02", "rgb24")?;
        assert!(filter_complex.contains("colorkey=0xFFFFFF:0.1:0.02"));
        assert!(filter_complex.contains("color=c=black:s=16x16"));
        assert!(filter_complex.ends_with("format=rgb24[v]"));
        Ok(())
    }

    #[test]
    fn preprocess_image_to_file_writes_output() -> Result<()> {
        if !ffmpeg_available() {
            return Ok(());
        }

        let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/image/input/frame_0001.png");
        let output_dir = temp_test_dir("image");
        let output = output_dir.join("frame_0001_preprocessed.png");

        preprocess_image_to_file(&input, find_preprocess_preset("bg-white").unwrap().filter, &output, &FfmpegConfig::default())?;

        assert!(output.exists());
        assert!(fs::metadata(&output)?.len() > 0);
        let _ = fs::remove_dir_all(&output_dir);
        Ok(())
    }

    #[test]
    fn preprocess_video_to_file_writes_output() -> Result<()> {
        if !ffmpeg_available() {
            return Ok(());
        }

        let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/video/input/test.mkv");
        let output_dir = temp_test_dir("video");
        let output = output_dir.join("test_preprocessed.mp4");

        preprocess_video_to_file(&input, find_preprocess_preset("bg-white").unwrap().filter, &output, None, None, &FfmpegConfig::default())?;

        assert!(output.exists());
        assert!(fs::metadata(&output)?.len() > 0);
        let _ = fs::remove_dir_all(&output_dir);
        Ok(())
    }
}
