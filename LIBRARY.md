# Using cascii as a Library

`cascii` can be used as both a CLI tool and a Rust library. This document shows how to use it as a dependency in your own projects.

## Adding to Your Project

Add cascii to your `Cargo.toml`:

```toml
[dependencies]
cascii = { path = "../path/to/cascii" }
# Or when published to crates.io:
# cascii = "0.2"
```

## Basic Usage

### Convert an Image to ASCII

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

### Convert a Directory of Images

```rust
use cascii::{AsciiConverter, ConversionOptions};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let converter = AsciiConverter::new();
    let options = ConversionOptions::default();

    converter.convert_directory(
        Path::new("input_images/"),
        Path::new("output_ascii/"),
        &options,
        false  // Don't keep original images
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

### Custom Configuration

```rust
use cascii::{AsciiConverter, AppConfig};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load configuration from a custom file
    let converter = AsciiConverter::from_config_file(
        Path::new("my_config.json")
    )?;

    // Or create with a custom config object
    let mut config = AppConfig::default();
    config.ascii_chars = " .:-=+*#%@".to_string();  // Custom character set
    let converter = AsciiConverter::with_config(config)?;

    Ok(())
}
```

## API Reference

### `AsciiConverter`

Main converter struct for ASCII art generation.

#### Methods

- `new()` - Create converter with default configuration
- `with_config(config: AppConfig)` - Create with custom configuration
- `from_config_file(path: &Path)` - Load configuration from file
- `convert_image(input, output, options)` - Convert image to ASCII file
- `image_to_string(input, options)` - Convert image to ASCII string
- `convert_video(input, output_dir, video_opts, conv_opts, keep_images)` - Convert video to ASCII frames
- `convert_directory(input_dir, output_dir, options, keep_images)` - Convert directory of images
- `get_preset(name)` - Get a preset by name
- `options_from_preset(name)` - Get conversion options from a preset

### `ConversionOptions`

Options for ASCII conversion.

#### Fields

- `columns: Option<u32>` - Target width in characters
- `font_ratio: f32` - Font aspect ratio (width/height)
- `luminance: u8` - Luminance threshold (0-255)
- `ascii_chars: String` - ASCII character set (darkest to lightest)

#### Methods

- `default()` - Create with default options
- `with_columns(columns)` - Set target width
- `with_font_ratio(ratio)` - Set font ratio
- `with_luminance(threshold)` - Set luminance threshold
- `with_ascii_chars(chars)` - Set custom character set
- `from_preset(preset, ascii_chars)` - Create from a preset

### `VideoOptions`

Options for video conversion.

#### Fields

- `fps: u32` - Frames per second to extract
- `start: Option<String>` - Start time (e.g., "00:01:23" or "83")
- `end: Option<String>` - End time
- `columns: u32` - Target width in characters

## Examples

See the `examples/` directory for complete examples:

- `simple_image.rs` - Basic image conversion
- `simple_video.rs` - Video conversion

Run examples with:

```bash
cargo run --example simple_image
cargo run --example simple_video
```

## Requirements

- Rust 1.70 or later
- For video conversion: `ffmpeg` must be installed and available in PATH

## License

MIT OR Apache-2.0

