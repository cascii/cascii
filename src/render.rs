use ab_glyph::{FontRef, PxScale, ScaleFont};
use anyhow::{anyhow, Context, Result};
use image::{DynamicImage, Rgb};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command as ProcCommand, Stdio};
use std::sync::OnceLock;

use crate::convert::AsciiFrameData;
use crate::FfmpegConfig;

/// Embedded monospace font for video rendering
const FONT_DATA: &[u8] = include_bytes!("../resources/DejaVuSansMono.ttf");
const ANALYSIS_FONT_SIZE: f32 = 16.0;
static ANALYSIS_GLYPH_ATLAS: OnceLock<std::result::Result<GlyphAtlas, String>> = OnceLock::new();

/// Pre-rasterized bitmap for a single glyph
struct GlyphBitmap {
    /// Alpha coverage values, row-major, cell_width * cell_height entries
    alpha: Vec<f32>,
    s_aa: f64,
    s_ab: f64,
    s_bb: f64,
    det: f64,
    degenerate: bool,
}

/// Pre-rasterized monospace glyph atlas for fast frame rendering
pub(crate) struct GlyphAtlas {
    /// Rasterized glyph bitmaps keyed by ASCII byte value
    glyphs: HashMap<u8, GlyphBitmap>,
    /// Width of each character cell in pixels
    pub(crate) cell_width: u32,
    /// Height of each character cell in pixels
    pub(crate) cell_height: u32,
}

pub(crate) struct BackgroundAnalysisContext {
    atlas: &'static GlyphAtlas,
    candidate_bytes: Vec<u8>,
}

pub(crate) fn build_glyph_atlas(font_size: f32) -> Result<GlyphAtlas> {
    use ab_glyph::Font;

    let font = FontRef::try_from_slice(FONT_DATA).map_err(|e| anyhow!("failed to load embedded font: {}", e))?;

    let scale = PxScale::from(font_size);
    let scaled_font = font.as_scaled(scale);

    // Determine cell dimensions from font metrics
    // Use 'M' as reference for advance width
    let h_advance = scaled_font.h_advance(font.glyph_id('M'));
    let cell_width = h_advance.ceil() as u32;
    let cell_height = (scaled_font.ascent() - scaled_font.descent()).ceil() as u32;
    let ascent = scaled_font.ascent();

    let mut glyphs = HashMap::new();

    for byte in 32u8..=126u8 {
        let ch = byte as char;
        let glyph_id = font.glyph_id(ch);
        let glyph = glyph_id.with_scale_and_position(scale, ab_glyph::point(0.0, ascent));

        let mut alpha = vec![0.0f32; (cell_width * cell_height) as usize];

        if let Some(outlined) = font.outline_glyph(glyph) {
            outlined.draw(|gx, gy, coverage| {
                let px = gx;
                let py = gy;
                if px < cell_width && py < cell_height {
                    alpha[(py * cell_width + px) as usize] = coverage;
                }
            });
        }

        let mut s_aa = 0.0f64;
        let mut s_ab = 0.0f64;
        let mut s_bb = 0.0f64;
        let mut sum_alpha = 0.0f64;
        for &value in &alpha {
            let a = value as f64;
            let b = 1.0 - a;
            sum_alpha += a;
            s_aa += a * a;
            s_ab += a * b;
            s_bb += b * b;
        }
        let mean_alpha = sum_alpha / alpha.len().max(1) as f64;
        let det = s_aa * s_bb - s_ab * s_ab;
        let degenerate = mean_alpha <= 1e-6 || mean_alpha >= 1.0 - 1e-6 || det.abs() <= 1e-9;

        glyphs.insert(byte, GlyphBitmap {alpha, s_aa, s_ab, s_bb, det, degenerate});
    }

    Ok(GlyphAtlas {glyphs, cell_width, cell_height})
}

fn analysis_glyph_atlas() -> Result<&'static GlyphAtlas> {
    match ANALYSIS_GLYPH_ATLAS.get_or_init(|| build_glyph_atlas(ANALYSIS_FONT_SIZE).map_err(|e| e.to_string())) {
        Ok(atlas) => Ok(atlas),
        Err(message) => Err(anyhow!(message.clone())),
    }
}

fn candidate_bytes_for_ascii_chars(ascii_chars: &[u8]) -> Vec<u8> {
    let candidate_bytes: Vec<u8> = ascii_chars.iter().copied().filter(|byte| *byte != b' ').collect();
    if candidate_bytes.is_empty() {
        vec![b' ']
    } else {
        candidate_bytes
    }
}

pub(crate) fn background_analysis_context(ascii_chars: &[u8]) -> Result<BackgroundAnalysisContext> {
    Ok(BackgroundAnalysisContext {atlas: analysis_glyph_atlas()?, candidate_bytes: candidate_bytes_for_ascii_chars(ascii_chars)})
}

pub(crate) fn render_ascii_frame_to_rgb(frame: &AsciiFrameData, atlas: &GlyphAtlas, use_colors: bool) -> Vec<u8> {
    let mut pixel_w = frame.width_chars * atlas.cell_width;
    let mut pixel_h = frame.height_chars * atlas.cell_height;

    // H.264 requires even dimensions
    if !pixel_w.is_multiple_of(2) {
        pixel_w += 1;
    }
    if !pixel_h.is_multiple_of(2) {
        pixel_h += 1;
    }

    let mut buffer = vec![0u8; (pixel_w * pixel_h * 3) as usize];

    let mut char_idx: usize = 0;
    let mut row: u32 = 0;
    let mut col: u32 = 0;

    for ch in frame.ascii_text.chars() {
        if ch == '\n' {
            row += 1;
            col = 0;
            continue;
        }

        let byte = ch as u8;

        // Get color for this character
        let (r, g, b) = if use_colors && char_idx * 3 + 2 < frame.rgb_colors.len() {
            (frame.rgb_colors[char_idx * 3], frame.rgb_colors[char_idx * 3 + 1], frame.rgb_colors[char_idx * 3 + 2])
        } else {
            (255, 255, 255) // white for text-only mode
        };

        let base_x = col * atlas.cell_width;
        let base_y = row * atlas.cell_height;

        if char_idx * 3 + 2 < frame.bg_rgb_colors.len() {
            let bg_r = frame.bg_rgb_colors[char_idx * 3];
            let bg_g = frame.bg_rgb_colors[char_idx * 3 + 1];
            let bg_b = frame.bg_rgb_colors[char_idx * 3 + 2];
            for gy in 0..atlas.cell_height {
                for gx in 0..atlas.cell_width {
                    let px = base_x + gx;
                    let py = base_y + gy;
                    if px >= pixel_w || py >= pixel_h {
                        continue;
                    }
                    let offset = ((py * pixel_w + px) * 3) as usize;
                    buffer[offset] = bg_r;
                    buffer[offset + 1] = bg_g;
                    buffer[offset + 2] = bg_b;
                }
            }
        }

        // Look up glyph bitmap
        if let Some(glyph_bitmap) = atlas.glyphs.get(&byte) {
            for gy in 0..atlas.cell_height {
                for gx in 0..atlas.cell_width {
                    let px = base_x + gx;
                    let py = base_y + gy;
                    if px >= pixel_w || py >= pixel_h {
                        continue;
                    }
                    let alpha = glyph_bitmap.alpha[(gy * atlas.cell_width + gx) as usize];
                    if alpha > 0.0 {
                        let offset = ((py * pixel_w + px) * 3) as usize;
                        buffer[offset] = blend_channel(buffer[offset], r, alpha);
                        buffer[offset + 1] = blend_channel(buffer[offset + 1], g, alpha);
                        buffer[offset + 2] = blend_channel(buffer[offset + 2], b, alpha);
                    }
                }
            }
        }

        char_idx += 1;
        col += 1;
    }

    buffer
}

pub(crate) fn fit_image_to_ascii_with_cell_backgrounds(img_path: &Path, font_ratio: f32, threshold: u8, bg_threshold: u8, columns: Option<u32>, ascii_chars: &[u8]) -> Result<AsciiFrameData> {
    let background_analysis = background_analysis_context(ascii_chars)?;
    fit_image_to_ascii_with_cell_backgrounds_with_context(img_path, font_ratio, threshold, bg_threshold, columns, &background_analysis)
}

pub(crate) fn fit_image_to_ascii_with_cell_backgrounds_with_context(img_path: &Path, font_ratio: f32, threshold: u8, bg_threshold: u8, columns: Option<u32>, background_analysis: &BackgroundAnalysisContext) -> Result<AsciiFrameData> {
    let atlas = background_analysis.atlas;
    let mut img = image::open(img_path).with_context(|| format!("opening {}", img_path.display()))?.to_rgb8();

    let (orig_w, orig_h) = img.dimensions();
    let (width_chars, height_chars) = if let Some(cols) = columns {
        let h = (orig_h as f32 / orig_w as f32 * cols as f32 * font_ratio).round() as u32;
        (cols, h.max(1))
    } else {
        let h = (orig_h as f32 * font_ratio).round() as u32;
        (orig_w, h.max(1))
    };

    let target_w = width_chars * atlas.cell_width;
    let target_h = height_chars * atlas.cell_height;
    if target_w != orig_w || target_h != orig_h {
        let dyn_img = DynamicImage::ImageRgb8(img);
        img = dyn_img.resize_exact(target_w, target_h, image::imageops::FilterType::Lanczos3).to_rgb8();
    }

    let cell_pixels = (atlas.cell_width * atlas.cell_height) as usize;
    let mut ascii_text = String::with_capacity((width_chars as usize + 1) * height_chars as usize);
    let mut rgb_colors = Vec::with_capacity((width_chars * height_chars * 3) as usize);
    let mut bg_rgb_colors = Vec::with_capacity((width_chars * height_chars * 3) as usize);
    let mut patch = vec![Rgb([0u8, 0u8, 0u8]); cell_pixels];

    for row in 0..height_chars {
        for col in 0..width_chars {
            let base_x = col * atlas.cell_width;
            let base_y = row * atlas.cell_height;
            let mut patch_index = 0usize;
            let mut total_luma = 0.0f64;
            let mut sum_rgb = [0u64; 3];

            for py in 0..atlas.cell_height {
                for px in 0..atlas.cell_width {
                    let pixel = *img.get_pixel(base_x + px, base_y + py);
                    total_luma += luminance(pixel) as f64;
                    sum_rgb[0] += pixel[0] as u64;
                    sum_rgb[1] += pixel[1] as u64;
                    sum_rgb[2] += pixel[2] as u64;
                    patch[patch_index] = pixel;
                    patch_index += 1;
                }
            }

            let avg_luma = total_luma / cell_pixels as f64;
            let emit_fg = avg_luma >= threshold as f64;
            let emit_bg = avg_luma >= bg_threshold as f64;

            // Quadrant 4: nothing visible at all — short-circuit the fit solver.
            if !emit_fg && !emit_bg {
                ascii_text.push(' ');
                rgb_colors.extend_from_slice(&[0, 0, 0]);
                bg_rgb_colors.extend_from_slice(&[0, 0, 0]);
                continue;
            }

            let avg_rgb = [(sum_rgb[0] / cell_pixels as u64) as u8, (sum_rgb[1] / cell_pixels as u64) as u8, (sum_rgb[2] / cell_pixels as u64) as u8];

            // Quadrant 3: background-only "mosaic" cell. The bg threshold is
            // met but the fg threshold isn't, so emit a space glyph (with
            // black fg) on the cell's average colour. Skipping the glyph
            // solver here keeps the bg surface flat per the avg patch colour
            // — a tighter aesthetic than letting the solver guess at a
            // best-fit fg/bg pair for a cell that's about to drop the fg.
            if !emit_fg {
                ascii_text.push(' ');
                rgb_colors.extend_from_slice(&[0, 0, 0]);
                bg_rgb_colors.extend_from_slice(&avg_rgb);
                continue;
            }

            let mut best_byte = b' ';
            let mut best_fg = avg_rgb;
            let mut best_bg = avg_rgb;
            let mut best_error = f64::INFINITY;

            for &byte in &background_analysis.candidate_bytes {
                if let Some(glyph) = atlas.glyphs.get(&byte) {
                    let (fg, bg, error) = fit_colors_for_glyph(&patch, glyph, avg_rgb);
                    if error < best_error {
                        best_byte = byte;
                        best_fg = fg;
                        best_bg = bg;
                        best_error = error;
                    }
                }
            }

            ascii_text.push(best_byte as char);
            rgb_colors.extend_from_slice(&best_fg);
            // Quadrant 2: glyph emitted but bg suppressed → black bg.
            if emit_bg {
                bg_rgb_colors.extend_from_slice(&best_bg);
            } else {
                bg_rgb_colors.extend_from_slice(&[0, 0, 0]);
            }
        }
        ascii_text.push('\n');
    }

    Ok(AsciiFrameData {ascii_text, width_chars, height_chars, rgb_colors, bg_rgb_colors})
}

fn blend_channel(background: u8, foreground: u8, alpha: f32) -> u8 {
    ((background as f32 * (1.0 - alpha)) + (foreground as f32 * alpha)).round().clamp(0.0, 255.0) as u8
}

fn fit_colors_for_glyph(patch: &[Rgb<u8>], glyph: &GlyphBitmap, avg_rgb: [u8; 3]) -> ([u8; 3], [u8; 3], f64) {
    if glyph.degenerate {
        return (avg_rgb, avg_rgb, constant_patch_error(patch, avg_rgb));
    }

    let mut fg = [0u8; 3];
    let mut bg = [0u8; 3];
    for channel in 0..3 {
        let mut s_ap = 0.0f64;
        let mut s_bp = 0.0f64;
        for (pixel, &value) in patch.iter().zip(glyph.alpha.iter()) {
            let a = value as f64;
            let b = 1.0 - a;
            let p = pixel[channel] as f64;
            s_ap += a * p;
            s_bp += b * p;
        }

        let fg_value = ((s_ap * glyph.s_bb) - (s_bp * glyph.s_ab)) / glyph.det;
        let bg_value = ((s_bp * glyph.s_aa) - (s_ap * glyph.s_ab)) / glyph.det;
        fg[channel] = fg_value.clamp(0.0, 255.0).round() as u8;
        bg[channel] = bg_value.clamp(0.0, 255.0).round() as u8;
    }

    let mut error = 0.0f64;
    for (pixel, &value) in patch.iter().zip(glyph.alpha.iter()) {
        let a = value as f64;
        let b = 1.0 - a;
        for channel in 0..3 {
            let predicted = a * fg[channel] as f64 + b * bg[channel] as f64;
            let diff = predicted - pixel[channel] as f64;
            error += diff * diff;
        }
    }
    (fg, bg, error)
}

fn constant_patch_error(patch: &[Rgb<u8>], color: [u8; 3]) -> f64 {
    let mut error = 0.0f64;
    for pixel in patch {
        for channel in 0..3 {
            let diff = color[channel] as f64 - pixel[channel] as f64;
            error += diff * diff;
        }
    }
    error
}

fn luminance(rgb: Rgb<u8>) -> u8 {
    let r = rgb[0] as f64;
    let g = rgb[1] as f64;
    let b = rgb[2] as f64;
    (0.2126 * r + 0.7152 * g + 0.0722 * b) as u8
}

pub(crate) fn spawn_ffmpeg_encoder(pixel_width: u32, pixel_height: u32, fps: u32, crf: u8, audio_path: Option<&Path>, output_path: &Path, ffmpeg_config: &FfmpegConfig) -> Result<std::process::Child> {
    let size = format!("{}x{}", pixel_width, pixel_height);

    let mut args: Vec<String> = vec!["-y".into(), "-loglevel".into(), "error".into(), "-f".into(), "rawvideo".into(), "-pix_fmt".into(), "rgb24".into(), "-s:v".into(), size, "-r".into(), fps.to_string(), "-i".into(), "pipe:0".into()];

    if let Some(audio) = audio_path {
        args.push("-i".into());
        args.push(audio.to_str().unwrap_or("audio.mp3").to_string());
        args.push("-c:a".into());
        args.push("aac".into());
        args.push("-b:a".into());
        args.push("192k".into());
        args.push("-shortest".into());
    }

    args.push("-c:v".into());
    args.push("libx264".into());
    args.push("-crf".into());
    args.push(crf.to_string());
    args.push("-preset".into());
    args.push("medium".into());
    args.push("-g".into());
    args.push(fps.to_string());
    args.push("-pix_fmt".into());
    args.push("yuv420p".into());
    args.push(output_path.to_str().ok_or_else(|| anyhow!("output path is not valid UTF-8"))?.to_string());

    let child = ProcCommand::new(ffmpeg_config.ffmpeg_cmd()).args(&args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().context("spawning ffmpeg encoder")?;
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_background_for_space_cells() -> Result<()> {
        let atlas = build_glyph_atlas(12.0)?;
        let frame = AsciiFrameData {ascii_text: " \n".to_string(), width_chars: 1, height_chars: 1, rgb_colors: Vec::new(), bg_rgb_colors: vec![255, 0, 0]};
        let buffer = render_ascii_frame_to_rgb(&frame, &atlas, false);
        assert!(buffer.chunks_exact(3).any(|pixel| pixel[0] > 200 && pixel[1] < 16 && pixel[2] < 16));
        Ok(())
    }

    #[test]
    fn blends_foreground_glyph_over_background() -> Result<()> {
        let atlas = build_glyph_atlas(12.0)?;
        let frame = AsciiFrameData {ascii_text: "M\n".to_string(), width_chars: 1, height_chars: 1, rgb_colors: vec![0, 255, 0], bg_rgb_colors: vec![0, 0, 255]};
        let buffer = render_ascii_frame_to_rgb(&frame, &atlas, true);
        assert!(buffer.chunks_exact(3).any(|pixel| pixel[1] == 0 && pixel[2] > 200));
        assert!(buffer.chunks_exact(3).any(|pixel| pixel[1] > 0 && pixel[2] < 255));
        Ok(())
    }

    /// Helper: writes a uniform mid-gray image (luminance ≈ 128) to a temp PNG.
    fn write_uniform_test_image(luma_target: u8) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("uniform.png");
        // Build a 32×32 image of uniform gray.
        let img = image::RgbImage::from_pixel(32, 32, Rgb([luma_target, luma_target, luma_target]));
        img.save(&path).expect("save");
        (dir, path)
    }

    fn last_cell_bg(frame: &AsciiFrameData) -> [u8; 3] {
        let n = frame.bg_rgb_colors.len();
        [frame.bg_rgb_colors[n - 3], frame.bg_rgb_colors[n - 2], frame.bg_rgb_colors[n - 1]]
    }

    fn first_glyph(frame: &AsciiFrameData) -> char {
        frame.ascii_text.chars().find(|ch| *ch != '\n').unwrap_or(' ')
    }

    #[test]
    fn bg_fit_quadrant_both_thresholds_met() -> Result<()> {
        // Uniform gray ≈ 128; thresholds well below it on both axes → glyph + bg.
        let (_dir, path) = write_uniform_test_image(128);
        let frame = fit_image_to_ascii_with_cell_backgrounds(&path, 0.5, 30, 30, Some(4), b" .M")?;
        let bg = last_cell_bg(&frame);
        // bg should be non-black (matches mid-gray-ish).
        assert!(bg[0] > 5 || bg[1] > 5 || bg[2] > 5, "expected coloured bg, got {:?}", bg);
        // Glyph should be from the candidate set (not always space).
        assert!(frame.ascii_text.chars().any(|ch| ch == 'M' || ch == '.'));
        Ok(())
    }

    #[test]
    fn bg_fit_quadrant_glyph_only_bg_suppressed() -> Result<()> {
        // fg threshold passes, bg threshold doesn't → glyph + black bg.
        let (_dir, path) = write_uniform_test_image(128);
        let frame = fit_image_to_ascii_with_cell_backgrounds(&path, 0.5, 30, 200, Some(4), b" .M")?;
        let bg = last_cell_bg(&frame);
        assert_eq!(bg, [0, 0, 0], "bg should be black when bg threshold not met");
        // Glyph still emitted (not all spaces).
        assert!(frame.ascii_text.chars().any(|ch| ch == 'M' || ch == '.'));
        Ok(())
    }

    #[test]
    fn bg_fit_quadrant_bg_only_glyph_suppressed() -> Result<()> {
        // fg threshold fails, bg threshold passes → space + coloured bg ("mosaic" cell).
        let (_dir, path) = write_uniform_test_image(128);
        let frame = fit_image_to_ascii_with_cell_backgrounds(&path, 0.5, 200, 30, Some(4), b" .M")?;
        let bg = last_cell_bg(&frame);
        assert!(bg[0] > 5 || bg[1] > 5 || bg[2] > 5, "expected coloured bg, got {:?}", bg);
        // Every glyph should be a space.
        assert!(frame.ascii_text.chars().all(|ch| ch == ' ' || ch == '\n'), "expected spaces only, got {:?}", frame.ascii_text);
        // fg buffer should be all black under spaces.
        assert!(frame.rgb_colors.iter().all(|&b| b == 0));
        Ok(())
    }

    #[test]
    fn bg_fit_quadrant_neither_threshold_met() -> Result<()> {
        // Both thresholds above luminance → empty cells.
        let (_dir, path) = write_uniform_test_image(64);
        let frame = fit_image_to_ascii_with_cell_backgrounds(&path, 0.5, 200, 200, Some(4), b" .M")?;
        assert!(frame.bg_rgb_colors.iter().all(|&b| b == 0), "expected all-black bg");
        assert!(frame.rgb_colors.iter().all(|&b| b == 0), "expected all-black fg");
        assert_eq!(first_glyph(&frame), ' ');
        Ok(())
    }

    #[test]
    fn bg_fit_defaults_match_when_thresholds_equal() -> Result<()> {
        // With bg_threshold == threshold, behaviour reduces to the legacy
        // single-threshold output: cells either fully present or fully empty.
        let (_dir, path) = write_uniform_test_image(128);
        let frame = fit_image_to_ascii_with_cell_backgrounds(&path, 0.5, 50, 50, Some(4), b" .M")?;
        // For every cell, bg is either entirely the cell colour or entirely black —
        // never partial.
        for chunk in frame.bg_rgb_colors.chunks_exact(3) {
            let all_zero = chunk[0] == 0 && chunk[1] == 0 && chunk[2] == 0;
            let any_colour = chunk[0] > 5 || chunk[1] > 5 || chunk[2] > 5;
            assert!(all_zero || any_colour);
        }
        Ok(())
    }
}
