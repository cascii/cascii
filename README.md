# cascii - Interactive ASCII Frame Generator

`cascii` is a high-performance, interactive tool for converting videos and image sequences into ASCII art frames.

**New:** `cascii` can now be used as both a CLI tool and a Rust library!

When converting a video, the output files will be placed in a directory named after the video file. For example, `cascii my_video.mp4` will create a `my_video` directory.

I recommend installing [cascii-viewer](https://github.com/cascii/cascii-viewer) to easily play any ascii animation generated or [decorator](https://github.com/cascii/decorator)

## Features

- **Interactive Mode**: If you don't provide arguments, `cascii` will prompt you for them.
- **Flexible Input**: Works with video files or directories of PNGs.
- **Performance**: Uses `ffmpeg` for fast frame extraction and parallel processing with Rayon for ASCII conversion.
- **Video Segments**: Specify start and end times to convert only a portion of a video.
- **Presets**: `--small` and `--large` flags for quick quality adjustments.
- **Non-interactive Mode**: Use `--default` to run without prompts, using default values.
- **Library Support**: Use cascii as a dependency in your own Rust projects.
- **Flexible FFmpeg**: Uses system ffmpeg by default, or accepts custom paths for bundled/embedded ffmpeg binaries.

## Requirements

- **FFmpeg**: Required for video conversion. cascii uses `ffmpeg` for frame extraction and `ffprobe` for video metadata.
  - By default, cascii looks for `ffmpeg` and `ffprobe` on your system PATH
  - For library usage, you can specify custom paths (useful for bundling ffmpeg with your application)

## Installation

### As a CLI Tool

An `install.sh` script is provided to build and install `cascii` to `/usr/local/bin`.

```bash
# Make sure you are in the cascii directory
./install.sh
```

You will be prompted for your password as it uses `sudo` to copy the binary.

Alternatively, install from crates.io (once published):

```bash
cargo install cascii
```

### As a Library

Add to your `Cargo.toml`:

```toml
[dependencies]
cascii = "0.1"
```

## CLI Usage

### cascii

#### Interactive

Run `cascii` without any arguments to be guided through the process:

```bash
cascii
```

It will first ask you to select an input file from the current directory, then prompt for the output directory, and finally for the quality settings.

#### With Arguments

You can also provide arguments directly:

```bash
# Basic usage with a video file
cascii my_video.mp4 --out ./my_frames

# Using presets
cascii my_video.mp4 --out ./my_frames --large

# Non-interactive mode (will fail if input is not provided)
cascii my_video.mp4 --out ./my_frames --default

# Convert a 5-second clip starting at 10 seconds into the video
cascii my_video.mp4 --start 00:00:10 --end 00:00:15
```

#### Options

- `[input]`: (Optional) The input video file or directory of images.
- `-o`, `--out`: (Optional) The output directory. Defaults to the current directory.
- `--columns`: (Optional) The width of the output ASCII art.
- `--fps`: (Optional) The frames per second to extract from a video.
- `--font-ratio`: (Optional) The aspect ratio of the font used for rendering.
- `--start`: (Optional) The start time for video conversion (e.g., `00:01:23.456` or `83.456`).
- `--end`: (Optional) The end time for video conversion.
- `--preprocess`: (Optional) ffmpeg `-vf` filtergraph applied before ASCII conversion (video and single-image inputs).
- `--preprocess-preset`: (Optional) Built-in preprocessing preset (use `--list-preprocess-presets`).
- `--list-preprocess-presets`: List built-in preprocessing presets and exit.
- `--default`: Skips all prompts and uses default values for any missing arguments.
- `-s`, `--small`: Uses smaller default values for quality settings.
- `-l`, `--large`: Uses larger default values for quality settings.
- `--colors`: Generate both `.txt` and `.cframe` (color) output files.
- `--color-only`: Generate only `.cframe` files (no `.txt`).
- `--audio`: Extract audio from the video to `audio.mp3`.
- `--luminance`: Luminance threshold (0-255) for what is considered transparent.
- `--keep-images`: Keep intermediate PNG frames after conversion.
- `--to-video`: Render ASCII frames into a video file (`.mp4`) instead of frame files. See [Export Movie](#export-movie).
- `--video-font-size`: Font size in pixels for `--to-video` rendering (default: `14`).
- `--crf`: CRF quality for `--to-video` encoding (0-51, lower = better, default: `18`).
- `--trim`: Trim equally from all sides of existing frames. Directional overrides: `--trim-left`, `--trim-right`, `--trim-top`, `--trim-bottom`.
- `--find-loop`: Detect repeated frame loops in a directory of `frame_*.txt` files.
- `-h`, `--help`: Shows the help message.
- `-V`, `--version`: Shows the version information.

# Export Movie

`cascii` can render ASCII art frames into an MP4 video file using `--to-video`. This works both from a source video (full pipeline) and from a directory of previously generated frames.

## From a Video File

Convert a video to an ASCII video in one command:

```bash
# White on black (default)
cascii input.mp4 --to-video --default

# Color — each character rendered in its original pixel color
cascii input.mp4 --to-video --colors --default

# Color with audio
cascii input.mp4 --to-video --colors --audio --default
```

## From Existing Frames

If you already have a directory of `.cframe` or `.txt` files from a previous `cascii` run, you can render them to video directly:

```bash
# From .cframe files (color) — auto-detected if present
cascii ./my_frames/ --to-video --fps 30 --default

# From .txt files (white on black) — used if no .cframe files exist
cascii ./my_frames/ --to-video --fps 24 --default

# With audio (uses audio.mp3 from the directory if present)
cascii ./my_frames/ --to-video --fps 30 --audio --default
```

When rendering from a directory, `cascii` scans for `.cframe` files first (full color). If none are found, it falls back to `.txt` files (white on black).

## Options

| Flag | Description | Default |
|------|-------------|---------|
| `--to-video` | Enable video output mode | off |
| `--colors` / `--color-only` | Generate color data (needed for color video from a source video) | off (white on black) |
| `--video-font-size <PX>` | Font size in pixels — controls output video resolution | `14` |
| `--crf <0-51>` | H.264 quality (lower = better quality, larger file) | `18` (visually lossless) |
| `--audio` | Mux audio into the output video | off |
| `--columns <N>` | ASCII width in characters | `400` |
| `--fps <N>` | Frames per second | `30` |

**Output resolution** is determined by `columns × font_size`. For example:

| Columns | Font Size | Approximate Width |
|---------|-----------|-------------------|
| 120 | 10 | ~960px |
| 200 | 14 | ~2800px |
| 200 | 32 | ~6400px |
| 400 | 14 | ~5600px |

## Examples

```bash
# Small compact video
cascii input.mp4 --to-video --colors --default --columns 120 --video-font-size 10

# Medium, readable characters
cascii input.mp4 --to-video --colors --default --columns 200 --video-font-size 14

# Large characters, high resolution
cascii input.mp4 --to-video --colors --default --columns 200 --video-font-size 32

# Lower quality, smaller file size
cascii input.mp4 --to-video --colors --default --crf 28

# Custom output path
cascii input.mp4 --to-video --colors --default -o my_ascii_video.mp4

# Contour-style preprocessing (raw ffmpeg filtergraph)
cascii input.mp4 --default --preprocess "format=gray,edgedetect=mode=colormix:high=0.2:low=0.05,eq=contrast=2.5:brightness=-0.1"

# Built-in preprocessing preset
cascii input.mp4 --default --preprocess-preset contours

# Specific time range with audio
cascii input.mp4 --to-video --colors --audio --default --start 00:00:10 --end 00:00:20

# Render existing frames to video
cascii ./my_frames/ --to-video --fps 30 --default --video-font-size 12
```

### Examples:

#### Source image:

![Source image](resources/source.png)

#### Test 1:

settings:

````
Luminance: 1
Font Ratio: 0.7
Columns: 400
````
![Test 1 output](resources/test_01.png)

#### Test 2:

settings:

````
Luminance: 35
Font Ratio: 0.7
Columns: 400
````
![Test 2 output](resources/test_02.png)

#### Test 3:

settings:

````
Luminance: 35
Font Ratio: 0.5
Columns: 400
````

![Test 3 output](resources/test_03.png)

#### Test 4:

settings:


````
Luminance: 35
Font Ratio: 1
Columns: 400
````
![Test 4 output](resources/test_04.png)

#### Test animation 1:

Reconstituting a few seconds from the clip [Aleph 2 by Max Cooper](https://www.youtube.com/watch?v=tNYfqklRehM) (around 2:30 to 3:00)

```
Frames: 960
Luminance: 30
Font Ratio: 0.7
Columns: 400
FPS: 30
```

![Demo](resources/demo_01.gif)

## Library Usage

`cascii` can be used as a Rust library in your own projects.

### Basic Example - Convert an Image to ASCII

```rust
use cascii::{AsciiConverter, ConversionOptions};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a converter with default configuration
    let converter = AsciiConverter::new();

    // Configure conversion options
    let options = ConversionOptions::default()
        .with_columns(200)
        .with_font_ratio(0.7)
        .with_luminance(20);

    // Convert image to ASCII file
    converter.convert_image(
        Path::new("input.png"),
        Path::new("output.txt"),
        &options
    )?;

    Ok(())
}
```

### Convert Image to String (No File)

```rust
use cascii::{AsciiConverter, ConversionOptions};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let converter = AsciiConverter::new();
    let options = ConversionOptions::default().with_columns(100);
    
    // Get ASCII as a string without writing to file
    let ascii_string = converter.image_to_string(
        Path::new("input.png"),
        &options
    )?;
    
    println!("{}", ascii_string);
    Ok(())
}
```

### Convert a Video to ASCII Frames

```rust
use cascii::{AsciiConverter, VideoOptions, ConversionOptions};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let converter = AsciiConverter::new();

    // Video extraction options
    let video_opts = VideoOptions {
        fps: 30,
        start: Some("0".to_string()),
        end: Some("10".to_string()),  // First 10 seconds
        columns: 400,
        extract_audio: false,
        preprocess_filter: None,
    };

    // ASCII conversion options
    let conv_opts = ConversionOptions::default()
        .with_font_ratio(0.7)
        .with_luminance(20);

    // Convert video
    converter.convert_video(
        Path::new("video.mp4"),
        Path::new("output_frames/"),
        &video_opts,
        &conv_opts,
        false  // Don't keep intermediate PNG files
    )?;

    Ok(())
}
```

### Using Presets

```rust
use cascii::AsciiConverter;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let converter = AsciiConverter::new();

    // Use built-in presets: "default", "small", or "large"
    let small_options = converter.options_from_preset("small")?;

    converter.convert_image(
        Path::new("input.png"),
        Path::new("output.txt"),
        &small_options
    )?;

    Ok(())
}
```

### Custom FFmpeg Path

By default, cascii uses `ffmpeg` and `ffprobe` from your system PATH. If you need to use bundled binaries or a custom installation, use `FfmpegConfig`:

```rust
use cascii::{AsciiConverter, FfmpegConfig, VideoOptions, ConversionOptions};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure custom ffmpeg paths
    let ffmpeg_config = FfmpegConfig::new()
        .with_ffmpeg("/path/to/ffmpeg")
        .with_ffprobe("/path/to/ffprobe");

    // Create converter with custom ffmpeg paths
    let converter = AsciiConverter::new()
        .with_ffmpeg_config(ffmpeg_config);

    let video_opts = VideoOptions {
        fps: 30,
        start: None,
        end: None,
        columns: 400,
        extract_audio: false,
        preprocess_filter: None,
    };

    let conv_opts = ConversionOptions::default();

    // Video conversion will use the specified ffmpeg binaries
    converter.convert_video(
        Path::new("video.mp4"),
        Path::new("output_frames/"),
        &video_opts,
        &conv_opts,
        false
    )?;

    Ok(())
}
```

This is useful for:
- **Desktop applications**: Bundle ffmpeg with your app for users who don't have it installed
- **Tauri/Electron apps**: Use sidecar binaries
- **Docker containers**: Use ffmpeg installed in a non-standard location
- **Testing**: Use a specific ffmpeg version

### API Reference

#### `AsciiConverter`

Main converter struct for ASCII art generation.

**Methods:**
- `new()` - Create converter with default configuration
- `with_config(config: AppConfig)` - Create with custom configuration
- `with_ffmpeg_config(config: FfmpegConfig)` - Set custom ffmpeg/ffprobe paths
- `from_config_file(path: &Path)` - Load configuration from file
- `convert_image(input, output, options)` - Convert image to ASCII file
- `image_to_string(input, options)` - Convert image to ASCII string
- `convert_video(input, output_dir, video_opts, conv_opts, keep_images)` - Convert video to ASCII frames
- `convert_video_to_video(input, video_opts, conv_opts, to_video_opts, callback)` - Convert video to ASCII video file (.mp4)
- `render_frames_to_video(input_dir, fps, to_video_opts, callback)` - Render existing .cframe/.txt frames to video file
- `convert_directory(input_dir, output_dir, options, keep_images)` - Convert directory of images
- `get_preset(name)` - Get a preset by name
- `options_from_preset(name)` - Get conversion options from a preset

#### `FfmpegConfig`

Configuration for ffmpeg/ffprobe binary paths.

**Methods:**
- `new()` - Create with default settings (uses system PATH)
- `with_ffmpeg(path)` - Set custom ffmpeg binary path
- `with_ffprobe(path)` - Set custom ffprobe binary path

#### `ConversionOptions`

Options for ASCII conversion.

**Fields:**
- `columns: Option<u32>` - Target width in characters
- `font_ratio: f32` - Font aspect ratio (width/height)
- `luminance: u8` - Luminance threshold (0-255)
- `ascii_chars: String` - ASCII character set (darkest to lightest)

**Methods:**
- `default()` - Create with default options
- `with_columns(columns)` - Set target width
- `with_font_ratio(ratio)` - Set font ratio
- `with_luminance(threshold)` - Set luminance threshold
- `with_ascii_chars(chars)` - Set custom character set

#### `VideoOptions`

Options for video conversion.

**Fields:**
- `fps: u32` - Frames per second to extract
- `start: Option<String>` - Start time (e.g., "00:01:23" or "83")
- `end: Option<String>` - End time
- `columns: u32` - Target width in characters
- `extract_audio: bool` - Whether to extract audio track from video

#### `ToVideoOptions`

Options for rendering ASCII frames to a video file.

**Fields:**
- `output_path: PathBuf` - Output video file path (e.g., "output.mp4")
- `font_size: f32` - Font size in pixels for rendering (default: 14.0)
- `crf: u8` - H.264 quality, 0-51 (default: 18, visually lossless)
- `mux_audio: bool` - Whether to mux audio into the output video

### Examples

See the `examples/` directory for complete examples:
- `simple_image.rs` - Basic image conversion
- `simple_video.rs` - Video conversion

Run examples with:
```bash
cargo run --example simple_image
cargo run --example simple_video
```

# Sample commands

## Test Image

./target/release/ascii-gen \
  --input ./some_frames_dir \
  --out ../output/sunset_hl \
  --font-ratio 0.7

## Test Video

./target/release/ascii-gen \
  --input ../input.webm \
  --out ../output/sunset_hl \
  --columns 800 \
  --fps 30 \
  --font-ratio 0.7

# Acknowledgements

This project is inspired by [developedbyed's video](https://www.youtube.com/watch?v=dUV8pobjZII) that I recommend watching, I reused the logic from his bash script and rewrote it in rust so that it could process faster files with more details. 
