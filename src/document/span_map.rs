use crate::BoundingBox;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocatedText {
    pub text: String,
    pub spans: Vec<OutputSpan>,
}

impl LocatedText {
    pub fn empty(text: String) -> Self {
        Self {
            text,
            spans: Vec::new(),
        }
    }

    pub fn slice_chars(&self, start: usize, len: usize, text: String) -> Self {
        let end = start.saturating_add(len);
        let mut spans = Vec::new();

        for span in &self.spans {
            let overlap_start = span.output_start.max(start);
            let overlap_end = span.output_end.min(end);
            if overlap_start >= overlap_end {
                continue;
            }

            let mut clipped = span.clone();
            clipped.output_start = overlap_start - start;
            clipped.output_end = overlap_end - start;
            clipped.source = span.source.clip_output_overlap(
                span.output_start,
                span.output_end,
                overlap_start,
                overlap_end,
            );
            spans.push(clipped);
        }

        Self { text, spans }
    }
}

pub fn compact_output_spans(spans: &[OutputSpan]) -> Vec<OutputSpan> {
    let mut compacted: Vec<OutputSpan> = Vec::with_capacity(spans.len());
    for span in spans {
        if let Some(last) = compacted.last_mut() {
            if try_merge_output_span(last, span) {
                continue;
            }
        }
        compacted.push(span.clone());
    }
    compacted
}

pub fn try_merge_output_span(last: &mut OutputSpan, next: &OutputSpan) -> bool {
    if last.output_end != next.output_start {
        return false;
    }

    match (&mut last.source, &next.source) {
        (
            SpanSource::Pdf {
                page: last_page,
                char_end: last_char_end,
                bbox: last_bbox,
                ..
            },
            SpanSource::Pdf {
                page: next_page,
                char_start: next_char_start,
                char_end: next_char_end,
                bbox: next_bbox,
            },
        ) if last_page == next_page
            && *last_char_end == *next_char_start
            && same_bbox(last_bbox, next_bbox) =>
        {
            last.output_end = next.output_end;
            *last_char_end = *next_char_end;
            true
        }
        (
            SpanSource::Synthetic {
                kind: last_kind,
                parent_refs: last_refs,
            },
            SpanSource::Synthetic {
                kind: next_kind,
                parent_refs: next_refs,
            },
        ) if last_kind == next_kind => {
            last.output_end = next.output_end;
            extend_unique_source_refs(last_refs, next_refs);
            true
        }
        _ => false,
    }
}

fn same_bbox(a: &BoundingBox, b: &BoundingBox) -> bool {
    const EPSILON: f64 = 1e-6;
    (a.x - b.x).abs() <= EPSILON
        && (a.y - b.y).abs() <= EPSILON
        && (a.width - b.width).abs() <= EPSILON
        && (a.height - b.height).abs() <= EPSILON
}

fn extend_unique_source_refs(target: &mut Vec<SourceRef>, refs: &[SourceRef]) {
    for source_ref in refs {
        if !target.iter().any(|existing| {
            existing.page == source_ref.page
                && existing.char_start == source_ref.char_start
                && existing.char_end == source_ref.char_end
        }) {
            target.push(source_ref.clone());
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputSpan {
    pub output_start: usize,
    pub output_end: usize,
    pub source: SpanSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SpanSource {
    Pdf {
        page: u32,
        char_start: usize,
        char_end: usize,
        bbox: BoundingBox,
    },
    Synthetic {
        kind: SyntheticKind,
        parent_refs: Vec<SourceRef>,
    },
}

impl SpanSource {
    fn clip_output_overlap(
        &self,
        span_output_start: usize,
        span_output_end: usize,
        overlap_start: usize,
        overlap_end: usize,
    ) -> Self {
        match self {
            SpanSource::Pdf {
                page,
                char_start,
                char_end,
                bbox,
            } => {
                let output_len = span_output_end.saturating_sub(span_output_start).max(1);
                let source_len = char_end.saturating_sub(*char_start);
                let rel_start = overlap_start.saturating_sub(span_output_start);
                let rel_end = overlap_end.saturating_sub(span_output_start);
                let clipped_start = *char_start + source_len * rel_start / output_len;
                let clipped_end = *char_start + source_len * rel_end / output_len;
                SpanSource::Pdf {
                    page: *page,
                    char_start: clipped_start.min(*char_end),
                    char_end: clipped_end.max(clipped_start).min(*char_end),
                    bbox: bbox.clone(),
                }
            }
            SpanSource::Synthetic { kind, parent_refs } => SpanSource::Synthetic {
                kind: *kind,
                parent_refs: parent_refs.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRef {
    pub page: u32,
    pub char_start: usize,
    pub char_end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyntheticKind {
    InsertedWhitespace,
    HeadingMarker,
    TableMarkdown,
    DehyphenationJoin,
    ContextHeading,
    NormalizationReplacement,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_chars_rebases_output_and_clips_pdf_source_range() {
        let located = LocatedText {
            text: "abcdefghij".to_string(),
            spans: vec![OutputSpan {
                output_start: 0,
                output_end: 10,
                source: SpanSource::Pdf {
                    page: 1,
                    char_start: 100,
                    char_end: 110,
                    bbox: BoundingBox {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 10.0,
                    },
                },
            }],
        };

        let sliced = located.slice_chars(3, 4, "defg".to_string());
        assert_eq!(sliced.text, "defg");
        assert_eq!(sliced.spans.len(), 1);
        assert_eq!(sliced.spans[0].output_start, 0);
        assert_eq!(sliced.spans[0].output_end, 4);
        match &sliced.spans[0].source {
            SpanSource::Pdf {
                char_start,
                char_end,
                ..
            } => {
                assert_eq!((*char_start, *char_end), (103, 107));
            }
            other => panic!("unexpected source: {:?}", other),
        }
    }

    #[test]
    fn compact_output_spans_merges_adjacent_pdf_runs_with_same_bbox() {
        let bbox = BoundingBox {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 10.0,
        };
        let spans = vec![
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
                    bbox,
                },
            },
        ];

        let compacted = compact_output_spans(&spans);

        assert_eq!(compacted.len(), 1);
        assert_eq!((compacted[0].output_start, compacted[0].output_end), (0, 2));
        match &compacted[0].source {
            SpanSource::Pdf {
                char_start,
                char_end,
                ..
            } => assert_eq!((*char_start, *char_end), (100, 102)),
            other => panic!("unexpected source: {:?}", other),
        }
    }
}
