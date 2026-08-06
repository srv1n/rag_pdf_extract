use pdf_extract::{
    decompress_content_ext, decompress_content_ext_bytes, extract_chunk_locations,
    extract_pdf_location, ContentExt, ExtractionOptions, OutputError, PdfPassword,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

fn assert_configuration_traits<T: Clone + PartialEq + Eq + Serialize + DeserializeOwned>() {}

#[test]
fn extraction_options_retain_serializable_equality_surface_and_redact_passwords() {
    assert_configuration_traits::<ExtractionOptions>();

    let options = ExtractionOptions {
        password: Some(PdfPassword::new("do-not-leak")),
        ..ExtractionOptions::default()
    };
    let debug = format!("{options:?}");
    let display = options.password.as_ref().map(ToString::to_string).unwrap();
    let json = serde_json::to_string(&options).expect("serialize options");
    let decoded: ExtractionOptions = serde_json::from_str(&json).expect("deserialize options");

    assert!(!debug.contains("do-not-leak"));
    assert!(!display.contains("do-not-leak"));
    assert!(!json.contains("do-not-leak"));
    assert!(
        decoded.password.is_none(),
        "password must remain runtime-only"
    );
}

#[test]
fn content_ext_helpers_return_typed_structure_errors() {
    let invalid_compression = ContentExt {
        chunk_id: "test".to_string(),
        ext_json: b"not-zstd".to_vec(),
    };

    let error = decompress_content_ext_bytes(&invalid_compression)
        .expect_err("invalid ContentExt compression should fail");
    assert!(matches!(error, OutputError::InvalidStructure { .. }));

    let json_bytes = zstd::bulk::compress(b"{}", 3).expect("compress empty object");
    let empty_payload = ContentExt {
        chunk_id: "test".to_string(),
        ext_json: json_bytes,
    };
    assert!(matches!(
        decompress_content_ext(&empty_payload),
        Ok(value) if value == serde_json::json!({})
    ));
    assert!(matches!(
        extract_chunk_locations(&empty_payload),
        Err(OutputError::InvalidStructure { .. })
    ));
    assert!(matches!(
        extract_pdf_location(&empty_payload),
        Err(OutputError::InvalidStructure { .. })
    ));
}
