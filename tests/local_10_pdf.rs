use pdf_extract::{parse_pdf, LAParams};
use std::path::Path;

// This test is gated on the presence of a local 10.pdf at the workspace root.
// It is intended for quick, local validation only and will be skipped in CI if the file is absent.
#[test]
fn layout_extracts_more_than_four_chunks_on_10_pdf() {
    let path = Path::new("10.pdf");
    if !path.exists() {
        eprintln!("Skipping: 10.pdf not found at repo root.");
        return;
    }

    let mut lp = LAParams::default();
    // Enable high-recall defaults to reduce surprises while comparing to pdfminer
    lp.all_texts = true;
    lp.detect_vertical = true;

    let docs = parse_pdf(
        path.to_str().unwrap(),
        1,
        "file",
        None,     // no OCR
        None,     // no OCR cache
        None,     // no resume
        Some(500),
        Some(lp),
        None,     // clean_text (default: true)
    )
    .expect("parse_pdf should succeed");

    assert!(docs.len() > 4, "Expected >4 chunks, got {}", docs.len());
}

