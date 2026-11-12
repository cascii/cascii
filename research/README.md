# Reproduce the cascii GPU investigation

Start with [REPORT.md](REPORT.md) for findings and [BLOG_DRAFT.md](BLOG_DRAFT.md) for the article. All paths below are relative to the cloned repository root, `D:\Desktop\cascii` on the measured machine.

The companion `cascii-gpu-investigation.zip` contains this `research/` directory, excluding build products, source media, and bulk generated frame files. Extract it over a clone of the upstream commit recorded in the report. The original Desktop checkout retains those locally generated files. `package_evidence.py` validates document links and records evidence/source checksums before creating the archive.

The upstream source is unchanged. `gpu-bench` is an independent Rust package using the root crate as a path dependency. Its own lockfile pins its dependencies. It uses wgpu 0.19.4/Vulkan and requires a real Vulkan GPU; the benchmark refuses a CPU software adapter. Current executable paths and fixture scripts target Windows. No CUDA toolkit is needed for the Vulkan prototype.

## Build and check

```powershell
Set-Location D:\Desktop\cascii
cargo build --release --locked
cargo test --locked
cargo test --no-default-features --lib --locked
cargo test --locked --test multisize video_is_sampled_once_with_shared_audio_and_consistent_variant_timing -- --ignored
cargo build --release --locked --manifest-path research/gpu-bench/Cargo.toml
```

The last integration test needs FFmpeg H.264/AAC encoding. The GPU benchmark itself performs its comparison/edge checks when invoked with `frames`.

## Fixtures and benchmarks

Keep FFmpeg and ffprobe on PATH. Run these sequentially with no competing build or benchmark processes. Preserve `research/results` before rerunning: the runner replaces summary JSON/logs, while per-run image/video output folders use new timestamps. Fixtures overwrite only the two explicitly named generated MP4 files.

```powershell
& research/make-fixtures.ps1
python research/run_benchmarks.py frames
python research/run_benchmarks.py images
python research/run_benchmarks.py videos
python research/run_benchmarks.py fitters
python research/verify_artifacts.py
```

The video run tests CPU decode, Vulkan compute, and CUDA decode/scaling, writing 120 color frames per run. It requires working CUDA decoding and the `scale_cuda` FFmpeg filter for the CUDA cases. It aborts on unsupported hardware instead of silently recording a CPU fallback. It does not require NVENC. Expect a few minutes and several gigabytes for build products, fixtures, and generated outputs. These generated directories are ignored by Git.

An individual prototype invocation:

```powershell
# Full one-image operation, fixed 400-column default settings:
research/gpu-bench/target/release/cascii-gpu-bench.exe image gpu resources/test_01.png research/results/example.cframe

# Video: backend, input, source width, source height, columns,
# decode backend, early-scale toggle, output directory:
research/gpu-bench/target/release/cascii-gpu-bench.exe video gpu research/fixtures/motion_1080p.mp4 1920 1080 400 cpu 1 research/results/manual-output
```

Use `cpu` instead of `gpu` for the API comparison; use `cuda` instead of the second `cpu` for CUDA decoding/scaling. Early-scale `1` retains an intermediate small frame; `0` sends full-resolution RGB through the pipe. The video harness requires correct source dimensions, 30-fps fixture semantics, a nonzero sensible column count, and a new output directory. It does not expose all cascii options, automatically probe dimensions, clean old outputs, or provide production cancellation/device-loss handling.

## Figures

The benchmark runner and verification scripts use Python's standard library. Plotting has an isolated environment:

```powershell
python -m venv research/.venv
research/.venv/Scripts/python.exe -m pip install -r research/plot-requirements.txt
research/.venv/Scripts/python.exe research/plot_results.py
```

Figures are generated from saved results, not freshly timed work, and are saved as PNG and SVG. Their labels state whether initialization/decoding are included. Charts were visually inspected for clipping and readability.

## Evidence layout

| File | Purpose |
|---|---|
| `results/frames.json` | Final 24 frame cases, 15 samples/backend, setup costs, 28 edge cases, fidelity |
| `results/frames-before-upload-fix.json` | Earlier measurements with full-image padding copy |
| `results/images.json` | Cold-process image timings and exact invocation arguments |
| `results/images-before-upload-fix.json` | Earlier cold-process comparison |
| `results/videos.json` | 56 video runs, including warmups; commands and internal stage timers |
| `results/video_summary.json` | Medians, ranges, throughput for 14 video configurations |
| `results/video_quality_*.json` | Every-frame hashes; five-frame sampled cell-error metrics |
| `results/fitters.json` | 36 foreground/legacy/optimized CPU runs including warmups |
| `results/artifact-verification.json` | Fitter byte parity and image CPU parity/GPU differences |
| `results/fixtures.json` | Sizes and SHA-256 identifiers for source PNGs and generated clips |
| `results/environment.json` | Hardware, toolchain, OS, filesystem, upstream commit |
| `results/nvenc-probe.txt` | Actual encoder compatibility failure |
| `*-tests.log`, `video-integration-test.log` | Upstream and core validation evidence |

Measurements for videos use the earlier aligned-input upload path. The final upload optimization only changes handling of RGB lengths not divisible by four; the tested videos are all aligned. The shader and video algorithms are unchanged. Final frames and cold-image measurements use the revised implementation. See the report for all comparability limits.
