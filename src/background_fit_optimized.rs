use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use anyhow::{anyhow, Context, Result};
use image::{DynamicImage, Rgb};
use rayon::prelude::*;
use std::path::Path;

use crate::convert::AsciiFrameData;

const FONT_DATA: &[u8] = include_bytes!("../resources/DejaVuSansMono.ttf");
const ANALYSIS_FONT_SIZE: f32 = 16.0;

#[derive(Debug)]
struct OptimizedGlyph {
    byte: u8,
    alpha: Vec<f32>,
    s_aa: f64,
    s_ab: f64,
    s_bb: f64,
    determinant: f64,
    degenerate: bool,
}

#[derive(Debug)]
pub(crate) struct OptimizedBackgroundAnalysisContext {
    glyphs: Vec<OptimizedGlyph>,
    pub(crate) cell_width: u32,
    pub(crate) cell_height: u32,
}

struct ConvertedRow {
    ascii: Vec<u8>,
    foreground: Vec<u8>,
    background: Vec<u8>,
}

pub(crate) fn background_analysis_context(ascii_chars: &[u8]) -> Result<OptimizedBackgroundAnalysisContext> {
    let font = FontRef::try_from_slice(FONT_DATA).map_err(|error| anyhow!("failed to load embedded font: {error}"))?;
    let scale = PxScale::from(ANALYSIS_FONT_SIZE);
    let scaled_font = font.as_scaled(scale);
    let cell_width = scaled_font.h_advance(font.glyph_id('M')).ceil() as u32;
    let cell_height = (scaled_font.ascent() - scaled_font.descent()).ceil() as u32;
    let ascent = scaled_font.ascent();

    let mut glyphs = Vec::with_capacity(ascii_chars.len());
    for &byte in ascii_chars.iter().filter(|byte| **byte != b' ') {
        let glyph = font.glyph_id(byte as char).with_scale_and_position(scale, ab_glyph::point(0.0, ascent));
        let mut alpha = vec![0.0f32; (cell_width * cell_height) as usize];
        if let Some(outlined) = font.outline_glyph(glyph) {
            outlined.draw(|x, y, coverage| {
                if x < cell_width && y < cell_height {
                    alpha[(y * cell_width + x) as usize] = coverage;
                }
            });
        }

        let mut s_aa = 0.0f64;
        let mut s_ab = 0.0f64;
        let mut s_bb = 0.0f64;
        let mut sum_alpha = 0.0f64;
        for &value in &alpha {
            let alpha = value as f64;
            let inverse = 1.0 - alpha;
            sum_alpha += alpha;
            s_aa += alpha * alpha;
            s_ab += alpha * inverse;
            s_bb += inverse * inverse;
        }
        let mean_alpha = sum_alpha / alpha.len().max(1) as f64;
        let determinant = s_aa * s_bb - s_ab * s_ab;
        glyphs.push(OptimizedGlyph {byte, alpha, s_aa, s_ab, s_bb, determinant, degenerate: mean_alpha <= 1e-6 || mean_alpha >= 1.0 - 1e-6 || determinant.abs() <= 1e-9});
    }

    if glyphs.is_empty() {
        glyphs.push(OptimizedGlyph {byte: b' ', alpha: vec![0.0; (cell_width * cell_height) as usize], s_aa: 0.0, s_ab: 0.0, s_bb: (cell_width * cell_height) as f64, determinant: 0.0, degenerate: true});
    }

    Ok(OptimizedBackgroundAnalysisContext {glyphs, cell_width, cell_height})
}

pub(crate) fn fit_image_to_ascii_with_cell_backgrounds(image_path: &Path, font_ratio: f32, threshold: u8, background_threshold: u8, columns: Option<u32>, ascii_chars: &[u8]) -> Result<AsciiFrameData> {
    let context = background_analysis_context(ascii_chars)?;
    fit_image_to_ascii_with_cell_backgrounds_with_context(image_path, font_ratio, threshold, background_threshold, columns, &context)
}

pub(crate) fn fit_image_to_ascii_with_cell_backgrounds_with_context(image_path: &Path, font_ratio: f32, threshold: u8, background_threshold: u8, columns: Option<u32>, context: &OptimizedBackgroundAnalysisContext) -> Result<AsciiFrameData> {
    let mut image = image::open(image_path).with_context(|| format!("opening {}", image_path.display()))?.to_rgb8();
    let (original_width, original_height) = image.dimensions();
    let (width_chars, height_chars) = if let Some(columns) = columns {
        let rows = (original_height as f32 / original_width as f32 * columns as f32 * font_ratio).round() as u32;
        (columns, rows.max(1))
    } else {
        let rows = (original_height as f32 * font_ratio).round() as u32;
        (original_width, rows.max(1))
    };

    let target_width = width_chars * context.cell_width;
    let target_height = height_chars * context.cell_height;
    if image.dimensions() != (target_width, target_height) {
        image = DynamicImage::ImageRgb8(image).resize_exact(target_width, target_height, image::imageops::FilterType::Lanczos3).to_rgb8();
    }

    let rows: Vec<ConvertedRow> = (0..height_chars).into_par_iter().map(|row| convert_row(&image, row, width_chars, threshold, background_threshold, context)).collect();

    let cell_count = (width_chars * height_chars) as usize;
    let mut ascii_text = String::with_capacity(cell_count + height_chars as usize);
    let mut rgb_colors = Vec::with_capacity(cell_count * 3);
    let mut bg_rgb_colors = Vec::with_capacity(cell_count * 3);
    for row in rows {
        for byte in row.ascii {
            ascii_text.push(byte as char);
        }
        ascii_text.push('\n');
        rgb_colors.extend_from_slice(&row.foreground);
        bg_rgb_colors.extend_from_slice(&row.background);
    }

    Ok(AsciiFrameData {ascii_text, width_chars, height_chars, rgb_colors, bg_rgb_colors})
}

fn convert_row(image: &image::RgbImage, row: u32, width_chars: u32, threshold: u8, background_threshold: u8, context: &OptimizedBackgroundAnalysisContext) -> ConvertedRow {
    let cell_pixels = (context.cell_width * context.cell_height) as usize;
    let mut ascii = Vec::with_capacity(width_chars as usize);
    let mut foreground = Vec::with_capacity(width_chars as usize * 3);
    let mut background = Vec::with_capacity(width_chars as usize * 3);
    let mut patch = vec![Rgb([0u8; 3]); cell_pixels];

    for column in 0..width_chars {
        let mut total_luminance = 0.0f64;
        let mut sum_rgb = [0u64; 3];
        let mut patch_index = 0usize;
        let base_x = column * context.cell_width;
        let base_y = row * context.cell_height;

        for y in 0..context.cell_height {
            for x in 0..context.cell_width {
                let pixel = *image.get_pixel(base_x + x, base_y + y);
                total_luminance += luminance(pixel) as f64;
                sum_rgb[0] += pixel[0] as u64;
                sum_rgb[1] += pixel[1] as u64;
                sum_rgb[2] += pixel[2] as u64;
                patch[patch_index] = pixel;
                patch_index += 1;
            }
        }

        let average_luminance = total_luminance / cell_pixels as f64;
        let emit_foreground = average_luminance >= threshold as f64;
        let emit_background = average_luminance >= background_threshold as f64;
        if !emit_foreground && !emit_background {
            ascii.push(b' ');
            foreground.extend_from_slice(&[0, 0, 0]);
            background.extend_from_slice(&[0, 0, 0]);
            continue;
        }

        let average_rgb = [(sum_rgb[0] / cell_pixels as u64) as u8, (sum_rgb[1] / cell_pixels as u64) as u8, (sum_rgb[2] / cell_pixels as u64) as u8];
        if !emit_foreground {
            ascii.push(b' ');
            foreground.extend_from_slice(&[0, 0, 0]);
            background.extend_from_slice(&average_rgb);
            continue;
        }

        let mut best_byte = b' ';
        let mut best_foreground = average_rgb;
        let mut best_background = average_rgb;
        let mut best_error = f64::INFINITY;
        for glyph in &context.glyphs {
            let (fitted_foreground, fitted_background, error) = fit_colors(&patch, glyph, average_rgb);
            if error < best_error {
                best_byte = glyph.byte;
                best_foreground = fitted_foreground;
                best_background = fitted_background;
                best_error = error;
            }
        }

        ascii.push(best_byte);
        foreground.extend_from_slice(&best_foreground);
        if emit_background {
            background.extend_from_slice(&best_background);
        } else {
            background.extend_from_slice(&[0, 0, 0]);
        }
    }

    ConvertedRow {ascii, foreground, background}
}

fn fit_colors(patch: &[Rgb<u8>], glyph: &OptimizedGlyph, average_rgb: [u8; 3]) -> ([u8; 3], [u8; 3], f64) {
    if glyph.degenerate {
        return (average_rgb, average_rgb, constant_patch_error(patch, average_rgb));
    }

    let mut sum_alpha_pixel = [0.0f64; 3];
    let mut sum_inverse_pixel = [0.0f64; 3];
    for (pixel, &value) in patch.iter().zip(&glyph.alpha) {
        let alpha = value as f64;
        let inverse = 1.0 - alpha;
        for channel in 0..3 {
            let pixel = pixel[channel] as f64;
            sum_alpha_pixel[channel] += alpha * pixel;
            sum_inverse_pixel[channel] += inverse * pixel;
        }
    }

    let mut foreground = [0u8; 3];
    let mut background = [0u8; 3];
    for channel in 0..3 {
        let foreground_value = (sum_alpha_pixel[channel] * glyph.s_bb - sum_inverse_pixel[channel] * glyph.s_ab) / glyph.determinant;
        let background_value = (sum_inverse_pixel[channel] * glyph.s_aa - sum_alpha_pixel[channel] * glyph.s_ab) / glyph.determinant;
        foreground[channel] = foreground_value.clamp(0.0, 255.0).round() as u8;
        background[channel] = background_value.clamp(0.0, 255.0).round() as u8;
    }

    let mut error = 0.0f64;
    for (pixel, &value) in patch.iter().zip(&glyph.alpha) {
        let alpha = value as f64;
        let inverse = 1.0 - alpha;
        for channel in 0..3 {
            let predicted = alpha * foreground[channel] as f64 + inverse * background[channel] as f64;
            let difference = predicted - pixel[channel] as f64;
            error += difference * difference;
        }
    }
    (foreground, background, error)
}

fn constant_patch_error(patch: &[Rgb<u8>], color: [u8; 3]) -> f64 {
    let mut error = 0.0f64;
    for pixel in patch {
        for channel in 0..3 {
            let difference = color[channel] as f64 - pixel[channel] as f64;
            error += difference * difference;
        }
    }
    error
}

fn luminance(pixel: Rgb<u8>) -> u8 {
    let red = pixel[0] as f64;
    let green = pixel[1] as f64;
    let blue = pixel[2] as f64;
    (0.2126 * red + 0.7152 * green + 0.0722 * blue) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimized_context_preserves_candidate_order() {
        let context = background_analysis_context(b" A#A").unwrap();
        let bytes: Vec<u8> = context.glyphs.iter().map(|glyph| glyph.byte).collect();
        assert_eq!(bytes, vec![b'A', b'#', b'A']);
    }

    #[test]
    fn optimized_output_matches_legacy_output() {
        let width = 37;
        let height = 29;
        let image = image::RgbImage::from_fn(width, height, |x, y| Rgb([((x * 17 + y * 3) % 256) as u8, ((x * 5 + y * 23) % 256) as u8, ((x * 11 + y * 7) % 256) as u8]));
        let input = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        image.save_with_format(input.path(), image::ImageFormat::Png).unwrap();
        let ascii_chars = b" .:-=+*#%@";

        let legacy = crate::render::fit_image_to_ascii_with_cell_backgrounds(input.path(), 0.7, 20, 20, Some(24), ascii_chars).unwrap();
        let optimized = fit_image_to_ascii_with_cell_backgrounds(input.path(), 0.7, 20, 20, Some(24), ascii_chars).unwrap();

        assert_eq!(optimized.ascii_text, legacy.ascii_text);
        assert_eq!(optimized.rgb_colors, legacy.rgb_colors);
        assert_eq!(optimized.bg_rgb_colors, legacy.bg_rgb_colors);
    }
}
