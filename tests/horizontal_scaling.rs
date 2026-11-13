use lopdf::{dictionary, Dictionary, Document, Stream};
use pdf_extract::{extract_pdf_location, output_doc_new_schema, ExtractionOptions, LAParams};

fn extract(scale: i32, show: &str) -> Vec<pdf_extract::ExtractionResult> {
    let mut doc = Document::new();
    let pages = doc.new_object_id();
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let content = doc.add_object(Stream::new(
        Dictionary::new(),
        format!("BT /F1 12 Tf {scale} Tz 1 0 0 1 300 400 Tm {show} ET").into_bytes(),
    ));
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 1000.into(), 842.into()],
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
    output_doc_new_schema(
        &doc,
        None,
        Some(350),
        1,
        "file",
        Some(&LAParams::default()),
        true,
        ExtractionOptions::default(),
    )
    .expect("synthetic horizontally scaled PDF")
}

#[test]
fn all_text_show_operators_keep_horizontally_scaled_words_together() {
    for scale in [50, 200, 400] {
        for show in [
            "(Wide words) Tj",
            "[(Wide words)] TJ",
            "(Wide words) '",
            "0 0 (Wide words) \"",
        ] {
            let results = extract(scale, show);
            let text = results
                .iter()
                .map(|result| result.content_core.content.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(text.trim(), "Wide words", "{scale} Tz {show}");
        }
    }
}

#[test]
fn negative_horizontal_scale_keeps_positive_bounds_on_the_left_of_origin() {
    for show in ["(W) Tj", "[(W)] TJ", "(W) '", "0 0 (W) \""] {
        let results = extract(-200, show);
        assert_eq!(
            results[0]
                .content_core
                .content
                .trim()
                .trim_start_matches("## "),
            "W"
        );
        let location = extract_pdf_location(&results[0].content_ext).unwrap();
        let bbox = &location.fragments[0].bbox;
        // Helvetica W is 944/1000 em: 12pt * 2 * 0.944 = 22.656pt.
        assert!((bbox.x - 277.344).abs() < 0.01, "{show}: {bbox:?}");
        assert!((bbox.width - 22.656).abs() < 0.01, "{show}: {bbox:?}");
        assert!((bbox.height - 12.0).abs() < 0.01, "{show}: {bbox:?}");
    }
}
