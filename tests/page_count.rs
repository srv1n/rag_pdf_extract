//! `page_count` exists so a caller that got zero characters can tell "this
//! document had nothing to extract" from "this document had pages and we
//! returned nothing anyway". Only the second case is a defect worth reporting,
//! and distinguishing them is what turns a silent corpus hole into a countable
//! failure.

use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/reportlab_empty_text")
        .join(name)
}

#[test]
fn page_count_reports_pages_without_extracting_text() {
    let pages = pdf_extract::page_count(fixture("reportlab_base14_helvetica.pdf"))
        .expect("fixture should parse");

    assert_eq!(pages, 1);
}

#[test]
fn page_count_from_mem_matches_page_count_from_path() {
    let path = fixture("reportlab_embedded_vera.pdf");
    let bytes = std::fs::read(&path).expect("fixture should be readable");

    let from_path = pdf_extract::page_count(&path).expect("fixture should parse");
    let from_mem = pdf_extract::page_count_from_mem(&bytes).expect("fixture should parse");

    assert_eq!(from_path, from_mem);
    assert_eq!(from_path, 1);
}

/// The guard this function is for: pages present, text absent, so the empty
/// result is ours and not the document's. These fixtures used to extract zero
/// characters -- that is why they were committed -- and now extract text, so the
/// assertion is written the way a caller would write the check.
#[test]
fn a_document_with_pages_that_yields_text_is_not_an_empty_extraction() {
    let path = fixture("reportlab_base14_helvetica.pdf");
    let bytes = std::fs::read(&path).expect("fixture should be readable");

    let pages = pdf_extract::page_count_from_mem(&bytes).expect("fixture should parse");
    let text = pdf_extract::extract_text_from_mem(&bytes, None).expect("fixture should extract");

    assert!(pages > 0, "fixture has pages");
    assert!(
        !text.trim().is_empty(),
        "{pages}-page document returned no text; this is the silent-loss case \
         page_count() is meant to make visible"
    );
}

#[test]
fn page_count_errors_rather_than_reporting_zero_for_a_non_pdf() {
    let err = pdf_extract::page_count_from_mem(b"this is not a pdf");

    assert!(
        err.is_err(),
        "garbage input must fail loudly, not report zero pages, or the \
         zero-character guard would read it as a legitimately empty document"
    );
}
