//! Experimental single-precision Metal backend with CPU correction for ambiguous
//! scores. The original CPU arithmetic remains the compatibility reference.

use anyhow::{anyhow, ensure, Context, Result};
use image::{DynamicImage, RgbImage};
use objc2::{rc::{autoreleasepool, Retained}, runtime::ProtocolObject};
use objc2_foundation::NSString;
use objc2_metal::*;
use rayon::prelude::*;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::sync::{Mutex, OnceLock};

use crate::{background_fit_optimized, convert::AsciiFrameData, BgFitQuality};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {}

type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type CellFit = (u8, [u8; 3], [u8; 3]);

struct Runtime {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipeline: Retained<ProtocolObject<dyn MTLComputePipelineState>>,
    input: Option<Buffer>,
    output: Option<Buffer>,
}

// SAFETY: Metal objects support use from different threads, but raw MTLBuffer
// contents are not inherently synchronized. Runtime is private and only accessed
// under RUNTIME's mutex; every submission finishes before that lock is released.
unsafe impl Send for Runtime {}

static RUNTIME: OnceLock<std::result::Result<Mutex<Runtime>, String>> = OnceLock::new();

fn runtime() -> Result<&'static Mutex<Runtime>> {
    RUNTIME.get_or_init(|| autoreleasepool(|_| Runtime::new().map(Mutex::new).map_err(|error| error.to_string()))).as_ref().map_err(|error| anyhow!(error.clone()))
}

impl Runtime {
    fn new() -> Result<Self> {
        let device = MTLCreateSystemDefaultDevice().context("no Metal device is available")?;
        let queue = device.newCommandQueue().context("creating Metal command queue")?;
        let options = MTLCompileOptions::new();
        // This selector also works on Macs older than macOS 15 (mathMode does not).
        #[allow(deprecated)]
        options.setFastMathEnabled(false);
        let source = NSString::from_str(include_str!("shaders/background_fit.metal"));
        let library = device.newLibraryWithSource_options_error(&source, Some(&options)).map_err(|error| anyhow!("compiling Metal fitter: {error}"))?;
        let function = library.newFunctionWithName(&NSString::from_str("fit_background")).context("finding Metal fit_background kernel")?;
        let pipeline = device.newComputePipelineStateWithFunction_error(&function).map_err(|error| anyhow!("creating Metal compute pipeline: {error}"))?;
        Ok(Self {device, queue, pipeline, input: None, output: None})
    }
}

fn allocate(device: &ProtocolObject<dyn MTLDevice>, length: usize) -> Result<Buffer> {
    ensure!(length > 0 && length <= device.maxBufferLength(), "Metal buffer size is unsupported: {length}");
    device.newBufferWithLength_options(length, MTLResourceOptions::StorageModeShared).context("allocating Metal buffer")
}

fn upload(device: &ProtocolObject<dyn MTLDevice>, bytes: &[u8]) -> Result<Buffer> {
    let buffer = allocate(device, bytes.len())?;
    // SAFETY: the buffer owns at least bytes.len() writable bytes, and no GPU
    // command references it yet. The Rust slice cannot alias this allocation.
    unsafe {std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.contents().as_ptr().cast::<u8>(), bytes.len());}
    Ok(buffer)
}

pub(crate) struct MetalBackgroundContext {
    alpha: Buffer,
    glyphs: Buffer,
    glyph_count: u32,
    cell_width: u32,
    cell_height: u32,
    reference: background_fit_optimized::OptimizedBackgroundAnalysisContext,
    uniform_cache: Mutex<HashMap<[u8; 3], CellFit>>,
}

// SAFETY: these buffers are fully initialized before publication, never mutated
// afterwards, and only read by GPU commands. Commands complete before fit returns,
// so a context cannot be dropped while its buffers are still in use.
unsafe impl Send for MetalBackgroundContext {}
unsafe impl Sync for MetalBackgroundContext {}

pub(crate) fn background_analysis_context(ascii_chars: &[u8], quality: BgFitQuality) -> Result<MetalBackgroundContext> {
    let source = background_fit_optimized::background_analysis_context(ascii_chars, quality)?;
    let mut alpha = Vec::new();
    let mut glyphs = Vec::new();
    for glyph in &source.glyphs {
        for value in &glyph.alpha {alpha.extend_from_slice(&value.to_le_bytes());}
        for value in [glyph.s_aa, glyph.s_ab, glyph.s_bb, glyph.determinant] {glyphs.extend_from_slice(&(value as f32).to_le_bytes());}
        for value in [u32::from(glyph.byte), u32::from(glyph.degenerate), 0, 0] {glyphs.extend_from_slice(&value.to_le_bytes());}
    }
    let runtime = runtime()?.lock().map_err(|_| anyhow!("Metal runtime lock poisoned"))?;
    Ok(MetalBackgroundContext {
        alpha: upload(&runtime.device, &alpha)?, glyphs: upload(&runtime.device, &glyphs)?,
        glyph_count: u32::try_from(source.glyphs.len()).context("too many glyph candidates")?,
        cell_width: source.cell_width, cell_height: source.cell_height,
        reference: source, uniform_cache: Mutex::new(HashMap::new()),
    })
}

pub(crate) fn fit_rgb_to_ascii_with_context(mut image: RgbImage, font_ratio: f32, threshold: u8, background_threshold: u8, columns: Option<u32>, context: &MetalBackgroundContext) -> Result<AsciiFrameData> {
    let (width, height) = image.dimensions();
    ensure!(width > 0 && height > 0 && font_ratio.is_finite() && font_ratio > 0.0, "invalid Metal input dimensions or font ratio");
    let columns = columns.unwrap_or(width);
    let rows = (height as f32 / width as f32 * columns as f32 * font_ratio).round().max(1.0) as u32;
    let cells = columns.checked_mul(rows).filter(|&cells| cells > 0 && cells <= 262_144).context("Metal grid must contain 1 to 262144 cells")?;
    let target_width = columns.checked_mul(context.cell_width).context("Metal image width overflow")?;
    let target_height = rows.checked_mul(context.cell_height).context("Metal image height overflow")?;
    if image.dimensions() != (target_width, target_height) {
        image = DynamicImage::ImageRgb8(image).resize_exact(target_width, target_height, image::imageops::FilterType::Triangle).to_rgb8();
    }

    let mut bytes = autoreleasepool(|_| -> Result<Vec<u8>> {
        // One bounded submission at a time. Immutable atlas buffers can be
        // shared by Rayon workers; input/output storage is reused under the lock.
        let mut runtime = runtime()?.lock().map_err(|_| anyhow!("Metal runtime lock poisoned"))?;
        let output_size = cells as usize * 8;
        if runtime.input.as_ref().is_none_or(|buffer| buffer.length() < image.as_raw().len()) {
            runtime.input = Some(allocate(&runtime.device, image.as_raw().len())?);
        }
        if runtime.output.as_ref().is_none_or(|buffer| buffer.length() < output_size) {
            runtime.output = Some(allocate(&runtime.device, output_size)?);
        }
        let input = runtime.input.as_ref().unwrap();
        let output = runtime.output.as_ref().unwrap();
        let mut parameters = Vec::with_capacity(32);
        for value in [columns, rows, target_width, context.cell_width, context.cell_height, context.glyph_count, threshold as u32, background_threshold as u32] {
            parameters.extend_from_slice(&value.to_le_bytes());
        }
        let command = runtime.queue.commandBuffer().context("creating Metal command buffer")?;
        let encoder = command.computeCommandEncoder().context("creating Metal compute encoder")?;
        encoder.setComputePipelineState(&runtime.pipeline);
        // SAFETY: buffer lengths and shader indices agree with the checked grid
        // above. The lock excludes simultaneous CPU/GPU access to mutable buffers.
        // setBytes copies the parameter data before it returns.
        unsafe {
            std::ptr::copy_nonoverlapping(image.as_raw().as_ptr(), input.contents().as_ptr().cast::<u8>(), image.as_raw().len());
            for (index, buffer) in [input, &context.alpha, &context.glyphs, output].into_iter().enumerate() {
                encoder.setBuffer_offset_atIndex(Some(buffer), 0, index);
            }
            encoder.setBytes_length_atIndex(NonNull::new(parameters.as_mut_ptr().cast()).unwrap(), parameters.len(), 4);
        }
        let threads = runtime.pipeline.maxTotalThreadsPerThreadgroup().min(128);
        encoder.dispatchThreads_threadsPerThreadgroup(MTLSize {width: cells as usize, height: 1, depth: 1}, MTLSize {width: threads, height: 1, depth: 1});
        encoder.endEncoding();
        command.commit();
        command.waitUntilCompleted();
        ensure!(command.status() == MTLCommandBufferStatus::Completed, "Metal fitting failed: {:?}", command.error());
        // SAFETY: the GPU has completed, the buffer contains output_size bytes,
        // and the copy finishes before another job can reuse this buffer.
        Ok(unsafe {std::slice::from_raw_parts(output.contents().as_ptr().cast::<u8>(), output_size)}.to_vec())
    })?;

    // Preserve sensitive character/color choices using the existing arithmetic.
    // The GPU does the bulk fitting; uniform-color results are cached because
    // their exact tie-break can recur across thousands of cells and video frames.
    bytes.par_chunks_exact_mut(columns as usize * 8).enumerate().try_for_each(|(row, output)| -> Result<()> {
        let mut patch = Vec::new();
        for (column, cell) in output.chunks_exact_mut(8).enumerate() {
            if cell[7] == 0 {continue;}
            let x = column as u32 * context.cell_width;
            let y = row as u32 * context.cell_height;
            let uniform_color = (cell[7] == 3).then(|| image.get_pixel(x, y).0);
            let cached = if let Some(color) = uniform_color {
                context.uniform_cache.lock().map_err(|_| anyhow!("Metal uniform cache lock poisoned"))?.get(&color).copied()
            } else {None};
            let (byte, foreground, background) = if let Some(cached) = cached {cached} else {
                patch.clear();
                for py in 0..context.cell_height {
                    for px in 0..context.cell_width {patch.push(*image.get_pixel(x + px, y + py));}
                }
                let fitted = reference_cell(&patch, &context.reference);
                if let Some(color) = uniform_color {
                    let mut cache = context.uniform_cache.lock().map_err(|_| anyhow!("Metal uniform cache lock poisoned"))?;
                    if cache.len() < 4096 {cache.insert(color, fitted);}
                }
                fitted
            };
            // The GPU already applied the exact luminance thresholds. A marked
            // cell always emits its foreground; recalculate only the bg decision.
            let total_luminance: u32 = if let Some(color) = uniform_color {
                u32::from(crate::cell_filter::luminance_rgb(color[0], color[1], color[2])) * context.cell_width * context.cell_height
            } else {
                patch.iter().map(|pixel| u32::from(crate::cell_filter::luminance_rgb(pixel[0], pixel[1], pixel[2]))).sum()
            };
            cell[0] = byte;
            cell[1..4].copy_from_slice(&foreground);
            cell[4..7].copy_from_slice(if total_luminance >= u32::from(background_threshold) * context.cell_width * context.cell_height {&background} else {&[0, 0, 0]});
        }
        Ok(())
    })?;

    let mut ascii_text = String::with_capacity(cells as usize + rows as usize);
    let mut rgb_colors = Vec::with_capacity(cells as usize * 3);
    let mut bg_rgb_colors = Vec::with_capacity(cells as usize * 3);
    for row in bytes.chunks_exact(columns as usize * 8) {
        for cell in row.chunks_exact(8) {
            ascii_text.push(cell[0] as char);
            rgb_colors.extend_from_slice(&cell[1..4]);
            bg_rgb_colors.extend_from_slice(&cell[4..7]);
        }
        ascii_text.push('\n');
    }
    Ok(AsciiFrameData {ascii_text, width_chars: columns, height_chars: rows, rgb_colors, bg_rgb_colors})
}

fn reference_cell(patch: &[image::Rgb<u8>], context: &background_fit_optimized::OptimizedBackgroundAnalysisContext) -> CellFit {
    let mut sum = [0u64; 3];
    let mut squared = 0u64;
    for pixel in patch {
        for channel in 0..3 {
            let value = u64::from(pixel[channel]);
            sum[channel] += value;
            squared += value * value;
        }
    }
    let average = sum.map(|value| (value / patch.len() as u64) as u8);
    let sum = sum.map(|value| value as f64);
    let mut best = (b' ', average, average);
    let mut best_error = f64::INFINITY;
    for glyph in &context.glyphs {
        let (foreground, background, error) = background_fit_optimized::fit_colors(patch, glyph, average, sum, squared as f64);
        if error < best_error {
            best = (glyph.byte, foreground, background);
            best_error = error;
        }
    }
    best
}

#[cfg(test)]
#[path = "metal_validation.rs"]
mod validation;
