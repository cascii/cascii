#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "Usage: $0 <image-input> <frames-directory> <video-input>" >&2
  echo "Example: $0 ./resources/input.png ./examples/frames ./resources/input.mp4" >&2
  exit 1
fi

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "hyperfine is required to run this benchmark script." >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_PATH="$ROOT_DIR/target/release/cascii"
IMAGE_INPUT="$1"
FRAMES_INPUT="$2"
VIDEO_INPUT="$3"
WORK_DIR="/tmp/cascii-fit-bg-bench"
IMAGE_OUTPUT="$WORK_DIR/image-output"
DIRECTORY_OUTPUT="$WORK_DIR/directory-output"
VIDEO_OUTPUT="$WORK_DIR/video-output.mp4"

mkdir -p "$WORK_DIR"

if [ ! -x "$BIN_PATH" ]; then
  cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"
fi

PREPARE_CMD="rm -rf '$IMAGE_OUTPUT' '$DIRECTORY_OUTPUT' '$VIDEO_OUTPUT'"

hyperfine \
  --warmup 1 \
  --prepare "$PREPARE_CMD" \
  --command-name "single-image-fit-bg" \
  "'$BIN_PATH' --small --color-only --fit-cell-backgrounds '$IMAGE_INPUT' '$IMAGE_OUTPUT'" \
  --command-name "directory-fit-bg" \
  "'$BIN_PATH' --small --color-only --fit-cell-backgrounds '$FRAMES_INPUT' '$DIRECTORY_OUTPUT'" \
  --command-name "video-to-video-fit-bg" \
  "'$BIN_PATH' --small --to-video --fit-cell-backgrounds '$VIDEO_INPUT' '$VIDEO_OUTPUT'"
