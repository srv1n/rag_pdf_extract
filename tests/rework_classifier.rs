use lopdf::{dictionary, Dictionary, Document, Object, Stream};
use pdf_extract::quality::TextQualitySignal;
use pdf_extract::{classify_pdf_from_mem, ClassifierOptions, DocumentType, OcrReason};

fn save_pdf(mut document: Document) -> Vec<u8> {
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .expect("serialize synthetic PDF");
    bytes
}

fn font_resource(document: &mut Document) -> Object {
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    font_id.into()
}

fn pdf_with_page(content: Vec<u8>, resources: Dictionary) -> Vec<u8> {
    pdf_with_page_in(Document::new(), content, resources)
}

fn pdf_with_page_in(mut document: Document, content: Vec<u8>, resources: Dictionary) -> Vec<u8> {
    let pages_id = document.new_object_id();
    let content_id = document.add_object(Stream::new(Dictionary::new(), content));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    document.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }
        .into(),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);
    save_pdf(document)
}

fn text_pdf(content: Vec<u8>) -> Vec<u8> {
    let mut document = Document::new();
    let font = font_resource(&mut document);
    pdf_with_page_in(
        document,
        content,
        dictionary! { "Font" => dictionary! { "F1" => font } },
    )
}

fn classify(bytes: &[u8]) -> pdf_extract::DocumentClassification {
    classify_pdf_from_mem(bytes, ClassifierOptions::default()).expect("classification")
}

#[test]
fn scanned_uses_image_pixels_not_page_points() {
    let mut document = Document::new();
    let image_id = document.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 1200,
            "Height" => 1600,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
        },
        Vec::new(),
    ));
    let pdf = pdf_with_page_in(
        document,
        b"q 612 0 0 792 0 0 cm /Im0 Do Q".to_vec(),
        dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
    );

    let assessment = classify(&pdf);
    let page = &assessment.pages[0];

    assert_eq!(assessment.document_type, DocumentType::Scanned);
    assert_eq!(page.ocr_reason, Some(OcrReason::Scanned));
    assert_eq!(page.image_operator_count, 1);
    assert_eq!(page.image_area, 1_920_000);
}

#[test]
fn inline_image_dimensions_are_counted_when_decoded_by_lopdf() {
    let assessment = classify(&pdf_with_page(
        b"BI /W 800 /H 800 /CS /DeviceGray /BPC 8 ID abcdef EI".to_vec(),
        Dictionary::new(),
    ));
    let page = &assessment.pages[0];

    assert_eq!(page.ocr_reason, Some(OcrReason::Scanned));
    assert_eq!(page.image_operator_count, 1, "inline image was {:?}", page);
}

#[test]
fn no_text_page_gets_no_text_reason_without_error() {
    let assessment = classify(&pdf_with_page(Vec::new(), Dictionary::new()));
    let page = &assessment.pages[0];

    assert_eq!(page.ocr_reason, Some(OcrReason::NoText));
    assert!(page.should_route_ocr());
}

#[test]
fn malformed_page_content_is_tolerant_and_routes_no_text() {
    let assessment = classify(&pdf_with_page(
        b"BT (unterminated".to_vec(),
        Dictionary::new(),
    ));
    let page = &assessment.pages[0];

    assert_eq!(page.text_operator_count, 0);
    assert_eq!(page.ocr_reason, Some(OcrReason::NoText));
}

#[test]
fn vector_text_gets_exact_vector_reason() {
    let content = (0..1_001)
        .map(|_| "0 0 m\n")
        .collect::<String>()
        .into_bytes();
    let assessment = classify(&pdf_with_page(content, Dictionary::new()));
    let page = &assessment.pages[0];

    assert_eq!(page.ocr_reason, Some(OcrReason::VectorText));
    assert!(page.should_route_ocr());
}

#[test]
fn suspected_garbled_text_comes_from_decoded_quality_path() {
    let content = b"BT /F1 12 Tf 72 720 Td ((cid:12) CID+99 uni0041 uni0042) Tj ET".to_vec();
    let assessment = classify(&text_pdf(content));
    let page = &assessment.pages[0];

    assert_eq!(page.ocr_reason, Some(OcrReason::SuspectedGarbledText));
    assert!(page
        .garble_signals
        .contains(&TextQualitySignal::BrokenCMapArtifact));
}

#[test]
fn form_xobject_text_counts_as_decoded_text_not_no_text() {
    let mut document = Document::new();
    let pages_id = document.new_object_id();
    let font = font_resource(&mut document);
    let form_stream = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 300.into(), 100.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
        },
        b"BT /F1 12 Tf 10 50 Td (Form decoded text is real) Tj ET".to_vec(),
    );
    let form_id = document.add_object(form_stream);
    let content_id = document.add_object(Stream::new(Dictionary::new(), b"q /Fm0 Do Q".to_vec()));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => dictionary! { "XObject" => dictionary! { "Fm0" => form_id } },
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    document.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }
        .into(),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);

    let assessment = classify(&save_pdf(document));
    let page = &assessment.pages[0];

    assert_eq!(assessment.document_type, DocumentType::TextBased);
    assert_eq!(page.ocr_reason, None);
    assert!(page.text_operator_count >= 1);
    assert!(page.text_char_count >= 8);
}

#[test]
fn identity_h_cjk_text_is_clean_not_suspected_garbled() {
    let mut document = Document::new();
    let cmap = r#"/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
3 beginbfchar
<0001> <672C>
<0002> <6587>
<0003> <6E05>
endbfchar
endcmap
CMapName currentdict /CMap defineresource pop
end
end"#;
    let to_unicode_id =
        document.add_object(Stream::new(Dictionary::new(), cmap.as_bytes().to_vec()));
    let font_descriptor_id = document.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => "TestIdentity",
        "Flags" => 4,
        "FontBBox" => vec![0.into(), (-200).into(), 1000.into(), 900.into()],
        "ItalicAngle" => 0,
        "Ascent" => 880,
        "Descent" => -120,
        "CapHeight" => 700,
        "StemV" => 80,
    });
    let cid_font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => "TestIdentity",
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("Identity"),
            "Supplement" => 0,
        },
        "DW" => 1000,
        "FontDescriptor" => font_descriptor_id,
    });
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "TestIdentity",
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![cid_font_id.into()],
        "ToUnicode" => to_unicode_id,
    });
    let text_codes = "<000100020003000100020003000100020003000100020003>";
    let content = format!("BT /F1 12 Tf 72 720 Td {text_codes} Tj ET").into_bytes();
    let pdf = pdf_with_page_in(
        document,
        content,
        dictionary! { "Font" => dictionary! { "F1" => font_id } },
    );

    let assessment = classify(&pdf);
    let page = &assessment.pages[0];

    assert_eq!(page.ocr_reason, None, "CJK page was {:?}", page);
    assert!(page.garble_signals.is_empty());
}

#[test]
fn accented_pdfdocencoding_text_is_clean_not_suspected_garbled() {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(b"BT /F1 12 Tf 72 720 Td <");
    for byte in b"Caf\xe9 r\xe9sum\xe9 No\xebl fa\xe7ade \xe9t\xe9 o\xf9 na\xefve co\xf6perate" {
        encoded.extend_from_slice(format!("{byte:02x}").as_bytes());
    }
    encoded.extend_from_slice(b"> Tj ET");

    let assessment = classify(&text_pdf(encoded));
    let page = &assessment.pages[0];

    assert_eq!(page.ocr_reason, None, "accented page was {:?}", page);
    assert!(page.garble_signals.is_empty());
}
