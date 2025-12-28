//! Example: Convert an image to ASCII art using cascii as a library
//! Run with: cargo run --example simple_image

use cascii::{AsciiConverter, ConversionOptions};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a converter with default configuration
    let converter = AsciiConverter::new();

    // Configure conversion options
    let options = ConversionOptions::default()
        .with_columns(100)
        .with_font_ratio(0.5)
        .with_luminance(20);

    // Example 1: Convert image to ASCII file
    let input = Path::new("resources/source.png");
    let output = Path::new("example_output.txt");

    if input.exists() {
        println!("Converting {} to ASCII art...", input.display());
        converter.convert_image(input, output, &options)?;
        println!("✓ ASCII art saved to {}", output.display());
    } else {
        println!(
            "Note: {} not found, skipping file conversion example",
            input.display()
        );
    }

    // Example 2: Convert image to string (no file)
    if input.exists() {
        println!("\nConverting image to string...");
        let ascii_string = converter.image_to_string(input, &options)?;
        println!(
            "✓ Generated ASCII string ({} characters)",
            ascii_string.len()
        );
        println!("\nFirst 500 characters:");
        println!("{}", &ascii_string[..500.min(ascii_string.len())]);
    }

    // Example 3: Using presets
    if input.exists() {
        println!("\n\nUsing 'small' preset...");
        let small_options = converter.options_from_preset("small")?;
        let output_small = Path::new("example_output_small.txt");
        converter.convert_image(input, output_small, &small_options)?;
        println!("✓ Small ASCII art saved to {}", output_small.display());
    }

    Ok(())
}
