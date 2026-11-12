use lopdf::{dictionary, Dictionary, Document, Object, Stream};
use pdf_extract::{extract_text_from_mem, output_doc_new_schema, ExtractionOptions, OutputError};

fn pdf_document(pages: usize) -> Document {
    let mut doc = Document::new();
    let pages_id = doc.new_object_id();
    let kids: Vec<Object> = (0..pages)
        .map(|_| {
            doc.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            })
            .into()
        })
        .collect();
    doc.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages", "Kids" => kids, "Count" => pages as i64,
        }
        .into(),
    );
    let root = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", root);
    doc
}

fn preflight(doc: &Document, options: ExtractionOptions) -> Result<(), OutputError> {
    output_doc_new_schema(doc, None, None, 1, "file", None, true, options).map(|_| ())
}

#[test]
fn identity_h_bfchar_map_with_bare_hyphenated_cmap_name_decodes_text() {
    let mut doc = pdf_document(1);
    let text = "screenwriter finds a new story";
    let mut cmap = String::from("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapName Synthetic-Font2-cmap def\n/CMapType 2 def\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n96 beginbfchar\n");
    for code in 32u16..128 {
        // Match the offset glyph IDs and tab-valued space produced by Qt/calibre.
        let unicode = if code == 32 { 9 } else { code };
        cmap.push_str(&format!("<{:04x}> <{unicode:04x}>\n", code - 29));
    }
    cmap.push_str("endbfchar\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let to_unicode = doc.add_object(Stream::new(Dictionary::new(), cmap.into_bytes()));
    let descriptor =
        doc.add_object(dictionary! { "Type" => "FontDescriptor", "FontName" => "SyntheticFont" });
    let descendant = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "SyntheticFont",
        "FontDescriptor" => descriptor, "DW" => 500,
        "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
    });
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "SyntheticFont",
        "Encoding" => "Identity-H", "DescendantFonts" => vec![descendant.into()], "ToUnicode" => to_unicode,
    });
    let codes: String = text.bytes().map(|c| format!("{:04x}", c - 29)).collect();
    let content = doc.add_object(Stream::new(
        Dictionary::new(),
        format!("BT /F1 12 Tf 72 720 Td <{codes}> Tj ET").into_bytes(),
    ));
    let page = *doc.get_pages().values().next().unwrap();
    let page = doc.get_dictionary_mut(page).unwrap();
    page.set(
        "Resources",
        dictionary! { "Font" => dictionary! { "F1" => font } },
    );
    page.set("Contents", content);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    let decoded = extract_text_from_mem(&bytes, None).expect("synthetic Type0 PDF");
    assert_eq!(
        decoded.split_whitespace().collect::<Vec<_>>().join(" "),
        text
    );
}

#[test]
fn invalid_image_dimensions_are_structure_errors() {
    let invalid = [
        None,
        Some(Object::Integer(-1)),
        Some(Object::Integer(0)),
        Some(Object::Real(1.5)),
        Some(Object::Name(b"bad".to_vec())),
    ];
    for key in ["Width", "Height"] {
        for value in &invalid {
            let mut doc = pdf_document(1);
            let mut dict = dictionary! { "Subtype" => "Image", "Width" => 16, "Height" => 16 };
            dict.remove(key.as_bytes());
            if let Some(value) = value {
                dict.set(key, value.clone());
            }
            doc.add_object(Stream::new(dict, vec![0]));
            let error =
                preflight(&doc, ExtractionOptions::default()).expect_err("invalid image dimension");
            assert!(
                matches!(error, OutputError::InvalidStructure { .. }),
                "{}={:?}: {}",
                key,
                value,
                error
            );
            assert!(!error.to_string().contains(&usize::MAX.to_string()));
        }
    }
}

#[test]
fn overflowing_image_dimensions_are_structure_errors() {
    let mut doc = pdf_document(1);
    doc.add_object(Stream::new(
        dictionary! {
            "Subtype" => "Image", "Width" => i64::MAX, "Height" => i64::MAX,
        },
        vec![0],
    ));
    let error =
        preflight(&doc, ExtractionOptions::default()).expect_err("image dimensions overflow");
    assert!(
        matches!(error, OutputError::InvalidStructure { .. }),
        "{}",
        error
    );
}

#[test]
fn default_budget_scales_for_long_books_and_explicit_limits_still_apply() {
    use std::io::Write;
    let mut doc = pdf_document(300);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let page_padding = vec![b' '; 1024 * 1024];
    for _ in 0..257 {
        encoder.write_all(&page_padding).unwrap();
    }
    doc.add_object(Stream::new(
        dictionary! { "Filter" => "FlateDecode" },
        encoder.finish().unwrap(),
    ));
    preflight(&doc, ExtractionOptions::default()).expect("257 MiB fits a 300-page book's default");
    let error = preflight(
        &doc,
        ExtractionOptions {
            max_decompressed_stream_bytes: Some(128 * 1024 * 1024),
            ..ExtractionOptions::default()
        },
    )
    .expect_err("explicit limits must not scale with pages");
    assert!(matches!(
        error,
        OutputError::ResourceLimit {
            limit: 134_217_728,
            ..
        }
    ));
}

#[test]
fn image_working_set_observed_bytes_exclude_previous_streams() {
    let mut doc = pdf_document(1);
    doc.add_object(Stream::new(Dictionary::new(), vec![0; 11]));
    doc.add_object(Stream::new(
        dictionary! {
            "Subtype" => "Image", "Width" => 100, "Height" => 100,
        },
        vec![0],
    ));
    let error = preflight(
        &doc,
        ExtractionOptions {
            max_decompressed_stream_bytes: Some(1024),
            ..ExtractionOptions::default()
        },
    )
    .expect_err("oversized image working set");
    assert!(
        matches!(
            error,
            OutputError::ResourceLimit {
                observed: Some(100_000),
                ..
            }
        ),
        "{}",
        error
    );
}
