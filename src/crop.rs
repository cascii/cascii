use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::{read_cframe_to_frame_data, write_cframe_binary};

/// Result of a crop operation
#[derive(Debug)]
pub struct CropResult {
    /// Number of frames cropped
    pub frame_count: usize,
    /// New width in characters
    pub new_width: u32,
    /// New height in characters (rows)
    pub new_height: u32,
    /// Total size in bytes of all output files
    pub total_size: u64,
}

/// Crop all frames in a directory, writing results to an output directory.
///
/// Removes `top` rows from the top, `bottom` rows from the bottom,
/// `left` columns from the left, and `right` columns from the right
/// of every frame. Both `.txt` and `.cframe` files are processed.
///
/// Frames are re-indexed starting from `frame_0001` in the output directory.
pub fn crop_frames(source_dir: &Path, top: usize, bottom: usize, left: usize, right: usize, output_dir: &Path) -> Result<CropResult> {
    if !source_dir.exists() {
        return Err(anyhow!("Source directory does not exist: {}", source_dir.display()));
    }

    fs::create_dir_all(output_dir).with_context(|| format!("creating output directory {}", output_dir.display()))?;

    // Collect and sort frame .txt files
    let mut txt_frames: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(source_dir)
        .with_context(|| format!("reading directory {}", source_dir.display()))?
        .flatten()
    {
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("frame_") && name.ends_with(".txt") {
                    txt_frames.push(path);
                }
            }
        }
    }
    txt_frames.sort();

    if txt_frames.is_empty() {
        return Err(anyhow!("No frame_*.txt files found in {}", source_dir.display()));
    }

    // Validate dimensions on the first frame
    let first_content = fs::read_to_string(&txt_frames[0]).with_context(|| format!("reading {}", txt_frames[0].display()))?;
    let first_lines: Vec<&str> = first_content.lines().collect();
    if first_lines.is_empty() {
        return Err(anyhow!("First frame is empty: {}", txt_frames[0].display()));
    }
    let frame_height = first_lines.len();
    let frame_width = first_lines[0].chars().count();

    if top + bottom >= frame_height {
        return Err(anyhow!("Crop rows ({} top + {} bottom = {}) exceed frame height ({})", top, bottom, top + bottom, frame_height));
    }
    if left + right >= frame_width {
        return Err(anyhow!("Crop columns ({} left + {} right = {}) exceed frame width ({})", left, right, left + right, frame_width));
    }

    let new_width = (frame_width - left - right) as u32;
    let new_height = (frame_height - top - bottom) as u32;
    let mut total_size: u64 = 0;

    for (idx, txt_path) in txt_frames.iter().enumerate() {
        let new_idx = idx + 1;

        // --- Crop .txt file ---
        let content = fs::read_to_string(txt_path)
            .with_context(|| format!("reading {}", txt_path.display()))?;
        let lines: Vec<&str> = content.lines().collect();

        let mut cropped_lines: Vec<String> = Vec::with_capacity(new_height as usize);
        for line in lines.iter().skip(top).take(new_height as usize) {
            let slice: String = line.chars().skip(left).take(new_width as usize).collect();
            cropped_lines.push(slice);
        }
        let cropped_text = cropped_lines.join("\n") + "\n";

        let out_txt = output_dir.join(format!("frame_{:04}.txt", new_idx));
        fs::write(&out_txt, &cropped_text)
            .with_context(|| format!("writing {}", out_txt.display()))?;
        total_size += fs::metadata(&out_txt).map(|m| m.len()).unwrap_or(0);

        // --- Crop .cframe file (if exists) ---
        let cframe_path = txt_path.with_extension("cframe");
        if cframe_path.exists() {
            let frame_data = read_cframe_to_frame_data(&cframe_path)?;
            let orig_w = frame_data.width_chars as usize;

            let mut cropped_ascii = String::with_capacity((new_width as usize + 1) * new_height as usize);
            let mut cropped_rgb: Vec<u8> = Vec::with_capacity((new_width * new_height * 3) as usize);

            for row in top..(frame_height - bottom) {
                for col in left..(frame_width - right) {
                    let src_idx = row * orig_w + col;
                    let char_offset = row * (orig_w + 1) + col;
                    if let Some(ch) = frame_data.ascii_text.as_bytes().get(char_offset) {
                        cropped_ascii.push(*ch as char);
                    }
                    let rgb_offset = src_idx * 3;
                    cropped_rgb.push(frame_data.rgb_colors[rgb_offset]);
                    cropped_rgb.push(frame_data.rgb_colors[rgb_offset + 1]);
                    cropped_rgb.push(frame_data.rgb_colors[rgb_offset + 2]);
                }
                cropped_ascii.push('\n');
            }

            let out_cframe = output_dir.join(format!("frame_{:04}.cframe", new_idx));
            write_cframe_binary(new_width, new_height, &cropped_ascii, &cropped_rgb, &out_cframe)?;
            total_size += fs::metadata(&out_cframe).map(|m| m.len()).unwrap_or(0);
        }
    }

    Ok(CropResult {frame_count: txt_frames.len(), new_width, new_height, total_size})
}
