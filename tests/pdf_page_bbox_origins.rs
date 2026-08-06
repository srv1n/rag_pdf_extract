use pdf_extract::{decompress_content_ext, parse_pdf, ExtractionOptions, LAParams};
use serde_json::Value;

fn fixture(name: &str) -> String {
    format!(
        "{}/tests/fixtures/pdf_page_bbox_origins/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn parse_fixture(name: &str) -> Vec<pdf_extract::ExtractionResult> {
    parse_pdf(
        &fixture(name),
        1,
        "file",
        None,
        None,
        None,
        Some(350),
        Some(LAParams::default()),
        Some(true),
        ExtractionOptions {
            emit_output_spans: true,
            ..ExtractionOptions::default()
        },
    )
    .expect("parse fixture")
}

fn assert_nonnegative_locations(docs: &[pdf_extract::ExtractionResult]) {
    for doc in docs {
        let ext = decompress_content_ext(&doc.content_ext).expect("decode content ext");

        for location in ext["chunk_locations"].as_array().expect("chunk locations") {
            for bbox in location["bboxes"].as_array().expect("bboxes") {
                assert!(bbox["x"].as_f64().unwrap() >= 0.0, "{bbox}");
                assert!(bbox["y"].as_f64().unwrap() >= 0.0, "{bbox}");
                assert!(bbox["width"].as_f64().unwrap() > 0.0, "{bbox}");
                assert!(bbox["height"].as_f64().unwrap() > 0.0, "{bbox}");
            }
        }

        for span in ext["output_spans"].as_array().expect("output spans") {
            if let Value::Object(source) = &span["source"] {
                if let Some(pdf) = source.get("Pdf") {
                    let bbox = &pdf["bbox"];
                    assert!(bbox["x"].as_f64().unwrap() >= 0.0, "{bbox}");
                    assert!(bbox["y"].as_f64().unwrap() >= 0.0, "{bbox}");
                    assert!(bbox["width"].as_f64().unwrap() > 0.0, "{bbox}");
                    assert!(bbox["height"].as_f64().unwrap() > 0.0, "{bbox}");
                }
            }
        }
    }
}

#[test]
fn page_local_negative_origins_translate_without_clamping_geometry() {
    for name in ["negative_x_origin.pdf", "negative_y_origin.pdf"] {
        let docs = parse_fixture(name);
        assert!(!docs.is_empty());
        assert!(docs
            .iter()
            .any(|doc| doc.content_core.content.contains("Page-relative bbox")));
        assert_nonnegative_locations(&docs);
    }
}

#[test]
fn zero_origin_page_coordinates_remain_unchanged() {
    let docs = parse_pdf(
        &format!(
            "{}/eval/fixtures/pdfs/ctm_scaled_text.pdf",
            env!("CARGO_MANIFEST_DIR")
        ),
        1,
        "file",
        None,
        None,
        None,
        Some(350),
        Some(LAParams::default()),
        Some(true),
        ExtractionOptions::default(),
    )
    .expect("parse origin-zero fixture");
    let ext = decompress_content_ext(&docs[0].content_ext).expect("decode content ext");
    let bbox = &ext["chunk_locations"][0]["bboxes"][0];
    assert_eq!(bbox["x"].as_f64(), Some(72.0));
    assert_eq!(bbox["y"].as_f64(), Some(672.0));
}
