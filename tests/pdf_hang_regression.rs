use pdf_extract::{parse_pdf, LAParams};
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
    .unwrap_or_else(|err| panic!("parse failed for {path}: {err}"));
    let elapsed = started.elapsed();
    let chars = docs
        .iter()
        .map(|doc| doc.content_core.content.chars().count())
        .sum::<usize>();

    assert!(!docs.is_empty(), "no chunks returned for {path}");
    assert!(
        chars >= min_chars,
        "too little text extracted from {path}: {chars} < {min_chars}"
    );
    assert!(
        elapsed.as_secs() < 60,
        "parse exceeded regression budget for {path}: {elapsed:?}"
    );
}

#[test]
#[ignore = "large legal PDF fixture; run for parser hardening verification"]
fn tomaso_bruno_layout_parse_does_not_spin() {
    parse_fixture(
        "tests/fixtures/pdf_hangs/14. Tomaso Bruno v State of UP.pdf",
        4_000,
    );
}

#[test]
#[ignore = "large legal PDF fixture; run for parser hardening verification"]
fn devas_antrix_layout_parse_does_not_spin() {
    parse_fixture("tests/fixtures/pdf_hangs/2. Devas v Antrix.pdf", 100_000);
}
