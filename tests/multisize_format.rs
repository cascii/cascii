//! Runs with and without the CLI feature, exercising the browser/WASM contract.
use cascii::multisize::{write_archive, ColumnSizes, MultiSizeArchive, MultiSizeJson};

#[test]
fn portable_codec_matches_the_shared_encoder_fixture() {
    let bytes = include_bytes!("fixtures/multisize.bin");
    let asset = MultiSizeArchive::parse(bytes).unwrap();
    let json: MultiSizeJson = serde_json::from_slice(include_bytes!("fixtures/multisize.json")).unwrap();
    json.validate().unwrap();
    let mut encoded = std::io::Cursor::new(Vec::new());
    write_archive(&mut encoded, &json.manifest, |columns, frame| asset.frame(columns, frame)).unwrap();
    assert_eq!(encoded.into_inner(), bytes);
    assert_eq!("55-73".parse::<ColumnSizes>().unwrap().values().len(), 19);
    assert!(asset.frame(55, 0).is_err());
    assert!(asset.frame(2, 4).is_err());
}
