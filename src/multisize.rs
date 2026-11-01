//! Portable column selections and the versioned multi-size asset contract.
use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Seek, SeekFrom, Write};
use std::str::FromStr;

pub const MAX_COLUMNS: u32 = 4096;
pub const MAX_VARIANTS: usize = 256;
pub const MAX_FRAME_CELLS: u64 = 16_777_216;
pub const ASSET_MAGIC: &[u8; 4] = b"CSMV";
pub const ASSET_VERSION: u32 = 1;
pub const ASSET_HEADER_BYTES: usize = 16;
pub const ASSET_INDEX_BYTES: usize = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnSizes(Vec<u32>);

impl ColumnSizes {
    pub fn new(mut columns: Vec<u32>) -> Result<Self> {
        ensure!(!columns.is_empty(), "at least one column count is required");
        ensure!(columns.iter().all(|&n| n > 0 && n <= MAX_COLUMNS), "columns must be between 1 and {MAX_COLUMNS}");
        columns.sort_unstable();
        columns.dedup();
        ensure!(columns.len() <= MAX_VARIANTS, "at most {MAX_VARIANTS} sizes are supported");
        Ok(Self(columns))
    }

    pub fn range(start: u32, end: u32) -> Result<Self> {
        ensure!(start > 0 && start <= end && end <= MAX_COLUMNS, "invalid inclusive column range");
        ensure!(u64::from(end) - u64::from(start) < MAX_VARIANTS as u64, "column range exceeds {MAX_VARIANTS} sizes");
        Self::new((start..=end).collect())
    }

    pub fn values(&self) -> &[u32] { &self.0 }
    pub fn largest(&self) -> u32 { *self.0.last().expect("validated nonempty sizes") }

    /// Approximate raw cell area at a shared aspect ratio, excluding row
    /// rounding, metadata and compression.
    pub fn relative_cell_count(&self, reference_columns: u32) -> Result<f64> {
        ensure!(reference_columns > 0, "reference columns must be positive");
        let cells = self.0.iter().map(|&columns| u64::from(columns) * u64::from(columns)).sum::<u64>();
        Ok(cells as f64 / (f64::from(reference_columns) * f64::from(reference_columns)))
    }
}

impl FromStr for ColumnSizes {
    type Err = anyhow::Error;

    fn from_str(input: &str) -> Result<Self> {
        let input = input.trim();
        if let Some((start, end)) = input.split_once('-') {
            return Self::range(start.trim().parse().context("invalid range start")?, end.trim().parse().context("invalid range end")?);
        }
        Self::new(input.split(',').map(|part| part.trim().parse::<u32>().context("invalid column count")).collect::<Result<Vec<_>>>()?)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SizeVariant {
    pub columns: u32,
    pub rows: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MultiSizeManifest {
    pub format: String,
    pub schema_version: u32,
    pub frame_count: u32,
    pub fps: u32,
    pub default_columns: u32,
    pub font_ratio: f32,
    pub luminance: u8,
    pub ascii_chars: String,
    pub text: bool,
    pub color: bool,
    pub audio: Option<String>,
    pub outputs: Vec<String>,
    pub variants: Vec<SizeVariant>,
}

impl MultiSizeManifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.format == "cascii-multisize" && self.schema_version == ASSET_VERSION, "unsupported multi-size asset format/version");
        ensure!(self.frame_count > 0 && self.frame_count <= 1_000_000 && self.fps > 0 && self.fps <= 1000, "invalid frame count or FPS");
        ensure!(self.font_ratio.is_finite() && self.font_ratio > 0.0, "invalid font ratio");
        ensure!(self.text || self.color, "asset has no frame output");
        ensure!(!self.outputs.is_empty() && self.outputs.iter().all(|path| ["variants/", "frames.bin", "frames.json", "frames.csv"].contains(&path.as_str())), "invalid asset output locations");
        let mut outputs = self.outputs.clone();
        outputs.sort();
        outputs.dedup();
        ensure!(outputs.len() == self.outputs.len(), "duplicate asset output locations");
        ensure!(!self.ascii_chars.is_empty() && self.ascii_chars.bytes().all(|c| (32..=126).contains(&c)), "character ramp must contain printable ASCII");
        let columns = self.variants.iter().map(|variant| variant.columns).collect::<Vec<_>>();
        let normalized = ColumnSizes::new(columns.clone())?;
        ensure!(normalized.values() == columns, "variants must have unique ascending column counts");
        ensure!(columns.contains(&self.default_columns), "default size is missing");
        for variant in &self.variants {
            ensure!(variant.rows.len() == self.frame_count as usize, "variant frame count mismatch");
            ensure!(variant.rows.iter().all(|&rows| rows > 0 && u64::from(rows) * u64::from(variant.columns) <= MAX_FRAME_CELLS), "invalid or excessive frame dimensions");
        }
        if let Some(audio) = &self.audio {
            ensure!(audio == "audio.mp3", "unsupported audio location");
        }
        Ok(())
    }

    pub fn variant(&self, columns: u32) -> Result<&SizeVariant> {
        self.variants.iter().find(|variant| variant.columns == columns).ok_or_else(|| anyhow!("size {columns} is not present"))
    }

    pub fn frame_path(&self, columns: u32, frame: u32, color: bool) -> Result<String> {
        self.variant(columns)?;
        ensure!(frame < self.frame_count, "frame is out of range");
        Ok(format!("variants/{columns}/frame_{frame:06}.{}", if color {"cframe"} else {"txt"}))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StoredFrame {
    pub text: String,
    pub cframe: Option<Vec<u8>>,
}

impl StoredFrame {
    pub fn validate(&self, columns: u32, rows: u32, color: bool) -> Result<()> {
        ensure!(rows > 0 && columns > 0 && u64::from(columns) * u64::from(rows) <= MAX_FRAME_CELLS, "invalid frame dimensions");
        let lines = self.text.strip_suffix('\n').unwrap_or(&self.text).split('\n').collect::<Vec<_>>();
        ensure!(lines.len() == rows as usize && lines.iter().all(|line| line.len() == columns as usize && line.bytes().all(|byte| (32..=126).contains(&byte))), "text frame dimensions or characters do not match metadata");
        ensure!(self.cframe.is_some() == color, "frame color payload does not match metadata");
        if let Some(bytes) = &self.cframe {
            let decoded = decode_frame_payload(bytes, columns, rows, true)?;
            ensure!(decoded.text == self.text, "text and color glyphs disagree");
        }
        Ok(())
    }
}

pub fn decode_frame_payload(bytes: &[u8], columns: u32, rows: u32, color: bool) -> Result<StoredFrame> {
    ensure!(rows > 0 && columns > 0 && u64::from(columns) * u64::from(rows) <= MAX_FRAME_CELLS, "invalid frame dimensions");
    if !color {
        let frame = StoredFrame {text: std::str::from_utf8(bytes).context("invalid frame UTF-8")?.to_string(), cframe: None};
        frame.validate(columns, rows, false)?;
        return Ok(frame);
    }
    let cells = columns as usize * rows as usize;
    let base = 8 + cells * 4;
    ensure!(bytes.len() >= base && read_u32(bytes, 0)? == columns && read_u32(bytes, 4)? == rows, "cframe dimensions or length mismatch");
    ensure!(bytes.len() == base || (bytes.len() == base + 1 + cells * 3 && bytes[base] == 1), "invalid cframe extension");
    let mut text = String::with_capacity(cells + rows as usize);
    for row in 0..rows as usize {
        for col in 0..columns as usize {
            let byte = bytes[8 + (row * columns as usize + col) * 4];
            ensure!((32..=126).contains(&byte), "invalid cframe glyph");
            text.push(byte as char);
        }
        text.push('\n');
    }
    Ok(StoredFrame {text, cframe: Some(bytes.to_vec())})
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetEntry {
    pub columns: u32,
    pub frame: u32,
    pub offset: u64,
    pub length: u32,
    pub color: bool,
}

/// Index entries are frame-major, then ascending columns. Offsets are relative
/// to the payload section, enabling one contiguous read for a playback window.
pub fn write_archive<W, F>(writer: &mut W, manifest: &MultiSizeManifest, mut frame: F) -> Result<()>
where W: Write + Seek, F: FnMut(u32, u32) -> Result<StoredFrame> {
    manifest.validate()?;
    let json = serde_json::to_vec(manifest)?;
    ensure!(json.len() <= 16 * 1024 * 1024, "asset metadata exceeds the 16 MiB binary limit");
    let count = manifest.frame_count as usize * manifest.variants.len();
    let metadata_len = u32::try_from(json.len())?;
    let entry_count = u32::try_from(count)?;
    writer.write_all(ASSET_MAGIC)?;
    writer.write_all(&ASSET_VERSION.to_le_bytes())?;
    writer.write_all(&metadata_len.to_le_bytes())?;
    writer.write_all(&entry_count.to_le_bytes())?;
    writer.write_all(&json)?;
    let payload_start = ASSET_HEADER_BYTES as u64 + json.len() as u64 + count as u64 * ASSET_INDEX_BYTES as u64;
    writer.seek(SeekFrom::Start(payload_start))?;
    let mut entries = Vec::with_capacity(count);
    let mut offset = 0u64;
    for index in 0..manifest.frame_count {
        for variant in &manifest.variants {
            let stored = frame(variant.columns, index)?;
            stored.validate(variant.columns, variant.rows[index as usize], manifest.color)?;
            let payload = stored.cframe.as_deref().unwrap_or(stored.text.as_bytes());
            let length = u32::try_from(payload.len())?;
            writer.write_all(payload)?;
            entries.push(AssetEntry {columns: variant.columns, frame: index, offset, length, color: manifest.color});
            offset = offset.checked_add(u64::from(length)).context("asset length overflow")?;
        }
    }
    writer.seek(SeekFrom::Start(ASSET_HEADER_BYTES as u64 + json.len() as u64))?;
    for entry in entries {
        writer.write_all(&entry.columns.to_le_bytes())?;
        writer.write_all(&entry.frame.to_le_bytes())?;
        writer.write_all(&entry.offset.to_le_bytes())?;
        writer.write_all(&entry.length.to_le_bytes())?;
        writer.write_all(&u32::from(entry.color).to_le_bytes())?;
    }
    writer.seek(SeekFrom::End(0))?;
    Ok(())
}

pub struct MultiSizeArchive<'a> {
    pub manifest: MultiSizeManifest,
    pub entries: Vec<AssetEntry>,
    payload: &'a [u8],
}

impl<'a> MultiSizeArchive<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        ensure!(bytes.len() >= ASSET_HEADER_BYTES && &bytes[..4] == ASSET_MAGIC, "invalid multi-size binary header");
        ensure!(read_u32(bytes, 4)? == ASSET_VERSION, "unsupported binary asset version");
        let metadata_len = read_u32(bytes, 8)? as usize;
        ensure!(metadata_len > 0 && metadata_len <= 16 * 1024 * 1024, "excessive manifest length");
        let count = read_u32(bytes, 12)? as usize;
        let metadata_end = ASSET_HEADER_BYTES.checked_add(metadata_len).context("metadata length overflow")?;
        let payload_start = count.checked_mul(ASSET_INDEX_BYTES).and_then(|n| metadata_end.checked_add(n)).context("index length overflow")?;
        ensure!(payload_start <= bytes.len(), "truncated asset metadata/index");
        let manifest: MultiSizeManifest = serde_json::from_slice(&bytes[ASSET_HEADER_BYTES..metadata_end])?;
        manifest.validate()?;
        ensure!(count == manifest.frame_count as usize * manifest.variants.len(), "asset index count mismatch");
        let payload = &bytes[payload_start..];
        let mut entries = Vec::with_capacity(count);
        let mut expected_offset = 0u64;
        for i in 0..count {
            let pos = metadata_end + i * ASSET_INDEX_BYTES;
            let offset = u64::from_le_bytes(bytes[pos + 8..pos + 16].try_into()?);
            let length = read_u32(bytes, pos + 16)?;
            let kind = read_u32(bytes, pos + 20)?;
            let columns = read_u32(bytes, pos)?;
            let frame = read_u32(bytes, pos + 4)?;
            ensure!(columns == manifest.variants[i % manifest.variants.len()].columns && frame as usize == i / manifest.variants.len(), "asset index ordering mismatch");
            ensure!(offset == expected_offset && length > 0 && kind == u32::from(manifest.color), "invalid asset index entry");
            expected_offset = offset.checked_add(u64::from(length)).context("payload offset overflow")?;
            ensure!(expected_offset <= payload.len() as u64, "truncated frame payload");
            entries.push(AssetEntry {columns, frame, offset, length, color: kind == 1});
        }
        ensure!(expected_offset == payload.len() as u64, "trailing asset data");
        Ok(Self {manifest, entries, payload})
    }

    pub fn frame(&self, columns: u32, frame: u32) -> Result<StoredFrame> {
        let variant = self.manifest.variant(columns)?;
        ensure!(frame < self.manifest.frame_count, "frame is out of range");
        let variant_index = self.manifest.variants.iter().position(|item| item.columns == columns).unwrap();
        let entry = &self.entries[frame as usize * self.manifest.variants.len() + variant_index];
        let start = usize::try_from(entry.offset)?;
        decode_frame_payload(&self.payload[start..start + entry.length as usize], columns, variant.rows[frame as usize], entry.color)
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes.get(offset..offset + 4).context("truncated integer")?.try_into()?))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JsonVariant {
    pub columns: u32,
    pub frames: Vec<StoredFrame>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MultiSizeJson {
    pub manifest: MultiSizeManifest,
    pub variants: Vec<JsonVariant>,
}

impl MultiSizeJson {
    pub fn validate(&self) -> Result<()> {
        self.manifest.validate()?;
        ensure!(self.variants.len() == self.manifest.variants.len(), "JSON variant count mismatch");
        for (stored, variant) in self.variants.iter().zip(&self.manifest.variants) {
            ensure!(stored.columns == variant.columns && stored.frames.len() == self.manifest.frame_count as usize, "JSON variant/frame mismatch");
            for (frame, &rows) in stored.frames.iter().zip(&variant.rows) {
                frame.validate(variant.columns, rows, self.manifest.color)?;
            }
        }
        Ok(())
    }
}

pub const CSV_HEADER: &str = "frame,columns,row,column,char_code,red,green,blue,bg_red,bg_green,bg_blue";

pub fn write_csv_frame<W: Write>(writer: &mut W, columns: u32, index: u32, rows: u32, frame: &StoredFrame) -> Result<()> {
    frame.validate(columns, rows, frame.cframe.is_some())?;
    let glyphs = frame.text.bytes().filter(|&c| c != b'\n').collect::<Vec<_>>();
    let base = 8 + glyphs.len() * 4;
    for (cell, glyph) in glyphs.iter().enumerate() {
        write!(writer, "{index},{columns},{},{},{glyph}", cell / columns as usize, cell % columns as usize)?;
        if let Some(data) = &frame.cframe {
            let start = 8 + cell * 4;
            write!(writer, ",{},{},{}", data[start + 1], data[start + 2], data[start + 3])?;
            if data.len() > base {
                let start = base + 1 + cell * 3;
                writeln!(writer, ",{},{},{}", data[start], data[start + 1], data[start + 2])?;
            } else {
                writeln!(writer, ",,,")?;
            }
        } else {
            writeln!(writer, ",,,,,,")?;
        }
    }
    Ok(())
}

/// Decode a numeric-cell CSV export with its companion manifest. Numeric glyph
/// codes make commas and quotes lossless without embedding JSON inside CSV.
pub fn read_csv(manifest: MultiSizeManifest, csv: &str) -> Result<MultiSizeJson> {
    manifest.validate()?;
    let mut lines = csv.lines();
    ensure!(lines.next() == Some(CSV_HEADER), "invalid CSV header");
    let mut variants = manifest.variants.iter().map(|variant| JsonVariant {columns: variant.columns, frames: Vec::new()}).collect::<Vec<_>>();
    for index in 0..manifest.frame_count {
        for (variant_index, variant) in manifest.variants.iter().enumerate() {
            let rows = variant.rows[index as usize];
            let mut text = String::new();
            let mut rgb = Vec::new();
            let mut bg = Vec::new();
            let mut has_bg = None;
            for cell in 0..rows as usize * variant.columns as usize {
                let line = lines.next().context("truncated cell CSV")?;
                let fields = line.split(',').collect::<Vec<_>>();
                ensure!(fields.len() == 11, "invalid CSV field count");
                let number = |i: usize| fields[i].parse::<u32>().context("invalid CSV number");
                ensure!(number(0)? == index && number(1)? == variant.columns && number(2)? as usize == cell / variant.columns as usize && number(3)? as usize == cell % variant.columns as usize, "CSV cells are missing, repeated or out of order");
                let glyph = u8::try_from(number(4)?)?;
                ensure!((32..=126).contains(&glyph), "invalid CSV glyph");
                text.push(glyph as char);
                if (cell + 1) % variant.columns as usize == 0 {text.push('\n');}
                if manifest.color {
                    for channel in 5..8 {rgb.push(u8::try_from(number(channel)?)?);}
                    let present = !fields[8].is_empty();
                    ensure!(has_bg.unwrap_or(present) == present, "inconsistent CSV background payload");
                    has_bg = Some(present);
                    if present {
                        for channel in 8..11 {bg.push(u8::try_from(number(channel)?)?);}
                    } else {
                        ensure!(fields[8..11].iter().all(|s| s.is_empty()), "partial CSV background");
                    }
                } else {
                    ensure!(fields[5..11].iter().all(|s| s.is_empty()), "unexpected CSV colors");
                }
            }
            let cframe = manifest.color.then(|| crate::frame::encode_cframe(variant.columns, rows, &text, &rgb, has_bg.unwrap_or(false).then_some(bg.as_slice())));
            variants[variant_index].frames.push(StoredFrame {text, cframe});
        }
    }
    if lines.next().is_some() {bail!("trailing CSV rows");}
    let asset = MultiSizeJson {manifest, variants};
    asset.validate()?;
    Ok(asset)
}
