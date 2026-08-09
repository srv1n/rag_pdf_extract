use lopdf::{
    dictionary, Dictionary, Document, EncryptionState, EncryptionVersion, Object, Permissions,
    Stream,
};
use pdf_extract::{
    estimate_page_count, extract_text_from_mem, output_doc_new_schema, EncryptionFailure,
    ExtractionOptions, OutputError, RepairKind, ResourceLimitKind,
};
use std::convert::TryFrom;
use std::io::Write;

fn minimal_pdf(page_count: usize) -> Vec<u8> {
    let mut objects = Vec::new();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids = (0..page_count)
        .map(|index| format!("{} 0 R", index + 3))
        .collect::<Vec<_>>()
        .join(" ");
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {page_count} >>").into_bytes());
    for _ in 0..page_count {
        objects.push(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec());
    }

    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Root 1 0 R /Size {} /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>] >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref_offset
        )
        .as_bytes(),
    );
    pdf
}

fn encrypted_pdf(user_password: &str) -> Vec<u8> {
    let mut document = Document::load_mem(&minimal_pdf(1)).expect("minimal PDF");
    let page_id = *document.get_pages().values().next().expect("page object");
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let content_id = document.add_object(Stream::new(
        Dictionary::new(),
        b"BT /F1 12 Tf 72 720 Td (OWNER PASSWORD TEXT) Tj ET".to_vec(),
    ));
    let page = document
        .get_dictionary_mut(page_id)
        .expect("page dictionary");
    page.set("Contents", content_id);
    page.set(
        "Resources",
        dictionary! { "Font" => dictionary! { "F1" => font_id } },
    );
    let root = document
        .trailer
        .get(b"Root")
        .expect("root")
        .as_reference()
        .expect("root reference");
    document.get_object(root).expect("load root");
    let version = EncryptionVersion::V1 {
        document: &document,
        owner_password: "owner-password",
        user_password,
        permissions: Permissions::all(),
    };
    let state = EncryptionState::try_from(version).expect("encryption state");
    document.encrypt(&state).expect("encrypt fixture");
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .expect("serialize encrypted PDF");
    bytes
}

fn mutate_encryption_dictionary(
    bytes: &[u8],
    mutation: impl FnOnce(&mut lopdf::Dictionary),
) -> Vec<u8> {
    let mut document = Document::load_mem(bytes).expect("encrypted fixture should parse");
    let encryption_id = document
        .trailer
        .get(b"Encrypt")
        .expect("encrypt reference")
        .as_reference()
        .expect("encrypt object reference");
    let encryption = document
        .get_object_mut(encryption_id)
        .expect("encryption object")
        .as_dict_mut()
        .expect("encryption dictionary");
    mutation(encryption);
    let mut mutated = Vec::new();
    document
        .save_to(&mut mutated)
        .expect("serialize mutated PDF");
    mutated
}

#[test]
fn byte_page_estimator_excludes_pages_node() {
    let bytes = b"<< /Type /Pages >> << /Type\n/Page >> << /Type /PageX >>";
    assert_eq!(estimate_page_count(bytes), 1);
}

#[test]
fn non_pdf_error_is_typed_and_carries_page_estimate() {
    let error = extract_text_from_mem(b"garbage /Type /Page", None).expect_err("not a PDF");
    assert!(matches!(error, OutputError::NotAPdf { .. }));
    assert_eq!(error.context().estimated_pages, Some(1));
    assert!(error.to_string().contains("estimated_pages=1"));
}

#[test]
fn page_budget_is_checked_before_extraction() {
    let document = Document::load_mem(&minimal_pdf(2)).expect("minimal PDF");
    let error = output_doc_new_schema(
        &document,
        None,
        None,
        1,
        "file",
        None,
        true,
        ExtractionOptions {
            max_pages: Some(1),
            ..ExtractionOptions::default()
        },
    )
    .expect_err("page budget");

    assert!(matches!(
        &error,
        OutputError::ResourceLimit {
            kind: ResourceLimitKind::Pages,
            limit: 1,
            observed: Some(2),
            ..
        }
    ));
}

#[test]
fn object_budget_is_checked_before_extraction() {
    let document = Document::load_mem(&minimal_pdf(2)).expect("minimal PDF");
    let error = output_doc_new_schema(
        &document,
        None,
        None,
        1,
        "file",
        None,
        true,
        ExtractionOptions {
            max_objects: Some(1),
            ..ExtractionOptions::default()
        },
    )
    .expect_err("object budget");

    assert!(matches!(
        &error,
        OutputError::ResourceLimit {
            kind: ResourceLimitKind::Objects,
            limit: 1,
            observed: Some(4),
            ..
        }
    ));
}

#[test]
fn recursion_budget_skips_nested_stream_without_aborting_document() {
    let document =
        Document::load("eval/fixtures/pdfs/nonzero_form_xobject.pdf").expect("form fixture");
    let _result = output_doc_new_schema(
        &document,
        None,
        None,
        1,
        "file",
        None,
        true,
        ExtractionOptions {
            max_recursion_depth: Some(0),
            ..ExtractionOptions::default()
        },
    )
    .expect("nested form should be skipped without aborting extraction");
}

#[test]
fn decompressed_stream_budget_rejects_before_output() {
    let mut document = Document::new();
    let mut compressed = Vec::new();
    let mut encoder = flate2::write::ZlibEncoder::new(&mut compressed, flate2::Compression::best());
    encoder
        .write_all(&vec![0u8; 64 * 1024])
        .expect("compress bomb fixture");
    encoder.finish().expect("finish bomb fixture");
    assert!(
        compressed.len() < 256,
        "fixture should be genuinely compressed"
    );
    let mut stream = Stream::new(Dictionary::new(), compressed);
    stream
        .dict
        .set("Filter", Object::Name(b"FlateDecode".to_vec()));
    document.add_object(Object::Stream(stream));
    let error = output_doc_new_schema(
        &document,
        None,
        None,
        1,
        "file",
        None,
        true,
        ExtractionOptions {
            max_pages: None,
            max_objects: None,
            max_decompressed_stream_bytes: Some(128),
            ..ExtractionOptions::default()
        },
    )
    .expect_err("decompression budget");
    assert!(matches!(
        error,
        OutputError::ResourceLimit {
            kind: ResourceLimitKind::DecompressedStreamBytes,
            limit: 128,
            observed: Some(observed),
            ..
        } if observed > 128
    ));
}

#[test]
fn decompressed_stream_budget_covers_ascii85_filter_expansion() {
    let mut document = Document::new();
    let mut stream = Stream::new(Dictionary::new(), b"zz".to_vec());
    stream
        .dict
        .set("Filter", Object::Name(b"ASCII85Decode".to_vec()));
    document.add_object(Object::Stream(stream));

    let error = output_doc_new_schema(
        &document,
        None,
        None,
        1,
        "file",
        None,
        true,
        ExtractionOptions {
            max_pages: None,
            max_objects: None,
            max_decompressed_stream_bytes: Some(4),
            ..ExtractionOptions::default()
        },
    )
    .expect_err("ASCII85 expansion should be budgeted");
    assert!(matches!(
        error,
        OutputError::ResourceLimit {
            kind: ResourceLimitKind::DecompressedStreamBytes,
            limit: 4,
            observed: Some(observed),
            ..
        } if observed > 4
    ));
}

#[test]
fn decompressed_stream_budget_charges_image_xobjects_at_stored_size() {
    let mut document = Document::new();
    let mut compressed = Vec::new();
    let mut encoder = flate2::write::ZlibEncoder::new(&mut compressed, flate2::Compression::best());
    encoder
        .write_all(&vec![0u8; 64 * 1024])
        .expect("compress image fixture");
    encoder.finish().expect("finish image fixture");
    let stored_len = compressed.len();
    let mut stream = Stream::new(Dictionary::new(), compressed);
    stream
        .dict
        .set("Filter", Object::Name(b"FlateDecode".to_vec()));
    stream.dict.set("Subtype", Object::Name(b"Image".to_vec()));
    stream.dict.set("Width", Object::Integer(256));
    stream.dict.set("Height", Object::Integer(256));
    document.add_object(Object::Stream(stream));

    output_doc_new_schema(
        &document,
        None,
        None,
        1,
        "file",
        None,
        true,
        ExtractionOptions {
            max_pages: None,
            max_objects: None,
            max_decompressed_stream_bytes: Some(stored_len),
            ..ExtractionOptions::default()
        },
    )
    .expect("raw image pixels must not be charged as simultaneous stream memory");
}

#[test]
fn resource_limit_errors_expose_stable_reason_codes() {
    let error =
        OutputError::resource_limit(ResourceLimitKind::DecompressedStreamBytes, 128, Some(129));
    assert_eq!(
        error.reason_code(),
        "pdf_resource_limit_decompressed_stream_bytes"
    );
}

#[test]
fn encrypted_pdf_owner_only_auto_opens_and_bad_handlers_are_distinct() {
    let owner_only = extract_text_from_mem(&encrypted_pdf(""), None)
        .expect("owner-only PDFs with an empty user password should auto-open");
    assert!(owner_only.contains("OWNER PASSWORD TEXT"));

    let unsupported = mutate_encryption_dictionary(&encrypted_pdf("secret"), |dictionary| {
        dictionary.set("Filter", Object::Name(b"Foo".to_vec()));
    });
    let unsupported_error =
        extract_text_from_mem(&unsupported, None).expect_err("unsupported handler");
    assert!(
        matches!(
            &unsupported_error,
            OutputError::Encrypted {
                reason: EncryptionFailure::UnsupportedHandler,
                ..
            }
        ),
        "unexpected unsupported-handler error: {:?}",
        unsupported_error
    );

    let corrupt = mutate_encryption_dictionary(&encrypted_pdf("secret"), |dictionary| {
        dictionary.remove(b"O");
    });
    let corrupt_error =
        extract_text_from_mem(&corrupt, None).expect_err("corrupt encryption dictionary");
    assert!(
        matches!(
            &corrupt_error,
            OutputError::Encrypted {
                reason: EncryptionFailure::InvalidDictionary,
                ..
            }
        ),
        "unexpected corrupt-dictionary error: {:?}",
        corrupt_error
    );
}

#[test]
fn password_is_redacted_from_options_debug_and_errors() {
    let options = ExtractionOptions {
        password: Some("super-secret".into()),
        ..ExtractionOptions::default()
    };
    assert!(!format!("{options:?}").contains("super-secret"));
    let path = std::env::temp_dir().join(format!(
        "pdf-extract-password-redaction-{}.pdf",
        std::process::id()
    ));
    std::fs::write(&path, encrypted_pdf("correct-password")).expect("write encrypted fixture");
    let error = pdf_extract::parse_pdf(
        path.to_str().expect("temporary path"),
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        None,
        Some(true),
        ExtractionOptions {
            password: Some("super-secret".into()),
            ..ExtractionOptions::default()
        },
    )
    .expect_err("wrong password should fail");
    std::fs::remove_file(&path).expect("remove encrypted fixture");
    assert!(matches!(
        &error,
        OutputError::Encrypted {
            reason: EncryptionFailure::IncorrectPassword,
            ..
        }
    ));
    assert!(!format!("{error}").contains("super-secret"));
}

#[test]
fn encrypted_pdf_has_distinct_missing_wrong_and_correct_password_outcomes() {
    let encrypted = encrypted_pdf("secret");
    let path =
        std::env::temp_dir().join(format!("pdf-extract-encrypted-{}.pdf", std::process::id()));
    std::fs::write(&path, encrypted).expect("write encrypted fixture");

    let missing = pdf_extract::parse_pdf(
        path.to_str().expect("temporary path"),
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        None,
        Some(true),
        ExtractionOptions::default(),
    )
    .expect_err("password should be required");
    assert!(matches!(
        missing,
        OutputError::Encrypted {
            reason: EncryptionFailure::PasswordRequired,
            ..
        }
    ));

    let wrong = pdf_extract::parse_pdf(
        path.to_str().expect("temporary path"),
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        None,
        Some(true),
        ExtractionOptions {
            password: Some("wrong".into()),
            ..ExtractionOptions::default()
        },
    )
    .expect_err("wrong password should fail");
    assert!(matches!(
        wrong,
        OutputError::Encrypted {
            reason: EncryptionFailure::IncorrectPassword,
            ..
        }
    ));

    let correct = pdf_extract::parse_pdf(
        path.to_str().expect("temporary path"),
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        None,
        Some(true),
        ExtractionOptions {
            password: Some("secret".into()),
            ..ExtractionOptions::default()
        },
    );
    std::fs::remove_file(&path).expect("remove encrypted fixture");
    assert!(
        correct.is_ok(),
        "correct password should decrypt: {:?}",
        correct
    );
}

#[test]
fn malformed_container_repairs_are_recorded_on_results() {
    let source = std::fs::read("eval/fixtures/pdfs/long_split_locations.pdf").expect("fixture");
    let eof = source
        .windows(5)
        .position(|window| window == b"%%EOF")
        .expect("fixture EOF");
    let mut malformed = b"garbage before header\n".to_vec();
    malformed.extend_from_slice(&source[..eof]);
    let mut eof_repaired = source[..eof].to_vec();
    eof_repaired.extend_from_slice(b"%%EOF\n");
    Document::load_mem(&eof_repaired).expect("EOF-only repair should be parseable");
    let path = std::env::temp_dir().join(format!("pdf-extract-repair-{}.pdf", std::process::id()));
    std::fs::write(&path, malformed).expect("write temporary malformed PDF");
    let result = pdf_extract::parse_pdf(
        path.to_str().expect("temporary path"),
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        None,
        Some(true),
        ExtractionOptions::default(),
    )
    .expect("bounded repairs should recover fixture");
    std::fs::remove_file(&path).expect("remove temporary fixture");

    let repairs = &result.first().expect("extracted result").repairs;
    assert!(repairs.contains(&RepairKind::HeaderGarbageStripped));
    assert!(repairs.contains(&RepairKind::TruncatedEofRepaired));
}
