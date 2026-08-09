use pdf_extract::{
    assess_parse_quality, measure_pdf_decompression, parse_pdf, ExtractionOptions, LAParams,
    ParseQualityStatus,
};
use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

const LARGE_NAME: &str = "2024_2_420_616_EN.pdf";
const LARGE_SIZE: u64 = 36_114_066;
const LARGE_SHA256: &str = "f7bac4d88acab0ece5f82a51366cc4440ff241af00d327568d8b934dc47137cd";
const LARGE_ACCOUNTED_BYTES: usize = 39_105_946;
const GARBLED_NAME: &str = "1985_3_461_463_EN.pdf";
const GARBLED_SIZE: u64 = 96_242;
const GARBLED_SHA256: &str = "e7b1cec546554cd17554412be39d7aa1061119a1a3689d75af902c5f27c7d1b8";

fn fixtures_dir() -> PathBuf {
    env::var_os("PDF_EXTRACT_LIMIT_FIXTURES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".tusker/scratch/PDF-T-0017/fixtures"))
}

fn verify_fixture(path: &PathBuf, expected_size: u64, expected_sha256: &str) {
    assert_eq!(
        fs::metadata(path).expect("fixture metadata").len(),
        expected_size
    );
    let bytes = fs::read(path).expect("read fixture");
    assert_eq!(format!("{:x}", Sha256::digest(bytes)), expected_sha256);
}

fn parse_quality(path: &PathBuf) -> pdf_extract::ParseQualityMetrics {
    let results = parse_pdf(
        path.to_str().expect("UTF-8 fixture path"),
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        Some(LAParams::product_layout()),
        Some(true),
        ExtractionOptions::default(),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
    assess_parse_quality(&results)
}

#[test]
#[ignore = "requires the supplied external PDF regression fixtures"]
fn supplied_large_judgment_and_garbled_control() {
    let fixtures = fixtures_dir();
    let large = fixtures.join(LARGE_NAME);
    let garbled = fixtures.join(GARBLED_NAME);
    assert!(
        large.exists() && garbled.exists(),
        "partial fixture set in {}",
        fixtures.display()
    );
    verify_fixture(&large, LARGE_SIZE, LARGE_SHA256);
    verify_fixture(&garbled, GARBLED_SIZE, GARBLED_SHA256);

    let usage = measure_pdf_decompression(&large, 128 * 1024 * 1024)
        .expect("large judgment should fit the default content-stream budget");
    assert_eq!(usage.accounted_stream_bytes, LARGE_ACCOUNTED_BYTES);
    assert_eq!(usage.pages, 197);

    let large_quality = parse_quality(&large);
    assert_eq!(large_quality.status, ParseQualityStatus::Usable);
    let garbled_quality = parse_quality(&garbled);
    assert_eq!(garbled_quality.status, ParseQualityStatus::LikelyGarbled);
    println!(
        "large: accounted_stream_bytes={} pages={} status={:?} normalized_chars={}; garbled: status={:?} score={:.3} decode_confidence={:.3}",
        usage.accounted_stream_bytes,
        usage.pages,
        large_quality.status,
        large_quality.normalized_chars,
        garbled_quality.status,
        garbled_quality.score,
        garbled_quality.decode_confidence,
    );
}

#[test]
fn decompression_limit_is_runtime_configurable_via_serde() {
    let options: ExtractionOptions = serde_json::from_str(
        r#"{"max_decompressed_stream_bytes":268435456,"max_pages":1000,"max_objects":1000000,"max_recursion_depth":8,"max_output_bytes":134217728,"emit_output_spans":false,"enable_repairs":true}"#,
    )
    .expect("deserialize runtime extraction configuration");
    assert_eq!(
        options.max_decompressed_stream_bytes,
        Some(256 * 1024 * 1024)
    );
}
