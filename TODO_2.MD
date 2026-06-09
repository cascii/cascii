# Background Video Conversion Performance Investigation

## Scope

This investigation traced `id="video-background-toggle-btn"` in `cascii-studio`
through the Tauri backend and into `cascii`'s video conversion and per-cell
background fitting code.

The toggle selects `CellColorMode::FitForegroundBackground`. The delay is not
caused by the button itself. It is primarily caused by the background fitting
algorithm, compounded by conversion-range integration, debug builds, frame
materialization, and inefficient output I/O.

## Findings

### 1. The background fitting algorithm is catastrophically expensive

Relevant code:

- `src/render.rs:203`:
  `fit_image_to_ascii_with_cell_backgrounds_with_context`
- `src/render.rs:281`: loops over every candidate glyph for every cell
- `src/render.rs:312`: `fit_colors_for_glyph`

For every frame, the algorithm:

1. Divides the output into character cells.
2. Copies every pixel in each cell into a patch.
3. Tests nearly the entire ASCII ramp, currently 67 non-space candidates.
4. For each candidate, scans the patch three times to solve foreground and
   background RGB values.
5. Scans the patch again to calculate the reconstruction error.

For the observed 170x67-character conversion:

- 11,390 cells per frame
- 763,130 glyph fits per frame
- Approximately 870 million pixel/channel inner-loop iterations per frame
- Approximately 130 billion iterations for five seconds at 30 FPS

The cost grows approximately with the square of the column count because both
the number of columns and rows increase:

- Around 181 billion iterations for five seconds at 200 columns
- Around 724 billion iterations for five seconds at 400 columns

The implementation also uses `f64` throughout the hottest loops.

### 2. The selected video range is ignored by Convert

Relevant Studio code:

- `cascii-studio/src/components/video_player.rs:7`
- `cascii-studio/src/components/video_player.rs:1070`
- `cascii-studio/src-tauri/src/commands/conversion.rs:449`

The range sliders affect playback and the scissors action, but the
`ConvertToAsciiRequest` contains no start or end timestamps. The backend
explicitly constructs:

```rust
VideoOptions {
    start: None,
    end: None,
    // ...
}
```

Therefore, selecting five seconds with the range controls does not make Convert
process five seconds. Convert processes the entire underlying video unless the
user first creates and selects a physical cut.

The active conversion inspected during this investigation was processing a
288.76-second source, not five seconds. At 30 FPS it produced 8,663 PNG frames.

### 3. Development runs use an unoptimized backend

Relevant Studio code:

- `cascii-studio/run.sh:217`
- `cascii-studio/src-tauri/Cargo.toml`

`run.sh` starts `cargo tauri dev`. No custom development profile enables
optimization for `cascii`, so the numerical fitting loop runs with Rust's
default development optimization level, `opt-level=0`.

This kind of floating-point, iterator-heavy inner loop can be dramatically
slower in a debug build than in a release build.

### 4. Frames are downscaled and immediately enlarged again

Relevant code:

- `src/preprocessing.rs:65`
- `src/render.rs:216`

ffmpeg extracts each frame at the requested character-column width:

```text
scale=<columns>:-2
```

The background fitter then enlarges that small image to:

```text
character columns * glyph pixel width
character rows * glyph pixel height
```

The observed PNG was only 170x96. The fitter then enlarged it to approximately
1700x1273 using Lanczos before analysis.

This has several problems:

- Detail is discarded during ffmpeg extraction.
- The discarded detail is then synthetically recreated by interpolation.
- A costly Lanczos resize is performed for every frame.
- The enlarged image consumes significantly more memory and memory bandwidth.

### 5. `.cframe` output performs one tiny write per cell

Relevant code:

- `src/convert.rs:188`
- `src/convert.rs:200`

`write_cframe_binary` calls `File::write_all` with four bytes for every
character cell.

At 170x67, this is 11,390 tiny writes per frame. Studio also chooses
`TextAndColor`, so it writes a separate `.txt` file in addition to each
`.cframe`.

The frame should instead be assembled in memory or written through a
`BufWriter` in large blocks.

### 6. The pipeline materializes every PNG before conversion

Relevant code:

- `src/lib.rs:693`
- `src/lib.rs:697`
- `src/convert.rs:315`

The pipeline is:

1. Extract every video frame to a PNG file.
2. Scan and sort the directory.
3. Decode the PNG files again in parallel.
4. Convert them.
5. Delete them after all conversion work finishes.

There is no bounded producer/consumer pipeline between ffmpeg and conversion.

One full-source attempt inspected during this investigation created 103,087 PNG
files and consumed approximately 2.5 GB.

### 7. FPS conversion creates duplicate work

Relevant Studio code:

- `cascii-studio/src/pages/project.rs:627`

Studio defaults to 30 FPS. The inspected source was 25 FPS. ffmpeg's `fps=30`
filter therefore manufactures duplicate/interpolated output frames, each of
which is independently passed through the extremely expensive background
fitter.

### 8. There is no backend conversion admission control or cancellation

Relevant Studio code:

- `cascii-studio/src-tauri/src/commands/conversion.rs:408`
- `cascii-studio/src-tauri/src/commands/conversion.rs:409`

Each request launches another Tokio task and blocking conversion. There is no
global semaphore, queue, cancellation token, or job registry in the backend.

Multiple conversions can therefore compete for:

- The global Rayon pool
- CPU time
- Memory bandwidth
- PNG decode bandwidth
- Filesystem I/O

The frontend tracks active conversions for display, but that is not backend
resource control.

## Measured Evidence

For the active 288.76-second conversion:

- ffmpeg frame extraction completed in approximately 42 seconds.
- 8,663 PNG frames were present.
- Only approximately 440 `.cframe` files existed after roughly 16 minutes.
- Conversion throughput was around one frame per second during observation.
- The bottleneck was conclusively the Rust background fitting phase, not ffmpeg
  frame extraction.

The inspected generated frame had:

- PNG dimensions: 170x96
- ASCII dimensions: 170x67
- `.cframe` size: 79,739 bytes

## Dependency Note

`cascii-studio` currently declares:

```toml
cascii = "0.21.0"
```

It therefore uses the crates.io package rather than a path dependency on the
adjacent `./cascii` checkout. The installed crates.io source and the local
`cascii/src` tree were compared during the investigation and were identical at
that time.

## Prioritized Fixes

### P0: Honor the selected video range

Add start and end timestamps to `ConvertToAsciiRequest`, populate them from the
range controls, and pass them into `VideoOptions`.

This prevents a five-second selection from silently converting the full source
video.

### P0: Replace exhaustive glyph fitting

Do not fit foreground/background colors for every glyph candidate.

A more practical pipeline would be:

1. Select a small number of glyph candidates using luminance, coverage,
   gradients, or a precomputed low-dimensional glyph descriptor.
2. Fit foreground/background colors only for those candidates.
3. Compute exact reconstruction error only for the final shortlist.

Even selecting the normal luminance glyph first and solving colors once would
reduce the fitting work by roughly the size of the character ramp.

### P0: Optimize the backend during development

Use a development profile that optimizes `cascii` and other numerical
dependencies, for example package-specific optimization, or run the Tauri
backend in release mode when measuring conversion performance.

Performance measurements from the current unoptimized development binary are
not representative of production, but the algorithm remains too expensive even
with compiler optimization.

### P1: Remove the downscale/upscale cycle

Choose one representation:

- Extract frames directly at glyph-analysis pixel dimensions, or
- Analyze the low-resolution character grid without enlarging it to full glyph
  resolution.

Avoid Lanczos enlargement of every already-downscaled PNG.

### P1: Stream frames instead of materializing all PNGs

Use ffmpeg rawvideo output over a pipe and feed frames into a bounded conversion
queue. This would avoid:

- PNG encoding in ffmpeg
- Tens of thousands of temporary files
- Directory scanning and sorting
- PNG decoding in Rust
- Multi-gigabyte temporary directories

### P1: Buffer `.cframe` output

Build the complete body in a `Vec<u8>` or use `BufWriter`, then write the header,
cell body, flags, and background payload in a few large operations.

### P1: Avoid duplicate frame processing

Default to the source FPS or cap requested FPS to source FPS unless explicit
frame duplication is required.

For repeated or identical frames, consider hash-based reuse of the previous
converted frame.

### P1: Add backend job control

Add:

- A bounded conversion queue or semaphore
- Cancellation tokens
- A backend job registry
- Cleanup for cancelled partial output
- Explicit limits on concurrent Rayon-heavy conversions

### P2: Optimize numerical kernels

After fixing the algorithmic complexity:

- Use `f32` where sufficient.
- Fuse the foreground/background accumulation loops.
- Precompute additional glyph statistics.
- Store glyph data contiguously instead of performing a `HashMap` lookup in the
  per-cell candidate loop.
- Consider SIMD for candidate scoring.
- Avoid repeated bounds checks and `get_pixel` calls in hot loops.
- Benchmark candidate reduction and approximation quality.

### P2: Improve progress reporting

Report separate phases and durations:

- Extracting video frames
- Converting frames
- Writing outputs
- Cleaning temporary data

Also display the actual frame count and source time range before starting so an
unexpected full-video conversion is visible immediately.

## Recommended Order

1. Pass the selected range into the backend.
2. Run `cascii` optimized in development.
3. Replace exhaustive 67-glyph fitting with candidate reduction or one-glyph
   fitting.
4. Remove the low-resolution PNG to high-resolution Lanczos expansion.
5. Buffer `.cframe` writes.
6. Replace temporary PNG materialization with a bounded raw-frame pipeline.
7. Add job cancellation and concurrency limits.
8. Apply SIMD and lower-level kernel optimizations only after the algorithmic
   problems are fixed.
