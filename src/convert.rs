use anyhow::{anyhow, Context, Result};
use image::DynamicImage;
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::{render, CellColorMode, OutputMode, Progress};

/// Intermediate representation of one converted ASCII frame
pub(crate) struct AsciiFrameData {
    /// The ASCII text (with newlines between rows)
    pub(crate) ascii_text: String,
    /// Width in characters
    pub(crate) width_chars: u32,
    /// Height in characters (rows)
    pub(crate) height_chars: u32,
    /// Flat RGB color data, 3 bytes per character, row-major
    pub(crate) rgb_colors: Vec<u8>,
    /// Optional per-cell background RGB data, 3 bytes per character, row-major
    pub(crate) bg_rgb_colors: Vec<u8>,
}

pub(crate) fn image_to_ascii_frame_data(img_path: &Path, font_ratio: f32, threshold: u8, columns: Option<u32>, ascii_chars: &[u8], cell_color_mode: CellColorMode) -> Result<AsciiFrameData> {
    match cell_color_mode {
        CellColorMode::ForegroundOnly => {
            let (ascii_text, width_chars, height_chars, rgb_colors) = image_to_ascii_with_colors(img_path, font_ratio, threshold, columns, ascii_chars)?;
            Ok(AsciiFrameData { ascii_text, width_chars, height_chars, rgb_colors, bg_rgb_colors: Vec::new() })
        }
        CellColorMode::FitForegroundBackground => render::fit_image_to_ascii_with_cell_backgrounds(img_path, font_ratio, threshold, columns, ascii_chars),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn convert_image_to_ascii(img_path: &Path, out_txt: &Path, font_ratio: f32, threshold: u8, columns: Option<u32>, ascii_chars: &[u8], output_mode: &OutputMode, cell_color_mode: CellColorMode) -> Result<()> {
    match output_mode {
        OutputMode::TextOnly => {
            let ascii_string = image_to_ascii_string(img_path, font_ratio, threshold, columns, ascii_chars)?;
            fs::write(out_txt, ascii_string).with_context(|| format!("writing {}", out_txt.display()))?;
        }
        OutputMode::ColorOnly => {
            let frame = image_to_ascii_frame_data(img_path, font_ratio, threshold, columns, ascii_chars, cell_color_mode)?;
            let cframe_path = out_txt.with_extension("cframe");
            write_cframe_binary(frame.width_chars, frame.height_chars, &frame.ascii_text, &frame.rgb_colors, if frame.bg_rgb_colors.is_empty() { None } else { Some(frame.bg_rgb_colors.as_slice()) }, &cframe_path)?;
        }
        OutputMode::TextAndColor => {
            let frame = image_to_ascii_frame_data(img_path, font_ratio, threshold, columns, ascii_chars, cell_color_mode)?;
            fs::write(out_txt, &frame.ascii_text).with_context(|| format!("writing {}", out_txt.display()))?;
            let cframe_path = out_txt.with_extension("cframe");
            write_cframe_binary(frame.width_chars, frame.height_chars, &frame.ascii_text, &frame.rgb_colors, if frame.bg_rgb_colors.is_empty() { None } else { Some(frame.bg_rgb_colors.as_slice()) }, &cframe_path)?;
        }
    }
    Ok(())
}

pub(crate) fn image_to_ascii_string(img_path: &Path, font_ratio: f32, threshold: u8, columns: Option<u32>, ascii_chars: &[u8]) -> Result<String> {
    let mut img = image::open(img_path)
        .with_context(|| format!("opening {}", img_path.display()))?
        .to_rgb8();

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
        img = dyn_img.resize_exact(target_w, target_h, image::imageops::FilterType::Lanczos3).to_rgb8();
    }

    let (w, h) = img.dimensions();
    let mut out = String::with_capacity((w as usize + 1) * (h as usize));
    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x, y);
            let l = luminance(*px);
            out.push(char_for(l, threshold, ascii_chars));
        }
        out.push('\n');
    }
    Ok(out)
}

/// Returns (ascii_string, width, height, rgb_bytes)
/// rgb_bytes is a flat Vec<u8> with 3 bytes (R, G, B) per character, row-major order
pub(crate) fn image_to_ascii_with_colors(img_path: &Path, font_ratio: f32, threshold: u8, columns: Option<u32>, ascii_chars: &[u8]) -> Result<(String, u32, u32, Vec<u8>)> {
    let mut img = image::open(img_path).with_context(|| format!("opening {}", img_path.display()))?.to_rgb8();

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
        img = dyn_img.resize_exact(target_w, target_h, image::imageops::FilterType::Lanczos3).to_rgb8();
    }

    let (w, h) = img.dimensions();
    let mut out = String::with_capacity((w as usize + 1) * (h as usize));
    let mut rgb_data: Vec<u8> = Vec::with_capacity((w as usize) * (h as usize) * 3);

    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x, y);
            let l = luminance(*px);
            out.push(char_for(l, threshold, ascii_chars));
            rgb_data.push(px[0]);
            rgb_data.push(px[1]);
            rgb_data.push(px[2]);
        }
        out.push('\n');
    }
    Ok((out, w, h, rgb_data))
}

/// Trailing payload flag bits.
///
/// Stored as the first byte of the optional extension area that follows the
/// legacy `8 + w*h*4` block. Each bit announces an optional payload that
/// follows in a fixed order (lowest bit = earliest payload). Adding a new
/// payload is a forward-compatible change as long as the new bit is appended.
pub(crate) const CFRAME_EXT_FLAG_HAS_BG: u8 = 0b0000_0001;

/// Combined binary format (.cframe): text + color in one file.
///
/// Layout:
/// 1. Header (8 bytes): `width: u32 LE` + `height: u32 LE`
/// 2. Body (`width * height * 4` bytes): `char: u8 + r: u8 + g: u8 + b: u8` per cell, row-major
/// 3. Optional extension area:
///    - `flags: u8` — bit 0 (`CFRAME_EXT_FLAG_HAS_BG`) announces a background payload
///    - if `flags & HAS_BG`: `width * height * 3` bytes of background RGB, row-major
///
/// Older readers that don't know about the extension still parse the body
/// correctly and ignore the trailing bytes. New readers detect the extension
/// by looking past the legacy body for the `flags` byte instead of inferring
/// payload presence from total file length.
pub(crate) fn write_cframe_binary(width: u32, height: u32, ascii_content: &str, rgb_data: &[u8], bg_rgb_data: Option<&[u8]>, path: &Path) -> Result<()> {
    use std::io::Write;
    let mut file = fs::File::create(path).with_context(|| format!("creating cframe file {}", path.display()))?;
    file.write_all(&width.to_le_bytes())?;
    file.write_all(&height.to_le_bytes())?;

    let mut char_idx = 0;
    for ch in ascii_content.chars() {
        if ch == '\n' {
            continue;
        }
        let rgb_offset = char_idx * 3;
        file.write_all(&[ch as u8, rgb_data[rgb_offset], rgb_data[rgb_offset + 1], rgb_data[rgb_offset + 2]])?;
        char_idx += 1;
    }
    if let Some(bg_rgb_data) = bg_rgb_data {
        file.write_all(&[CFRAME_EXT_FLAG_HAS_BG])?;
        file.write_all(bg_rgb_data)?;
    }
    Ok(())
}

/// Read a .cframe binary file into AsciiFrameData.
///
/// Recognises both the legacy fg-only layout and the new extension area. For
/// backward compatibility with `.cframe` files written by older builds that
/// appended the background payload **without** a leading flag byte, the reader
/// also accepts an exact `width * height * 3` trailing block.
pub(crate) fn read_cframe_to_frame_data(path: &Path) -> Result<AsciiFrameData> {
    let data = fs::read(path).with_context(|| format!("reading cframe {}", path.display()))?;
    if data.len() < 8 {
        return Err(anyhow!("cframe file too small: {}", path.display()));
    }

    let width = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let height = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let expected_body = (width * height * 4) as usize;
    let cell_count = (width * height) as usize;

    if data.len() < 8 + expected_body {
        return Err(anyhow!("cframe file truncated: expected {} body bytes, got {} in {}", expected_body, data.len() - 8, path.display()));
    }

    let mut ascii_text = String::with_capacity((width as usize + 1) * height as usize);
    let mut rgb_colors = Vec::with_capacity((width * height * 3) as usize);
    let mut bg_rgb_colors = Vec::new();

    for row in 0..height {
        for col in 0..width {
            let idx = 8 + ((row * width + col) * 4) as usize;
            let ch = data[idx] as char;
            ascii_text.push(ch);
            rgb_colors.push(data[idx + 1]); // R
            rgb_colors.push(data[idx + 2]); // G
            rgb_colors.push(data[idx + 3]); // B
        }
        ascii_text.push('\n');
    }

    let ext_offset = 8 + expected_body;
    let expected_bg_len = cell_count * 3;
    if data.len() > ext_offset {
        let trailing = data.len() - ext_offset;
        if trailing > expected_bg_len && (data[ext_offset] & CFRAME_EXT_FLAG_HAS_BG) != 0 {
            // New format: leading flag byte announces the bg payload.
            let bg_start = ext_offset + 1;
            bg_rgb_colors.extend_from_slice(&data[bg_start..bg_start + expected_bg_len]);
        } else if trailing == expected_bg_len {
            // Legacy bg-augmented format: exact bg-sized trailing block, no flag byte.
            bg_rgb_colors.extend_from_slice(&data[ext_offset..ext_offset + expected_bg_len]);
        }
    }

    Ok(AsciiFrameData {ascii_text, width_chars: width, height_chars: height, rgb_colors, bg_rgb_colors})
}

/// Read a .txt ASCII frame file into AsciiFrameData (white-on-black, no color)
pub(crate) fn read_txt_to_frame_data(path: &Path) -> Result<AsciiFrameData> {
    let content = fs::read_to_string(path).with_context(|| format!("reading txt frame {}", path.display()))?;
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() {
        return Err(anyhow!("empty frame file: {}", path.display()));
    }

    let width = lines[0].len() as u32;
    let height = lines.len() as u32;

    // Rebuild with consistent newlines
    let ascii_text = lines.join("\n") + "\n";

    Ok(AsciiFrameData {ascii_text, width_chars: width, height_chars: height, rgb_colors: Vec::new(), /* empty = renderer uses white */ bg_rgb_colors: Vec::new()})
}

fn luminance(rgb: image::Rgb<u8>) -> u8 {
    let r = rgb[0] as f64;
    let g = rgb[1] as f64;
    let b = rgb[2] as f64;
    (0.2126 * r + 0.7152 * g + 0.0722 * b) as u8
}

fn char_for(luma: u8, threshold: u8, ascii_chars: &[u8]) -> char {
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn convert_directory_parallel(src_dir: &Path, dst_dir: &Path, font_ratio: f32, threshold: u8, keep_images: bool, ascii_chars: &[u8], output_mode: &OutputMode, cell_color_mode: CellColorMode) -> Result<usize> {
    convert_directory_parallel_with_progress(src_dir, dst_dir, font_ratio, threshold, keep_images, ascii_chars, output_mode, cell_color_mode, None::<fn(usize, usize)>)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn convert_directory_parallel_with_progress<F>(src_dir: &Path, dst_dir: &Path, font_ratio: f32, threshold: u8, keep_images: bool, ascii_chars: &[u8], output_mode: &OutputMode, cell_color_mode: CellColorMode, progress_callback: Option<F>) -> Result<usize> where F: Fn(usize, usize) + Send + Sync {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fs::create_dir_all(dst_dir)?;
    let mut pngs: Vec<PathBuf> = WalkDir::new(src_dir).min_depth(1).max_depth(1).into_iter().filter_map(|e| e.ok()).map(|e| e.into_path()).filter(|p| p.extension().map(|e| e == "png").unwrap_or(false)).collect();
    pngs.sort();

    let total = pngs.len();
    let completed = Arc::new(AtomicUsize::new(0));

    pngs.par_iter().try_for_each(|img_path| -> Result<()> {
        let file_stem = img_path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| anyhow!("bad file name"))?;
        let out_txt = dst_dir.join(format!("{}.txt", file_stem));
        convert_image_to_ascii(img_path, &out_txt, font_ratio, threshold, None, ascii_chars, output_mode, cell_color_mode)?;

        // Update progress
        let current = completed.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(ref callback) = progress_callback {
            callback(current, total);
        }

        Ok(())
    })?;

    if !keep_images {
        for img_path in &pngs {
            fs::remove_file(img_path)?;
        }
    }

    Ok(total)
}

/// Internal function for directory conversion with detailed Progress reporting
#[allow(clippy::too_many_arguments)]
pub(crate) fn convert_directory_parallel_with_detailed_progress<F>(src_dir: &Path, dst_dir: &Path, font_ratio: f32, threshold: u8, keep_images: bool, ascii_chars: &[u8], output_mode: &OutputMode, cell_color_mode: CellColorMode, progress_callback: &F) -> Result<usize> where F: Fn(Progress) + Send + Sync {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fs::create_dir_all(dst_dir)?;
    let mut pngs: Vec<PathBuf> = WalkDir::new(src_dir).min_depth(1).max_depth(1).into_iter().filter_map(|e| e.ok()).map(|e| e.into_path()).filter(|p| p.extension().map(|e| e == "png").unwrap_or(false)).collect();
    pngs.sort();

    let total = pngs.len();
    let completed = Arc::new(AtomicUsize::new(0));
    let last_reported_percent = Arc::new(AtomicUsize::new(0));

    // Report initial progress
    progress_callback(Progress::converting_frames(0, total));

    pngs.par_iter().try_for_each(|img_path| -> Result<()> {
        let file_stem = img_path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| anyhow!("bad file name"))?;
        let out_txt = dst_dir.join(format!("{}.txt", file_stem));
        convert_image_to_ascii(img_path, &out_txt, font_ratio, threshold, None, ascii_chars, output_mode, cell_color_mode)?;

        // Update progress - throttle to only report every 1% change
        let current = completed.fetch_add(1, Ordering::SeqCst) + 1;
        let current_percent = current.checked_mul(100).and_then(|value| value.checked_div(total)).unwrap_or(0);
        let last_percent = last_reported_percent.load(Ordering::SeqCst);

        // Only report if percentage changed (throttle to ~100 updates max)
        if current_percent > last_percent || current == total {
            last_reported_percent.store(current_percent, Ordering::SeqCst);
            progress_callback(Progress::converting_frames(current, total));
        }

        Ok(())
    })?;

    if !keep_images {
        for img_path in &pngs {
            fs::remove_file(img_path)?;
        }
    }

    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn ascii_content_for(width: u32, height: u32, chars: &[u8]) -> String {
        let mut out = String::with_capacity(((width as usize) + 1) * height as usize);
        for row in 0..height as usize {
            let start = row * width as usize;
            let end = start + width as usize;
            for &b in &chars[start..end] {
                out.push(b as char);
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn cframe_round_trip_without_background() {
        let chars = [b'A', b'B', b'C', b'D'];
        let rgb = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 128, 128, 128];
        let text = ascii_content_for(2, 2, &chars);

        let tmp = NamedTempFile::new().unwrap();
        write_cframe_binary(2, 2, &text, &rgb, None, tmp.path()).unwrap();

        let frame = read_cframe_to_frame_data(tmp.path()).unwrap();
        assert_eq!(frame.width_chars, 2);
        assert_eq!(frame.height_chars, 2);
        assert_eq!(frame.rgb_colors, rgb);
        assert!(frame.bg_rgb_colors.is_empty());
    }

    #[test]
    fn cframe_round_trip_with_background_uses_flag_byte() {
        let chars = [b'X', b'Y'];
        let rgb = vec![10, 20, 30, 40, 50, 60];
        let bg = vec![100, 110, 120, 130, 140, 150];
        let text = ascii_content_for(2, 1, &chars);

        let tmp = NamedTempFile::new().unwrap();
        write_cframe_binary(2, 1, &text, &rgb, Some(&bg), tmp.path()).unwrap();

        // 8-byte header + 8-byte body + 1 flag byte + 6 bg bytes = 23 bytes.
        let raw = fs::read(tmp.path()).unwrap();
        assert_eq!(raw.len(), 8 + 8 + 1 + 6);
        assert_eq!(raw[16], CFRAME_EXT_FLAG_HAS_BG);

        let frame = read_cframe_to_frame_data(tmp.path()).unwrap();
        assert_eq!(frame.rgb_colors, rgb);
        assert_eq!(frame.bg_rgb_colors, bg);
    }

    #[test]
    fn cframe_reads_legacy_bg_without_flag_byte() {
        // Simulate a file written by the pre-flag-byte build: no leading flag,
        // bg payload appended directly after the body.
        let header: Vec<u8> = [2u32.to_le_bytes(), 1u32.to_le_bytes()].concat();
        let body: Vec<u8> = vec![b'X', 10, 20, 30, b'Y', 40, 50, 60];
        let bg: Vec<u8> = vec![100, 110, 120, 130, 140, 150];
        let mut file_bytes = Vec::new();
        file_bytes.extend_from_slice(&header);
        file_bytes.extend_from_slice(&body);
        file_bytes.extend_from_slice(&bg);

        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), &file_bytes).unwrap();

        let frame = read_cframe_to_frame_data(tmp.path()).unwrap();
        assert_eq!(frame.rgb_colors, vec![10, 20, 30, 40, 50, 60]);
        assert_eq!(frame.bg_rgb_colors, bg);
    }
}
