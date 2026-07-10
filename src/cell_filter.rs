//! Per-cell filtering for `.cframe` payloads and text frames.
//!
//! Used to gate cell selections (e.g. a lasso capture) by brightness or by local color coherence: failing cells are reported so callers can blank them per frame. 
//! Filters only ever fail cells they can positively judge out-of-bounds cells and characters absent from the ascii ramp pass.

use anyhow::{anyhow, Result};

const HEADER_SIZE: usize = 8;
const CELL_SIZE: usize = 4;

/// Integer Rec.709 relative luminance. Coefficients sum to 10000, so pure white maps to 255 and pure black to 0.
#[inline]
pub fn luminance_rgb(r: u8, g: u8, b: u8) -> u8 {
    ((2126 * r as u32 + 7152 * g as u32 + 722 * b as u32) / 10000) as u8
}

/// One-sided luminance bound. Cells strictly past `threshold` are dropped; `inclusive` also drops cells sitting exactly on the threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LuminanceBound {
    pub threshold: u8,
    pub inclusive: bool,
}

/// Independent lower/upper luminance gates. Each side is optional, so the filter can drop only dark cells, only bright cells, both, or nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LuminanceFilter {
    /// Drop cells darker than the threshold (`< t`, or `<= t` when inclusive).
    pub drop_below: Option<LuminanceBound>,
    /// Drop cells brighter than the threshold (`> t`, or `>= t` when inclusive).
    pub drop_above: Option<LuminanceBound>,
}

impl LuminanceFilter {
    #[inline]
    pub fn is_active(&self) -> bool {
        self.drop_below.is_some() || self.drop_above.is_some()
    }

    #[inline]
    pub fn passes(&self, luminance: u8) -> bool {
        if let Some(bound) = self.drop_below {
            if luminance < bound.threshold || (bound.inclusive && luminance == bound.threshold) {
                return false;
            }
        }
        if let Some(bound) = self.drop_above {
            if luminance > bound.threshold || (bound.inclusive && luminance == bound.threshold) {
                return false;
            }
        }
        true
    }
}

/// Evaluate the foreground luminance of each `(row, col)` cell in a raw `.cframe` payload against `filter`. 
/// Returns one bool per entry in `cells` (parallel array): `true` = keep, `false` = the cell's luminance is dropped by the filter.
/// Cells outside the frame grid return `true` — the mask is purely a "would luminance blank this cell" predicate, and consumers already skip out-of-bounds cells themselves.
pub fn cframe_cells_luminance_mask(data: &[u8], cells: &[(usize, usize)], filter: LuminanceFilter) -> Result<Vec<bool>> {
    let (width, height) = validated_cframe_dimensions(data)?;

    Ok(cells.iter().map(|&(row, col)| {
        if row >= height || col >= width {
            return true;
        }
        filter.passes(luminance_rgb_at(data, width, row, col))
    }).collect())
}

/// Scaled Euclidean RGB distance in `0..=255` (white vs black = 255).
#[inline]
pub fn rgb_distance(left: (u8, u8, u8), right: (u8, u8, u8)) -> u8 {
    let dr = left.0 as i32 - right.0 as i32;
    let dg = left.1 as i32 - right.1 as i32;
    let db = left.2 as i32 - right.2 as i32;
    (((dr * dr + dg * dg + db * db) as f32 / 3.0).sqrt().round() as u32).min(255) as u8
}

/// Local color-coherence gate: a cell passes when at least `min_neighbors` of its (up to 8) in-grid neighbors sit within `tolerance` color distance of it. 
/// Coherent same-color regions pass; cells contrasting with their surroundings fail. `min_neighbors` is clamped to `1..=8` and to the number of in-grid neighbors, so border cells are not unfairly dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProximityFilter {
    pub tolerance: u8,
    pub min_neighbors: u8,
}

/// Evaluate the local color coherence of each `(row, col)` cell in a raw `.cframe` payload. Returns one bool per entry in `cells` (parallel array):
/// `true` = keep. Cells outside the frame grid return `true`. Blank cells carry their stored foreground color (black), so a blank neighborhood is self-coherent — pair this filter with a luminance gate to drop it too.
pub fn cframe_cells_proximity_mask(data: &[u8], cells: &[(usize, usize)], filter: ProximityFilter) -> Result<Vec<bool>> {
    let (width, height) = validated_cframe_dimensions(data)?;

    Ok(cells.iter().map(|&(row, col)| {
        if row >= height || col >= width {
            return true;
        }
        let cell_rgb = rgb_at(data, width, row, col);
        let mut neighbors = 0u8;
        let mut similar = 0u8;
        for row_delta in -1isize..=1 {
            for col_delta in -1isize..=1 {
                if row_delta == 0 && col_delta == 0 {
                    continue;
                }
                let (Some(neighbor_row), Some(neighbor_col)) = (row.checked_add_signed(row_delta), col.checked_add_signed(col_delta)) else {
                    continue;
                };
                if neighbor_row >= height || neighbor_col >= width {
                    continue;
                }
                neighbors += 1;
                if rgb_distance(cell_rgb, rgb_at(data, width, neighbor_row, neighbor_col)) <= filter.tolerance {
                    similar += 1;
                }
            }
        }
        similar >= filter.min_neighbors.clamp(1, 8).min(neighbors.max(1))
    }).collect())
}

fn validated_cframe_dimensions(data: &[u8]) -> Result<(usize, usize)> {
    if data.len() < HEADER_SIZE {
        return Err(anyhow!("cframe file too small"));
    }

    let width = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let height = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    if width == 0 || height == 0 {
        return Err(anyhow!("cframe dimensions must be non-zero"));
    }

    let cell_count = width.checked_mul(height).ok_or_else(|| anyhow!("cframe dimensions overflow"))?;
    let body_len = cell_count.checked_mul(CELL_SIZE).ok_or_else(|| anyhow!("cframe body size overflow"))?;
    let body_end = HEADER_SIZE.checked_add(body_len).ok_or_else(|| anyhow!("cframe body offset overflow"))?;
    if data.len() < body_end {
        return Err(anyhow!("cframe file truncated: expected at least {} bytes, got {}", body_end, data.len()));
    }

    Ok((width, height))
}

#[inline]
fn rgb_at(data: &[u8], width: usize, row: usize, col: usize) -> (u8, u8, u8) {
    let offset = HEADER_SIZE + (row * width + col) * CELL_SIZE + 1;
    (data[offset], data[offset + 1], data[offset + 2])
}

#[inline]
fn luminance_rgb_at(data: &[u8], width: usize, row: usize, col: usize) -> u8 {
    let (r, g, b) = rgb_at(data, width, row, col);
    luminance_rgb(r, g, b)
}

/// Maps ASCII glyphs to a 0-255 pseudo-luminance from their position in an ascii ramp (dark chars first), for filtering text-only frames.
#[derive(Clone, Debug)]
pub struct RampLuminance {
    positions: [i16; 256],
    scale_max: u16,
}

impl RampLuminance {
    pub fn new(ramp: &str) -> Result<Self> {
        if ramp.is_empty() {
            return Err(anyhow!("ascii ramp must not be empty"));
        }
        if !ramp.is_ascii() {
            return Err(anyhow!("ascii ramp must contain only ASCII characters"));
        }

        let mut positions = [-1i16; 256];
        for (index, byte) in ramp.bytes().enumerate() {
            if positions[byte as usize] < 0 {
                positions[byte as usize] = index as i16;
            }
        }

        Ok(Self {positions, scale_max: (ramp.len() - 1).max(1) as u16})
    }

    /// `None` when the character is not ASCII or not part of the ramp.
    #[inline]
    pub fn luminance_of(&self, ch: char) -> Option<u8> {
        if !ch.is_ascii() {
            return None;
        }
        let position = self.positions[ch as usize];
        if position < 0 {
            return None;
        }
        Some(((position as u32 * 255) / self.scale_max as u32) as u8)
    }

    /// Characters absent from the ramp pass (return `true`).
    #[inline]
    pub fn char_passes(&self, ch: char, filter: LuminanceFilter) -> bool {
        self.luminance_of(ch).map(|luminance| filter.passes(luminance)).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cframe(width: u32, height: u32, cells: &[[u8; 4]]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&width.to_le_bytes());
        data.extend_from_slice(&height.to_le_bytes());
        for cell in cells {
            data.extend_from_slice(cell);
        }
        data
    }

    #[test]
    fn test_luminance_rgb_reference_values() {
        assert_eq!(luminance_rgb(0, 0, 0), 0);
        assert_eq!(luminance_rgb(255, 255, 255), 255);
        assert_eq!(luminance_rgb(255, 0, 0), 54);
        assert_eq!(luminance_rgb(0, 255, 0), 182);
        assert_eq!(luminance_rgb(0, 0, 255), 18);
    }

    fn drop_below(threshold: u8, inclusive: bool) -> LuminanceFilter {
        LuminanceFilter {drop_below: Some(LuminanceBound {threshold, inclusive}), drop_above: None}
    }

    fn drop_above(threshold: u8, inclusive: bool) -> LuminanceFilter {
        LuminanceFilter {drop_below: None, drop_above: Some(LuminanceBound {threshold, inclusive})}
    }

    #[test]
    fn test_filter_bounds_respect_strictness_and_activity() {
        assert!(!LuminanceFilter::default().is_active());
        assert!(LuminanceFilter::default().passes(0));
        assert!(LuminanceFilter::default().passes(255));

        let strict = drop_below(100, false);
        assert!(strict.is_active());
        assert!(!strict.passes(99));
        assert!(strict.passes(100));

        let inclusive = drop_below(100, true);
        assert!(!inclusive.passes(100));
        assert!(inclusive.passes(101));

        let strict = drop_above(200, false);
        assert!(strict.passes(200));
        assert!(!strict.passes(201));

        let inclusive = drop_above(200, true);
        assert!(!inclusive.passes(200));
        assert!(inclusive.passes(199));

        let band = LuminanceFilter {drop_below: Some(LuminanceBound {threshold: 100, inclusive: false}), drop_above: Some(LuminanceBound {threshold: 200, inclusive: false})};
        assert!(!band.passes(99));
        assert!(band.passes(100));
        assert!(band.passes(200));
        assert!(!band.passes(201));
    }

    #[test]
    fn test_cframe_mask_keeps_bright_and_drops_dark_cells() {
        let data = cframe(2, 1, &[[b'#', 255, 255, 255], [b'.', 10, 10, 10]]);
        let mask = cframe_cells_luminance_mask(&data, &[(0, 0), (0, 1)], drop_below(128, false)).unwrap();
        assert_eq!(mask, vec![true, false]);
        let mask = cframe_cells_luminance_mask(&data, &[(0, 0), (0, 1)], drop_above(128, false)).unwrap();
        assert_eq!(mask, vec![false, true]);
    }

    #[test]
    fn test_cframe_mask_out_of_bounds_cells_pass() {
        let data = cframe(2, 1, &[[b'#', 255, 255, 255], [b'.', 10, 10, 10]]);
        let mask = cframe_cells_luminance_mask(&data, &[(5, 5), (0, 2), (1, 0)], drop_below(255, true)).unwrap();
        assert_eq!(mask, vec![true, true, true]);
    }

    #[test]
    fn test_cframe_mask_rejects_malformed_payloads() {
        let filter = drop_below(0, false);
        assert!(cframe_cells_luminance_mask(&[0, 0, 0], &[], filter).is_err());
        assert!(cframe_cells_luminance_mask(&cframe(0, 0, &[]), &[], filter).is_err());
        let truncated = cframe(2, 1, &[[b'#', 255, 255, 255]]);
        assert!(cframe_cells_luminance_mask(&truncated, &[], filter).is_err());
    }

    #[test]
    fn test_rgb_distance_reference_values() {
        assert_eq!(rgb_distance((0, 0, 0), (0, 0, 0)), 0);
        assert_eq!(rgb_distance((255, 255, 255), (0, 0, 0)), 255);
        assert_eq!(rgb_distance((255, 0, 0), (0, 0, 0)), 147);
        assert_eq!(rgb_distance((200, 100, 0), (210, 110, 10)), 10);
    }

    #[test]
    fn test_proximity_mask_keeps_coherent_regions_and_drops_outliers() {
        // 3x3 frame: all orange except an isolated bright blue center-right cell; center cell surrounded by its own color everywhere.
        let orange = [b'#', 220, 120, 10];
        let blue = [b'#', 20, 40, 220];
        let data = cframe(3, 3, &[orange, orange, orange, orange, orange, blue, orange, orange, orange]);
        let filter = ProximityFilter {tolerance: 40, min_neighbors: 3};

        let mask = cframe_cells_proximity_mask(&data, &[(1, 1), (1, 2), (0, 0)], filter).unwrap();
        // Center: 7 similar of 8. Blue outlier: 0 similar. Corner: 2 of 3 in-grid neighbors are orange (one is the blue cell); min_neighbors clamps to the 3 available, so 2 < 3 fails... unless similar >= min.
        assert!(mask[0]);
        assert!(!mask[1]);

        // Corner (0,0): neighbors (0,1) orange, (1,0) orange, (1,1) orange -> 3 similar.
        assert!(mask[2]);
    }

    #[test]
    fn test_proximity_mask_border_cells_clamp_required_neighbors() {
        // 1x2 frame: each cell has exactly one neighbor. min_neighbors=8 clamps to the single available neighbor.
        let orange = [b'#', 220, 120, 10];
        let data = cframe(2, 1, &[orange, orange]);
        let filter = ProximityFilter {tolerance: 10, min_neighbors: 8};
        let mask = cframe_cells_proximity_mask(&data, &[(0, 0), (0, 1)], filter).unwrap();
        assert_eq!(mask, vec![true, true]);

        let blue = [b'#', 20, 40, 220];
        let data = cframe(2, 1, &[orange, blue]);
        let mask = cframe_cells_proximity_mask(&data, &[(0, 0), (0, 1)], filter).unwrap();
        assert_eq!(mask, vec![false, false]);
    }

    #[test]
    fn test_proximity_mask_out_of_bounds_and_malformed() {
        let orange = [b'#', 220, 120, 10];
        let data = cframe(2, 1, &[orange, orange]);
        let filter = ProximityFilter {tolerance: 10, min_neighbors: 1};
        let mask = cframe_cells_proximity_mask(&data, &[(9, 9)], filter).unwrap();
        assert_eq!(mask, vec![true]);

        assert!(cframe_cells_proximity_mask(&[0, 0, 0], &[], filter).is_err());
        assert!(cframe_cells_proximity_mask(&cframe(0, 0, &[]), &[], filter).is_err());
    }

    #[test]
    fn test_ramp_luminance_scales_positions_to_full_range() {
        let ramp = RampLuminance::new(" .:-=+*#%@").unwrap();
        assert_eq!(ramp.luminance_of(' '), Some(0));
        assert_eq!(ramp.luminance_of('@'), Some(255));
        let mid = ramp.luminance_of('=').unwrap();
        assert!(mid > 0 && mid < 255);
        assert!(ramp.luminance_of('.').unwrap() < mid);
    }

    #[test]
    fn test_ramp_luminance_unknown_chars_pass() {
        let ramp = RampLuminance::new(" .:-=+*#%@").unwrap();
        let filter = drop_below(128, false);
        assert_eq!(ramp.luminance_of('Z'), None);
        assert!(ramp.char_passes('Z', filter));
        assert!(ramp.char_passes('é', filter));
        assert!(ramp.char_passes('@', filter));
        assert!(!ramp.char_passes(' ', filter));
    }

    #[test]
    fn test_ramp_luminance_validation() {
        assert!(RampLuminance::new("").is_err());
        assert!(RampLuminance::new("café").is_err());
        assert_eq!(RampLuminance::new("#").unwrap().luminance_of('#'), Some(0));
    }
}
