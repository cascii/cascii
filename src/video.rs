use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::{Command as ProcCommand, Stdio};

use crate::preprocessing::build_frame_extraction_vf;
use crate::{FfmpegConfig, Progress, VideoOptions};

#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_video_frames(input: &Path, out_dir: &Path, columns: u32, fps: u32, start: Option<&str>, end: Option<&str>, preprocess_filter: Option<&str>, ffmpeg_config: &FfmpegConfig) -> Result<()> {
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

    let vf_option = build_frame_extraction_vf(columns, fps, preprocess_filter);
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
pub(crate) fn get_video_duration_us(input: &Path, ffmpeg_config: &FfmpegConfig) -> Result<u64> {
    let output = ProcCommand::new(ffmpeg_config.ffprobe_cmd())
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
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
pub(crate) fn extract_video_frames_with_progress<F>(input: &Path, out_dir: &Path, video_opts: &VideoOptions, ffmpeg_config: &FfmpegConfig, progress_callback: &F) -> Result<()> where F: Fn(Progress) + Send + Sync {
    let columns = video_opts.columns;
    let fps = video_opts.fps;
    let start = video_opts.start.as_deref();
    let end = video_opts.end.as_deref();

    let out_pattern = out_dir.join("frame_%04d.png");

    // Get video duration for progress calculation
    let _total_duration_us = get_video_duration_us(input, ffmpeg_config).unwrap_or(0);

    let mut ffmpeg_args: Vec<String> = vec![
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
    ];

    if let Some(s) = start {
        if !s.is_empty() && s != "0" {
            ffmpeg_args.push("-ss".into());
            ffmpeg_args.push(s.to_string());
        }
    }

    ffmpeg_args.push("-i".into());
    ffmpeg_args.push(
        input
            .to_str()
            .ok_or_else(|| anyhow!("input path is not valid UTF-8"))?
            .to_string(),
    );

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

    let vf_option = build_frame_extraction_vf(columns, fps, video_opts.preprocess_filter.as_deref());
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

pub(crate) fn extract_audio(input: &Path, out_dir: &Path, start: Option<&str>, end: Option<&str>, ffmpeg_config: &FfmpegConfig) -> Result<()> {
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

pub(crate) fn parse_timestamp(s: &str) -> f64 {
    s.split(':').rev().enumerate().fold(0.0, |acc, (i, v)| {
        acc + v.parse::<f64>().unwrap_or(0.0) * 60f64.powi(i as i32)
    })
}
