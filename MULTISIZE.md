# Multiple sizes and live playback

One asset represents one timeline at several column counts. Every variant has the
same frame IDs, FPS and duration. Rows are recorded per frame so image sequences
with different aspect ratios remain representable.

## Generate

The existing single-size command keeps its flat frame layout:

~~~sh
cascii input.mp4 --columns 73 --out ./output --default
~~~

Lists and inclusive ranges create a multi-size asset:

~~~sh
cascii input.mp4 --columns 55,58,61,67,73,80 --colors --out ./output --default
cascii input.mp4 --columns 55-73 --colors --out ./output --default
cascii image.png --columns 73 --size-format all --out ./output --default
cascii ./images --columns 55,73 --size-format directory,json,binary,csv --out ./output --default
~~~

As with the existing CLI, a file input appends its stem to the output directory:
the video commands above write to `output/input/`. Directory inputs write directly
to the requested output. The legacy positional output argument is also accepted.

The default multi-size exports are directories plus `frames.bin`. `--size-format`
accepts `directory`, `binary`, `json`, `csv`, `all`, or a comma-separated selection.
It also enables asset output for a single width. Color flags retain their existing
meaning: no flags is text only, `--colors` writes text and color, and `--color-only`
writes color only. JSON and binary preserve optional per-cell backgrounds.

Sizes are sorted and deduplicated. Ranges include both endpoints, with a step of
one. Limits are 1–4096 columns, at most 256 variants, 1–1000 FPS and 1,000,000
logical frames. Oversized grids and background-fitting workloads are rejected
before allocating their buffers. Binary manifests are limited to 16 MiB.

Multi-size conversion decodes/preprocesses each source frame once, converts its
variants in parallel, and processes one logical frame at a time. Video sampling
and optional audio extraction happen once. Original inputs are retained. Only
owned temporary video frames are removed; `--keep-images` retains those under
`images/`. Progress counts frame variants, while `frame_count` counts logical
frames. Outputs must be absent or empty; a sibling staging directory is published
only after every requested export succeeds. Failures and cancellation remove the
staging directory.

Multi-size `--to-video` is rejected: render a selected `variants/<columns>`
directory through the existing single-size video renderer.

## Library API

~~~rust
use cascii::{AsciiConverter, MultiSizeOptions, OutputMode};
use std::path::Path;

let mut options = MultiSizeOptions::new("55,58,61,67,73,80".parse()?);
options.conversion.output_mode = OutputMode::TextAndColor;
options.video.fps = 30;
let manifest = AsciiConverter::new().convert_multi_size(
    Path::new("input.mp4"), Path::new("output"), &options,
    |completed, total| println!("{completed}/{total} frame variants"),
)?;
~~~

`options.sizes` is authoritative; the legacy scalar columns fields in its
conversion/video settings are not used by this new method. Existing conversion
entry points remain available. Directory conversion now honors its scalar column
setting, and separate input directories are retained even with `keep_images=false`.

The portable core (`default-features=false`) exposes `ColumnSizes`,
`image_bytes_to_frames`, and the asset codecs without filesystem or subprocess
dependencies. The in-memory image API retains its foreground-only limitation;
both native background fitters are supported by `convert_multi_size`.

## Files and metadata

~~~text
output/
  manifest.json
  frames.bin                 # when binary requested
  frames.json                # when JSON requested
  frames.csv                 # when CSV requested
  audio.mp3                  # once, when requested from video
  variants/                  # when directory requested
    55/
      details.toml
      frame_000000.txt
      frame_000000.cframe
    73/
      ...
~~~

The manifest uses `format="cascii-multisize"` and `schema_version=1`, and stores
shared timing and conversion settings, the default width, output locations,
optional audio location and ordered variants. Each variant has `columns` and a
`rows` array indexed by the shared, zero-based frame ID. The default width is the
largest requested size. Audio remains a shared sidecar, including for binary and
JSON exports.

JSON is `{manifest, variants: [{columns, frames: [{text, cframe}]}]}`. `cframe` is
either null or an array of complete .cframe bytes. Existing single-size JSON
exports from other applications are not changed.

CSV is paired with manifest.json, one row per cell in frame-major, size-major,
row-major order:

~~~text
frame,columns,row,column,char_code,red,green,blue,bg_red,bg_green,bg_blue
~~~

Glyphs use numeric ASCII codes, so commas, quotes and spaces are lossless.
Foreground fields are empty for text-only assets. Background fields are either
all empty for a whole frame or populated for every cell. `read_csv` reconstructs
the JSON model and rejects missing, repeated, out-of-order or malformed cells.

## Binary format: CSMV version 1

All integers are little-endian. Existing .cframe, CFPK and legacy packed formats
remain unchanged. CSMV has its own magic and must not be passed to those decoders.

| Header offset | Type | Meaning |
|---|---|---|
| 0 | 4 bytes | ASCII CSMV |
| 4 | u32 | Version, 1 |
| 8 | u32 | UTF-8 manifest byte length |
| 12 | u32 | Index entry count |
| 16 | bytes | Manifest JSON |
| after manifest | entries | 24 bytes per entry |
| after index | bytes | Frame payloads |

Each index entry is `columns:u32`, `frame:u32`, `offset:u64`, `length:u32`,
`kind:u32`. Kind 0 is UTF-8 frame text; kind 1 is a complete .cframe payload,
including any background extension. Offsets are relative to the payload section.

There is exactly one entry for each (frame, size), ordered by frame then
ascending columns. Payloads are contiguous in the same order. Offsets, lengths,
ordering, frame counts, dimensions, glyphs and extensions are validated. This
layout permits a single contiguous byte-range request for a short time window
across every size. Payloads are currently uncompressed. Archives can be inspected
through `MultiSizeArchive` without changing the legacy binary APIs.

## Playback

The TypeScript core-view implementation provides `AdaptiveFramePlayer` with binary,
directory, JSON and CSV sources. Binary sources support in-memory bytes and HTTP
Range requests, with a full-response fallback for servers without Range support.
The player preserves one controller when changing sizes; FPS, position, loop range
and pause/play state survive selection. Latest asynchronous selection wins.

Decoded frames are bounded by both frame count and a byte budget (defaults:
256 frames, 32 MiB). A short window at all sizes is prefetched for immediate
switching. A cold target keeps the current rendition until loading completes.
`fitToColumns` selects the largest fitting width, clamping to the smallest.
`fitToContainer` also accounts for height and cell dimensions.

The Rust cascii-core-view crate provides the `multisize` feature, a borrowing
`MultiSizeAsset` decoder and `MultiSizePlayer` with bounded decoded caches. Native
consumers can supply memory-mapped encoded bytes and render `current()` through
their existing frame renderer. Audio remains owned by the application; changing
column count never seeks or restarts its clock.

## Validation and fixtures

~~~sh
cargo test --all-targets
cargo test --test multisize -- --include-ignored
cargo test --no-default-features
cargo check --target wasm32-unknown-unknown --no-default-features
~~~

The explicit integration command includes a generated FFmpeg video/audio test.
The fixture in tests/fixtures is shared byte-for-byte with the TypeScript and
Rust players. Regenerate deliberately with:

~~~sh
cargo run --example multisize_fixture -- tests/fixtures
~~~

Tests cover scalar/list/range parsing, exact dimensions, mixed-aspect sequences,
equivalence with independent conversions, both background fitters, all export
formats, source retention, cancellation, atomic publication, corrupt archives,
and switching size while playing, paused, seeking and looping.
