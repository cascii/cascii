# DEDUP.MD — Plan: frame deduplication during video conversion

Scope: the **library-side** optimization in the `cascii` crate. When extracted
PNG frames are identical, convert one representative and **copy** its output to
the duplicates instead of recomputing — covering both **adjacent** duplicates
(the common case) and **non-adjacent / looped** duplicates (the edge case).

Status: **planning only — no code changes yet.**

---

## 1. Motivation

Each frame's output is a **pure function of the PNG bytes + conversion params**:
`convert_image_to_ascii_with_analysis` (`src/convert.rs:85`) reads a PNG and
writes `{stem}.txt` and/or `{stem}.cframe` (`src/convert.rs:87-101`) with no
dependence on frame index or neighbours. Therefore, if two PNGs are byte-
identical, their outputs are **guaranteed** byte-identical — copying one onto the
other is exactly equivalent to recomputing, with zero quality risk.

The expensive work is per-frame and large (the `FitForegroundBackground*` glyph-
fit path is billions of f64 ops per frame — see `TODO_1.MD`). Skipping duplicate
frames multiplies directly into that cost.

### Where duplicate frames come from

Frames are extracted with `scale={columns}:-2,fps={fps}`
(`src/preprocessing.rs:66`). The `fps` filter is a frame-rate conversion: when
the target fps exceeds the source's true rate (or the source holds frames),
ffmpeg emits **byte-identical** PNGs. Common for animation, screen recordings,
slideshows, memes, title cards. Plus:

- **Looped / boomerang content:** the same frames recur **non-adjacently**.
  A 30-frame clip looped 10× (300 frames) collapses to 30 conversions + 270
  copies — a 10× saving.

For high-motion live action, few duplicates are found — but detection is so
cheap that this is **pure upside, never a regression**.

---

## 2. Design: global content-hash grouping (loop-aware by construction)

Group frames by a hash of their **content**, not their position. All occurrences
of an identical frame — adjacent or 500 frames apart — land in the same bucket
and share a single computation. This makes the looped-video case fall out for
free; **no loop-specific logic is needed.**

> A consecutive-only ("compare to previous frame") variant was considered and
> **rejected**: it catches fps-filter duplicates but misses non-adjacent loop
> repeats, which is exactly the edge case we must cover.

### Algorithm (inserted into the directory-conversion funnels)

```
1. Collect + sort the PNG paths (already done at convert.rs:399-400 / :459-460).
2. HASH PASS (parallel): for each PNG, hash its bytes -> HashMap<u64, Vec<idx>>.
3. For each bucket, pick a deterministic REPRESENTATIVE (first idx by sorted
   order). Representatives = one per unique content; the rest are DUPLICATES.
4. CONVERT only representatives (existing par_iter, restricted to that set).
5. COPY pass (parallel): for each duplicate, fs::copy the representative's
   output file(s) to the duplicate's stem.
6. Progress still counts ALL N frames so the percentage stays accurate.
7. keep_images / cleanup behaves exactly as today (delete PNGs unless kept).
```

### Two-tier hashing (default safe; perceptual optional)

**Tier 1 — byte-exact (default).** Hash the scaled PNG file bytes with a fast
64-bit non-crypto hash. Catches:
- fps-filter duplicates (ffmpeg copies the frame → always identical), and
- true lossless / source-level loops (GIFs, PNG sequences, concatenated clips).
Zero quality risk. Optional `memcmp` fallback on hash collision makes it
bulletproof (collisions on small inputs are already astronomically unlikely).

**Tier 2 — perceptual (optional, opt-in, future).** A video that is *visually*
looped but **lossily re-encoded** (H.264/HEVC) can have decoded pixels that
drift slightly between loop iterations (GOP boundaries, motion comp,
quantization), so byte-exact misses it. Mitigation already in our favour:
frames are **downscaled to `columns` width** before conversion, and Lanczos
averaging suppresses most codec noise — so post-scale, near-identical loop
frames become byte-identical far more often than at full resolution.

To *guarantee* catching lossy loops, add a perceptual tier behind a
flag/threshold: hash a small quantized luminance thumbnail with a tolerance to
coalesce frames identical-to-the-eye but not to the byte. This trades a little
quality risk, so it must be **opt-in, never the default**.

**The hashing seam must be designed so Tier 2 can slot in later** (a
`FrameFingerprint` abstraction — see §4) **without restructuring** the dedup
flow.

---

## 3. Prior art already in the codebase

`src/loop_detect.rs` already uses this exact technique — hashing frames into
`HashMap<u64, Vec<usize>>` and finding repeated content
(`src/loop_detect.rs:32-45`). Differences:

- It runs **post-conversion**, hashing output `.txt` files — i.e. *after* the
  expensive work. Our dedup hashes **input PNGs** to *avoid* that work.
- It deliberately **excludes immediate neighbours** (`:64`, `fn_end > fn_start + 1`)
  because it hunts for *loops*; our dedup wants *all* duplicates including
  neighbours.

Not reusable as-is, but it proves the pattern is trusted here. Factor the 64-bit
`DefaultHasher` grouping into a shared helper (e.g. `frame_hash::hash_bytes`)
that both `loop_detect` and the new dedup pass can use.

---

## 4. Touch points (exact anchors)

All directory conversion funnels through two functions; the dedup pass lives in
both (or in a shared helper they call):

| File | Anchor | Change |
|------|--------|--------|
| `src/convert.rs` | `:394` `convert_directory_parallel_with_progress_at_columns` | insert hash pass → representatives/duplicates; restrict the `par_iter` (`:406`) to representatives; add copy pass before the `keep_images` block (`:420`) |
| `src/convert.rs` | `:454` `convert_directory_parallel_with_detailed_progress_at_columns` | same, around the `par_iter` at `:470` and cleanup at the tail |
| `src/convert.rs` | new | `fn outputs_for_stem(out_dir, stem, output_mode) -> Vec<PathBuf>` — the `.txt`/`.cframe` set a frame produces, derived from `OutputMode` (TextOnly→`.txt`; ColorOnly→`.cframe`; TextAndColor→both); used by the copy pass |
| `src/convert.rs` | new | `fn dedup_buckets(pngs: &[PathBuf]) -> (Vec<usize> representatives, Vec<(usize dup, usize rep)>)` — the shared hash/grouping logic |
| new `src/frame_hash.rs` (or in `convert.rs`) | — | `FrameFingerprint` enum/struct + `hash_bytes(&[u8]) -> u64`; Tier-2 perceptual variant added here later |
| `src/loop_detect.rs` | `:32-45` | (optional) switch to the shared hashing helper |

Both funnels are reached by the standard path, the `*_optimized*` path
(`:376`/`:436`), and `convert_directory` (`:366`), so all conversion modes
benefit with one implementation.

### Output-copy detail

For a duplicate at index `d` with representative `r`:
```
for ext in outputs_for_stem(dst_dir, stem(r), output_mode):
    fs::copy(ext_for(r), ext_for(d))   // e.g. frame_0041.txt -> frame_0042.txt
```
Representative output must be fully written before its duplicates are copied —
i.e. **copy pass runs after the convert pass completes**, not interleaved.

### Avoiding a double read (minor, optional)

The hash pass reads PNG bytes that the convert pass reads again. The second read
is page-cache-warm, so the cost is negligible; keep it simple initially. If
desired later, read bytes once and decode representatives via
`image::load_from_memory`.

### Progress accuracy

`total` must remain the full frame count. Increment the progress counter for
**both** converted representatives and copied duplicates so the bar reaches 100%
and reflects real wall-clock completion. (Copies are near-instant, so the bar
will accelerate through duplicate-heavy stretches — expected and fine.)

---

## 5. Interaction with other planned work

- **Cancellation (`CANCEL.MD`):** the cancel check goes on the **representative
  convert loop** (the existing `try_for_each`) and should also short-circuit the
  copy pass. The two features are orthogonal and compose cleanly.
- **`keep_images`:** unchanged — the existing delete-unless-kept block
  (`src/convert.rs:420`) still removes all PNGs (representatives and duplicates)
  after conversion.
- **Studio:** no API change required if dedup is internal to the funnels;
  conversion just gets faster. Optionally surface "converted X unique of N
  frames" in progress later.

---

## 6. Correctness & edge cases

- **Determinism:** byte-identical PNG ⇒ byte-identical output. Holds because
  conversion is a pure function of content + params. ✓
- **Representative choice** must be deterministic (lowest sorted index) so reruns
  are stable and copies are reproducible.
- **Single-frame buckets** (no duplicates) convert exactly as today.
- **All-identical video** (e.g. a static screen) → 1 conversion + (N-1) copies.
- **Hash collision** → optional `memcmp` fallback; otherwise 64-bit collision on
  small frame data is negligible.
- **Empty / unreadable PNG** → falls back to converting it normally (don't let a
  hash failure abort the batch; treat as its own unique bucket).
- **Lossy-re-encoded loops** → may not byte-match (see §2 Tier 2); default
  behaviour is "miss it and convert both," which is correct, just less optimal.

---

## 7. Testing plan

- **Unit (grouping):** synthetic PNG set with known duplicates (adjacent and
  non-adjacent) → assert `dedup_buckets` returns the expected representative and
  duplicate→rep mapping.
- **Unit (output equivalence):** convert a small dir with dedup ON vs OFF →
  assert **byte-identical** output files for every frame. This is the key
  guarantee: dedup must never change results.
- **Loop case:** build N PNGs as K unique frames repeated (e.g. ABCABCABC) →
  assert only K conversions happened (instrument the convert count) and all N
  outputs are present and correct.
- **All-identical:** N copies of one PNG → exactly 1 conversion, N outputs.
- **No duplicates:** all-unique frames → identical results and frame count to
  today (no regression), dedup overhead only the hash pass.
- **Progress:** assert the counter reaches `total` and is monotonic across the
  convert + copy passes.
- **Modes:** run for TextOnly / ColorOnly / TextAndColor and the optimized
  path → correct output-file set copied in each.

---

## 8. Implementation order (smallest shippable increments)

1. **Shared hash helper** (`hash_bytes`, `FrameFingerprint` skeleton) +
   `outputs_for_stem`. No behaviour change yet.
2. **Tier-1 byte-exact dedup** in the progress funnel (`:394`): hash pass →
   representatives → restricted `par_iter` → copy pass. Gate behind an internal
   flag if cautious; default on once the equivalence test passes.
3. Apply the same to the **detailed-progress funnel** (`:454`).
4. Point `loop_detect.rs` at the shared helper (optional cleanup).
5. **Tier-2 perceptual** fingerprint behind a flag/threshold (future, opt-in).

Steps 1–3 deliver the full requirement (adjacent **and** non-adjacent/looped
byte-exact dedup). Step 5 is the optional robustness upgrade for lossy loops.
