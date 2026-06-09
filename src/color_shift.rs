use anyhow::{anyhow, Result};

const HEADER_SIZE: usize = 8;
const CELL_SIZE: usize = 4;
const RGB_SIZE: usize = 3;
const CFRAME_EXT_FLAG_HAS_BG: u8 = 0b0000_0001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorShiftTarget {
    Foreground,
    Background,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorShift {
    pub target: ColorShiftTarget,
    pub foreground_degrees: f32,
    pub background_degrees: f32,
}

impl ColorShift {
    pub fn foreground(degrees: f32) -> Self {
        Self {
            target: ColorShiftTarget::Foreground,
            foreground_degrees: degrees,
            background_degrees: 0.0,
        }
    }

    pub fn background(degrees: f32) -> Self {
        Self {
            target: ColorShiftTarget::Background,
            foreground_degrees: 0.0,
            background_degrees: degrees,
        }
    }

    pub fn both(foreground_degrees: f32, background_degrees: f32) -> Self {
        Self {
            target: ColorShiftTarget::Both,
            foreground_degrees,
            background_degrees,
        }
    }
}

pub fn shift_rgb_triplets(rgb: &mut [u8], degrees: f32) -> Result<()> {
    if rgb.len() % RGB_SIZE != 0 {
        return Err(anyhow!(
            "RGB payload length must be divisible by 3, got {}",
            rgb.len()
        ));
    }
    if !degrees.is_finite() {
        return Err(anyhow!("hue shift must be finite"));
    }

    for color in rgb.chunks_exact_mut(RGB_SIZE) {
        let shifted = shift_rgb([color[0], color[1], color[2]], degrees);
        color.copy_from_slice(&shifted);
    }
    Ok(())
}

pub fn shift_cframe_bytes(data: &[u8], shift: ColorShift) -> Result<Vec<u8>> {
    if data.len() < HEADER_SIZE {
        return Err(anyhow!("cframe file too small"));
    }

    let width = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let height = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    if width == 0 || height == 0 {
        return Err(anyhow!("cframe dimensions must be non-zero"));
    }
    if width == 0 || height == 0 {
        return Err(anyhow!("cframe dimensions must be non-zero"));
    }

    let cell_count = width
        .checked_mul(height)
        .ok_or_else(|| anyhow!("cframe dimensions overflow"))?;
    let body_len = cell_count
        .checked_mul(CELL_SIZE)
        .ok_or_else(|| anyhow!("cframe body size overflow"))?;
    let body_end = HEADER_SIZE
        .checked_add(body_len)
        .ok_or_else(|| anyhow!("cframe body offset overflow"))?;
    if data.len() < body_end {
        return Err(anyhow!(
            "cframe file truncated: expected at least {} bytes, got {}",
            body_end,
            data.len()
        ));
    }

    let mut output = data.to_vec();

    if matches!(
        shift.target,
        ColorShiftTarget::Foreground | ColorShiftTarget::Both
    ) {
        if !shift.foreground_degrees.is_finite() {
            return Err(anyhow!("foreground hue shift must be finite"));
        }
        for cell in 0..cell_count {
            let offset = HEADER_SIZE + cell * CELL_SIZE + 1;
            let shifted = shift_rgb(
                [output[offset], output[offset + 1], output[offset + 2]],
                shift.foreground_degrees,
            );
            output[offset..offset + RGB_SIZE].copy_from_slice(&shifted);
        }
    }

    if matches!(
        shift.target,
        ColorShiftTarget::Background | ColorShiftTarget::Both
    ) {
        if !shift.background_degrees.is_finite() {
            return Err(anyhow!("background hue shift must be finite"));
        }
        let background_len = cell_count
            .checked_mul(RGB_SIZE)
            .ok_or_else(|| anyhow!("cframe background size overflow"))?;
        if let Some(background_start) = background_payload_start(&output, body_end, background_len)
        {
            shift_rgb_triplets(
                &mut output[background_start..background_start + background_len],
                shift.background_degrees,
            )?;
        }
    }

    Ok(output)
}

pub fn cframe_has_background(data: &[u8]) -> Result<bool> {
    if data.len() < HEADER_SIZE {
        return Err(anyhow!("cframe file too small"));
    }
    let width = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let height = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let cell_count = width
        .checked_mul(height)
        .ok_or_else(|| anyhow!("cframe dimensions overflow"))?;
    let body_end = HEADER_SIZE
        .checked_add(
            cell_count
                .checked_mul(CELL_SIZE)
                .ok_or_else(|| anyhow!("cframe body size overflow"))?,
        )
        .ok_or_else(|| anyhow!("cframe body offset overflow"))?;
    if data.len() < body_end {
        return Err(anyhow!("cframe file truncated"));
    }
    let background_len = cell_count
        .checked_mul(RGB_SIZE)
        .ok_or_else(|| anyhow!("cframe background size overflow"))?;
    Ok(background_payload_start(data, body_end, background_len).is_some())
}

fn background_payload_start(data: &[u8], body_end: usize, background_len: usize) -> Option<usize> {
    let trailing = data.len().checked_sub(body_end)?;
    if trailing >= background_len + 1 && (data[body_end] & CFRAME_EXT_FLAG_HAS_BG) != 0 {
        Some(body_end + 1)
    } else if trailing == background_len {
        Some(body_end)
    } else {
        None
    }
}

fn shift_rgb(rgb: [u8; 3], degrees: f32) -> [u8; 3] {
    if degrees.rem_euclid(360.0).abs() <= f32::EPSILON {
        return rgb;
    }

    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    if delta <= f32::EPSILON {
        return rgb;
    }

    let hue = if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    let shifted_hue = (hue + degrees).rem_euclid(360.0);
    hsv_to_rgb(shifted_hue, saturation, max)
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> [u8; 3] {
    let chroma = value * saturation;
    let sector = hue / 60.0;
    let x = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match sector.floor() as u8 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = value - chroma;
    [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cframe(with_flag: bool) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&[b'A', 255, 0, 0, b'B', 0, 255, 0]);
        if with_flag {
            data.push(CFRAME_EXT_FLAG_HAS_BG);
        }
        data.extend_from_slice(&[0, 0, 255, 255, 0, 0]);
        data
    }

    #[test]
    fn rotates_primary_colors() {
        let mut rgb = vec![255, 0, 0, 0, 255, 0, 0, 0, 255];
        shift_rgb_triplets(&mut rgb, 120.0).unwrap();
        assert_eq!(rgb, vec![0, 255, 0, 0, 0, 255, 255, 0, 0]);
    }

    #[test]
    fn leaves_grayscale_unchanged() {
        let mut rgb = vec![0, 0, 0, 128, 128, 128, 255, 255, 255];
        let original = rgb.clone();
        shift_rgb_triplets(&mut rgb, 137.0).unwrap();
        assert_eq!(rgb, original);
    }

    #[test]
    fn shifts_flagged_foreground_and_background_independently() {
        let shifted = shift_cframe_bytes(&cframe(true), ColorShift::both(120.0, -120.0)).unwrap();
        assert_eq!(&shifted[9..12], &[0, 255, 0]);
        assert_eq!(&shifted[13..16], &[0, 0, 255]);
        assert_eq!(shifted[16], CFRAME_EXT_FLAG_HAS_BG);
        assert_eq!(&shifted[17..20], &[0, 255, 0]);
        assert_eq!(&shifted[20..23], &[0, 0, 255]);
    }

    #[test]
    fn shifts_legacy_unflagged_background() {
        let shifted = shift_cframe_bytes(&cframe(false), ColorShift::background(120.0)).unwrap();
        assert_eq!(&shifted[16..19], &[255, 0, 0]);
        assert_eq!(&shifted[19..22], &[0, 255, 0]);
    }

    #[test]
    fn background_shift_is_noop_without_background_payload() {
        let data = cframe(true)[..16].to_vec();
        let shifted = shift_cframe_bytes(&data, ColorShift::background(90.0)).unwrap();
        assert_eq!(shifted, data);
    }

    #[test]
    fn detects_flagged_and_legacy_background_payloads() {
        assert!(cframe_has_background(&cframe(true)).unwrap());
        assert!(cframe_has_background(&cframe(false)).unwrap());
        assert!(!cframe_has_background(&cframe(true)[..16]).unwrap());
    }
}
