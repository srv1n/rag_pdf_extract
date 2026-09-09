//! Differential coverage for the located cleaner's Unicode-scalar offsets.
//! The reference retains the implementation from main at 281679e886b3.

use super::*;

// Keep this independent of the running output counter in the production cleaner.
fn clean_with_rescanned_offsets(located: &LocatedText) -> LocatedText {
    let chars: Vec<char> = located.text.chars().collect();
    let mut out = String::new();
    let mut spans = Vec::new();
    let mut idx = 0usize;
    let mut pending_whitespace_refs: Vec<SourceRef> = Vec::new();

    while idx < chars.len() {
        let ch = chars[idx];

        if ch == '-'
            && idx + 1 < chars.len()
            && matches!(chars[idx + 1], '\n' | '\r')
            && idx > 0
            && chars[idx - 1].is_alphabetic()
        {
            let mut next_idx = idx + 1;
            while next_idx < chars.len() && chars[next_idx].is_whitespace() {
                collect_source_refs(located, next_idx, &mut pending_whitespace_refs);
                next_idx += 1;
            }
            if next_idx < chars.len() && chars[next_idx].is_alphabetic() {
                collect_source_refs(located, idx, &mut pending_whitespace_refs);
                idx = next_idx;
                continue;
            }
        }

        if ch.is_control() && !matches!(ch, '\n' | '\t') {
            collect_source_refs(located, idx, &mut pending_whitespace_refs);
            idx += 1;
            continue;
        }

        let normalized = match ch {
            '\u{00A0}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}' | '\t' => ' ',
            other => other,
        };

        if normalized == ' ' {
            collect_source_refs(located, idx, &mut pending_whitespace_refs);
            if !out.ends_with(' ') && !out.ends_with('\n') {
                let start = out.chars().count();
                out.push(' ');
                push_clean_span(
                    &mut spans,
                    start,
                    start + 1,
                    SpanSource::Synthetic {
                        kind: SyntheticKind::InsertedWhitespace,
                        parent_refs: pending_whitespace_refs.clone(),
                    },
                );
            }
            pending_whitespace_refs.clear();
            idx += 1;
            continue;
        }

        if normalized == '\n' {
            collect_source_refs(located, idx, &mut pending_whitespace_refs);
            if !out.ends_with('\n') {
                let start = out.chars().count();
                out.push('\n');
                push_clean_span(
                    &mut spans,
                    start,
                    start + 1,
                    SpanSource::Synthetic {
                        kind: SyntheticKind::InsertedWhitespace,
                        parent_refs: pending_whitespace_refs.clone(),
                    },
                );
            }
            pending_whitespace_refs.clear();
            idx += 1;
            continue;
        }

        let start = out.chars().count();
        out.push(normalized);
        if let Some(source) = source_for_char(located, idx, normalized) {
            push_clean_span(&mut spans, start, start + 1, source);
        }
        pending_whitespace_refs.clear();
        idx += 1;
    }

    LocatedText {
        text: out,
        spans: compact_output_spans(&spans),
    }
}

fn pdf_source(index: usize, len: usize) -> SpanSource {
    SpanSource::Pdf {
        page: 1 + (index % 3) as u32,
        char_start: 100 + index,
        char_end: 100 + index + len,
        bbox: BoundingBox {
            x: 10.0 + index as f64,
            y: 20.0 + (index % 7) as f64,
            width: 12.0,
            height: 10.0,
        },
    }
}

fn assert_matches_reference(text: &str) {
    let len = text.chars().count();
    let coarse = vec![OutputSpan {
        output_start: 0,
        output_end: len,
        source: pdf_source(0, len),
    }];
    // Include unmapped characters, separate PDF boxes/pages, and synthetic parents.
    let fragmented = (0..len)
        .filter(|index| index % 5 != 4)
        .map(|index| OutputSpan {
            output_start: index,
            output_end: index + 1,
            source: if index % 3 == 1 {
                SpanSource::Synthetic {
                    kind: SyntheticKind::InsertedWhitespace,
                    parent_refs: vec![SourceRef {
                        page: 4,
                        char_start: 200 + index,
                        char_end: 201 + index,
                    }],
                }
            } else {
                pdf_source(index, 1)
            },
        })
        .collect::<Vec<_>>();

    for spans in vec![Vec::new(), coarse, fragmented] {
        let input = LocatedText {
            text: text.to_string(),
            spans,
        };
        let expected = clean_with_rescanned_offsets(&input);
        let actual = clean_located_text_for_indexing(&input);
        // No normalization: compare text, order, offsets, boxes, and every parent ref.
        assert_eq!(
            serde_json::to_value(&actual).unwrap(),
            serde_json::to_value(&expected).unwrap(),
            "input: {:?}",
            input
        );
        assert!(actual.spans.iter().all(|span| {
            span.output_start < span.output_end && span.output_end <= actual.text.chars().count()
        }));
    }
}

#[test]
fn located_cleaner_offsets_match_legacy_edge_cases() {
    for text in [
        "",
        "plain ASCII text",
        "é中🦀e\u{0301}",
        " \t  \n\n \tA  \n B\n",
        "A\u{00A0}\u{2009}\u{200A}\u{202F}\u{205F}B",
        "A\u{2000}\u{200B}\u{2028}\u{3000}B",
        "A\0\u{000B}\u{007F}\u{0085}\rB\r\nC",
        "regu-\nlation self-\ncontained",
        "é-\r\n \t中🦀-\n7 -\nstart end-",
        "---- .... ____ ====\n",
    ] {
        assert_matches_reference(text);
    }
    assert_matches_reference(&"é中🦀e\u{0301} \t\n".repeat(128));
}

#[test]
fn located_cleaner_offsets_match_legacy_generated_inputs() {
    let alphabet = [
        'a', 'Z', 'é', '中', '🦀', '\u{0301}', '-', '.', '7', ' ', '\t', '\n', '\r', '\0',
        '\u{0085}', '\u{00A0}', '\u{2009}', '\u{200A}', '\u{202F}', '\u{205F}', '\u{200B}',
        '\u{2028}',
    ];
    let mut state = 0x5eed_u64;
    for _ in 0..128 {
        let mut text = String::new();
        for _ in 0..64 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            text.push(alphabet[((state >> 32) as usize) % alphabet.len()]);
        }
        assert_matches_reference(&text);
    }
}
