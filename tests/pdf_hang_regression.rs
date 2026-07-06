use pdf_extract::{
    assess_parse_quality, decompress_content_ext, parse_pdf, LAParams, ParseQualityStatus,
};
use std::time::Instant;

fn parse_fixture(path: &str, min_chars: usize) {
    let mut laparams = LAParams::default();
    laparams.all_texts = true;

    let started = Instant::now();
    let docs = parse_pdf(
        path,
        1,
        "file",
        None,
        None,
        None,
        Some(350),
        Some(laparams),
        Some(true),
        Default::default(),
    )
    .unwrap_or_else(|err| panic!("parse failed for {}: {}", path, err));
    let elapsed = started.elapsed();
    let chars = docs
        .iter()
        .map(|doc| doc.content_core.content.chars().count())
        .sum::<usize>();

    assert!(!docs.is_empty(), "no chunks returned for {}", path);
    assert!(
        chars >= min_chars,
        "too little text extracted from {}: {} < {}",
        path,
        chars,
        min_chars
    );
    assert!(
        elapsed.as_secs() < 60,
        "parse exceeded regression budget for {}: {:?}",
        path,
        elapsed
    );
}

#[test]
#[ignore = "large legal PDF fixture; run for parser hardening verification"]
fn tomaso_bruno_layout_parse_does_not_spin() {
    let path = "tests/fixtures/pdf_hangs/14. Tomaso Bruno v State of UP.pdf";
    let mut laparams = LAParams::default();
    laparams.all_texts = true;

    let started = Instant::now();
    let docs = parse_pdf(
        path,
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        Some(laparams),
        Some(true),
        Default::default(),
    )
    .unwrap_or_else(|err| panic!("parse failed for {}: {}", path, err));
    let elapsed = started.elapsed();
    let text = docs
        .iter()
        .map(|doc| doc.content_core.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let metrics = assess_parse_quality(&docs);

    assert!(!docs.is_empty(), "no chunks returned for {}", path);
    assert!(
        text.chars().count() >= 40_000,
        "too little text extracted from {path}: {}",
        text.chars().count()
    );
    assert!(
        elapsed.as_secs() < 60,
        "parse exceeded regression budget for {}: {:?}",
        path,
        elapsed
    );
    assert!(text.contains("IN THE SUPREME COURT OF INDIA"));
    assert!(text.contains("This appeal is directed against the judgment"));
    assert!(text.contains("non-production of CCTV footage"));
    assert_eq!(metrics.status, ParseQualityStatus::Usable);
    assert!(
        metrics.decode_confidence > 0.90,
        "decode confidence too low: {}",
        metrics.decode_confidence
    );
    assert_eq!(metrics.mojibake_char_ratio, 0.0);
    assert!(
        !text.chars().any(is_known_mojibake_marker),
        "decoded text contains known mojibake marker"
    );

    let ext = decompress_content_ext(&docs[0].content_ext).expect("decompress content_ext");
    assert!(
        ext.get("decode_quality")
            .and_then(|quality| quality.get("confidence"))
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0)
            > 0.90,
        "missing or low per-chunk decode_quality in content_ext: {}",
        ext
    );
}

#[test]
#[ignore = "large legal PDF fixture; run for parser hardening verification"]
fn devas_antrix_layout_parse_does_not_spin() {
    parse_fixture("tests/fixtures/pdf_hangs/2. Devas v Antrix.pdf", 100_000);
}

fn is_known_mojibake_marker(ch: char) -> bool {
    matches!(
        ch,
        '\u{02d8}'
            | '\u{02c7}'
            | '\u{02c6}'
            | '\u{02d9}'
            | '\u{02dd}'
            | '\u{02db}'
            | '\u{02da}'
            | '\u{02dc}'
    )
}
