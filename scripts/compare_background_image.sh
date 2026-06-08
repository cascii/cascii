#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 3 ]; then
  echo "Usage: $0 <image> [output-dir] [columns]" >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_PATH="$ROOT_DIR/target/release/cascii"
INPUT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
SOURCE_NAME="$(basename "$INPUT")"
SOURCE_STEM="${SOURCE_NAME%.*}"
OUTPUT_ROOT="${2:-/tmp/cascii-image-comparison-${SOURCE_STEM}-$(date +%Y%m%d-%H%M%S)}"
COLUMNS="${3:-170}"

LEGACY_ROOT="$OUTPUT_ROOT/legacy"
OPTIMIZED_ROOT="$OUTPUT_ROOT/optimized"
LEGACY_VIDEO="$OUTPUT_ROOT/legacy.mp4"
OPTIMIZED_VIDEO="$OUTPUT_ROOT/optimized.mp4"
LEGACY_PNG="$OUTPUT_ROOT/legacy.png"
OPTIMIZED_PNG="$OUTPUT_ROOT/optimized.png"
SIDE_BY_SIDE_PNG="$OUTPUT_ROOT/side-by-side.png"
DIFFERENCE_PNG="$OUTPUT_ROOT/difference.png"

if [ ! -f "$INPUT" ]; then
  echo "Image not found: $INPUT" >&2
  exit 1
fi

if [ -e "$OUTPUT_ROOT" ]; then
  echo "Refusing to overwrite existing comparison output: $OUTPUT_ROOT" >&2
  exit 1
fi

mkdir -p "$OUTPUT_ROOT"
cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"

COMMON_ARGS=(
  --small
  --columns "$COLUMNS"
  --color-only
)

echo "Running legacy background fitter..."
/usr/bin/time -p -o "$OUTPUT_ROOT/legacy.time" \
  "$BIN_PATH" "${COMMON_ARGS[@]}" --fit-cell-backgrounds "$INPUT" "$LEGACY_ROOT"

echo "Running optimized background fitter..."
/usr/bin/time -p -o "$OUTPUT_ROOT/optimized.time" \
  "$BIN_PATH" "${COMMON_ARGS[@]}" --fit-cell-backgrounds-optimized "$INPUT" "$OPTIMIZED_ROOT"

LEGACY_CFRAME="$(find "$LEGACY_ROOT" -type f -name '*.cframe' | sort | head -n 1)"
OPTIMIZED_CFRAME="$(find "$OPTIMIZED_ROOT" -type f -name '*.cframe' | sort | head -n 1)"

if [ -z "$LEGACY_CFRAME" ] || [ -z "$OPTIMIZED_CFRAME" ]; then
  echo "A conversion did not produce a .cframe file." >&2
  exit 1
fi

if cmp -s "$LEGACY_CFRAME" "$OPTIMIZED_CFRAME"; then
  IDENTICAL="yes"
else
  IDENTICAL="no"
fi

LEGACY_FRAME_DIR="$(dirname "$LEGACY_CFRAME")"
OPTIMIZED_FRAME_DIR="$(dirname "$OPTIMIZED_CFRAME")"
"$BIN_PATH" --small --to-video --fps 1 "$LEGACY_FRAME_DIR" "$LEGACY_VIDEO"
"$BIN_PATH" --small --to-video --fps 1 "$OPTIMIZED_FRAME_DIR" "$OPTIMIZED_VIDEO"

ffmpeg -y -loglevel error -i "$LEGACY_VIDEO" -frames:v 1 "$LEGACY_PNG"
ffmpeg -y -loglevel error -i "$OPTIMIZED_VIDEO" -frames:v 1 "$OPTIMIZED_PNG"
ffmpeg -y -loglevel error \
  -i "$LEGACY_PNG" \
  -i "$OPTIMIZED_PNG" \
  -filter_complex hstack=inputs=2 \
  "$SIDE_BY_SIDE_PNG"
ffmpeg -y -loglevel error \
  -i "$LEGACY_PNG" \
  -i "$OPTIMIZED_PNG" \
  -filter_complex blend=all_mode=difference \
  "$DIFFERENCE_PNG"

{
  echo "Input: $INPUT"
  echo "Columns: $COLUMNS"
  echo "Cframes byte-identical: $IDENTICAL"
  echo
  echo "Legacy:"
  cat "$OUTPUT_ROOT/legacy.time"
  echo
  echo "Optimized:"
  cat "$OUTPUT_ROOT/optimized.time"
  echo
  shasum -a 256 "$LEGACY_CFRAME" "$OPTIMIZED_CFRAME"
  echo
  echo "Legacy image: $LEGACY_PNG"
  echo "Optimized image: $OPTIMIZED_PNG"
  echo "Side-by-side image: $SIDE_BY_SIDE_PNG"
  echo "Difference image: $DIFFERENCE_PNG"
} | tee "$OUTPUT_ROOT/comparison.txt"

if [ "$IDENTICAL" != "yes" ]; then
  echo "ERROR: legacy and optimized outputs differ." >&2
  exit 1
fi
