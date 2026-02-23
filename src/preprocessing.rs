use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::FfmpegConfig;

#[derive(Debug, Clone, Copy)]
pub struct PreprocessPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub filter: &'static str,
}

pub const PREPROCESS_PRESETS: &[PreprocessPreset] = &[
    PreprocessPreset {
        name: "contours",
        description: "Grayscale edge-detection with strong contrast (good for outlines).",
        filter: "format=gray,edgedetect=mode=colormix:high=0.2:low=0.05,eq=contrast=2.5:brightness=-0.1",
    },
    PreprocessPreset {
        name: "contours-soft",
        description: "Softer contour extraction with less aggressive edges.",
        filter: "format=gray,edgedetect=mode=colormix:high=0.12:low=0.03,eq=contrast=2.0:brightness=-0.05",
    },
    PreprocessPreset {
        name: "contours-strong",
        description: "Very sharp contour extraction for bold linework.",
        filter: "format=gray,edgedetect=mode=colormix:high=0.35:low=0.08,eq=contrast=3.2:brightness=-0.12",
    },
    PreprocessPreset {
        name: "bw-contrast",
        description: "Simple grayscale + contrast boost for clean monochrome ASCII.",
        filter: "format=gray,eq=contrast=2.2:brightness=-0.08",
    },
    PreprocessPreset {
        name: "noir-detail",
        description: "Grayscale sharpened look that emphasizes texture.",
        filter: "format=gray,unsharp=5:5:1.0:5:5:0.0,eq=contrast=1.8:brightness=-0.04",
    },
    PreprocessPreset {
        name: "vivid",
        description: "Boost color saturation/contrast and sharpen for colorful ASCII.",
        filter: "eq=saturation=1.8:contrast=1.2:brightness=0.02,unsharp=5:5:0.8:5:5:0.0",
    },
    PreprocessPreset {
        name: "warm-pop",
        description: "Warmer color balance with moderate saturation boost.",
        filter: "colorbalance=rs=0.06:gs=0.02:bs=-0.04,eq=saturation=1.35:contrast=1.12",
    },
    PreprocessPreset {
        name: "cool-pop",
        description: "Cooler color balance with moderate saturation boost.",
        filter: "colorbalance=rs=-0.04:gs=0.02:bs=0.07,eq=saturation=1.28:contrast=1.10",
    },
    PreprocessPreset {
        name: "soft-glow",
        description: "Gentle blur and color lift for smoother gradients.",
        filter: "gblur=sigma=1.0,eq=saturation=1.15:contrast=1.08:brightness=0.02",
    },
];

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
    let preprocess = preprocess_filter
        .map(str::trim)
        .map(|s| s.trim_end_matches(','))
        .filter(|s| !s.is_empty());
    match preprocess {
        Some(filter) => format!("{},{}", filter, base),
        None => base,
    }
}

pub struct TempFileGuard {
    path: PathBuf,
}

impl TempFileGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
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

pub fn preprocess_image_to_temp(input: &Path, filter: &str, ffmpeg_config: &FfmpegConfig) -> Result<TempFileGuard> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let out_path = std::env::temp_dir().join(format!("cascii_preprocessed_{}_{}.png", std::process::id(), stamp));

    let status = ProcCommand::new(ffmpeg_config.ffmpeg_cmd())
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(input)
        .arg("-vf")
        .arg(filter)
        .arg("-frames:v")
        .arg("1")
        .arg(&out_path)
        .status()
        .context("running ffmpeg preprocessing for image input")?;

    if !status.success() {
        return Err(anyhow!("ffmpeg image preprocessing failed"));
    }

    Ok(TempFileGuard::new(out_path))
}
