//! Differential checks against the metadata serializer before this optimization.

use super::*;
use crate::document::{LocatedText, OutputSpan, SourceRef, SpanSource, SyntheticKind};

fn legacy_create_content_ext_with_spans(
    chunk_id: &str,
    format_location: &FormatLocation,
    page_char_start: Option<usize>,
    page_char_end: Option<usize>,
    bbox: Option<&BoundingBox>,
    located_text: Option<&LocatedText>,
    options: ExtractionOptions,
) -> Result<ContentExt, OutputError> {
    #[derive(serde::Serialize)]
    struct ContentExtPayload<'a> {
        format_location: &'a FormatLocation,
        page_char_start: Option<usize>,
        page_char_end: Option<usize>,
        bbox: Option<&'a BoundingBox>,
        chunk_locations: &'a [ChunkLocation],
        #[serde(skip_serializing_if = "Option::is_none")]
        output_spans: Option<&'a [OutputSpan]>,
        output_text_len: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        decode_quality: Option<&'a DecodeQualityMetrics>,
        location_model: ContentExtLocationModel<'a>,
    }
    #[derive(serde::Serialize)]
    struct ContentExtLocationModel<'a> {
        source_span_granularity: &'a str,
        synthetic_spans: &'a str,
        notes: &'a str,
    }

    let compacted_spans = located_text.map(|located| {
        if located.spans.is_empty() {
            Vec::new()
        } else {
            crate::document::compact_output_spans(&located.spans)
        }
    });
    let chunk_locations = located_text
        .map(|located| crate::document::chunk_locations_from_output_spans(&located.spans))
        .unwrap_or_default();
    let output_spans = if options.emit_output_spans {
        compacted_spans.as_deref()
    } else {
        None
    };
    let source_span_granularity = if options.emit_output_spans && located_text.is_some() {
        "output_span"
    } else if located_text.is_some() {
        "chunk_location"
    } else {
        "segment"
    };
    let synthetic_spans = if options.emit_output_spans && located_text.is_some() {
        "typed"
    } else if located_text.is_some() {
        "omitted"
    } else {
        "coarse"
    };
    let notes = if options.emit_output_spans {
        "chunk_locations summarize PDF-backed fragments; output_spans preserve PDF-backed fragments plus typed synthetic spans"
    } else {
        "chunk_locations summarize PDF-backed fragments; synthetic spans do not contribute bboxes"
    };
    let decode_quality = located_text.map(|located| assess_decode_quality(&located.text));
    let ext_data = ContentExtPayload {
        format_location,
        page_char_start,
        page_char_end,
        bbox,
        chunk_locations: &chunk_locations,
        output_spans,
        output_text_len: located_text.map(|located| located.text.chars().count()),
        decode_quality: decode_quality.as_ref(),
        location_model: ContentExtLocationModel {
            source_span_granularity,
            synthetic_spans,
            notes,
        },
    };
    let json_bytes = serde_json::to_vec(&ext_data).map_err(|error| OutputError::Format {
        message: error.to_string(),
        context: ErrorContext::default(),
    })?;
    Ok(ContentExt {
        chunk_id: chunk_id.to_string(),
        ext_json: zstd::bulk::compress(&json_bytes, 3)?,
    })
}

#[test]
fn complete_metadata_matches_legacy_with_spans_off_and_on() {
    let bbox = BoundingBox {
        x: 10.0,
        y: 20.0,
        width: 100.0,
        height: 12.0,
    };
    let mixed = LocatedText {
        text: "ab  中".to_string(),
        spans: vec![
            OutputSpan {
                output_start: 0,
                output_end: 1,
                source: SpanSource::Pdf {
                    page: 1,
                    char_start: 100,
                    char_end: 101,
                    bbox: bbox.clone(),
                },
            },
            OutputSpan {
                output_start: 1,
                output_end: 2,
                source: SpanSource::Pdf {
                    page: 1,
                    char_start: 101,
                    char_end: 102,
                    bbox: bbox.clone(),
                },
            },
            OutputSpan {
                output_start: 2,
                output_end: 3,
                source: SpanSource::Synthetic {
                    kind: SyntheticKind::InsertedWhitespace,
                    parent_refs: vec![SourceRef {
                        page: 1,
                        char_start: 102,
                        char_end: 103,
                    }],
                },
            },
            OutputSpan {
                output_start: 3,
                output_end: 4,
                source: SpanSource::Synthetic {
                    kind: SyntheticKind::InsertedWhitespace,
                    parent_refs: vec![SourceRef {
                        page: 2,
                        char_start: 199,
                        char_end: 200,
                    }],
                },
            },
            OutputSpan {
                output_start: 4,
                output_end: 5,
                source: SpanSource::Pdf {
                    page: 2,
                    char_start: 200,
                    char_end: 201,
                    bbox: BoundingBox {
                        y: 60.0,
                        ..bbox.clone()
                    },
                },
            },
        ],
    };
    let synthetic_only = LocatedText {
        text: " ".to_string(),
        spans: vec![OutputSpan {
            output_start: 0,
            output_end: 1,
            source: SpanSource::Synthetic {
                kind: SyntheticKind::HeadingMarker,
                parent_refs: vec![SourceRef {
                    page: 1,
                    char_start: 300,
                    char_end: 301,
                }],
            },
        }],
    };
    let location = create_pdf_location_from_output_spans(&mixed, &[]);
    let cases = vec![
        None,
        Some(LocatedText::empty(String::new())),
        Some(LocatedText::empty("unlocated é中".to_string())),
        Some(synthetic_only),
        Some(mixed),
    ];
    assert!(!ExtractionOptions::default().emit_output_spans);

    for located in &cases {
        let before = serde_json::to_value(located).unwrap();
        for emit_output_spans in [false, true] {
            let options = ExtractionOptions {
                emit_output_spans,
                ..ExtractionOptions::default()
            };
            let expected = legacy_create_content_ext_with_spans(
                "stable-test-chunk",
                &location,
                Some(100),
                Some(201),
                Some(&bbox),
                located.as_ref(),
                options.clone(),
            )
            .unwrap();
            let actual = create_content_ext_with_spans(
                "stable-test-chunk",
                &location,
                Some(100),
                Some(201),
                Some(&bbox),
                located.as_ref(),
                options,
            )
            .unwrap();
            assert_eq!(actual.chunk_id, expected.chunk_id);
            assert_eq!(actual.ext_json, expected.ext_json);
            let actual_bytes = zstd::bulk::decompress(&actual.ext_json, 16 * 1024 * 1024).unwrap();
            let expected_bytes =
                zstd::bulk::decompress(&expected.ext_json, 16 * 1024 * 1024).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&actual_bytes).unwrap(),
                serde_json::from_slice::<serde_json::Value>(&expected_bytes).unwrap()
            );
            assert_eq!(serde_json::to_value(located).unwrap(), before);
        }
    }
}
