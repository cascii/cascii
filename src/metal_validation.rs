use super::*;
use crate::render;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Deserialize, Serialize)]
struct Scenario {
    name: String,
    source: String,
    columns: u32,
    font_ratio: f32,
    threshold: u8,
    background_threshold: u8,
    quality: String,
    ramp: String,
    previews: bool,
}

#[derive(Serialize)]
struct FrameMetrics {
    frame: String,
    cells: usize,
    identical: bool,
    changed_glyphs: usize,
    changed_color_bytes: usize,
    changed_pixels: usize,
    pixel_mae: f64,
    pixel_max_error: u8,
    temporal_delta_mae: Option<f64>,
    cpu_ms: f64,
    metal_ms: f64,
}

#[derive(Serialize)]
struct ScenarioResult {
    settings: Scenario,
    setup_ms: f64,
    frames: Vec<FrameMetrics>,
}

fn frame_bytes(frame: &AsciiFrameData) -> Vec<u8> {
    crate::frame::encode_cframe(frame.width_chars, frame.height_chars, &frame.ascii_text, &frame.rgb_colors, Some(&frame.bg_rgb_colors))
}

fn save_preview(path: &Path, pixels: &[u8], width: u32, height: u32, label: &str, atlas: &render::GlyphAtlas) -> Result<()> {
    let header_height = 56u32;
    let mut canvas = vec![0u8; (width * (height + header_height) * 3) as usize];
    canvas[(width * header_height * 3) as usize..].copy_from_slice(pixels);
    let title = AsciiFrameData {ascii_text: format!("{label}\n"), width_chars: label.len() as u32, height_chars: 1, rgb_colors: Vec::new(), bg_rgb_colors: Vec::new()};
    let mut title_pixels = Vec::new();
    render::render_ascii_frame_into_rgb(&title, atlas, false, &mut title_pixels);
    let title_width = (title.width_chars * atlas.cell_width + 1) & !1;
    for (y, row) in title_pixels.chunks_exact(title_width as usize * 3).enumerate() {
        let count = row.len().min(width.saturating_sub(24) as usize * 3);
        let offset = ((y + 12) * width as usize + 12) * 3;
        canvas[offset..offset + count].copy_from_slice(&row[..count]);
    }
    image::save_buffer(path, &canvas, width, height + header_height, image::ColorType::Rgb8)?;
    Ok(())
}

#[test]
#[ignore = "requires a Metal device; run explicitly with --features metal"]
fn metal_threshold_quadrants_and_empty_ramp() -> Result<()> {
    for quality in [BgFitQuality::Fast, BgFitQuality::Fidelity] {
        for ramp in [b" ".as_slice(), b"".as_slice()] {
            let gpu = background_analysis_context(ramp, quality)?;
            let cpu = background_fit_optimized::background_analysis_context(ramp, quality)?;
            for (foreground, background) in [(0, 0), (128, 128), (128, 129), (129, 128), (255, 255)] {
                let image = RgbImage::from_pixel(32, 24, image::Rgb([128, 128, 128]));
                let reference = background_fit_optimized::fit_rgb_to_ascii_with_context(image.clone(), 0.7, foreground, background, Some(8), &cpu)?;
                let actual = fit_rgb_to_ascii_with_context(image, 0.7, foreground, background, Some(8), &gpu)?;
                assert_eq!(frame_bytes(&reference), frame_bytes(&actual));
            }
            let image = RgbImage::new(1, 1);
            assert!(fit_rgb_to_ascii_with_context(image, 0.7, 0, 0, Some(0), &gpu).is_err());
        }
    }
    Ok(())
}

#[test]
#[ignore = "animation benchmark; set CASCII_METAL_VALIDATION_ROOT and run explicitly"]
fn metal_animation_comparison() -> Result<()> {
    let root = PathBuf::from(std::env::var("CASCII_METAL_VALIDATION_ROOT").context("set CASCII_METAL_VALIDATION_ROOT to a prepared comparison directory")?);
    let scenarios: Vec<Scenario> = serde_json::from_slice(&fs::read(root.join("scenarios.json"))?)?;
    let atlas = render::build_glyph_atlas(14.0)?;
    let title_atlas = render::build_glyph_atlas(28.0)?;
    let mut results = Vec::new();
    for scenario in scenarios {
        let directory = root.join("results").join(&scenario.name);
        fs::create_dir_all(directory.join("cpu"))?;
        fs::create_dir_all(directory.join("metal"))?;
        fs::create_dir_all(directory.join("difference"))?;
        let quality = if scenario.quality == "fast" {BgFitQuality::Fast} else {BgFitQuality::Fidelity};
        let cpu = background_fit_optimized::background_analysis_context(scenario.ramp.as_bytes(), quality)?;
        let start = Instant::now();
        let gpu = background_analysis_context(scenario.ramp.as_bytes(), quality)?;
        let setup_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut paths: Vec<_> = fs::read_dir(root.join(&scenario.source))?.map(|entry| entry.map(|entry| entry.path())).collect::<std::io::Result<_>>()?;
        paths.retain(|path| path.extension().is_some_and(|extension| extension == "png"));
        paths.sort();
        ensure!(!paths.is_empty(), "no frames for {}", scenario.name);
        let mut frames = Vec::new();
        let mut previous: Option<(Vec<u8>, Vec<u8>)> = None;
        for (index, path) in paths.iter().enumerate() {
            let image = image::open(path)?.to_rgb8();
            let cpu_fit = || background_fit_optimized::fit_rgb_to_ascii_with_context(image.clone(), scenario.font_ratio, scenario.threshold, scenario.background_threshold, Some(scenario.columns), &cpu);
            let gpu_fit = || fit_rgb_to_ascii_with_context(image.clone(), scenario.font_ratio, scenario.threshold, scenario.background_threshold, Some(scenario.columns), &gpu);
            if index == 0 {
                std::hint::black_box(cpu_fit()?);
                std::hint::black_box(gpu_fit()?);
            }
            // Alternate ordering to reduce systematic cache/temperature bias.
            let (reference, actual, cpu_ms, metal_ms) = if index % 2 == 0 {
                let start = Instant::now();
                let reference = cpu_fit()?;
                let cpu_ms = start.elapsed().as_secs_f64() * 1000.0;
                let start = Instant::now();
                let actual = gpu_fit()?;
                (reference, actual, cpu_ms, start.elapsed().as_secs_f64() * 1000.0)
            } else {
                let start = Instant::now();
                let actual = gpu_fit()?;
                let metal_ms = start.elapsed().as_secs_f64() * 1000.0;
                let start = Instant::now();
                let reference = cpu_fit()?;
                (reference, actual, start.elapsed().as_secs_f64() * 1000.0, metal_ms)
            };
            assert_eq!((reference.width_chars, reference.height_chars), (actual.width_chars, actual.height_chars));
            let ref_bytes = frame_bytes(&reference);
            let gpu_bytes = frame_bytes(&actual);
            let name = format!("frame_{index:04}");
            fs::write(directory.join("cpu").join(format!("{name}.cframe")), &ref_bytes)?;
            fs::write(directory.join("metal").join(format!("{name}.cframe")), &gpu_bytes)?;
            let changed_glyphs = reference.ascii_text.bytes().zip(actual.ascii_text.bytes()).filter(|(a, b)| a != b).count();
            let changed_color_bytes = reference.rgb_colors.iter().chain(&reference.bg_rgb_colors).zip(actual.rgb_colors.iter().chain(&actual.bg_rgb_colors)).filter(|(a, b)| a != b).count();
            let mut cpu_rgb = Vec::new();
            let mut gpu_rgb = Vec::new();
            render::render_ascii_frame_into_rgb(&reference, &atlas, true, &mut cpu_rgb);
            render::render_ascii_frame_into_rgb(&actual, &atlas, true, &mut gpu_rgb);
            let differences: Vec<u8> = cpu_rgb.iter().zip(&gpu_rgb).map(|(&a, &b)| a.abs_diff(b)).collect();
            let pixel_mae = differences.iter().map(|&value| value as u64).sum::<u64>() as f64 / differences.len() as f64;
            let temporal_delta_mae = previous.as_ref().map(|(prev_cpu, prev_gpu)| {
                cpu_rgb.iter().zip(&gpu_rgb).zip(prev_cpu.iter().zip(prev_gpu)).map(|((&cpu, &gpu), (&previous_cpu, &previous_gpu))| {
                    ((gpu as i32 - previous_gpu as i32) - (cpu as i32 - previous_cpu as i32)).unsigned_abs() as u64
                }).sum::<u64>() as f64 / cpu_rgb.len() as f64
            });
            frames.push(FrameMetrics {
                frame: path.file_name().unwrap().to_string_lossy().into_owned(), cells: (reference.width_chars * reference.height_chars) as usize,
                identical: ref_bytes == gpu_bytes, changed_glyphs, changed_color_bytes,
                changed_pixels: differences.chunks_exact(3).filter(|pixel| pixel.iter().any(|&v| v != 0)).count(),
                pixel_mae, pixel_max_error: differences.iter().copied().max().unwrap_or(0), temporal_delta_mae, cpu_ms, metal_ms,
            });
            if scenario.previews {
                let width = (reference.width_chars * atlas.cell_width + 1) & !1;
                let height = (reference.height_chars * atlas.cell_height + 1) & !1;
                for (kind, label, data) in [("cpu", "CPU reference", &cpu_rgb), ("metal", "Metal + tie correction", &gpu_rgb)] {
                    save_preview(&directory.join(kind).join(format!("{name}.png")), data, width, height, label, &title_atlas)?;
                }
                let amplified: Vec<u8> = differences.iter().map(|&v| v.saturating_mul(8)).collect();
                save_preview(&directory.join("difference").join(format!("{name}.png")), &amplified, width, height, "Difference x8", &title_atlas)?;
            }
            previous = Some((cpu_rgb, gpu_rgb));
        }
        let identical = frames.iter().filter(|frame| frame.identical).count();
        let cpu_ms: f64 = frames.iter().map(|frame| frame.cpu_ms).sum::<f64>() / frames.len() as f64;
        let metal_ms: f64 = frames.iter().map(|frame| frame.metal_ms).sum::<f64>() / frames.len() as f64;
        println!("{}: {identical}/{} byte-identical, CPU {cpu_ms:.2} ms, Metal {metal_ms:.2} ms, {:.2}x", scenario.name, frames.len(), cpu_ms / metal_ms);
        results.push(ScenarioResult {settings: scenario, setup_ms, frames});
        fs::write(root.join("comparison.json"), serde_json::to_vec_pretty(&results)?)?;
    }
    Ok(())
}
