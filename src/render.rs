use ab_glyph::{FontRef, PxScale, ScaleFont};
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command as ProcCommand, Stdio};

use crate::convert::AsciiFrameData;
use crate::FfmpegConfig;

/// Embedded monospace font for video rendering
const FONT_DATA: &[u8] = include_bytes!("../resources/DejaVuSansMono.ttf");

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

    let font = FontRef::try_from_slice(FONT_DATA)
        .map_err(|e| anyhow!("failed to load embedded font: {}", e))?;

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

    Ok(GlyphAtlas {
        glyphs,
        cell_width,
        cell_height,
    })
}

pub(crate) fn render_ascii_frame_to_rgb(
    frame: &AsciiFrameData,
    atlas: &GlyphAtlas,
    use_colors: bool,
) -> Vec<u8> {
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
            (
                frame.rgb_colors[char_idx * 3],
                frame.rgb_colors[char_idx * 3 + 1],
                frame.rgb_colors[char_idx * 3 + 2],
            )
        } else {
            (255, 255, 255) // white for text-only mode
        };

        // Look up glyph bitmap
        if let Some(glyph_bitmap) = atlas.glyphs.get(&byte) {
            let base_x = col * atlas.cell_width;
            let base_y = row * atlas.cell_height;

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
                        buffer[offset] = (r as f32 * alpha) as u8;
                        buffer[offset + 1] = (g as f32 * alpha) as u8;
                        buffer[offset + 2] = (b as f32 * alpha) as u8;
                    }
                }
            }
        }

        char_idx += 1;
        col += 1;
    }

    buffer
}

pub(crate) fn spawn_ffmpeg_encoder(
    pixel_width: u32,
    pixel_height: u32,
    fps: u32,
    crf: u8,
    audio_path: Option<&Path>,
    output_path: &Path,
    ffmpeg_config: &FfmpegConfig,
) -> Result<std::process::Child> {
    let size = format!("{}x{}", pixel_width, pixel_height);

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-loglevel".into(),
        "error".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgb24".into(),
        "-s:v".into(),
        size,
        "-r".into(),
        fps.to_string(),
        "-i".into(),
        "pipe:0".into(),
    ];

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
    args.push("-pix_fmt".into());
    args.push("yuv420p".into());
    args.push(output_path.to_str().ok_or_else(|| anyhow!("output path is not valid UTF-8"))?.to_string());

    let child = ProcCommand::new(ffmpeg_config.ffmpeg_cmd())
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning ffmpeg encoder")?;

    Ok(child)
}
