#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 6 ]; then
  echo "Usage: $0 <video> [output-dir] [columns] [fps] [start] [end]" >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_PATH="$ROOT_DIR/target/release/cascii"
INPUT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
OUTPUT_ROOT="${2:-/tmp/cascii-background-comparison}"
COLUMNS="${3:-170}"
FPS="${4:-30}"
START="${5:-0}"
END="${6:-5}"
SOURCE_NAME="$(basename "$INPUT")"
SOURCE_STEM="${SOURCE_NAME%.*}"

LEGACY_ROOT="$OUTPUT_ROOT/legacy"
OPTIMIZED_ROOT="$OUTPUT_ROOT/optimized"
LEGACY_FRAMES="$LEGACY_ROOT/$SOURCE_STEM"
OPTIMIZED_FRAMES="$OPTIMIZED_ROOT/$SOURCE_STEM"
LEGACY_VIDEO="$OUTPUT_ROOT/legacy.mp4"
OPTIMIZED_VIDEO="$OUTPUT_ROOT/optimized.mp4"
SIDE_BY_SIDE_VIDEO="$OUTPUT_ROOT/side-by-side.mp4"

mkdir -p "$OUTPUT_ROOT"
cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"

for path in "$LEGACY_ROOT" "$OPTIMIZED_ROOT" "$LEGACY_VIDEO" "$OPTIMIZED_VIDEO" "$SIDE_BY_SIDE_VIDEO"; do
  if [ -e "$path" ]; then
    echo "Refusing to overwrite existing comparison output: $path" >&2
    exit 1
  fi
done

COMMON_ARGS=(
  --small
  --columns "$COLUMNS"
  --fps "$FPS"
  --start "$START"
  --end "$END"
  --colors
)

echo "Running legacy background fitter..."
/usr/bin/time -p -o "$OUTPUT_ROOT/legacy.time" \
  "$BIN_PATH" "${COMMON_ARGS[@]}" --fit-cell-backgrounds "$INPUT" "$LEGACY_ROOT"

echo "Running optimized background fitter..."
/usr/bin/time -p -o "$OUTPUT_ROOT/optimized.time" \
  "$BIN_PATH" "${COMMON_ARGS[@]}" --fit-cell-backgrounds-optimized "$INPUT" "$OPTIMIZED_ROOT"

"$BIN_PATH" --small --to-video --fps "$FPS" "$LEGACY_FRAMES" "$LEGACY_VIDEO"
"$BIN_PATH" --small --to-video --fps "$FPS" "$OPTIMIZED_FRAMES" "$OPTIMIZED_VIDEO"

if command -v ffmpeg >/dev/null 2>&1; then
  ffmpeg -y -loglevel error \
    -i "$LEGACY_VIDEO" \
    -i "$OPTIMIZED_VIDEO" \
    -filter_complex hstack=inputs=2 \
    "$SIDE_BY_SIDE_VIDEO"
fi

LEGACY_COUNT="$(find "$LEGACY_FRAMES" -maxdepth 1 -name '*.cframe' | wc -l | tr -d ' ')"
OPTIMIZED_COUNT="$(find "$OPTIMIZED_FRAMES" -maxdepth 1 -name '*.cframe' | wc -l | tr -d ' ')"

{
  echo "Input: $INPUT"
  echo "Range: $START to $END"
  echo "Columns: $COLUMNS"
  echo "FPS: $FPS"
  echo
  echo "Legacy frames: $LEGACY_COUNT"
  cat "$OUTPUT_ROOT/legacy.time"
  echo
  echo "Optimized frames: $OPTIMIZED_COUNT"
  cat "$OUTPUT_ROOT/optimized.time"
  echo
  echo "Legacy output: $LEGACY_FRAMES"
  echo "Optimized output: $OPTIMIZED_FRAMES"
  echo "Side-by-side video: $SIDE_BY_SIDE_VIDEO"
} | tee "$OUTPUT_ROOT/comparison.txt"
