use lopdf::{dictionary, Dictionary, Document, Stream};
use pdf_extract::{extract_pdf_location, output_doc_new_schema, ExtractionOptions, LAParams};

#[test]
fn all_text_show_operators_apply_text_matrix_before_ctm() {
    for show in [
        "(Quartz words) Tj",
        "[(Quartz words)] TJ",
        "(Quartz words) '",
        "0 0 (Quartz words) \"",
    ] {
        let mut doc = Document::new();
        let pages = doc.new_object_id();
        let font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        });
        // Quartz Word PDFs put scale in Tm and translation/scale in the CTM.
        let content = doc.add_object(Stream::new(
            Dictionary::new(),
            format!("q 0.24 0 0 0.24 9 635.73 cm BT /F1 1 Tf 50 0 0 50 300 400 Tm {show} ET Q")
                .into_bytes(),
        ));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
        });
        doc.objects.insert(
            pages,
            dictionary! {
                "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1,
            }
            .into(),
        );
        let root = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", root);
        let results = output_doc_new_schema(
            &doc,
            None,
            Some(350),
            1,
            "file",
            Some(&LAParams::default()),
            true,
            ExtractionOptions::default(),
        )
        .expect("Quartz-style synthetic PDF");
        let text = results
            .iter()
            .map(|result| result.content_core.content.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Quartz words"), "{show}: {text}");
        let location = extract_pdf_location(&results[0].content_ext).unwrap();
        let bbox = &location.fragments[0].bbox;
        assert!((bbox.x - 81.0).abs() < 1.0, "{show}: {bbox:?}");
        assert!((bbox.y - 110.27).abs() < 1.0, "{show}: {bbox:?}");
        assert!((bbox.height - 12.0).abs() < 1.0, "{show}: {bbox:?}");
    }
}
