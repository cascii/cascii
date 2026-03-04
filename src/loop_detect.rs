use anyhow::{anyhow, Context, Result};
use dialoguer::Select;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use walkdir::WalkDir;

pub fn run_find_loop(dir: &Path) -> Result<()> {
    // Load frames in order
    let mut frames: Vec<(usize, String)> = Vec::new();
    let mut entries: Vec<std::path::PathBuf> = WalkDir::new(dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|p| p.extension().map(|e| e == "txt").unwrap_or(false))
        .collect();
    entries.sort();

    for p in entries {
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if !name.starts_with("frame_") {
            continue;
        }
        // parse frame number
        let num = name
            .trim_start_matches("frame_")
            .trim_end_matches(".txt")
            .parse::<usize>()
            .unwrap_or(frames.len());
        let content = fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        frames.push((num, content));
    }
    if frames.is_empty() {
        return Err(anyhow!("No frame_*.txt files found in {}", dir.display()));
    }
    frames.sort_by_key(|(n, _)| *n);

    // Hash frames and map to indices
    use std::collections::hash_map::DefaultHasher;
    let mut hash_to_indices: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut repeated_hashes: Vec<u64> = Vec::new();

    for (idx, (_, content)) in frames.iter().enumerate() {
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        let h = hasher.finish();
        let entry = hash_to_indices.entry(h).or_default();
        entry.push(idx);
        if entry.len() == 2 {
            // first time we see a repeat
            repeated_hashes.push(h);
        }
    }

    if repeated_hashes.is_empty() {
        println!("No repeated frames detected.");
        return Ok(());
    }

    // Build candidate loops: for each repeated hash, all non-adjacent pairs between occurrences
    // Ignore immediate number neighbors (e.g., frame N and frame N+1)
    let mut loops: Vec<(usize, usize)> = Vec::new();
    for h in &repeated_hashes {
        if let Some(indices) = hash_to_indices.get(h) {
            let n = indices.len();
            for a in 0..n.saturating_sub(1) {
                for b in (a + 1)..n {
                    let s = indices[a];
                    let e = indices[b];
                    let fn_start = frames[s].0;
                    let fn_end = frames[e].0;
                    if fn_end > fn_start + 1 {
                        // exclude immediate neighbors
                        loops.push((s, e));
                    }
                }
            }
        }
    }
    // Deduplicate loops
    loops.sort();
    loops.dedup();

    if loops.is_empty() {
        println!("No loopable segments detected.");
        return Ok(());
    }

    println!("Found loops:");
    for (i, (s, e)) in loops.iter().enumerate() {
        println!(
            "{}: frames {}..{} (inclusive start, exclusive end)",
            i + 1,
            frames[*s].0,
            frames[*e].0
        );
    }

    // Interactive menu
    loop {
        let choices = vec!["Export loop", "Repeat loop", "Quit"];
        let sel = Select::new()
            .with_prompt("Choose an action")
            .default(0)
            .items(&choices)
            .interact()?;
        match sel {
            0 => {
                // Export
                let labels: Vec<String> = loops
                    .iter()
                    .map(|(s, e)| format!("{}..{}", frames[*s].0, frames[*e].0))
                    .collect();
                let idx = Select::new()
                    .with_prompt("Select loop to export")
                    .default(0)
                    .items(&labels)
                    .interact()?;
                let (s, e) = loops[idx];
                export_loop(dir, &frames, s, e)?;
                println!("Exported loop {}..{}", frames[s].0, frames[e].0);
            }
            1 => {
                // Repeat
                let labels: Vec<String> = loops
                    .iter()
                    .map(|(s, e)| format!("{}..{}", frames[*s].0, frames[*e].0))
                    .collect();
                let idx = Select::new()
                    .with_prompt("Select loop to repeat")
                    .default(0)
                    .items(&labels)
                    .interact()?;
                let (s, e) = loops[idx];
                repeat_loop(dir, &frames, s, e)?;
                println!("Loop repeated");
            }
            _ => break,
        }
    }

    Ok(())
}

fn export_loop(dir: &Path, frames: &[(usize, String)], start_idx: usize, end_idx: usize) -> Result<()> {
    let start_frame = frames[start_idx].0;
    let end_frame = frames[end_idx].0;
    let out = dir.with_file_name(format!(
        "{}_loop_{}_{}",
        dir.file_name().and_then(|s| s.to_str()).unwrap_or("frames"),
        start_frame,
        end_frame
    ));
    fs::create_dir_all(&out)?;
    let mut counter: usize = 1;
    for frame in frames.iter().take(end_idx + 1).skip(start_idx) {
        // inclusive both ends as per example ABCD A
        let filename = out.join(format!("frame_{:04}.txt", counter));
        fs::write(filename, &frame.1)?;
        counter += 1;
    }
    Ok(())
}

fn repeat_loop(dir: &Path, frames: &[(usize, String)], start_idx: usize, end_idx: usize) -> Result<()> {
    // Reinsert the selected loop immediately after the end index
    // We will renumber and rewrite all frames to the same directory
    let mut new_seq: Vec<String> = Vec::with_capacity(frames.len() + (end_idx - start_idx + 1));
    for (_, content) in frames.iter().take(end_idx + 1) {
        new_seq.push(content.clone());
    }
    for frame in frames.iter().take(end_idx + 1).skip(start_idx) {
        new_seq.push(frame.1.clone());
    }
    for (_, content) in frames.iter().skip(end_idx + 1) {
        new_seq.push(content.clone());
    }

    // Write back with new numbering
    // First, remove existing frame_*.txt
    for entry in WalkDir::new(dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = entry.path().to_path_buf();
        if p.is_file() {
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if name.starts_with("frame_") && name.ends_with(".txt") {
                    let _ = fs::remove_file(p);
                }
            }
        }
    }
    for (i, content) in new_seq.iter().enumerate() {
        let filename = dir.join(format!("frame_{:04}.txt", i + 1));
        fs::write(filename, content)?;
    }
    Ok(())
}
