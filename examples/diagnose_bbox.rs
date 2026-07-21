use pdf_extract::{decompress_content_ext, parse_pdf, ExtractionOptions, LAParams};
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in env::args().skip(1) {
        let docs = parse_pdf(
            &path,
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
            },
        )?;
        let mut count = 0usize;
        let mut invalid_bboxes = 0usize;
        let mut negative_x = 0usize;
        let mut negative_y = 0usize;
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut content_chars = 0usize;
        let mut content_bytes = 0usize;
        let mut output_text_len = 0usize;
        let mut output_spans = 0usize;
        let mut invalid_output_spans = 0usize;
        let mut max_output_end = 0usize;
        let mut negative_samples = Vec::new();

        for doc in &docs {
            let doc_content_chars = doc.content_core.content.chars().count();
            content_chars += doc_content_chars;
            content_bytes += doc.content_core.content.len();
            let value = decompress_content_ext(&doc.content_ext)?;
            output_text_len += value
                .get("output_text_len")
                .and_then(|v| v.as_u64())
                .unwrap_or_default() as usize;
            if let Some(spans) = value.get("output_spans").and_then(|v| v.as_array()) {
                output_spans += spans.len();
                for span in spans {
                    let start = span
                        .get("output_start")
                        .and_then(|v| v.as_u64())
                        .unwrap_or_default() as usize;
                    let end = span
                        .get("output_end")
                        .and_then(|v| v.as_u64())
                        .unwrap_or_default() as usize;
                    max_output_end = max_output_end.max(end);
                    invalid_output_spans += usize::from(start >= end || end > doc_content_chars);
                }
            }
            if let Some(locations) = value.get("chunk_locations").and_then(|v| v.as_array()) {
                for location in locations {
                    if let Some(bboxes) = location.get("bboxes").and_then(|v| v.as_array()) {
                        for bbox in bboxes {
                            let x = bbox.get("x").and_then(|v| v.as_f64()).unwrap();
                            let y = bbox.get("y").and_then(|v| v.as_f64()).unwrap();
                            let width = bbox
                                .get("width")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(f64::NAN);
                            let height = bbox
                                .get("height")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(f64::NAN);
                            invalid_bboxes += usize::from(
                                !x.is_finite()
                                    || !y.is_finite()
                                    || !width.is_finite()
                                    || !height.is_finite()
                                    || width <= 0.0
                                    || height <= 0.0,
                            );
                            count += 1;
                            negative_x += usize::from(x < 0.0);
                            negative_y += usize::from(y < 0.0);
                            if (x < 0.0 || y < 0.0) && negative_samples.len() < 8 {
                                negative_samples.push(format!(
                                    "page={} x={x:.3} y={y:.3} w={:.3} h={:.3}",
                                    location
                                        .get("page")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or_default(),
                                    bbox.get("width")
                                        .and_then(|v| v.as_f64())
                                        .unwrap_or_default(),
                                    bbox.get("height")
                                        .and_then(|v| v.as_f64())
                                        .unwrap_or_default(),
                                ));
                            }
                            min_x = min_x.min(x);
                            min_y = min_y.min(y);
                            max_x = max_x.max(x);
                            max_y = max_y.max(y);
                        }
                    }
                }
            }
        }

        println!("{path}");
        println!(
            "  chunks={} bboxes={count} invalid_bboxes={invalid_bboxes} negative_x={negative_x} negative_y={negative_y} min_x={min_x:.3} min_y={min_y:.3} max_x={max_x:.3} max_y={max_y:.3}",
            docs.len(),
        );
        println!(
            "  content_chars={content_chars} content_bytes={content_bytes} output_text_len={output_text_len} output_spans={output_spans} max_output_end={max_output_end} invalid_output_spans={invalid_output_spans}"
        );
        if !negative_samples.is_empty() {
            println!("  negative_samples={}", negative_samples.join("; "));
        }
    }
    Ok(())
}
