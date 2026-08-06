use lopdf::{dictionary, Dictionary, Document, Stream};
use pdf_extract::{
    classify_pdf, classify_pdf_from_mem, ClassifierOptions, DocumentType, OcrReason,
};

fn synthetic_pdf(content: Vec<u8>) -> Vec<u8> {
    let mut document = Document::new();
    let pages_id = document.new_object_id();
    let content_id = document.add_object(Stream::new(Dictionary::new(), content));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
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
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .expect("serialize synthetic PDF");
    bytes
}

#[test]
fn classifier_samples_pages_and_exposes_one_ocr_reason_per_page() {
    let assessment = classify_pdf(
        "eval/fixtures/pdfs/long_split_locations.pdf",
        ClassifierOptions {
            max_sample_pages: 2,
        },
    )
    .expect("fixture classification");

    assert!(assessment.sampled_pages >= 1);
    assert!(assessment.sampled_pages <= 2);
    assert!(assessment.total_pages >= assessment.sampled_pages);
    assert!((0.0..=1.0).contains(&assessment.confidence));
    assert!(matches!(
        assessment.document_type,
        DocumentType::TextBased
            | DocumentType::Mixed
            | DocumentType::ImageBased
            | DocumentType::Scanned
    ));
    for page in assessment.pages {
        assert!(page.sampled);
        assert_eq!(page.needs_ocr, page.should_route_ocr());
        if page.needs_ocr {
            assert!(page.ocr_reason_code().is_some());
        }
    }
}

#[test]
fn vector_outlined_text_gets_a_vector_reason() {
    let content = (0..1_001)
        .map(|_| "0 0 m\n")
        .collect::<String>()
        .into_bytes();
    let assessment = classify_pdf_from_mem(&synthetic_pdf(content), ClassifierOptions::default())
        .expect("vector fixture classification");
    assert_eq!(assessment.pages[0].ocr_reason, Some(OcrReason::VectorText));
    assert!(assessment.pages[0].should_route_ocr());
}

#[test]
fn garbled_text_quality_feeds_the_classifier_reason() {
    let replacement = "���";
    let content = format!("BT\n({replacement}) Tj\n({replacement}) Tj\n({replacement}) Tj\nET\n")
        .into_bytes();
    let assessment = classify_pdf_from_mem(&synthetic_pdf(content), ClassifierOptions::default())
        .expect("garbled fixture classification");
    assert_eq!(
        assessment.pages[0].ocr_reason,
        Some(OcrReason::SuspectedGarbledText)
    );
    assert!(assessment.pages[0].should_route_ocr());
}
