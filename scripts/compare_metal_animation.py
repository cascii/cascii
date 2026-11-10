#!/usr/bin/env python3
"""Generate animated samples and compare the original fitter with Metal on macOS.

Requires cargo and ffmpeg. Writes only to a new output directory. No formatters.
"""

import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys


def run(*command, **kwargs):
    subprocess.run(command, check=True, **kwargs)


def scenarios():
    ramp = " .'`^,:;Il!i><~+_-?][}{1)(|/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$"
    short = " .:-=+*#%@"
    specs = [
        ("demo-160-fidelity-low", "source-contiguous", 160, "fidelity", 0, 0, 0.7, ramp, True),
        ("demo-160-fidelity-default", "source-contiguous", 160, "fidelity", 20, 20, 0.7, ramp, False),
        ("demo-320-fidelity", "source-contiguous", 320, "fidelity", 0, 0, 0.7, ramp, False),
        ("demo-160-fast", "source-contiguous", 160, "fast", 0, 0, 0.7, ramp, False),
        ("demo-80-short-ramp", "source-contiguous", 80, "fidelity", 0, 0, 0.7, short, False),
        ("color-160-fidelity", "source-color", 160, "fidelity", 20, 20, 0.7, ramp, True),
        ("color-320-fidelity", "source-color", 320, "fidelity", 20, 20, 0.7, ramp, False),
        ("color-160-fast", "source-color", 160, "fast", 20, 20, 0.7, ramp, True),
        ("color-80-short-ramp", "source-color", 80, "fidelity", 20, 20, 0.7, short, False),
        ("color-background-only", "source-color", 160, "fidelity", 200, 20, 0.7, ramp, False),
        ("color-background-suppressed", "source-color", 160, "fidelity", 20, 200, 0.7, ramp, True),
        ("color-threshold-255", "source-color", 160, "fidelity", 255, 255, 0.7, ramp, False),
        ("color-font-ratio-half", "source-color", 160, "fidelity", 20, 20, 0.5, ramp, False),
        ("demo-spread", "source-spread", 160, "fidelity", 0, 0, 0.7, ramp, False),
        ("color-space-only", "source-color", 160, "fidelity", 20, 20, 0.7, " ", False),
    ]
    keys = ("name", "source", "columns", "quality", "threshold", "background_threshold", "font_ratio", "ramp", "previews")
    return [dict(zip(keys, values)) for values in specs]


def render_comparisons(root):
    # The lossless PNGs and raw cframes remain the numerical source of truth.
    for scenario in scenarios():
        if not scenario["previews"]:
            continue
        directory = root / "results" / scenario["name"]
        inputs = []
        for kind in ("cpu", "metal", "difference"):
            inputs.extend(["-framerate", "12", "-i", str(directory / kind / "frame_%04d.png")])
        graph = ";".join(f"[{index}:v]scale=640:-2:flags=neighbor[v{index}]" for index in range(3))
        graph += ";[v0][v1][v2]hstack=inputs=3,format=yuv420p[out]"
        run("ffmpeg", "-hide_banner", "-loglevel", "error", *inputs, "-filter_complex", graph,
            "-map", "[out]", "-c:v", "libx264", "-crf", "12", "-preset", "fast", "-movflags", "+faststart",
            str(directory / "cpu-metal-difference-x8.mp4"))


def summarize(root):
    rows = json.loads((root / "comparison.json").read_text())
    summary = []
    for row in rows:
        frames = row["frames"]
        cpu = statistics.median(frame["cpu_ms"] for frame in frames)
        gpu = statistics.median(frame["metal_ms"] for frame in frames)
        summary.append({
            "scenario": row["settings"]["name"],
            "frames": len(frames),
            "identical_frames": sum(frame["identical"] for frame in frames),
            "total_cells": sum(frame["cells"] for frame in frames),
            "changed_glyphs": sum(frame["changed_glyphs"] for frame in frames),
            "changed_color_bytes": sum(frame["changed_color_bytes"] for frame in frames),
            "pixel_mae": statistics.mean(frame["pixel_mae"] for frame in frames),
            "pixel_max_error": max(frame["pixel_max_error"] for frame in frames),
            "temporal_delta_mae": statistics.mean(frame["temporal_delta_mae"] for frame in frames[1:]),
            "cpu_median_ms": cpu,
            "metal_median_ms": gpu,
            "speedup": cpu / gpu,
        })
    (root / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("animation", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("Metal comparisons require macOS")
    source = args.animation.resolve(strict=True)
    root = args.out.resolve()
    root.mkdir(parents=True, exist_ok=False)
    repository = Path(__file__).resolve().parents[1]
    for name in ("source-contiguous", "source-spread", "source-color"):
        (root / name).mkdir()
    run("ffmpeg", "-hide_banner", "-loglevel", "error", "-i", str(source), "-ss", "3", "-t", "3",
        "-vf", "fps=12,scale=640:-2", str(root / "source-contiguous/frame_%04d.png"))
    run("ffmpeg", "-hide_banner", "-loglevel", "error", "-i", str(source), "-vf", "fps=1,scale=640:-2",
        "-frames:v", "27", str(root / "source-spread/frame_%04d.png"))
    run("ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=12",
        "-t", "3", str(root / "source-color/frame_%04d.png"))
    (root / "scenarios.json").write_text(json.dumps(scenarios(), indent=2) + "\n")
    environment = dict(os.environ, CASCII_METAL_VALIDATION_ROOT=str(root))
    run("cargo", "test", "--release", "--features", "metal", "--locked", "--lib", "metal_animation_comparison",
        "--", "--ignored", "--nocapture", cwd=repository, env=environment)
    render_comparisons(root)
    summarize(root)


if __name__ == "__main__":
    main()
