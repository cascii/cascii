#![cfg(feature = "cli")]
use cascii::multisize::{read_csv, write_archive, write_csv_frame, ColumnSizes, MultiSizeArchive, MultiSizeJson, CSV_HEADER};
use cascii::{AsciiConverter, BgFitQuality, CancelToken, CellColorMode, ConversionOptions, MultiSizeFormat, MultiSizeOptions, OutputMode};
use std::{fs, io::Cursor, path::Path, process::Command};

fn image(path: &Path, width: u32, height: u32) {
    image::RgbImage::from_fn(width, height, |x, y| image::Rgb([(x * 11 % 256) as u8, (y * 17 % 256) as u8, ((x + y) * 7 % 256) as u8])).save(path).unwrap();
}

fn options(columns: &str) -> MultiSizeOptions {
    let mut options = MultiSizeOptions::new(columns.parse().unwrap());
    options.conversion.output_mode = OutputMode::TextAndColor;
    options.formats = vec![MultiSizeFormat::Directory, MultiSizeFormat::Binary, MultiSizeFormat::Json, MultiSizeFormat::Csv];
    options
}

fn no_partial(root: &Path) {
    assert!(fs::read_dir(root).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().contains(".partial-")));
}

#[test]
fn sizes_accept_scalars_lists_and_inclusive_ranges_and_reject_invalid_work() {
    assert_eq!("73".parse::<ColumnSizes>().unwrap().values(), &[73]);
    assert_eq!(" 73,55,58,61,67,73,80 ".parse::<ColumnSizes>().unwrap().values(), &[55, 58, 61, 67, 73, 80]);
    assert_eq!("55-73".parse::<ColumnSizes>().unwrap().values(), &(55..=73).collect::<Vec<_>>());
    assert!(("55-73".parse::<ColumnSizes>().unwrap().relative_cell_count(73).unwrap() - 14.7108).abs() < 0.0001);
    for invalid in ["", "0", "-1", "4097", "73-55", "1-4096", "55,,73", "55-73,80", "nan", "55..73", "1-4294967295"] {
        assert!(invalid.parse::<ColumnSizes>().is_err(), "{invalid}");
    }
}

#[test]
fn golden_binary_json_and_cell_csv_round_trip_foregrounds_backgrounds_and_punctuation() {
    let json: MultiSizeJson = serde_json::from_slice(include_bytes!("fixtures/multisize.json")).unwrap();
    json.validate().unwrap();
    let bytes = include_bytes!("fixtures/multisize.bin");
    let archive = MultiSizeArchive::parse(bytes).unwrap();
    let mut output = Cursor::new(Vec::new());
    write_archive(&mut output, &json.manifest, |columns, index| archive.frame(columns, index)).unwrap();
    assert_eq!(output.into_inner(), bytes);
    let mut csv = format!("{CSV_HEADER}\n").into_bytes();
    for index in 0..json.manifest.frame_count {
        for variant in &json.manifest.variants {
            let frame = archive.frame(variant.columns, index).unwrap();
            assert_eq!(frame, json.variants.iter().find(|item| item.columns == variant.columns).unwrap().frames[index as usize]);
            write_csv_frame(&mut csv, variant.columns, index, variant.rows[index as usize], &frame).unwrap();
        }
    }
    assert_eq!(read_csv(json.manifest.clone(), std::str::from_utf8(&csv).unwrap()).unwrap(), json);
    let truncated = std::str::from_utf8(&csv).unwrap().lines().take(3).collect::<Vec<_>>().join("\n");
    assert!(read_csv(json.manifest, &truncated).is_err());
}

#[test]
fn binary_rejects_truncation_trailing_data_wrong_indices_and_bad_frame_payloads() {
    let bytes = include_bytes!("fixtures/multisize.bin");
    for end in [0, 4, 15, 16, bytes.len() - 1] {
        assert!(MultiSizeArchive::parse(&bytes[..end]).is_err());
    }
    let mut corrupt = bytes.to_vec();
    corrupt.push(0);
    assert!(MultiSizeArchive::parse(&corrupt).is_err());
    let metadata_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let index_start = 16 + metadata_len;
    for offset in [4, 8, 12, index_start, index_start + 4, index_start + 8, index_start + 16, index_start + 20] {
        let mut corrupt = bytes.to_vec();
        corrupt[offset] ^= 0xff;
        assert!(MultiSizeArchive::parse(&corrupt).is_err(), "offset {offset}");
    }
    let mut corrupt = bytes.to_vec();
    let count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let payload = index_start + count * 24;
    corrupt[payload + 8] = 0;
    assert!(MultiSizeArchive::parse(&corrupt).unwrap().frame(2, 0).is_err());
}

#[test]
fn all_exports_produce_exact_sizes_from_one_image_and_preserve_source() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("image.png");
    image(&input, 32, 20);
    let original = fs::read(&input).unwrap();
    let output = temp.path().join("output");
    let options = options("55,58,61,67,73,80");
    let manifest = AsciiConverter::new().convert_multi_size(&input, &output, &options, |_, _| {}).unwrap();
    assert_eq!(manifest.frame_count, 1);
    assert_eq!(manifest.variants.len(), 6);
    let binary = fs::read(output.join("frames.bin")).unwrap();
    let archive = MultiSizeArchive::parse(&binary).unwrap();
    let json: MultiSizeJson = serde_json::from_slice(&fs::read(output.join("frames.json")).unwrap()).unwrap();
    json.validate().unwrap();
    assert_eq!(json.manifest, manifest);
    assert_eq!(read_csv(manifest.clone(), &fs::read_to_string(output.join("frames.csv")).unwrap()).unwrap(), json);
    for variant in &manifest.variants {
        let actual = archive.frame(variant.columns, 0).unwrap();
        let mut single = options.conversion.clone();
        single.columns = Some(variant.columns);
        let single_path = temp.path().join(format!("single-{}.txt", variant.columns));
        AsciiConverter::new().convert_image(&input, &single_path, &single).unwrap();
        assert_eq!(actual.text, fs::read_to_string(single_path).unwrap());
        assert_eq!(actual.cframe.unwrap(), fs::read(output.join(manifest.frame_path(variant.columns, 0, true).unwrap())).unwrap());
    }
    assert_eq!(fs::read(input).unwrap(), original);
    no_partial(temp.path());
}

#[test]
fn a_directory_keeps_originals_and_frame_identity_even_with_mixed_aspect_ratios() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input");
    fs::create_dir(&input).unwrap();
    image(&input.join("a.png"), 32, 16);
    image(&input.join("b.png"), 16, 32);
    let originals = [fs::read(input.join("a.png")).unwrap(), fs::read(input.join("b.png")).unwrap()];
    let output = temp.path().join("output");
    let manifest = AsciiConverter::new().convert_multi_size(&input, &output, &options("55-73"), |_, _| {}).unwrap();
    assert_eq!(manifest.frame_count, 2);
    assert_eq!(manifest.variants.len(), 19);
    for variant in &manifest.variants {
        assert!(variant.rows[1] > variant.rows[0]);
        assert!(output.join(manifest.frame_path(variant.columns, 1, true).unwrap()).exists());
    }
    assert_eq!(fs::read(input.join("a.png")).unwrap(), originals[0]);
    assert_eq!(fs::read(input.join("b.png")).unwrap(), originals[1]);
}

#[test]
fn legacy_directory_api_honors_columns_and_keeps_separate_input_originals() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input");
    fs::create_dir(&input).unwrap();
    image(&input.join("frame_0001.png"), 16, 16);
    let converter = AsciiConverter::new();
    let options = ConversionOptions::default().with_columns(73);
    for detailed in [false, true] {
        let output = temp.path().join(format!("output-{detailed}"));
        if detailed {
            converter.convert_directory_with_progress(&input, &output, &options, false, |_| {}).unwrap();
        } else {
            converter.convert_directory(&input, &output, &options, false).unwrap();
        }
        let text = fs::read_to_string(output.join("frame_0001.txt")).unwrap();
        assert!(text.lines().all(|line| line.len() == 73));
        assert!(input.join("frame_0001.png").exists());
    }
}

#[test]
fn background_modes_match_independent_single_size_conversions() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("image.png");
    image(&input, 12, 8);
    for (index, mode) in [CellColorMode::FitForegroundBackground, CellColorMode::FitForegroundBackgroundOptimized].into_iter().enumerate() {
        let mut options = options("3,5");
        options.conversion.cell_color_mode = mode;
        options.conversion.bg_fit_quality = BgFitQuality::Fast;
        let output = temp.path().join(format!("output-{index}"));
        let manifest = AsciiConverter::new().convert_multi_size(&input, &output, &options, |_, _| {}).unwrap();
        for variant in &manifest.variants {
            let mut single = options.conversion.clone();
            single.columns = Some(variant.columns);
            let single_path = temp.path().join(format!("single-{index}-{}.txt", variant.columns));
            AsciiConverter::new().convert_image(&input, &single_path, &single).unwrap();
            let actual = fs::read(output.join(manifest.frame_path(variant.columns, 0, true).unwrap())).unwrap();
            assert_eq!(actual, fs::read(single_path.with_extension("cframe")).unwrap());
            let base = 8 + variant.columns as usize * variant.rows[0] as usize * 4;
            assert_eq!(actual[base], 1);
        }
    }
}

#[test]
fn failed_and_cancelled_jobs_remove_only_their_staging_files() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input");
    fs::create_dir(&input).unwrap();
    image(&input.join("a.png"), 16, 16);
    fs::write(input.join("b.png"), b"not a PNG").unwrap();
    let output = temp.path().join("failed");
    assert!(AsciiConverter::new().convert_multi_size(&input, &output, &options("3,5"), |_, _| {}).is_err());
    assert!(!output.exists());
    assert!(input.join("a.png").exists());
    no_partial(temp.path());
    fs::remove_file(input.join("b.png")).unwrap();
    image(&input.join("c.png"), 16, 16);
    let token = CancelToken::new();
    let converter = AsciiConverter::new().with_cancel_token(token.clone());
    let cancelled = temp.path().join("cancelled");
    assert!(converter.convert_multi_size(&input, &cancelled, &options("3,5"), |done, _| {
        if done > 0 {token.cancel();}
    }).is_err());
    assert!(!cancelled.exists());
    no_partial(temp.path());
    fs::create_dir(&output).unwrap();
    fs::write(output.join("keep.txt"), b"keep").unwrap();
    assert!(AsciiConverter::new().convert_multi_size(&input, &output, &options("3,5"), |_, _| {}).is_err());
    assert_eq!(fs::read(output.join("keep.txt")).unwrap(), b"keep");
}

#[test]
fn text_only_binary_and_single_size_asset_output_work_without_color_sidecars() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("image.png");
    image(&input, 16, 16);
    let mut options = options("73");
    options.conversion = ConversionOptions::default();
    options.formats = vec![MultiSizeFormat::Binary];
    let output = temp.path().join("output");
    let manifest = AsciiConverter::new().convert_multi_size(&input, &output, &options, |_, _| {}).unwrap();
    assert!(!output.join("variants").exists());
    let bytes = fs::read(output.join("frames.bin")).unwrap();
    assert!(MultiSizeArchive::parse(&bytes).unwrap().frame(73, 0).unwrap().cframe.is_none());
    assert!(!manifest.color);
}

#[test]
fn cli_preserves_single_size_layout_and_accepts_the_three_column_forms() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("image.png");
    image(&input, 16, 16);
    for (index, columns) in ["73", "55,58,61,67,73,80", "55-73"].iter().enumerate() {
        let output = temp.path().join(format!("cli-{index}"));
        let status = Command::new(env!("CARGO_BIN_EXE_cascii")).arg(&input).args(["--out"]).arg(&output).args(["--columns", columns, "--default"]).output().unwrap();
        assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
        let output = output.join("image");
        if index == 0 {
            assert!(output.join("image.txt").exists());
            assert!(!output.join("manifest.json").exists());
        } else {
            let manifest: cascii::multisize::MultiSizeManifest = serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
            assert_eq!(manifest.variants.len(), if index == 1 {6} else {19});
        }
    }
}

#[test]
#[ignore = "requires FFmpeg with H.264/AAC encoders; run with --include-ignored"]
fn video_is_sampled_once_with_shared_audio_and_consistent_variant_timing() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("clip.mp4");
    let result = Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=32x24:rate=4", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=8000", "-t", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"]).arg(&input).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let original = fs::read(&input).unwrap();
    let output = temp.path().join("output");
    let mut options = options("5,7,9");
    options.video.fps = 4;
    options.video.extract_audio = true;
    let manifest = AsciiConverter::new().convert_multi_size(&input, &output, &options, |_, _| {}).unwrap();
    assert_eq!(manifest.frame_count, 4);
    assert_eq!(manifest.fps, 4);
    assert_eq!(manifest.audio.as_deref(), Some("audio.mp3"));
    assert!(output.join("audio.mp3").metadata().unwrap().len() > 0);
    assert!(!output.join("images").exists());
    for variant in &manifest.variants {
        assert_eq!(variant.rows.len(), 4);
        assert!(!output.join(format!("variants/{}/audio.mp3", variant.columns)).exists());
    }
    assert_eq!(fs::read(input).unwrap(), original);
}
