# cascii - Interactive ASCII Frame Generator

`cascii` is a high-performance, interactive tool for converting videos and image sequences into ASCII art frames.

**New:** `cascii` can now be used as both a CLI tool and a Rust library! See [LIBRARY.md](LIBRARY.md) for library usage documentation.

When converting a video, the output files will be placed in a directory named after the video file. For example, `cascii my_video.mp4` will create a `my_video` directory.

I recommend installing [cascii-viewer](https://github.com/cascii/cascii-viewer) to easily play any ascii animation generated

## Features

- **Interactive Mode**: If you don't provide arguments, `cascii` will prompt you for them.
- **Flexible Input**: Works with video files or directories of PNGs.
- **Performance**: Uses `ffmpeg` for fast frame extraction and parallel processing with Rayon for ASCII conversion.
- **Video Segments**: Specify start and end times to convert only a portion of a video.
- **Presets**: `--small` and `--large` flags for quick quality adjustments.
- **Non-interactive Mode**: Use `--default` to run without prompts, using default values.

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
cascii = "0.3"
```

See [LIBRARY.md](LIBRARY.md) for detailed usage examples.

## Usage

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
- `--default`: Skips all prompts and uses default values for any missing arguments.
- `-s`, `--small`: Uses smaller default values for quality settings.
- `-l`, `--large`: Uses larger default values for quality settings.
- `-h`, `--help`: Shows the help message.
- `-V`, `--version`: Shows the version information.


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
