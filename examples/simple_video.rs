//! Example: Convert a video to ASCII frames using cascii as a library
//!
//! Run with: cargo run --example simple_video

use cascii::{AsciiConverter, ConversionOptions, VideoOptions};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a converter
    let converter = AsciiConverter::new();

    // Configure video options
    let video_opts = VideoOptions {
        fps: 10,
        start: Some("0".to_string()),
        end: Some("2".to_string()), // Extract first 2 seconds
        columns: 200,
    };

    // Configure conversion options
    let conv_opts = ConversionOptions::default()
        .with_font_ratio(0.5)
        .with_luminance(20);

    // Convert video
    let input = Path::new("tests/video/input/test.mkv");
    let output_dir = Path::new("example_video_output");

    if input.exists() {
        println!("Converting video to ASCII frames...");
        println!("Input: {}", input.display());
        println!("Output: {}", output_dir.display());
        println!("Settings: {}fps, {}s duration", video_opts.fps, 2);

        converter.convert_video(
            input,
            output_dir,
            &video_opts,
            &conv_opts,
            false, // Don't keep intermediate PNG files
        )?;

        println!("✓ Video conversion complete!");
        println!("ASCII frames saved to {}", output_dir.display());
    } else {
        println!("Note: {} not found.", input.display());
        println!("To use this example, provide a video file at that path.");
    }

    Ok(())
}

