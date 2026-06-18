use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const FULL_CFRAME_PACK_MAGIC: &[u8; 4] = b"CFPK";
const FULL_CFRAME_PACK_VERSION: u32 = 1;
const FULL_CFRAME_PACK_HEADER_SIZE: usize = 12;

/// A packed archive containing complete `.cframe` files.
///
/// Unlike the legacy packed cframe transport, this format stores each source
/// `.cframe` byte-for-byte. That preserves optional extension payloads such as
/// per-cell background RGB data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullCFramePack {
    pub frames: Vec<Vec<u8>>,
}

impl FullCFramePack {
    pub fn new(frames: Vec<Vec<u8>>) -> Self {
        Self { frames }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

fn collect_cframe_paths(source_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = WalkDir::new(source_dir).min_depth(1).max_depth(1).into_iter().filter_map(|entry| entry.ok()).map(|entry| entry.into_path()).filter(|path| path.is_file() && path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("frame_")) && path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("cframe"))).collect();

    paths.sort_by(|left, right| left.file_name().and_then(|name| name.to_str()).cmp(&right.file_name().and_then(|name| name.to_str())));

    if paths.is_empty() {
        return Err(anyhow!("No .cframe files found in {}", source_dir.display()));
    }

    Ok(paths)
}

/// Pack all `frame_*.cframe` files in a directory into one full-fidelity blob.
///
/// Format:
/// - bytes 0..4: magic `CFPK`
/// - bytes 4..8: version (`u32`, currently `1`)
/// - bytes 8..12: frame count (`u32`)
/// - repeated per frame:
///   - byte length (`u32`)
///   - complete `.cframe` bytes
pub fn pack_full_cframes_from_dir(source_dir: &Path) -> Result<Vec<u8>> {
    let paths = collect_cframe_paths(source_dir)?;
    let mut frames = Vec::with_capacity(paths.len());
    for path in paths {
        let data = fs::read(&path).with_context(|| format!("reading cframe {}", path.display()))?;
        frames.push(data);
    }
    pack_full_cframes(frames.iter().map(Vec::as_slice))
}

/// Pack complete `.cframe` byte slices into one full-fidelity blob.
pub fn pack_full_cframes<'a, I>(frames: I) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let frames: Vec<&[u8]> = frames.into_iter().collect();
    if frames.is_empty() {
        return Err(anyhow!("No .cframe files provided"));
    }
    let frame_count = u32::try_from(frames.len()).map_err(|_| anyhow!("Too many frames to pack"))?;

    let payload_len = frames.iter().try_fold(0usize, |acc, frame| {
        let frame_len = u32::try_from(frame.len()).map_err(|_| anyhow!("A .cframe file is too large to pack"))?;
        acc.checked_add(4).and_then(|value| value.checked_add(frame_len as usize)).ok_or_else(|| anyhow!("Packed cframe payload is too large"))
    })?;

    let mut out = Vec::with_capacity(FULL_CFRAME_PACK_HEADER_SIZE + payload_len);
    out.extend_from_slice(FULL_CFRAME_PACK_MAGIC);
    out.extend_from_slice(&FULL_CFRAME_PACK_VERSION.to_le_bytes());
    out.extend_from_slice(&frame_count.to_le_bytes());
    for frame in frames {
        let frame_len = u32::try_from(frame.len()).map_err(|_| anyhow!("A .cframe file is too large to pack"))?;
        out.extend_from_slice(&frame_len.to_le_bytes());
        out.extend_from_slice(frame);
    }
    Ok(out)
}

/// Parse a full-fidelity packed `.cframe` blob.
pub fn unpack_full_cframes(data: &[u8]) -> Result<FullCFramePack> {
    if data.len() < FULL_CFRAME_PACK_HEADER_SIZE {
        return Err(anyhow!("packed cframe blob is too small"));
    }
    if &data[0..4] != FULL_CFRAME_PACK_MAGIC {
        return Err(anyhow!("packed cframe blob has invalid magic"));
    }

    let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
    if version != FULL_CFRAME_PACK_VERSION {
        return Err(anyhow!("unsupported packed cframe version: {}", version));
    }

    let frame_count = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
    if frame_count == 0 {
        return Err(anyhow!("packed cframe blob contains no frames"));
    }

    let mut offset = FULL_CFRAME_PACK_HEADER_SIZE;
    let mut frames = Vec::with_capacity(frame_count);
    for _ in 0..frame_count {
        if offset + 4 > data.len() {
            return Err(anyhow!("packed cframe blob is truncated before a frame length"));
        }
        let frame_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        if offset + frame_len > data.len() {
            return Err(anyhow!("packed cframe blob is truncated inside a frame payload"));
        }
        frames.push(data[offset..offset + frame_len].to_vec());
        offset += frame_len;
    }

    if offset != data.len() {
        return Err(anyhow!("packed cframe blob has {} trailing bytes", data.len() - offset));
    }

    Ok(FullCFramePack::new(frames))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::CFRAME_EXT_FLAG_HAS_BG;

    fn cframe_with_background() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&[b'A', 10, 20, 30, b'B', 40, 50, 60]);
        data.push(CFRAME_EXT_FLAG_HAS_BG);
        data.extend_from_slice(&[100, 110, 120, 130, 140, 150]);
        data
    }

    #[test]
    fn full_pack_preserves_background_extension_bytes() {
        let frame = cframe_with_background();
        let packed = pack_full_cframes([frame.as_slice()]).unwrap();
        let unpacked = unpack_full_cframes(&packed).unwrap();

        assert_eq!(unpacked.frames, vec![frame]);
    }

    #[test]
    fn full_pack_from_dir_sorts_and_preserves_complete_files() {
        let dir = tempfile::tempdir().unwrap();
        let first = cframe_with_background();
        let second = {
            let mut data = cframe_with_background();
            data[8] = b'Z';
            data
        };

        fs::write(dir.path().join("frame_0002.cframe"), &second).unwrap();
        fs::write(dir.path().join("frame_0001.cframe"), &first).unwrap();

        let packed = pack_full_cframes_from_dir(dir.path()).unwrap();
        let unpacked = unpack_full_cframes(&packed).unwrap();

        assert_eq!(unpacked.frames, vec![first, second]);
    }
}
