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
    /// Same coverage quantized to 0..=255 for integer blending in the renderer
    alpha_u8: Vec<u8>,
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
    build_glyph_atlas_with_stroke(font_size, 0.0)
}

pub(crate) fn build_glyph_atlas_with_stroke(font_size: f32, text_stroke_width: f32) -> Result<GlyphAtlas> {
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

        thicken_glyph_alpha(&mut alpha, cell_width, cell_height, text_stroke_width);

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
        let alpha_u8 = alpha.iter().map(|value| (value * 255.0).round().clamp(0.0, 255.0) as u8).collect();

        glyphs.insert(byte, GlyphBitmap {alpha, alpha_u8, s_aa, s_ab, s_bb, det, degenerate});
    }

    Ok(GlyphAtlas {glyphs, cell_width, cell_height})
}

fn thicken_glyph_alpha(alpha: &mut [f32], cell_width: u32, cell_height: u32, text_stroke_width: f32) {
    let radius = text_stroke_width.clamp(0.0, 1.5);
    if radius <= 0.0 || cell_width == 0 || cell_height == 0 {
        return;
    }

    let source = alpha.to_vec();
    let radius_px = radius.ceil() as i32;
    let radius_squared = f64::from(radius) * f64::from(radius);

    for y in 0..cell_height as i32 {
        for x in 0..cell_width as i32 {
            let mut coverage = source[(y as u32 * cell_width + x as u32) as usize];

            for dy in -radius_px..=radius_px {
                for dx in -radius_px..=radius_px {
                    if dx == 0 && dy == 0 {
                        continue;
                    }

                    let distance_squared = f64::from(dx * dx + dy * dy);
                    if distance_squared > radius_squared + 0.0001 {
                        continue;
                    }

                    let sx = x + dx;
                    let sy = y + dy;
                    if sx < 0 || sy < 0 || sx >= cell_width as i32 || sy >= cell_height as i32 {
                        continue;
                    }

                    let neighbor = source[(sy as u32 * cell_width + sx as u32) as usize];
                    let distance = distance_squared.sqrt() as f32;
                    let strength = if radius >= distance {
                        1.0
                    } else {
                        (radius / distance).clamp(0.0, 1.0)
                    };
                    coverage = coverage.max(neighbor * strength);
                }
            }

            alpha[(y as u32 * cell_width + x as u32) as usize] = coverage;
        }
    }
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

pub(crate) fn render_ascii_frame_into_rgb(frame: &AsciiFrameData, atlas: &GlyphAtlas, use_colors: bool, buffer: &mut Vec<u8>) {
    let mut pixel_w = frame.width_chars * atlas.cell_width;
    let mut pixel_h = frame.height_chars * atlas.cell_height;

    // H.264 requires even dimensions
    if !pixel_w.is_multiple_of(2) {
        pixel_w += 1;
    }
    if !pixel_h.is_multiple_of(2) {
        pixel_h += 1;
    }

    buffer.clear();
    buffer.resize((pixel_w * pixel_h * 3) as usize, 0);

    let mut char_idx: usize = 0;
    let mut row: u32 = 0;
    let mut col: u32 = 0;

    for &byte in frame.ascii_text.as_bytes() {
        if byte == b'\n' {
            row += 1;
            col = 0;
            continue;
        }

        // Get color for this character
        let (r, g, b) = if use_colors && char_idx * 3 + 2 < frame.rgb_colors.len() {
            (frame.rgb_colors[char_idx * 3], frame.rgb_colors[char_idx * 3 + 1], frame.rgb_colors[char_idx * 3 + 2])
        } else {
            (255, 255, 255) // white for text-only mode
        };

        let base_x = col * atlas.cell_width;
        let base_y = row * atlas.cell_height;
        let x_end = (base_x + atlas.cell_width).min(pixel_w);
        let y_end = (base_y + atlas.cell_height).min(pixel_h);
        let cell_cols = (x_end - base_x) as usize;

        if char_idx * 3 + 2 < frame.bg_rgb_colors.len() {
            let bg = [frame.bg_rgb_colors[char_idx * 3], frame.bg_rgb_colors[char_idx * 3 + 1], frame.bg_rgb_colors[char_idx * 3 + 2]];
            for py in base_y..y_end {
                let offset = ((py * pixel_w + base_x) * 3) as usize;
                for pixel in buffer[offset..offset + cell_cols * 3].chunks_exact_mut(3) {
                    pixel.copy_from_slice(&bg);
                }
            }
        }

        // Look up glyph bitmap
        if let Some(glyph_bitmap) = atlas.glyphs.get(&byte) {
            for py in base_y..y_end {
                let alpha_row = ((py - base_y) * atlas.cell_width) as usize;
                let offset = ((py * pixel_w + base_x) * 3) as usize;
                for gx in 0..cell_cols {
                    let alpha = glyph_bitmap.alpha_u8[alpha_row + gx] as u32;
                    if alpha == 0 {
                        continue;
                    }
                    let pixel = offset + gx * 3;
                    if alpha == 255 {
                        buffer[pixel] = r;
                        buffer[pixel + 1] = g;
                        buffer[pixel + 2] = b;
                    } else {
                        buffer[pixel] = blend_channel(buffer[pixel], r, alpha);
                        buffer[pixel + 1] = blend_channel(buffer[pixel + 1], g, alpha);
                        buffer[pixel + 2] = blend_channel(buffer[pixel + 2], b, alpha);
                    }
                }
            }
        }

        char_idx += 1;
        col += 1;
    }
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
        img = dyn_img.resize_exact(target_w, target_h, image::imageops::FilterType::Triangle).to_rgb8();
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
            let mut total_luma = 0u64;
            let mut sum_rgb = [0u64; 3];
            let mut sum_sq = 0u64;

            for py in 0..atlas.cell_height {
                for px in 0..atlas.cell_width {
                    let pixel = *img.get_pixel(base_x + px, base_y + py);
                    total_luma += luminance(pixel) as u64;
                    sum_rgb[0] += pixel[0] as u64;
                    sum_rgb[1] += pixel[1] as u64;
                    sum_rgb[2] += pixel[2] as u64;
                    sum_sq += pixel[0] as u64 * pixel[0] as u64 + pixel[1] as u64 * pixel[1] as u64 + pixel[2] as u64 * pixel[2] as u64;
                    patch[patch_index] = pixel;
                    patch_index += 1;
                }
            }

            let avg_luma = total_luma as f64 / cell_pixels as f64;
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

            let sum_p = [sum_rgb[0] as f64, sum_rgb[1] as f64, sum_rgb[2] as f64];
            let sum_p_sq = sum_sq as f64;
            let mut best_byte = b' ';
            let mut best_fg = avg_rgb;
            let mut best_bg = avg_rgb;
            let mut best_error = f64::INFINITY;

            for &byte in &background_analysis.candidate_bytes {
                if let Some(glyph) = atlas.glyphs.get(&byte) {
                    let (fg, bg, error) = fit_colors_for_glyph(&patch, glyph, avg_rgb, sum_p, sum_p_sq);
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

fn blend_channel(background: u8, foreground: u8, alpha: u32) -> u8 {
    ((background as u32 * (255 - alpha) + foreground as u32 * alpha + 127) / 255) as u8
}

// The fit error Σ(pred − p)² is expanded algebraically from the accumulated sums (pred = a·fg + b·bg, b = 1 − a, s_bp = Σp − s_ap),
// so no second pass over the patch is needed: error = Σp² − 2(fg·s_ap + bg·s_bp) + fg²·s_aa + 2·fg·bg·s_ab + bg²·s_bb per channel.
fn fit_colors_for_glyph(patch: &[Rgb<u8>], glyph: &GlyphBitmap, avg_rgb: [u8; 3], sum_p: [f64; 3], sum_p_sq: f64) -> ([u8; 3], [u8; 3], f64) {
    if glyph.degenerate {
        return (avg_rgb, avg_rgb, constant_patch_error(patch.len(), avg_rgb, sum_p, sum_p_sq));
    }

    let mut s_ap = [0.0f64; 3];
    for (pixel, &value) in patch.iter().zip(glyph.alpha.iter()) {
        if value == 0.0 {
            continue;
        }
        let a = value as f64;
        s_ap[0] += a * pixel[0] as f64;
        s_ap[1] += a * pixel[1] as f64;
        s_ap[2] += a * pixel[2] as f64;
    }

    let mut fg = [0u8; 3];
    let mut bg = [0u8; 3];
    let mut error = sum_p_sq;
    for channel in 0..3 {
        let s_bp = sum_p[channel] - s_ap[channel];
        let fg_value = ((s_ap[channel] * glyph.s_bb) - (s_bp * glyph.s_ab)) / glyph.det;
        let bg_value = ((s_bp * glyph.s_aa) - (s_ap[channel] * glyph.s_ab)) / glyph.det;
        fg[channel] = fg_value.clamp(0.0, 255.0).round() as u8;
        bg[channel] = bg_value.clamp(0.0, 255.0).round() as u8;
        let fg_f = fg[channel] as f64;
        let bg_f = bg[channel] as f64;
        error += fg_f * fg_f * glyph.s_aa + 2.0 * fg_f * bg_f * glyph.s_ab + bg_f * bg_f * glyph.s_bb - 2.0 * (fg_f * s_ap[channel] + bg_f * s_bp);
    }
    (fg, bg, error)
}

fn constant_patch_error(cell_pixels: usize, color: [u8; 3], sum_p: [f64; 3], sum_p_sq: f64) -> f64 {
    let mut error = sum_p_sq;
    for channel in 0..3 {
        let c = color[channel] as f64;
        error += cell_pixels as f64 * c * c - 2.0 * c * sum_p[channel];
    }
    error
}

fn luminance(rgb: Rgb<u8>) -> u8 {
    ((2126 * rgb[0] as u32 + 7152 * rgb[1] as u32 + 722 * rgb[2] as u32) / 10000) as u8
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
        let mut buffer = Vec::new();
        render_ascii_frame_into_rgb(&frame, &atlas, false, &mut buffer);
        assert!(buffer.chunks_exact(3).any(|pixel| pixel[0] > 200 && pixel[1] < 16 && pixel[2] < 16));
        Ok(())
    }

    #[test]
    fn blends_foreground_glyph_over_background() -> Result<()> {
        let atlas = build_glyph_atlas(12.0)?;
        let frame = AsciiFrameData {ascii_text: "M\n".to_string(), width_chars: 1, height_chars: 1, rgb_colors: vec![0, 255, 0], bg_rgb_colors: vec![0, 0, 255]};
        let mut buffer = Vec::new();
        render_ascii_frame_into_rgb(&frame, &atlas, true, &mut buffer);
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
