# Performance investigation: `video-background-toggle-btn` is catastrophically slow

Converting ~5 seconds of video to frames takes tens of minutes when the
background-cells toggle is enabled. This document records the root cause and the
ranked inefficiencies. Investigation only — no code has been changed.

## Summary

The slowness is **entirely** in the code path the `video-background-toggle-btn`
enables. That toggle sets `fit_cell_backgrounds`/`generate_colors`, which makes
the Tauri command (`cascii-studio/src-tauri/src/commands/conversion.rs:417`)
select `CellColorMode::FitForegroundBackground` instead of the default
`ForegroundOnly`. Those two modes run completely different conversion functions,
and the background one is pathologically expensive.

- **ForegroundOnly** -> `convert::image_to_ascii_with_colors`
  (`src/convert.rs:130`): per cell it does ~1 luminance calc + 3 byte copies.
  No image resize. ~5 ops/cell.
- **FitForegroundBackground** ->
  `render::fit_image_to_ascii_with_cell_backgrounds_with_context`
  (`src/render.rs:203`): per cell it brute-force fits **every glyph in the
  charset** via a least-squares solver over a full glyph-sized pixel patch,
  *after* upscaling the whole frame with Lanczos3.

## The smoking gun: a useless ~190x Lanczos3 upscale

Frames are extracted by ffmpeg already downscaled to `columns` width
(`src/preprocessing.rs:66`, `scale={columns}:-2`). So each extracted PNG is
`columns` px wide.

Then in the convert step, `convert_directory_parallel_with_progress` calls the
converter with **`columns = None`** (hardcoded, `src/convert.rs:325`), so
`width_chars = orig_png_width = columns`. But the background path then does this
(`src/render.rs:216-221`):

```rust
let target_w = width_chars * atlas.cell_width;    // columns * ~10
let target_h = height_chars * atlas.cell_height;  // rows * ~19
img = dyn_img.resize_exact(target_w, target_h, FilterType::Lanczos3)...
```

It **upscales each frame by the glyph cell size (~10x19 ~= 190x the pixels)
using the most expensive resampling filter that exists**, every frame. For a
400-col 16:9 frame that's resizing 400x225 -> ~4000x2983 (~12M px) per frame,
single-threaded within the frame.

And it's wasted work: because `width_chars == orig_png_width`, each character
cell's patch is derived from roughly **one source pixel** smeared across 190
sub-pixels by Lanczos3. The expensive glyph-fitting then runs against this
invented, near-uniform detail. The algorithm was clearly designed to fit glyphs
against *real* high-resolution detail — but the source is never extracted at
that resolution, so the entire upscale-and-fit is solving for detail that
doesn't exist.

## The second cost: brute-force per-cell glyph fitting

For every non-empty cell (`src/render.rs:281-291`):

```rust
for &byte in &background_analysis.candidate_bytes {   // ~66 chars (default set)
    if let Some(glyph) = atlas.glyphs.get(&byte) {    // HashMap lookup, 66x/cell
        let (fg, bg, error) = fit_colors_for_glyph(&patch, glyph, avg_rgb);
        ...
    }
}
```

`fit_colors_for_glyph` (`src/render.rs:312`) scans the whole ~190-px patch ~3x
in **f64**. Rough cost per frame at 400 cols (~63K cells x 66 glyphs x ~1100
f64 ops) ~= **~19 billion f64 ops per frame**, on top of the resize. A 5s clip
at 30fps ~= 150 frames -> low-trillions of f64 ops. Rayon parallelizes only
*across* frames (`src/convert.rs:322`), not within, and the work scales with
**columns^2** (the UI allows up to 2000,
`cascii-studio/src/components/video_player.rs:1607`).

## Ranked inefficiencies (all investigation-only)

1. **The Lanczos3 upscale (`src/render.rs:218-221`) is the biggest and most
   pointless cost.** It's ~190x oversized and uses the costliest filter, to
   manufacture detail the solver can't meaningfully use. Either skip it, use a
   cheap filter (Nearest/Triangle), or — to fit glyphs properly — extract frames
   at `columns*cell_w` from ffmpeg instead of upscaling here.

2. **`HashMap<u8, GlyphBitmap>` lookup in the hottest loop**
   (`src/render.rs:282`). ~4M hash lookups/frame. Glyphs are keyed by contiguous
   bytes 32..=126 — a flat `Vec`/array indexed by byte, or pre-collecting
   `candidate_bytes` into a `Vec<&GlyphBitmap>` once, removes this entirely.

3. **All-f64 arithmetic on 8-bit color** (`src/render.rs:312-346`), with the
   `Vec<Rgb<u8>>` patch re-read 3x per candidate. f32 or integer fixed-point
   roughly halves the work and helps SIMD/cache.

4. **No pruning in the glyph search.** Every cell always evaluates all 66 glyphs
   even when the patch is near-uniform. The foreground path already maps
   luminance->density via `char_for` (`src/convert.rs:289`); the same idea could
   pick (or seed) the glyph directly instead of brute force.

5. **No intra-frame parallelism.** The `for row` loop (`src/render.rs:229`) is
   serial. For short clips with few-but-huge frames, cores idle. Could
   `par_iter` over rows.

6. **Compounding on the encode side:** `render_ascii_frame_to_rgb`
   (`src/render.rs:115`) also fills each cell's full bg rectangle then blends the
   glyph per-pixel — another O(cells x cell_pixels) pass per frame when
   re-encoding to video. Not the conversion bottleneck, but it adds to total
   wall time for the same toggle.

## Bottom line

Flipping that toggle swaps an O(cells) byte-copy for an
O(cells x charset x cell_pixels) f64 optimization *plus* an
O(cells x cell_pixels) Lanczos3 upscale — and the upscale (#1) is both the
largest cost and provably wasted given `columns=None` makes each cell ~= one
source pixel. Fixing #1 and #2 alone should turn tens of minutes into seconds.
