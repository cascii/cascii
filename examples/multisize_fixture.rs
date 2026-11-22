//! Regenerate the shared cross-language golden fixture with an explicit output directory.
use cascii::multisize::{write_archive, write_csv_frame, JsonVariant, MultiSizeJson, MultiSizeManifest, SizeVariant, StoredFrame, CSV_HEADER};
use std::{fs, io::Cursor, path::PathBuf};

fn main() -> anyhow::Result<()> {
    let output = PathBuf::from(std::env::args().nth(1).expect("pass fixture output directory"));
    fs::create_dir_all(&output)?;
    let manifest = MultiSizeManifest {
        format: "cascii-multisize".into(), schema_version: 1, frame_count: 4, fps: 24, default_columns: 3, column_map: None,
        font_ratio: 0.7, luminance: 20, ascii_chars: " .,#\"".into(), text: true, color: true, audio: None,
        outputs: vec!["variants/".into(), "frames.bin".into(), "frames.json".into(), "frames.csv".into()],
        variants: vec![SizeVariant {columns: 2, rows: vec![2; 4]}, SizeVariant {columns: 3, rows: vec![2; 4]}],
    };
    let variants = manifest.variants.iter().map(|variant| JsonVariant {
        columns: variant.columns,
        frames: (0..4).map(|index| {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&variant.columns.to_le_bytes());
            bytes.extend_from_slice(&2u32.to_le_bytes());
            let mut text = String::new();
            for cell in 0..variant.columns * 2 {
                let glyph = b" .,#\""[((cell + index) % 5) as usize];
                text.push(glyph as char);
                if (cell + 1) % variant.columns == 0 {text.push('\n');}
                bytes.extend_from_slice(&[glyph, index as u8 * 30, cell as u8 * 20, 200]);
            }
            if index % 2 == 0 {
                bytes.push(1);
                for _ in 0..variant.columns * 2 {bytes.extend_from_slice(&[1, 2, 3]);}
            }
            StoredFrame {text, cframe: Some(bytes)}
        }).collect(),
    }).collect();
    let mut asset = MultiSizeJson {manifest, variants};
    asset.validate()?;
    let mut buffer = Cursor::new(Vec::new());
    write_archive(&mut buffer, &asset.manifest, |columns, frame| {
        Ok(asset.variants.iter().find(|variant| variant.columns == columns).unwrap().frames[frame as usize].clone())
    })?;
    fs::write(output.join("multisize.bin"), buffer.into_inner())?;
    fs::write(output.join("multisize.json"), serde_json::to_vec_pretty(&asset)?)?;
    let mut csv = format!("{CSV_HEADER}\n").into_bytes();
    for index in 0..asset.manifest.frame_count {
        for variant in &asset.variants {
            write_csv_frame(&mut csv, variant.columns, index, 2, &variant.frames[index as usize])?;
        }
    }
    fs::write(output.join("multisize.csv"), csv)?;
    asset.manifest.schema_version=2;
    asset.manifest.column_map=Some(vec![2,3,2,3]);
    let mut buffer=Cursor::new(Vec::new());
    write_archive(&mut buffer,&asset.manifest,|columns,index| Ok(asset.variants.iter().find(|v|v.columns==columns).unwrap().frames[index as usize].clone()))?;
    fs::write(output.join("multisize-v2.bin"),buffer.into_inner())?;
    fs::write(output.join("multisize-v2.json"),serde_json::to_vec_pretty(&asset)?)?;
    Ok(())
}
