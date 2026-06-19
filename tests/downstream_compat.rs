use pdf_extract::{decompress_content_ext, extract_pdf_location, parse_pdf, LAParams, OcrConfig};

fn fixture(path: &str) -> String {
    format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path)
}

#[test]
fn basic_parse_pdf_example_compiles_and_runs() {
    let docs = parse_pdf(
        &fixture("eval/fixtures/pdfs/long_split_locations.pdf"),
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        None,
        Some(true),
    )
    .expect("parse fixture");

    assert!(!docs.is_empty());
    assert_eq!(docs[0].content_core.schema_version, 2);
}

#[test]
fn product_layout_parse_pdf_example_compiles_and_runs() {
    let mut laparams = LAParams::product_layout();
    laparams.all_texts = false;

    let docs = parse_pdf(
        &fixture("eval/fixtures/pdfs/nonzero_form_xobject.pdf"),
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        Some(laparams),
        Some(true),
    )
    .expect("parse layout fixture");

    assert!(!docs.is_empty());
}

#[test]
fn location_and_output_spans_are_copy_paste_usable() {
    let docs = parse_pdf(
        &fixture("eval/fixtures/pdfs/span_bbox_two_lines.pdf"),
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        None,
        Some(true),
    )
    .expect("parse fixture");
    let first = docs.first().expect("at least one chunk");

    let location = extract_pdf_location(&first.content_ext).expect("pdf location");
    assert!(!location.fragments.is_empty());
    assert!(location
        .fragments
        .iter()
        .all(|fragment| fragment.char_range.end <= first.content_core.content.chars().count()));

    let metadata = decompress_content_ext(&first.content_ext).expect("content ext");
    assert!(metadata
        .get("output_spans")
        .and_then(|value| value.as_array())
        .map(|spans| !spans.is_empty())
        .unwrap_or(false));
}

#[test]
fn repeated_content_has_distinct_chunk_id_and_content_hash_semantics() {
    let docs = parse_pdf(
        &fixture("eval/fixtures/pdfs/long_split_locations.pdf"),
        1,
        "file",
        None,
        None,
        None,
        Some(20),
        None,
        Some(true),
    )
    .expect("parse fixture");

    let first = docs.first().expect("first chunk");
    assert_ne!(first.content_core.chunk_id, first.content_core.content_hash);
    assert_eq!(first.content_core.schema_version, 2);
}

#[cfg(not(any(feature = "ocr-ocrs", feature = "ocr-tesseract")))]
#[test]
fn ocr_disabled_build_reports_disabled_runtime() {
    let config = OcrConfig {
        detection_model: None,
        recognition_model: None,
    };
    let err = match pdf_extract::OcrHandler::new(&config) {
        Ok(_) => panic!("OCR handler unexpectedly initialized without OCR feature"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("OCR support is disabled"));
}

#[cfg(any(feature = "ocr-ocrs", feature = "ocr-tesseract"))]
#[test]
fn ocr_enabled_build_accepts_ocrs_config_shape() {
    let _config = OcrConfig {
        detection_model: Some("models/text-detection.rten".to_string()),
        recognition_model: Some("models/text-recognition.rten".to_string()),
    };
}
