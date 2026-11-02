use crate::convert::{background_analysis_for_mode, decoded_image_to_frame};
use crate::multisize::{write_archive, write_csv_frame, ColumnSizes, MultiSizeManifest, SizeVariant, StoredFrame, CSV_HEADER, MAX_FRAME_CELLS};
use crate::{AsciiConverter, Cancelled, CellColorMode, ConversionOptions, OutputMode, VideoOptions};
use anyhow::{ensure, Context, Result};
use rayon::prelude::*;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiSizeFormat { Directory, Json, Binary, Csv }

/// New multi-size API; the explicit sizes are authoritative. Existing scalar
/// conversion entry points and their file layouts remain unchanged.
#[derive(Clone, Debug)]
pub struct MultiSizeOptions {
    pub sizes: ColumnSizes,
    pub conversion: ConversionOptions,
    pub video: VideoOptions,
    pub formats: Vec<MultiSizeFormat>,
    pub keep_images: bool,
}

impl MultiSizeOptions {
    pub fn new(sizes: ColumnSizes) -> Self {
        Self {sizes, conversion: ConversionOptions::default(), video: VideoOptions::default(), formats: vec![MultiSizeFormat::Directory, MultiSizeFormat::Binary], keep_images: false}
    }
}

struct StagingDirectory(PathBuf);
impl Drop for StagingDirectory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn image_paths(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(directory)?.map(|entry| entry.map(|entry| entry.path())).collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| path.is_file() && image::ImageFormat::from_path(path).is_ok_and(|format| matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg)));
    paths.sort();
    ensure!(!paths.is_empty(), "no PNG or JPEG input frames found");
    Ok(paths)
}

impl AsciiConverter {
    /// Generate one synchronized asset from a still, image directory or video.
    /// All originals are retained, including on cancellation and export failure.
    /// Output must be absent or empty; publication uses a sibling staging dir.
    pub fn convert_multi_size<F: Fn(usize, usize)>(&self, input: &Path, output: &Path, options: &MultiSizeOptions, progress: F) -> Result<MultiSizeManifest> {
        ensure!(options.video.fps > 0 && options.video.fps <= 1000, "FPS must be between 1 and 1000");
        ensure!(options.conversion.font_ratio.is_finite() && options.conversion.font_ratio > 0.0 && options.conversion.font_ratio <= 8.0, "font ratio must be positive and at most 8");
        ensure!(!options.conversion.ascii_chars.is_empty() && options.conversion.ascii_chars.bytes().all(|byte| (32..=126).contains(&byte)), "character ramp must contain printable ASCII");
        ensure!(!options.formats.is_empty(), "at least one export format is required");
        self.check_multi_size_cancel()?;
        let is_directory = input.is_dir();
        ensure!(is_directory || input.is_file(), "input does not exist");
        let is_image = input.is_file() && image::ImageFormat::from_path(input).is_ok();
        if output.exists() {
            ensure!(output.is_dir() && fs::read_dir(output)?.next().is_none(), "multi-size output must be empty; existing data will not be overwritten");
        }
        let parent = output.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let name = output.file_name().context("output must name a directory")?.to_string_lossy();
        let staging = StagingDirectory(parent.join(format!(".{name}.partial-{}-{}", std::process::id(), NEXT_STAGING.fetch_add(1, Ordering::Relaxed))));
        fs::create_dir(&staging.0).context("creating staging directory")?;
        let source_cache = staging.0.join("images");
        let paths = if is_directory {
            image_paths(input)?
        } else if is_image {
            vec![input.to_path_buf()]
        } else {
            fs::create_dir(&source_cache)?;
            crate::video::extract_source_frames(input, &source_cache, &options.video, &self.ffmpeg_config, self.cancel_token.as_ref())?;
            image_paths(&source_cache)?
        };
        let frame_count = u32::try_from(paths.len()).context("too many frames")?;
        let text = !matches!(options.conversion.output_mode, OutputMode::ColorOnly);
        let color = !matches!(options.conversion.output_mode, OutputMode::TextOnly);
        let analysis_mode = if color {options.conversion.cell_color_mode} else {CellColorMode::ForegroundOnly};
        let analysis = background_analysis_for_mode(options.conversion.ascii_chars.as_bytes(), analysis_mode, options.conversion.bg_fit_quality)?;
        let mut manifest = MultiSizeManifest {
            format: "cascii-multisize".to_string(), schema_version: 1, frame_count, fps: options.video.fps,
            default_columns: options.sizes.largest(), font_ratio: options.conversion.font_ratio, luminance: options.conversion.luminance,
            ascii_chars: options.conversion.ascii_chars.clone(), text, color, audio: None,
            outputs: [MultiSizeFormat::Directory, MultiSizeFormat::Binary, MultiSizeFormat::Json, MultiSizeFormat::Csv].into_iter().filter(|format| options.formats.contains(format)).map(|format| match format {
                MultiSizeFormat::Directory => "variants/", MultiSizeFormat::Binary => "frames.bin", MultiSizeFormat::Json => "frames.json", MultiSizeFormat::Csv => "frames.csv",
            }.to_string()).collect(),
            variants: options.sizes.values().iter().map(|&columns| SizeVariant {columns, rows: Vec::with_capacity(paths.len())}).collect(),
        };
        for variant in &manifest.variants { fs::create_dir_all(staging.0.join(format!("variants/{}", variant.columns)))?; }
        let total = paths.len().checked_mul(manifest.variants.len()).context("work count overflow")?;
        progress(0, total);
        for (index, path) in paths.iter().enumerate() {
            self.check_multi_size_cancel()?;
            let preprocessed = if (is_image || is_directory) && options.video.preprocess_filter.is_some() {
                Some(crate::preprocessing::preprocess_image_to_temp(path, options.video.preprocess_filter.as_deref().unwrap(), &self.ffmpeg_config)?)
            } else {None};
            let decoded = image::open(preprocessed.as_ref().map_or(path.as_path(), |image| image.path())).with_context(|| format!("decoding {}", path.display()))?.to_rgb8();
            let (width, height) = decoded.dimensions();
            ensure!(width > 0 && height > 0, "empty source image");
            // Width variants share this single decoded/preprocessed source. A
            // frame is the bounded batch; Rayon never retains the whole movie.
            let rows = manifest.variants.par_iter().map(|variant| -> Result<u32> {
                self.check_multi_size_cancel()?;
                let rows = ((height as f64 / width as f64 * variant.columns as f64 * options.conversion.font_ratio as f64).round() as u64).max(1);
                let cells = rows.checked_mul(u64::from(variant.columns)).context("grid size overflow")?;
                // Background fitting expands each cell into a glyph patch.
                let cell_limit = if analysis_mode.fits_cell_backgrounds() {262_144} else {MAX_FRAME_CELLS};
                ensure!(cells <= cell_limit && rows <= u32::MAX as u64, "requested grid is too large");
                let frame = decoded_image_to_frame(&decoded, &options.conversion, variant.columns, analysis.as_ref())?;
                let prefix = staging.0.join(format!("variants/{}/frame_{index:06}", variant.columns));
                if text {fs::write(prefix.with_extension("txt"), &frame.ascii_text)?;}
                if color {
                    fs::write(prefix.with_extension("cframe"), crate::frame::encode_cframe(frame.width_chars, frame.height_chars, &frame.ascii_text, &frame.rgb_colors, (!frame.bg_rgb_colors.is_empty()).then_some(frame.bg_rgb_colors.as_slice())))?;
                }
                Ok(frame.height_chars)
            }).collect::<Result<Vec<_>>>()?;
            for (variant, rows) in manifest.variants.iter_mut().zip(rows) {variant.rows.push(rows);}
            progress((index + 1) * manifest.variants.len(), total);
        }
        self.check_multi_size_cancel()?;
        if !is_directory && !is_image && options.video.extract_audio {
            crate::video::extract_audio(input, &staging.0, options.video.start.as_deref(), options.video.end.as_deref(), &self.ffmpeg_config, self.cancel_token.as_ref())?;
            manifest.audio = Some("audio.mp3".to_string());
        }
        manifest.validate()?;
        for variant in &manifest.variants {
            let details = crate::ConversionResult {
                frame_count: paths.len(), columns: variant.columns, font_ratio: options.conversion.font_ratio, luminance: options.conversion.luminance,
                fps: Some(options.video.fps), output_mode: if text && color {"text+color"} else if color {"color-only"} else {"text-only"}.to_string(),
                audio_extracted: false, output_dir: staging.0.join(format!("variants/{}", variant.columns)), background_color: "black".to_string(), color: "white".to_string(),
                fit_cell_backgrounds: analysis_mode.fits_cell_backgrounds(), cell_background_mode: analysis_mode.as_str().to_string(),
                bg_fit_quality: options.conversion.bg_fit_quality.as_str().to_string(), bg_luminance: options.conversion.resolve_bg_threshold(), ascii_chars: options.conversion.ascii_chars.clone(),
            };
            details.write_details_file()?;
        }
        if options.formats.contains(&MultiSizeFormat::Binary) {
            let mut file = BufWriter::new(File::create(staging.0.join("frames.bin"))?);
            write_archive(&mut file, &manifest, |columns, index| {
                self.check_multi_size_cancel()?;
                read_staged_frame(&staging.0, &manifest, columns, index)
            })?;
            file.flush()?;
        }
        if options.formats.contains(&MultiSizeFormat::Json) {
            let mut file = BufWriter::new(File::create(staging.0.join("frames.json"))?);
            write!(file, "{{\"manifest\":{},\"variants\":[", serde_json::to_string(&manifest)?)?;
            for (position, variant) in manifest.variants.iter().enumerate() {
                if position > 0 {write!(file, ",")?;}
                write!(file, "{{\"columns\":{},\"frames\":[", variant.columns)?;
                for index in 0..frame_count {
                    self.check_multi_size_cancel()?;
                    if index > 0 {write!(file, ",")?;}
                    serde_json::to_writer(&mut file, &read_staged_frame(&staging.0, &manifest, variant.columns, index)?)?;
                }
                write!(file, "]}}")?;
            }
            writeln!(file, "]}}")?;
            file.flush()?;
        }
        if options.formats.contains(&MultiSizeFormat::Csv) {
            let mut file = BufWriter::new(File::create(staging.0.join("frames.csv"))?);
            writeln!(file, "{CSV_HEADER}")?;
            for index in 0..frame_count {
                for variant in &manifest.variants {
                    self.check_multi_size_cancel()?;
                    write_csv_frame(&mut file, variant.columns, index, variant.rows[index as usize], &read_staged_frame(&staging.0, &manifest, variant.columns, index)?)?;
                }
            }
            file.flush()?;
        }
        if !options.formats.contains(&MultiSizeFormat::Directory) { fs::remove_dir_all(staging.0.join("variants"))?; }
        if source_cache.exists() && !options.keep_images {fs::remove_dir_all(&source_cache)?;}
        fs::write(staging.0.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
        self.check_multi_size_cancel()?;
        if output.exists() {fs::remove_dir(output).context("output changed while generating; refusing to overwrite it")?;}
        fs::rename(&staging.0, output).context("publishing completed multi-size asset")?;
        Ok(manifest)
    }

    fn check_multi_size_cancel(&self) -> Result<()> {
        if self.cancel_token.as_ref().is_some_and(|token| token.is_cancelled()) {return Err(Cancelled.into());}
        Ok(())
    }
}

fn read_staged_frame(root: &Path, manifest: &MultiSizeManifest, columns: u32, index: u32) -> Result<StoredFrame> {
    let variant = manifest.variant(columns)?;
    let bytes = fs::read(root.join(manifest.frame_path(columns, index, manifest.color)?))?;
    crate::multisize::decode_frame_payload(&bytes, columns, variant.rows[index as usize], manifest.color)
}
