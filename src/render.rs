use ab_glyph::{FontRef, PxScale, ScaleFont};
use anyhow::{anyhow, Context, Result};
use image::{DynamicImage, Rgb};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command as ProcCommand, Stdio};

use crate::convert::AsciiFrameData;
use crate::FfmpegConfig;

/// Embedded monospace font for video rendering
const FONT_DATA: &[u8] = include_bytes!("../resources/DejaVuSansMono.ttf");
const ANALYSIS_FONT_SIZE: f32 = 16.0;

/// Pre-rasterized bitmap for a single glyph
struct GlyphBitmap {
    /// Alpha coverage values, row-major, cell_width * cell_height entries
    alpha: Vec<f32>,
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

        glyphs.insert(byte, GlyphBitmap { alpha });
    }

    Ok(GlyphAtlas {glyphs, cell_width, cell_height})
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

pub(crate) fn fit_image_to_ascii_with_cell_backgrounds(img_path: &Path, font_ratio: f32, threshold: u8, columns: Option<u32>, ascii_chars: &[u8]) -> Result<AsciiFrameData> {
    let atlas = build_glyph_atlas(ANALYSIS_FONT_SIZE)?;
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

    let candidate_bytes: Vec<u8> = ascii_chars.iter().copied().filter(|byte| *byte != b' ').collect();
    let candidate_bytes = if candidate_bytes.is_empty() { vec![b' '] } else { candidate_bytes };
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

            for py in 0..atlas.cell_height {
                for px in 0..atlas.cell_width {
                    let pixel = *img.get_pixel(base_x + px, base_y + py);
                    total_luma += luminance(pixel) as f64;
                    patch[patch_index] = pixel;
                    patch_index += 1;
                }
            }

            let avg_luma = total_luma / cell_pixels as f64;
            if avg_luma < threshold as f64 {
                ascii_text.push(' ');
                rgb_colors.extend_from_slice(&[0, 0, 0]);
                bg_rgb_colors.extend_from_slice(&[0, 0, 0]);
                continue;
            }

            let avg_rgb = average_rgb(&patch);
            let mut best_byte = b' ';
            let mut best_fg = avg_rgb;
            let mut best_bg = avg_rgb;
            let mut best_error = f64::INFINITY;

            for &byte in &candidate_bytes {
                if let Some(glyph) = atlas.glyphs.get(&byte) {
                    let (fg, bg, error) = fit_colors_for_glyph(&patch, &glyph.alpha);
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
            bg_rgb_colors.extend_from_slice(&best_bg);
        }
        ascii_text.push('\n');
    }

    Ok(AsciiFrameData { ascii_text, width_chars, height_chars, rgb_colors, bg_rgb_colors })
}

fn blend_channel(background: u8, foreground: u8, alpha: f32) -> u8 {
    ((background as f32 * (1.0 - alpha)) + (foreground as f32 * alpha)).round().clamp(0.0, 255.0) as u8
}

fn average_rgb(patch: &[Rgb<u8>]) -> [u8; 3] {
    let mut sum = [0u64; 3];
    for pixel in patch {
        sum[0] += pixel[0] as u64;
        sum[1] += pixel[1] as u64;
        sum[2] += pixel[2] as u64;
    }
    let len = patch.len().max(1) as u64;
    [(sum[0] / len) as u8, (sum[1] / len) as u8, (sum[2] / len) as u8]
}

fn fit_colors_for_glyph(patch: &[Rgb<u8>], alpha: &[f32]) -> ([u8; 3], [u8; 3], f64) {
    let avg_rgb = average_rgb(patch);
    let mean_alpha = alpha.iter().map(|value| *value as f64).sum::<f64>() / alpha.len().max(1) as f64;
    if mean_alpha <= 1e-6 || mean_alpha >= 1.0 - 1e-6 {
        return (avg_rgb, avg_rgb, constant_patch_error(patch, avg_rgb));
    }

    let mut s_aa = 0.0f64;
    let mut s_ab = 0.0f64;
    let mut s_bb = 0.0f64;
    for &value in alpha {
        let a = value as f64;
        let b = 1.0 - a;
        s_aa += a * a;
        s_ab += a * b;
        s_bb += b * b;
    }

    let det = s_aa * s_bb - s_ab * s_ab;
    if det.abs() <= 1e-9 {
        return (avg_rgb, avg_rgb, constant_patch_error(patch, avg_rgb));
    }

    let mut fg = [0u8; 3];
    let mut bg = [0u8; 3];
    for channel in 0..3 {
        let mut s_ap = 0.0f64;
        let mut s_bp = 0.0f64;
        for (pixel, &value) in patch.iter().zip(alpha.iter()) {
            let a = value as f64;
            let b = 1.0 - a;
            let p = pixel[channel] as f64;
            s_ap += a * p;
            s_bp += b * p;
        }

        let fg_value = ((s_ap * s_bb) - (s_bp * s_ab)) / det;
        let bg_value = ((s_bp * s_aa) - (s_ap * s_ab)) / det;
        fg[channel] = fg_value.clamp(0.0, 255.0).round() as u8;
        bg[channel] = bg_value.clamp(0.0, 255.0).round() as u8;
    }

    let mut error = 0.0f64;
    for (pixel, &value) in patch.iter().zip(alpha.iter()) {
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
        let frame = AsciiFrameData { ascii_text: " \n".to_string(), width_chars: 1, height_chars: 1, rgb_colors: Vec::new(), bg_rgb_colors: vec![255, 0, 0] };
        let buffer = render_ascii_frame_to_rgb(&frame, &atlas, false);
        assert!(buffer.chunks_exact(3).any(|pixel| pixel[0] > 200 && pixel[1] < 16 && pixel[2] < 16));
        Ok(())
    }

    #[test]
    fn blends_foreground_glyph_over_background() -> Result<()> {
        let atlas = build_glyph_atlas(12.0)?;
        let frame = AsciiFrameData { ascii_text: "M\n".to_string(), width_chars: 1, height_chars: 1, rgb_colors: vec![0, 255, 0], bg_rgb_colors: vec![0, 0, 255] };
        let buffer = render_ascii_frame_to_rgb(&frame, &atlas, true);
        assert!(buffer.chunks_exact(3).any(|pixel| pixel[1] == 0 && pixel[2] > 200));
        assert!(buffer.chunks_exact(3).any(|pixel| pixel[1] > 0 && pixel[2] < 255));
        Ok(())
    }
}
