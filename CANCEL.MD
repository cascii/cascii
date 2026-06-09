# CANCEL.MD — Plan: cancellation support in the `cascii` library

Scope: the **library-side** changes needed so cascii-studio's interrupt button
can actually stop an in-flight conversion. The studio/Tauri wiring and the
button itself are planned separately in
`../cascii-studio/INTERRUPT_BUTTON.MD`; this file covers only the `cascii` crate.

Status: **planning only — no code changes yet.**

---

## 1. Why the library must participate

The studio runs conversions inside `tokio::task::spawn_blocking`, which **cannot
be force-aborted** — the blocking thread runs to completion regardless of what
the async side does. The only way to actually stop the work is **cooperative
cancellation**: the library code must periodically check a shared flag and bail,
and any `ffmpeg` child process must be **killed** explicitly. None of that
exists in the crate today.

Two seams make this cheap:

1. **The conversion loop already short-circuits on `Err`.** Both directory
   converters funnel into `par_iter().try_for_each(...)`:
   - `convert_directory_parallel_with_progress_at_columns` — `src/convert.rs:406`
   - `convert_directory_parallel_with_detailed_progress_at_columns` — `src/convert.rs:470`
   Returning `Err` from the closure makes rayon stop scheduling new frames. A
   one-line cancel check at the top of each closure stops the dominant phase.

2. **`ffmpeg` runs as a child process.** Extraction/encode spawn ffmpeg
   (`src/video.rs:154`, `:185`, `.status()` at `:52`/`:253`). Switching the
   blocking waits to a spawn + poll + `kill()` loop lets us interrupt the
   extraction phase too.

---

## 2. Chosen mechanism: `CancelToken` carried on `AsciiConverter`

Design goal: **add cancellation without changing the signature of any existing
public method**, so the CLI (`src/main.rs`) and the studio keep compiling and
opt in only where they want it.

`AsciiConverter` (`src/lib.rs:539`) is already a builder-style struct
(`new()`, `with_ffmpeg_config()`). Add an optional token field and a builder:

```rust
// src/lib.rs
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cheap, clonable cancellation flag. Cloning shares the same underlying flag.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self { Self::default() }
    pub fn cancel(&self) { self.0.store(true, Ordering::Relaxed); }
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::Relaxed) }
}

pub struct AsciiConverter {
    config: AppConfig,
    ffmpeg_config: FfmpegConfig,
    cancel_token: Option<CancelToken>,   // NEW
}

impl AsciiConverter {
    pub fn with_cancel_token(mut self, token: CancelToken) -> Self {
        self.cancel_token = Some(token);
        self
    }
}
```

The three struct literals that build `AsciiConverter` must initialise the new
field to `None`:
- `src/lib.rs:547` (`new`)
- `src/lib.rs:556` (`with_config` / from-config)
- `src/lib.rs:589` (the other from-config constructor)

Because the token lives on `self`, every conversion method already has access to
it via `self.cancel_token.as_ref()` and passes it down into the (crate-private)
free functions. **No public method signature changes.** The studio just does:

```rust
let token = cascii::CancelToken::new();          // store clone in the job registry
let converter = AsciiConverter::new()
    .with_ffmpeg_config(cfg)
    .with_cancel_token(token.clone());
```

### Distinguishing a cancel from a real failure

Conversions return `anyhow::Result`. A cancel should be recognisable so the
studio can suppress the "conversion failed" UX and instead clean up quietly.
Two options:

- **(A) Sentinel error** — return `anyhow!("cascii: cancelled")` and expose a
  helper `pub fn is_cancelled_error(e: &anyhow::Error) -> bool` that matches it.
  Lowest churn. Slightly stringly-typed.
- **(B) Typed error** — introduce `pub enum CancelError { Cancelled }` or a
  marker error type downcastable via `err.downcast_ref::<Cancelled>()`. Cleaner;
  the studio matches on `downcast_ref`.

Recommend **(B)** with a zero-size `Cancelled` error type so the studio can do
`if err.downcast_ref::<cascii::Cancelled>().is_some()`. Either is fine.

---

## 3. Threading the token down (exact touch points)

### 3a. `src/convert.rs` — the conversion loop (highest value)

The two `_at_columns` funnels are where all directory conversion flows through.
Add a `cancel: Option<&CancelToken>` parameter to:

- `convert_directory_parallel_with_progress_at_columns` (`:394`)
- `convert_directory_parallel_with_detailed_progress_at_columns` (`:454`)

…and at the top of each `try_for_each` closure (`:406`, `:470`):

```rust
if cancel.map_or(false, |c| c.is_cancelled()) {
    return Err(Cancelled.into());   // rayon stops scheduling further frames
}
```

Propagate the new parameter through the thin wrappers that call these
(`convert_directory_parallel_with_progress` `:371`,
`convert_directory_parallel_optimized_with_progress` `:376`, and the two
detailed equivalents `:431`/`:436`). `convert_directory_parallel` (`:366`,
no progress) passes `None`.

Note: in-flight frames already executing when the flag flips will finish
(milliseconds each) — acceptable. Cancellation is "stop soon," not "stop this
instruction."

The optimized path (`FitForegroundBackgroundOptimized`) shares the same
`_at_columns` funnel, so it gets the check for free.

### 3b. `src/video.rs` — kill ffmpeg mid-extraction

Replace the blocking waits with a cancellable poll. Add `cancel: Option<&CancelToken>`
to `extract_video_frames` (`:9`), `extract_video_frames_with_progress` (`:75`),
`extract_audio` (`:136`), and `extract_video_frames_optimized` (called from
`lib.rs:726`). Pattern:

```rust
let mut child = cmd.spawn().context("spawning ffmpeg")?;
loop {
    if let Some(status) = child.try_wait()? {
        if !status.success() { return Err(anyhow!("ffmpeg failed")); }
        break;
    }
    if cancel.map_or(false, |c| c.is_cancelled()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Cancelled.into());
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
}
```

~100 ms cancel latency during extraction — fine. (The `.status()` call sites at
`:52`/`:253` are the non-progress variants; convert them the same way.)

### 3c. `src/lib.rs` — pass `self.cancel_token.as_ref()` through

In each conversion method, forward the token into the calls above:

- `convert_video_with_progress` (`:719`) → `video::extract_*` and
  `convert::convert_directory_parallel_*` and the optimized variants
  (`:726`, `:728`, `:733`, `:740`).
- `convert_video_with_detailed_progress` (`:819`) → same (`:834` etc.).
- `convert_video_to_video_inner` (`:970`) → extraction (`:978`), audio
  (`:983`), **and the encode loop** (see 3d).
- `convert_directory` / `convert_directory_with_progress` (`:886`/`:937`) →
  the directory funnels.
- `convert_image` (`:622`) is effectively instantaneous — leave as-is (no token
  needed), or add a single pre-flight check for symmetry.

### 3d. `src/lib.rs` — the encode loop in `convert_video_to_video_inner`

This path (`:970`) renders ASCII frames and pipes raw RGB to an `ffmpeg`
encoder over a channel, across a producer thread + consumer. Two checkpoints:

- In the frame-render loop (the `par`/producer side that walks `png_paths`),
  check the token per frame and stop producing.
- When stopping, drop the encoder stdin / break the consumer so the child
  ffmpeg encoder sees EOF, then `kill()` it if still alive.

This path is CLI-only today (`main.rs:514`), so it is lower priority than 3a/3b
but should be included for completeness since it can run for minutes.

---

## 4. Cleanup-on-cancel: whose job?

When a conversion is cancelled it leaves **partial artifacts**: some `.txt`/
`.cframe` files written, the extracted PNGs still on disk (the
`if !keep_images { remove }` block at `src/convert.rs:420` is skipped because we
returned `Err` before reaching it), and for `to_video` a partial output file.

Decision: **the library should NOT silently delete the caller's output
directory** — it doesn't own it and the studio may want to inspect partials.
Recommended contract:

- The library guarantees only that it **stops work and kills its child
  processes**, returning the `Cancelled` error.
- The library still removes its **own** temp scratch (e.g.
  `convert_video_to_video` already does `fs::remove_dir_all(&temp_dir)` at
  `:965` on every exit path — that covers the to_video temp dir).
- For `convert_video*` the output dir is caller-owned, so **the studio** is
  responsible for deleting the partial `output_dir` and skipping the DB insert
  on cancel (documented in `INTERRUPT_BUTTON.MD`).

This keeps the library's responsibility crisp and avoids destructive surprises.

---

## 5. CLI integration (optional, nice-to-have)

`src/main.rs` could wire `Ctrl-C` to a `CancelToken` for graceful interruption
instead of a hard process kill:

- Add a `ctrlc` dependency (or a manual `signal` handler), create a
  `CancelToken`, hand a clone to the signal handler that calls `token.cancel()`,
  and build the converter with `.with_cancel_token(token)`.
- On a `Cancelled` result, print "Interrupted" and exit non-zero without the
  scary anyhow backtrace.

Not required for the studio feature; can be a follow-up. No new dependency is
needed for the core library work — `CancelToken` is std-only.

---

## 6. Files & anchors summary

| File | Anchor | Change |
|------|--------|--------|
| `src/lib.rs` | `:539`, `:547/:556/:589` | `CancelToken` + `Cancelled` types; `cancel_token` field; `with_cancel_token`; init field in 3 constructors |
| `src/lib.rs` | `:719`, `:819`, `:886`, `:937`, `:970` | forward `self.cancel_token.as_ref()` into convert/video calls + encode loop |
| `src/convert.rs` | `:394`, `:406`, `:454`, `:470` | add `cancel` param to the two `_at_columns` funnels; cancel check in both `try_for_each` closures; thread through wrappers `:366/:371/:376/:431/:436` |
| `src/video.rs` | `:9`, `:52`, `:75`, `:136`, `:154`, `:185`, `:253` | add `cancel` param; spawn+poll+kill instead of blocking wait |
| `src/main.rs` | `:514/:577/:674` | (optional) Ctrl-C → token |

No public method signatures change; only crate-private free-function signatures
in `convert.rs`/`video.rs` gain a `cancel` parameter.

---

## 7. Testing plan

- **Unit (convert):** build a temp dir of N PNGs, run
  `convert_directory_parallel_with_progress` with a token pre-set to cancelled;
  assert it returns the `Cancelled` error and wrote fewer than N outputs.
- **Unit (mid-flight):** flip the token from the progress callback after the
  first frame; assert early stop and `Cancelled`.
- **Integration (video):** start a `convert_video_with_progress` on a short
  clip, cancel from another thread during extraction; assert the ffmpeg child is
  gone (no orphan) and the call returns `Cancelled` within ~a few hundred ms.
- **Regression:** with `cancel_token = None` (default), all existing behaviour
  and existing tests pass unchanged.
- **Error classification:** assert `is_cancelled_error`/`downcast_ref::<Cancelled>`
  distinguishes a cancel from a genuine ffmpeg failure.

---

## 8. Implementation order (smallest shippable increments)

1. Add `CancelToken` + `Cancelled` + the `AsciiConverter` field/builder
   (compiles, no behaviour change; token unused).
2. Thread `cancel` into the two `convert.rs` funnels + the `try_for_each`
   checks. — This alone makes the **conversion phase** (the multi-minute cost)
   interruptible; ships real value.
3. Convert `video.rs` ffmpeg waits to spawn+poll+kill. — Makes the
   **extraction phase** interruptible.
4. `convert_video_to_video` encode-loop checkpoints (CLI path).
5. Optional CLI Ctrl-C wiring + tests.

Steps 1–2 are the minimum that lets the studio's interrupt button stop the
dominant workload; 3 closes the extraction gap; 4–5 are completeness.
