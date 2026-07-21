//! In-memory single-image conversion.
//!
//! Everything in this module works on bytes and decoded images only — no filesystem, no
//! subprocesses, no threads — so it compiles and runs on any target, including
//! `wasm32-unknown-unknown` (build the crate with `default-features = false`).

use anyhow::{bail, Context, Result};
use image::{DynamicImage, RgbImage};

use crate::cell_filter::luminance_rgb;
use crate::{CellColorMode, ConversionOptions};

/// Trailing payload flag bits.
///
/// Stored as the first byte of the optional extension area that follows the legacy `8 + w*h*4` block. Each bit announces an optional payload that
/// follows in a fixed order (lowest bit = earliest payload). Adding a new payload is a forward-compatible change as long as the new bit is appended.
pub(crate) const CFRAME_EXT_FLAG_HAS_BG: u8 = 0b0000_0001;

/// A single converted ASCII frame held in memory.
pub struct ImageFrame {
    /// The ASCII text, rows separated by `\n`
    pub text: String,
    /// Width in characters
    pub width: u32,
    /// Height in characters (rows)
    pub height: u32,
    /// Flat RGB color data, 3 bytes per character, row-major
    pub rgb: Vec<u8>,
}

impl ImageFrame {
    /// Encode this frame as `.cframe` bytes (foreground colors only).
    pub fn cframe_bytes(&self) -> Vec<u8> {
        encode_cframe(self.width, self.height, &self.text, &self.rgb, None)
    }
}

/// Convert encoded image bytes (PNG or JPEG) into an in-memory ASCII frame.
pub fn image_bytes_to_frame(bytes: &[u8], options: &ConversionOptions) -> Result<ImageFrame> {
    let image = image::load_from_memory(bytes).context("decoding image bytes")?;
    image_to_frame(&image, options)
}

/// Convert an already-decoded image into an in-memory ASCII frame.
///
/// Only `CellColorMode::ForegroundOnly` is supported here; the background-fitting modes live in the filesystem pipeline.
pub fn image_to_frame(image: &DynamicImage, options: &ConversionOptions) -> Result<ImageFrame> {
    if options.cell_color_mode != CellColorMode::ForegroundOnly {
        bail!("in-memory conversion supports only CellColorMode::ForegroundOnly");
    }
    if options.ascii_chars.is_empty() {
        bail!("ascii_chars must not be empty");
    }
    let (text, width, height, rgb) = rgb_image_to_ascii_with_colors(image.to_rgb8(), options.font_ratio, options.luminance, options.columns, options.ascii_chars.as_bytes());
    Ok(ImageFrame {text, width, height, rgb})
}

/// Returns (ascii_string, width, height, rgb_bytes)
/// rgb_bytes is a flat Vec<u8> with 3 bytes (R, G, B) per character, row-major order
pub(crate) fn rgb_image_to_ascii_with_colors(mut img: RgbImage, font_ratio: f32, threshold: u8, columns: Option<u32>, ascii_chars: &[u8]) -> (String, u32, u32, Vec<u8>) {
    let (orig_w, orig_h) = img.dimensions();
    let (target_w, target_h) = if let Some(cols) = columns {
        let w = cols;
        let h = (orig_h as f32 / orig_w as f32 * cols as f32 * font_ratio).round() as u32;
        (w, h.max(1))
    } else {
        let w = orig_w;
        let h = (orig_h as f32 * font_ratio).round() as u32;
        (w, h.max(1))
    };

    if target_w != orig_w || target_h != orig_h {
        let dyn_img = DynamicImage::ImageRgb8(img);
        img = dyn_img.resize_exact(target_w, target_h, image::imageops::FilterType::Triangle).to_rgb8();
    }

    let (w, h) = img.dimensions();
    let rgb_data = img.into_raw();
    let mut out = String::with_capacity((w as usize + 1) * (h as usize));
    for row in rgb_data.chunks_exact(w as usize * 3) {
        for px in row.chunks_exact(3) {
            let l = luminance_rgb(px[0], px[1], px[2]);
            out.push(char_for(l, threshold, ascii_chars));
        }
        out.push('\n');
    }
    (out, w, h, rgb_data)
}

pub(crate) fn char_for(luma: u8, threshold: u8, ascii_chars: &[u8]) -> char {
    if luma < threshold {
        return ' ';
    }

    let effective_luma = (luma as u32).saturating_sub(threshold as u32);
    let range = (255u32).saturating_sub(threshold as u32).max(1);
    let num_chars_minus_1 = (ascii_chars.len() as u32).saturating_sub(1);

    let idx = (effective_luma * num_chars_minus_1) / range;
    let idx = idx.min(num_chars_minus_1) as usize;

    ascii_chars[idx] as char
}

/// Encode the combined binary format (.cframe): text + color in one buffer.
///
/// Layout:
/// 1. Header (8 bytes): `width: u32 LE` + `height: u32 LE`
/// 2. Body (`width * height * 4` bytes): `char: u8 + r: u8 + g: u8 + b: u8` per cell, row-major
/// 3. Optional extension area:
///    - `flags: u8` — bit 0 (`CFRAME_EXT_FLAG_HAS_BG`) announces a background payload
///    - if `flags & HAS_BG`: `width * height * 3` bytes of background RGB, row-major
///
/// Older readers that don't know about the extension still parse the body correctly and ignore the trailing bytes. New readers detect the extension
/// by looking past the legacy body for the `flags` byte instead of inferring payload presence from total file length.
pub(crate) fn encode_cframe(width: u32, height: u32, ascii_content: &str, rgb_data: &[u8], bg_rgb_data: Option<&[u8]>) -> Vec<u8> {
    let cell_count = (width * height) as usize;
    let mut output = Vec::with_capacity(8 + cell_count * 4 + bg_rgb_data.map_or(0, |background| 1 + background.len()));
    output.extend_from_slice(&width.to_le_bytes());
    output.extend_from_slice(&height.to_le_bytes());

    for (char_idx, byte) in ascii_content.bytes().filter(|byte| *byte != b'\n').enumerate() {
        let rgb_offset = char_idx * 3;
        output.extend_from_slice(&[byte, rgb_data[rgb_offset], rgb_data[rgb_offset + 1], rgb_data[rgb_offset + 2]]);
    }
    if let Some(bg_rgb_data) = bg_rgb_data {
        output.push(CFRAME_EXT_FLAG_HAS_BG);
        output.extend_from_slice(bg_rgb_data);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputMode;

    fn gradient_image(width: u32, height: u32) -> DynamicImage {
        let mut img = RgbImage::new(width, height);
        for (x, _y, pixel) in img.enumerate_pixels_mut() {
            let value = ((x * 255) / width.max(1)) as u8;
            *pixel = image::Rgb([value, value, value]);
        }
        DynamicImage::ImageRgb8(img)
    }

    fn options() -> ConversionOptions {
        ConversionOptions {columns: Some(8), font_ratio: 1.0, ..ConversionOptions::default()}
    }

    #[test]
    fn test_char_for_thresholds() {
        let chars = b" .:#";
        assert_eq!(char_for(0, 10, chars), ' ');
        assert_eq!(char_for(9, 10, chars), ' ');
        assert_eq!(char_for(255, 10, chars), '#');
    }

    #[test]
    fn test_image_to_frame_dimensions_and_payloads() {
        let frame = image_to_frame(&gradient_image(16, 16), &options()).expect("conversion should succeed");
        assert_eq!(frame.width, 8);
        assert_eq!(frame.height, 8);
        assert_eq!(frame.text.lines().count(), 8);
        assert!(frame.text.lines().all(|line| line.chars().count() == 8));
        assert_eq!(frame.rgb.len(), 8 * 8 * 3);
    }

    #[test]
    fn test_cframe_bytes_layout() {
        let frame = image_to_frame(&gradient_image(16, 16), &options()).expect("conversion should succeed");
        let bytes = frame.cframe_bytes();
        assert_eq!(bytes.len(), 8 + 8 * 8 * 4);
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 8);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 8);
    }

    #[test]
    fn test_image_bytes_to_frame_decodes_png() {
        let mut png = Vec::new();
        gradient_image(16, 16).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("png encoding should succeed");
        let frame = image_bytes_to_frame(&png, &options()).expect("conversion should succeed");
        assert_eq!((frame.width, frame.height), (8, 8));
    }

    #[test]
    fn test_image_to_frame_rejects_background_modes() {
        let unsupported = ConversionOptions {cell_color_mode: crate::CellColorMode::FitForegroundBackground, output_mode: OutputMode::ColorOnly, ..options()};
        assert!(image_to_frame(&gradient_image(4, 4), &unsupported).is_err());
    }

    #[test]
    fn test_encode_cframe_with_background_extension() {
        let bytes = encode_cframe(2, 1, "ab\n", &[1, 2, 3, 4, 5, 6], Some(&[7, 8, 9, 10, 11, 12]));
        assert_eq!(bytes.len(), 8 + 2 * 4 + 1 + 6);
        assert_eq!(bytes[8..12], [b'a', 1, 2, 3]);
        assert_eq!(bytes[16], CFRAME_EXT_FLAG_HAS_BG);
    }
}
