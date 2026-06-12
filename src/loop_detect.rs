use crate::convert::read_cframe_to_frame_data;
use anyhow::{anyhow, Context, Result};
use dialoguer::Select;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const DEFAULT_ASCII_RAMP: &str = " .'`^,:;Il!i><~+_-?][}{1)(|/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$";
const QUICK_SAMPLE_CELLS: usize = 64;
const QUICK_THRESHOLD_MARGIN: f32 = 0.03;
const FRAME_THRESHOLD_MARGIN: f32 = 0.08;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMatchMode {
    ExactText,
    VisualText,
    VisualTextAndColor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopDetectionOptions {
    pub mode: LoopMatchMode,
    pub minimum_distance: usize,
    pub validation_window: usize,
    pub similarity_threshold: f32,
    pub ascii_ramp: String,
}

impl Default for LoopDetectionOptions {
    fn default() -> Self {
        Self {mode: LoopMatchMode::VisualText, minimum_distance: 24, validation_window: 8, similarity_threshold: 0.93, ascii_ramp: DEFAULT_ASCII_RAMP.to_string()}
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopCandidate {
    /// Frame numbers at which the repeated sequence starts.
    pub occurrences: Vec<usize>,
    /// Distance between occurrences in the sorted frame sequence.
    pub period_frames: usize,
    /// Combined text/color confidence in the range 0.0..=1.0.
    pub confidence: f32,
    pub average_text_similarity: f32,
    pub average_color_similarity: Option<f32>,
}

#[derive(Default)]
struct FramePaths {
    text: Option<PathBuf>,
    color: Option<PathBuf>,
}

struct LoadedFrame {
    number: usize,
    width: usize,
    height: usize,
    glyphs: Vec<u8>,
    exact_text: Vec<u8>,
    exact_hash: u64,
    foreground: Option<Vec<u8>>,
    background: Option<Vec<u8>>,
}

#[derive(Clone, Copy)]
struct FrameMetrics {
    combined: f32,
    text: f32,
    color: Option<f32>,
}

#[derive(Clone, Copy)]
struct SequenceMetrics {
    combined: f32,
    text: f32,
    color: Option<f32>,
}

struct FrameComparisonCache<'a> {
    frames: &'a [LoadedFrame],
    mode: LoopMatchMode,
    ramp: &'a RampLookup,
    quick: Vec<Option<FrameMetrics>>,
    full: Vec<Option<FrameMetrics>>,
}

impl<'a> FrameComparisonCache<'a> {
    fn new(frames: &'a [LoadedFrame], mode: LoopMatchMode, ramp: &'a RampLookup) -> Self {
        let pair_count = frames.len().saturating_mul(frames.len().saturating_sub(1)) / 2;
        Self {frames, mode, ramp, quick: vec![None; pair_count], full: vec![None; pair_count]}
    }

    fn compare(&mut self, left: usize, right: usize, quick: bool) -> FrameMetrics {
        debug_assert!(left < right);
        let frame_count = self.frames.len();
        let index = left * frame_count - left * (left + 1) / 2 + right - left - 1;
        let metrics = if quick {&mut self.quick} else {&mut self.full};
        if let Some(metrics) = metrics[index] {
            return metrics;
        }

        let computed = compare_frames(&self.frames[left], &self.frames[right], self.mode, self.ramp, quick);
        metrics[index] = Some(computed);
        computed
    }
}

struct RampLookup {
    positions: [i16; 256],
    max_distance: f32,
}

impl RampLookup {
    fn new(ramp: &str) -> Result<Self> {
        if ramp.is_empty() {
            return Err(anyhow!("ASCII ramp cannot be empty"));
        }
        if !ramp.is_ascii() {
            return Err(anyhow!("ASCII ramp must contain only ASCII characters"));
        }

        let mut positions = [-1; 256];
        for (index, byte) in ramp.bytes().enumerate() {
            positions[byte as usize] = index as i16;
        }

        Ok(Self {positions, max_distance: ramp.len().saturating_sub(1).max(1) as f32})
    }

    fn distance(&self, left: u8, right: u8) -> f32 {
        if left == right {
            return 0.0;
        }

        let left_position = self.positions[left as usize];
        let right_position = self.positions[right as usize];
        if left_position < 0 || right_position < 0 {
            return 1.0;
        }

        (left_position - right_position).unsigned_abs() as f32 / self.max_distance
    }
}

pub fn detect_frame_loops(directory: &Path, options: &LoopDetectionOptions) -> Result<Vec<LoopCandidate>> {
    validate_options(options)?;
    let frames = load_frames(directory)?;
    let window = options.validation_window;

    if frames.len() < options.minimum_distance + window {
        return Ok(Vec::new());
    }

    let ramp = RampLookup::new(&options.ascii_ramp)?;
    let mut comparison_cache = FrameComparisonCache::new(&frames, options.mode, &ramp);
    let mut candidates = Vec::new();
    let maximum_period = frames.len() - window;

    for period in options.minimum_distance..=maximum_period {
        let maximum_start = frames.len() - period - window;
        let mut matching_run = Vec::new();

        for start in 0..=maximum_start {
            let quick_metrics = compare_sequence(&mut comparison_cache, start, start + period, window, options, true);
            let full_metrics = quick_metrics.and_then(|_| compare_sequence(&mut comparison_cache, start, start + period, window, options, false));

            if let Some(metrics) = full_metrics {
                matching_run.push((start, metrics));
            } else if !matching_run.is_empty() {
                push_best_candidate(&mut candidates, &frames, period, window, options, &mut comparison_cache, &matching_run);
                matching_run.clear();
            }
        }
        if !matching_run.is_empty() {
            push_best_candidate(&mut candidates, &frames, period, window, options, &mut comparison_cache, &matching_run);
        }
    }

    Ok(remove_redundant_candidates(candidates, &frames, options.mode))
}

fn push_best_candidate(candidates: &mut Vec<LoopCandidate>, frames: &[LoadedFrame], period: usize, window: usize, options: &LoopDetectionOptions, comparison_cache: &mut FrameComparisonCache<'_>, matching_run: &[(usize, SequenceMetrics)]) {
    let Some(&(mut start, mut first_metrics)) = matching_run.first() else {
        return;
    };
    for &(candidate_start, candidate_metrics) in matching_run.iter().skip(1) {
        if candidate_metrics.combined > first_metrics.combined + 0.002 {
            start = candidate_start;
            first_metrics = candidate_metrics;
        }
    }

    let mut occurrences = vec![frames[start].number, frames[start + period].number];
    let mut combined_total = first_metrics.combined;
    let mut text_total = first_metrics.text;
    let mut color_total = first_metrics.color.unwrap_or(0.0);
    let mut color_count = usize::from(first_metrics.color.is_some());
    let mut comparison_count = 1usize;
    let mut next_start = start + period * 2;

    // Every later occurrence is compared directly with the strongest first
    // occurrence. Similarity is not allowed to chain through an intermediate
    // occurrence.
    while next_start + window <= frames.len() {
        let quick = compare_sequence(comparison_cache, start, next_start, window, options, true);
        let metrics = quick.and_then(|_| compare_sequence(comparison_cache, start, next_start, window, options, false));

        let Some(metrics) = metrics else {
            break;
        };

        occurrences.push(frames[next_start].number);
        combined_total += metrics.combined;
        text_total += metrics.text;
        if let Some(color) = metrics.color {
            color_total += color;
            color_count += 1;
        }
        comparison_count += 1;
        next_start += period;
    }

    candidates.push(LoopCandidate {occurrences, period_frames: period, confidence: combined_total / comparison_count as f32, average_text_similarity: text_total / comparison_count as f32, average_color_similarity: (color_count > 0).then_some(color_total / color_count as f32)});
}

fn validate_options(options: &LoopDetectionOptions) -> Result<()> {
    if options.minimum_distance == 0 {
        return Err(anyhow!("minimum_distance must be at least 1"));
    }
    if options.validation_window == 0 {
        return Err(anyhow!("validation_window must be at least 1"));
    }
    if !options.similarity_threshold.is_finite() || !(0.0..=1.0).contains(&options.similarity_threshold) {
        return Err(anyhow!("similarity_threshold must be finite and between 0 and 1"));
    }
    Ok(())
}

fn compare_sequence(comparison_cache: &mut FrameComparisonCache<'_>, left_start: usize, right_start: usize, window: usize, options: &LoopDetectionOptions, quick: bool) -> Option<SequenceMetrics> {
    if left_start + window > comparison_cache.frames.len() || right_start + window > comparison_cache.frames.len() {
        return None;
    }

    let threshold_margin = if quick && options.mode != LoopMatchMode::ExactText {QUICK_THRESHOLD_MARGIN} else {0.0};
    let average_threshold = (options.similarity_threshold - threshold_margin).max(0.0);
    let per_frame_threshold = (average_threshold - FRAME_THRESHOLD_MARGIN).max(0.0);
    let mut combined_total = 0.0;
    let mut text_total = 0.0;
    let mut color_total = 0.0;
    let mut color_count = 0usize;

    for offset in 0..window {
        let metrics = comparison_cache.compare(left_start + offset, right_start + offset, quick);
        if metrics.combined < per_frame_threshold {
            return None;
        }

        combined_total += metrics.combined;
        text_total += metrics.text;
        if let Some(color) = metrics.color {
            color_total += color;
            color_count += 1;
        }
    }

    let divisor = window as f32;
    let combined = combined_total / divisor;
    if combined < average_threshold {
        return None;
    }

    Some(SequenceMetrics {combined, text: text_total / divisor, color: (color_count > 0).then_some(color_total / color_count as f32)})
}

fn compare_frames(left: &LoadedFrame, right: &LoadedFrame, mode: LoopMatchMode, ramp: &RampLookup, quick: bool) -> FrameMetrics {
    if left.width != right.width || left.height != right.height {
        return FrameMetrics {combined: 0.0, text: 0.0, color: None};
    }

    if mode == LoopMatchMode::ExactText {
        let equal = left.exact_hash == right.exact_hash && left.exact_text == right.exact_text;
        let score = if equal {1.0} else {0.0};
        return FrameMetrics {combined: score, text: score, color: None};
    }

    let cell_count = left.glyphs.len();
    if cell_count == 0 || right.glyphs.len() != cell_count {
        return FrameMetrics {combined: 0.0, text: 0.0, color: None};
    }

    let sample_count = if quick {cell_count.min(QUICK_SAMPLE_CELLS)} else {cell_count};
    let mut literal_mismatches = 0usize;
    let mut occupancy_mismatches = 0usize;
    let mut glyph_distance = 0.0;
    let mut foreground_distance = 0.0;
    let mut foreground_cells = 0usize;
    let mut background_distance = 0.0;
    let mut background_cells = 0usize;
    let compare_color = mode == LoopMatchMode::VisualTextAndColor;
    let foregrounds = left.foreground.as_ref().zip(right.foreground.as_ref());
    let backgrounds = left.background.as_ref().zip(right.background.as_ref());
    let background_payload_mismatch = left.background.is_some() != right.background.is_some();

    for sample in 0..sample_count {
        let index = sample * cell_count / sample_count;
        let left_glyph = left.glyphs[index];
        let right_glyph = right.glyphs[index];
        let left_occupied = left_glyph != b' ';
        let right_occupied = right_glyph != b' ';

        literal_mismatches += usize::from(left_glyph != right_glyph);
        occupancy_mismatches += usize::from(left_occupied != right_occupied);
        glyph_distance += ramp.distance(left_glyph, right_glyph);

        if compare_color && (left_occupied || right_occupied) {
            if let Some((left_colors, right_colors)) = foregrounds {
                foreground_distance += rgb_distance(left_colors, right_colors, index);
                foreground_cells += 1;
            }
        }

        if compare_color {
            if let Some((left_colors, right_colors)) = backgrounds {
                background_distance += rgb_distance(left_colors, right_colors, index);
                background_cells += 1;
            }
        }
    }

    let sample_divisor = sample_count as f32;
    let literal_similarity = 1.0 - literal_mismatches as f32 / sample_divisor;
    let occupancy_similarity = 1.0 - occupancy_mismatches as f32 / sample_divisor;
    let glyph_similarity = 1.0 - glyph_distance / sample_divisor;
    let text_similarity = (literal_similarity * 0.65 + occupancy_similarity * 0.15 + glyph_similarity * 0.20).clamp(0.0, 1.0);

    if !compare_color {
        return FrameMetrics {combined: text_similarity, text: text_similarity, color: None};
    }

    let foreground_similarity = (foreground_cells > 0).then_some(1.0 - foreground_distance / foreground_cells as f32);
    let background_similarity = if background_payload_mismatch {Some(0.0)} else {(background_cells > 0).then_some(1.0 - background_distance / background_cells as f32)};
    let color_similarity = match (foreground_similarity, background_similarity) {
        (Some(foreground), Some(background)) => Some(foreground * 0.7 + background * 0.3),
        (Some(foreground), None) => Some(foreground),
        (None, Some(background)) => Some(background),
        (None, None) => None,
    };

    let combined = color_similarity.map(|color| text_similarity * 0.65 + color * 0.35).unwrap_or(0.0);

    FrameMetrics {combined: combined.clamp(0.0, 1.0), text: text_similarity, color: color_similarity.map(|value| value.clamp(0.0, 1.0))}
}

fn rgb_distance(left: &[u8], right: &[u8], cell_index: usize) -> f32 {
    let offset = cell_index * 3;
    let channel_total = (left[offset] as f32 - right[offset] as f32).abs() + (left[offset + 1] as f32 - right[offset + 1] as f32).abs() + (left[offset + 2] as f32 - right[offset + 2] as f32).abs();
    channel_total / (255.0 * 3.0)
}

fn frames_are_identical(left: &LoadedFrame, right: &LoadedFrame, mode: LoopMatchMode) -> bool {
    if left.width != right.width || left.height != right.height {
        return false;
    }

    match mode {
        LoopMatchMode::ExactText => left.exact_hash == right.exact_hash && left.exact_text == right.exact_text,
        LoopMatchMode::VisualText => left.glyphs == right.glyphs,
        LoopMatchMode::VisualTextAndColor => left.glyphs == right.glyphs && left.foreground == right.foreground && left.background == right.background,
    }
}

fn canonical_duplicate_frames(frames: &[LoadedFrame], mode: LoopMatchMode) -> HashMap<usize, usize> {
    let mut canonical = HashMap::with_capacity(frames.len());
    let mut run_start = None;

    for (index, frame) in frames.iter().enumerate() {
        let continues_run = index > 0
            && frames[index - 1].number.checked_add(1) == Some(frame.number)
            && frames_are_identical(&frames[index - 1], frame, mode);
        if !continues_run {
            run_start = Some(frame.number);
        }
        canonical.insert(frame.number, run_start.unwrap_or(frame.number));
    }

    canonical
}

fn remove_redundant_candidates(mut candidates: Vec<LoopCandidate>, frames: &[LoadedFrame], mode: LoopMatchMode) -> Vec<LoopCandidate> {
    candidates.sort_by(|left, right| right.occurrences.len().cmp(&left.occurrences.len()).then_with(|| right.confidence.total_cmp(&left.confidence)).then_with(|| left.period_frames.cmp(&right.period_frames)).then_with(|| left.occurrences.cmp(&right.occurrences)));

    let canonical_frames = canonical_duplicate_frames(frames, mode);
    let mut retained: Vec<LoopCandidate> = Vec::new();
    for candidate in candidates {
        let candidate_pair = candidate.occurrences.get(0..2).map(|pair| {
            (
                canonical_frames.get(&pair[0]).copied().unwrap_or(pair[0]),
                canonical_frames.get(&pair[1]).copied().unwrap_or(pair[1]),
            )
        });
        if let Some(existing_index) = retained.iter().position(|existing| {
            let existing_pair = existing.occurrences.get(0..2).map(|pair| {
                (
                    canonical_frames.get(&pair[0]).copied().unwrap_or(pair[0]),
                    canonical_frames.get(&pair[1]).copied().unwrap_or(pair[1]),
                )
            });
            candidate_pair.is_some() && candidate_pair == existing_pair
        }) {
            if candidate.occurrences.get(0..2) < retained[existing_index].occurrences.get(0..2) {
                retained[existing_index] = candidate;
            }
            continue;
        }

        let redundant = retained.iter().any(|existing| {
            candidate.period_frames % existing.period_frames == 0
                && candidate.occurrences.iter().all(|occurrence| existing.occurrences.contains(occurrence))
        });
        if !redundant {
            retained.push(candidate);
        }
    }

    retained.sort_by(|left, right| left.occurrences[0].cmp(&right.occurrences[0]).then_with(|| left.period_frames.cmp(&right.period_frames)).then_with(|| right.confidence.total_cmp(&left.confidence)));
    retained
}

fn load_frames(directory: &Path) -> Result<Vec<LoadedFrame>> {
    let mut paths_by_number: BTreeMap<usize, FramePaths> = BTreeMap::new();

    for entry in WalkDir::new(directory).min_depth(1).max_depth(1).into_iter().filter_map(Result::ok) {
        let path = entry.into_path();
        if !path.is_file() {
            continue;
        }

        let Some(number) = frame_number(&path) else {
            continue;
        };
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };

        let paths = paths_by_number.entry(number).or_default();
        match extension.to_ascii_lowercase().as_str() {
            "txt" => paths.text = Some(path),
            "cframe" => paths.color = Some(path),
            _ => {}
        }
    }

    if paths_by_number.is_empty() {
        return Err(anyhow!("No frame_*.txt or frame_*.cframe files found in {}", directory.display()));
    }

    paths_by_number.into_iter().map(|(number, paths)| load_frame(number, paths)).collect()
}

fn load_frame(number: usize, paths: FramePaths) -> Result<LoadedFrame> {
    let text_bytes = paths.text.as_ref().map(|path| fs::read(path).with_context(|| format!("reading {}", path.display()))).transpose()?;

    let (width, height, glyphs, foreground, background) = if let Some(color_path) = paths.color.as_ref() {
        let data = read_cframe_to_frame_data(color_path)?;
        let glyphs = data.ascii_text.bytes().filter(|byte| *byte != b'\n' && *byte != b'\r').collect::<Vec<_>>();
        let expected_cells = data.width_chars as usize * data.height_chars as usize;
        if glyphs.len() != expected_cells {
            return Err(anyhow!("cframe {} contains {} glyphs, expected {}", color_path.display(), glyphs.len(), expected_cells));
        }

        let foreground = (data.rgb_colors.len() == expected_cells * 3).then_some(data.rgb_colors);
        let background = (data.bg_rgb_colors.len() == expected_cells * 3).then_some(data.bg_rgb_colors);
        (data.width_chars as usize, data.height_chars as usize, glyphs, foreground, background)
    } else {
        let bytes = text_bytes.as_deref().ok_or_else(|| anyhow!("frame {} has no readable data", number))?;
        let (width, height, glyphs) = normalize_text_frame(bytes)?;
        (width, height, glyphs, None, None)
    };

    let exact_text = if let Some(bytes) = text_bytes {bytes} else {glyphs.clone()};
    let mut hasher = DefaultHasher::new();
    exact_text.hash(&mut hasher);

    Ok(LoadedFrame {number, width, height, glyphs, exact_text, exact_hash: hasher.finish(), foreground, background})
}

fn normalize_text_frame(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    if !bytes.is_ascii() {
        return Err(anyhow!("ASCII frame contains non-ASCII data"));
    }

    let text = std::str::from_utf8(bytes).context("decoding ASCII frame")?;
    let lines = text.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return Err(anyhow!("ASCII frame is empty"));
    }

    let width = lines.iter().map(|line| line.len()).max().unwrap_or(0);
    if width == 0 {
        return Err(anyhow!("ASCII frame has zero width"));
    }

    let mut glyphs = Vec::with_capacity(width * lines.len());
    for line in &lines {
        glyphs.extend_from_slice(line.as_bytes());
        glyphs.resize(glyphs.len() + width - line.len(), b' ');
    }

    Ok((width, lines.len(), glyphs))
}

fn frame_number(path: &Path) -> Option<usize> {
    let stem = path.file_stem()?.to_str()?;
    stem.strip_prefix("frame_")?.parse().ok()
}

pub fn run_find_loop(dir: &Path) -> Result<()> {
    run_find_loop_with_options(dir, &LoopDetectionOptions::default())
}

pub fn run_find_loop_with_options(dir: &Path, options: &LoopDetectionOptions) -> Result<()> {
    let candidates = detect_frame_loops(dir, options)?;
    if candidates.is_empty() {
        println!("No loopable sequences detected.");
        return Ok(());
    }

    println!("Found loops:");
    for (index, candidate) in candidates.iter().enumerate() {
        println!("{}: frames {} (period {}, {:.1}% confidence)", index + 1, candidate.occurrences.iter().map(usize::to_string).collect::<Vec<_>>().join(", "), candidate.period_frames, candidate.confidence * 100.0);
    }

    let frames = load_text_frames(dir)?;
    if frames.is_empty() {
        println!("Loop editing requires frame_*.txt files; detection completed without editing.");
        return Ok(());
    }

    let frame_indices = frames.iter().enumerate().map(|(index, (number, _))| (*number, index)).collect::<HashMap<_, _>>();
    let loops = candidates
        .iter()
        .filter_map(|candidate| {
            let start = frame_indices.get(candidate.occurrences.first()?)?;
            let end = frame_indices.get(candidate.occurrences.get(1)?)?;
            Some((*start, *end))
        })
        .collect::<Vec<_>>();

    if loops.is_empty() {
        println!("Detected loops could not be mapped to editable text frames.");
        return Ok(());
    }

    loop {
        let choices = vec!["Export loop", "Repeat loop", "Quit"];
        let selection = Select::new().with_prompt("Choose an action").default(0).items(&choices).interact()?;
        match selection {
            0 => {
                let labels = loop_labels(&frames, &loops);
                let index = Select::new().with_prompt("Select loop to export").default(0).items(&labels).interact()?;
                let (start, end) = loops[index];
                export_loop(dir, &frames, start, end)?;
                println!("Exported loop {}..{}", frames[start].0, frames[end].0);
            }
            1 => {
                let labels = loop_labels(&frames, &loops);
                let index = Select::new().with_prompt("Select loop to repeat").default(0).items(&labels).interact()?;
                let (start, end) = loops[index];
                repeat_loop(dir, &frames, start, end)?;
                println!("Loop repeated");
            }
            _ => break,
        }
    }

    Ok(())
}

fn load_text_frames(dir: &Path) -> Result<Vec<(usize, String)>> {
    let mut frames = Vec::new();
    for entry in WalkDir::new(dir).min_depth(1).max_depth(1).into_iter().filter_map(Result::ok) {
        let path = entry.into_path();
        if path.extension().and_then(|value| value.to_str()) != Some("txt") {
            continue;
        }
        let Some(number) = frame_number(&path) else {
            continue;
        };
        let content = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        frames.push((number, content));
    }
    frames.sort_by_key(|(number, _)| *number);
    Ok(frames)
}

fn loop_labels(frames: &[(usize, String)], loops: &[(usize, usize)]) -> Vec<String> {
    loops.iter().map(|(start, end)| format!("{}..{}", frames[*start].0, frames[*end].0)).collect()
}

fn export_loop(dir: &Path, frames: &[(usize, String)], start_idx: usize, end_idx: usize) -> Result<()> {
    let start_frame = frames[start_idx].0;
    let end_frame = frames[end_idx].0;
    let out = dir.with_file_name(format!("{}_loop_{}_{}", dir.file_name().and_then(|value| value.to_str()).unwrap_or("frames"), start_frame, end_frame));
    fs::create_dir_all(&out)?;
    for (counter, frame) in frames.iter().take(end_idx + 1).skip(start_idx).enumerate() {
        let filename = out.join(format!("frame_{:04}.txt", counter + 1));
        fs::write(filename, &frame.1)?;
    }
    Ok(())
}

fn repeat_loop(dir: &Path, frames: &[(usize, String)], start_idx: usize, end_idx: usize) -> Result<()> {
    let mut new_sequence = Vec::with_capacity(frames.len() + (end_idx - start_idx + 1));
    for (_, content) in frames.iter().take(end_idx + 1) {
        new_sequence.push(content.clone());
    }
    for frame in frames.iter().take(end_idx + 1).skip(start_idx) {
        new_sequence.push(frame.1.clone());
    }
    for (_, content) in frames.iter().skip(end_idx + 1) {
        new_sequence.push(content.clone());
    }

    for entry in WalkDir::new(dir).min_depth(1).max_depth(1).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if path.is_file() && path.file_name().and_then(|value| value.to_str()).is_some_and(|name| name.starts_with("frame_") && name.ends_with(".txt")) {
            let _ = fs::remove_file(path);
        }
    }
    for (index, content) in new_sequence.iter().enumerate() {
        let filename = dir.join(format!("frame_{:04}.txt", index + 1));
        fs::write(filename, content)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::write_cframe_binary;
    use tempfile::TempDir;

    fn options(mode: LoopMatchMode, minimum_distance: usize, validation_window: usize, threshold: f32) -> LoopDetectionOptions {
        LoopDetectionOptions {mode, minimum_distance, validation_window, similarity_threshold: threshold, ascii_ramp: " .:-=+*#@".to_string()}
    }

    fn write_text(dir: &Path, number: usize, text: &str) {
        fs::write(dir.join(format!("frame_{number:04}.txt")), text).unwrap();
    }

    fn write_color(dir: &Path, number: usize, text: &str, foreground: &[[u8; 3]], background: Option<&[[u8; 3]]>) {
        let foreground = foreground.iter().flat_map(|color| color.iter().copied()).collect::<Vec<_>>();
        let background = background.map(|colors| colors.iter().flat_map(|color| color.iter().copied()).collect::<Vec<_>>());
        let width = text.lines().next().unwrap().len() as u32;
        let height = text.lines().count() as u32;
        write_cframe_binary(width, height, text, &foreground, background.as_deref(), &dir.join(format!("frame_{number:04}.cframe"))).unwrap();
    }

    #[test]
    fn exact_mode_finds_a_distant_repeated_sequence() {
        let temp = TempDir::new().unwrap();
        for index in 1..=100 {
            write_text(temp.path(), index, &format!("{index:03}\n"));
        }
        write_text(temp.path(), 101, "001\n");
        write_text(temp.path(), 102, "002\n");
        write_text(temp.path(), 103, "003\n");

        let candidates = detect_frame_loops(temp.path(), &options(LoopMatchMode::ExactText, 50, 3, 1.0)).unwrap();

        assert!(candidates.iter().any(|candidate| {candidate.period_frames == 100 && candidate.occurrences == vec![1, 101] && candidate.confidence == 1.0}));
    }

    #[test]
    fn visual_text_finds_adjacent_glyph_substitutions() {
        let temp = TempDir::new().unwrap();
        let first = ["....::::\n", "::::----\n", "----====\n", "====++++\n"];
        let second = ["...:::::\n", ":::-----\n", "---=====\n", "===+++++\n"];
        for (index, text) in first.iter().chain(second.iter()).enumerate() {
            write_text(temp.path(), index + 1, text);
        }

        let candidates = detect_frame_loops(temp.path(), &options(LoopMatchMode::VisualText, 4, 4, 0.9)).unwrap();

        assert!(candidates.iter().any(|candidate| {candidate.period_frames == 4 && candidate.occurrences == vec![1, 5] && candidate.average_text_similarity > 0.9}));
    }

    #[test]
    fn isolated_similar_frames_do_not_form_a_visual_loop() {
        let temp = TempDir::new().unwrap();
        for (index, text) in ["....\n", "####\n", "@@@@\n", ".::.\n", "    \n", "++++\n"].iter().enumerate() {
            write_text(temp.path(), index + 1, text);
        }

        let candidates = detect_frame_loops(temp.path(), &options(LoopMatchMode::VisualText, 3, 2, 0.9)).unwrap();

        assert!(candidates.is_empty());
    }

    #[test]
    fn color_mode_compares_foreground_and_background() {
        let temp = TempDir::new().unwrap();
        let first_foregrounds = [[[200, 20, 20], [20, 200, 20]], [[180, 30, 30], [30, 180, 30]]];
        let second_foregrounds = [[[202, 21, 19], [19, 201, 21]], [[181, 29, 31], [31, 181, 29]]];
        let first_backgrounds = [[[5, 5, 20], [5, 5, 20]], [[10, 10, 25], [10, 10, 25]]];
        let second_backgrounds = [[[6, 5, 19], [5, 6, 21]], [[11, 9, 25], [9, 11, 24]]];

        for index in 0..2 {
            write_color(temp.path(), index + 1, "##\n", &first_foregrounds[index], Some(&first_backgrounds[index]));
            write_color(temp.path(), index + 3, "##\n", &second_foregrounds[index], Some(&second_backgrounds[index]));
        }

        let candidates = detect_frame_loops(temp.path(), &options(LoopMatchMode::VisualTextAndColor, 2, 2, 0.98)).unwrap();

        assert!(candidates.iter().any(|candidate| {candidate.period_frames == 2 && candidate.occurrences == vec![1, 3] && candidate.average_color_similarity.unwrap() > 0.99}));
    }

    #[test]
    fn later_occurrences_must_match_the_first_occurrence() {
        let temp = TempDir::new().unwrap();
        for (index, text) in ["..........\n", "..........\n", "........--\n", "........--\n", "......----\n", "......----\n"].iter().enumerate() {
            write_text(temp.path(), index + 1, text);
        }

        let candidates = detect_frame_loops(temp.path(), &options(LoopMatchMode::VisualText, 2, 2, 0.8)).unwrap();
        let candidate = candidates.iter().find(|candidate| candidate.period_frames == 2).unwrap();

        assert_eq!(candidate.occurrences, vec![1, 3]);
    }

    #[test]
    fn detects_three_occurrences_in_cframe_only_directory() {
        let temp = TempDir::new().unwrap();
        let sequence = [("##\n", [[120, 20, 20], [20, 120, 20]]), ("++\n", [[20, 20, 120], [120, 120, 20]])];
        for cycle in 0..3 {
            for (offset, (text, colors)) in sequence.iter().enumerate() {
                write_color(temp.path(), cycle * 2 + offset + 1, text, colors, None);
            }
        }

        let candidates = detect_frame_loops(temp.path(), &options(LoopMatchMode::VisualTextAndColor, 2, 2, 0.99)).unwrap();

        assert!(candidates.iter().any(|candidate| {candidate.period_frames == 2 && candidate.occurrences == vec![1, 3, 5]}));
    }

    #[test]
    fn collapses_candidates_whose_endpoints_are_adjacent_duplicate_frames() {
        let temp = TempDir::new().unwrap();
        for (index, text) in ["A\n", "B\n", "C\n", "D\n", "A\n", "A\n", "B\n"].iter().enumerate() {
            write_text(temp.path(), index + 1, text);
        }
        let frames = load_frames(temp.path()).unwrap();
        let candidates = vec![
            LoopCandidate {occurrences: vec![1, 5], period_frames: 4, confidence: 0.98, average_text_similarity: 0.98, average_color_similarity: None},
            LoopCandidate {occurrences: vec![1, 6], period_frames: 5, confidence: 0.99, average_text_similarity: 0.99, average_color_similarity: None},
        ];

        let deduplicated = remove_redundant_candidates(candidates, &frames, LoopMatchMode::VisualText);

        assert_eq!(deduplicated.len(), 1);
        assert_eq!(deduplicated[0].occurrences, vec![1, 5]);
    }

    #[test]
    fn visual_text_deduplicates_adjacent_endpoints_even_when_colors_differ() {
        let temp = TempDir::new().unwrap();
        for number in 1..=4 {
            write_color(temp.path(), number, "A\n", &[[10, 10, 10]], None);
        }
        write_color(temp.path(), 5, "A\n", &[[20, 20, 20]], None);
        write_color(temp.path(), 6, "A\n", &[[200, 200, 200]], None);
        let frames = load_frames(temp.path()).unwrap();
        let candidates = vec![
            LoopCandidate {occurrences: vec![1, 5], period_frames: 4, confidence: 0.98, average_text_similarity: 1.0, average_color_similarity: None},
            LoopCandidate {occurrences: vec![1, 6], period_frames: 5, confidence: 0.99, average_text_similarity: 1.0, average_color_similarity: None},
        ];

        let visual_text = remove_redundant_candidates(candidates.clone(), &frames, LoopMatchMode::VisualText);
        let visual_color = remove_redundant_candidates(candidates, &frames, LoopMatchMode::VisualTextAndColor);

        assert_eq!(visual_text.len(), 1);
        assert_eq!(visual_text[0].occurrences, vec![1, 5]);
        assert_eq!(visual_color.len(), 2);
    }
}
