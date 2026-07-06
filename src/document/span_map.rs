use crate::{BoundingBox, ChunkLocation};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

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

pub fn chunk_locations_from_output_spans(spans: &[OutputSpan]) -> Vec<ChunkLocation> {
    let mut entries = spans
        .iter()
        .filter_map(|span| {
            if span.output_start >= span.output_end {
                return None;
            }
            let SpanSource::Pdf { page, bbox, .. } = &span.source else {
                return None;
            };
            if !valid_location_bbox(bbox) {
                return None;
            }
            Some(PdfBoxEntry {
                page: *page,
                output_start: span.output_start,
                bbox: normalize_bbox(bbox),
            })
        })
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| {
        a.page
            .cmp(&b.page)
            .then_with(|| a.output_start.cmp(&b.output_start))
    });

    let mut locations = Vec::new();
    let mut idx = 0usize;
    while idx < entries.len() {
        let page = entries[idx].page;
        let start = idx;
        while idx < entries.len() && entries[idx].page == page {
            idx += 1;
        }
        let page_entries = entries[start..idx]
            .iter()
            .map(|entry| PageBoxEntry {
                output_start: entry.output_start,
                bbox: entry.bbox.clone(),
            })
            .collect::<Vec<_>>();
        let bboxes = fold_page_bboxes(&page_entries);
        if !bboxes.is_empty() {
            locations.push(ChunkLocation { page, bboxes });
        }
    }

    locations
}

#[derive(Clone, Debug)]
struct PdfBoxEntry {
    page: u32,
    output_start: usize,
    bbox: BoundingBox,
}

#[derive(Clone, Debug)]
struct PageBoxEntry {
    output_start: usize,
    bbox: BoundingBox,
}

fn fold_page_bboxes(entries: &[PageBoxEntry]) -> Vec<BoundingBox> {
    if entries.is_empty() {
        return Vec::new();
    }

    let whole = union_page_entries(entries);
    if entries.len() == 1 {
        return vec![whole];
    }

    if let Some(split) = column_split_bboxes(entries, &whole) {
        return split;
    }
    if let Some(split) = vertical_split_bboxes(entries, &whole) {
        return split;
    }

    vec![whole]
}

fn column_split_bboxes(entries: &[PageBoxEntry], whole: &BoundingBox) -> Option<Vec<BoundingBox>> {
    let mut by_x = entries.to_vec();
    by_x.sort_by(|a, b| {
        cmp_f64(a.bbox.x, b.bbox.x).then_with(|| a.output_start.cmp(&b.output_start))
    });

    let median_width = median(by_x.iter().map(|entry| entry.bbox.width).collect()).max(1.0);
    let min_gap = (median_width * 0.75).max(72.0);
    let mut candidates = Vec::new();
    for idx in 0..by_x.len().saturating_sub(1) {
        let left_edge = by_x[idx].bbox.x + by_x[idx].bbox.width;
        let right_edge = by_x[idx + 1].bbox.x;
        let gap = right_edge - left_edge;
        if gap >= min_gap {
            candidates.push((idx + 1, gap));
        }
    }
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| cmp_f64(b.1, a.1));

    for split_count in 1..=candidates.len().min(2) {
        let mut boundaries = candidates
            .iter()
            .take(split_count)
            .map(|(boundary, _)| *boundary)
            .collect::<Vec<_>>();
        boundaries.sort_unstable();
        let groups = groups_by_boundaries(&by_x, &boundaries);
        if groups.len() < 2 || groups.len() > 3 || groups.iter().any(|group| group.is_empty()) {
            continue;
        }

        let group_bboxes = groups
            .iter()
            .map(|group| union_page_entries(group))
            .collect::<Vec<_>>();
        let sum_area = group_bboxes.iter().map(bbox_area).sum::<f64>().max(1.0);
        let ratio = bbox_area(whole) / sum_area;
        let overlapping_columns = group_bboxes
            .windows(2)
            .all(|pair| vertical_overlap_ratio(&pair[0], &pair[1]) >= 0.20);

        if overlapping_columns && ratio >= 1.15 {
            return Some(sort_bboxes_by_output_order(groups));
        }
    }

    None
}

fn vertical_split_bboxes(
    entries: &[PageBoxEntry],
    whole: &BoundingBox,
) -> Option<Vec<BoundingBox>> {
    let mut ordered = entries.to_vec();
    ordered.sort_by_key(|entry| entry.output_start);

    let median_height = median(ordered.iter().map(|entry| entry.bbox.height).collect()).max(1.0);
    let min_gap = (median_height * 6.0).max(96.0);
    let mut candidates = Vec::new();
    for idx in 0..ordered.len().saturating_sub(1) {
        let gap = vertical_gap(&ordered[idx].bbox, &ordered[idx + 1].bbox);
        if gap >= min_gap {
            candidates.push((idx + 1, gap));
        }
    }
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| cmp_f64(b.1, a.1));

    for split_count in 1..=candidates.len().min(2) {
        let mut boundaries = candidates
            .iter()
            .take(split_count)
            .map(|(boundary, _)| *boundary)
            .collect::<Vec<_>>();
        boundaries.sort_unstable();
        let groups = groups_by_boundaries(&ordered, &boundaries);
        if groups.len() < 2 || groups.len() > 3 || groups.iter().any(|group| group.is_empty()) {
            continue;
        }

        let group_bboxes = groups
            .iter()
            .map(|group| union_page_entries(group))
            .collect::<Vec<_>>();
        let sum_area = group_bboxes.iter().map(bbox_area).sum::<f64>().max(1.0);
        let ratio = bbox_area(whole) / sum_area;
        if ratio >= 1.75 {
            return Some(sort_bboxes_by_output_order(groups));
        }
    }

    None
}

fn groups_by_boundaries(entries: &[PageBoxEntry], boundaries: &[usize]) -> Vec<Vec<PageBoxEntry>> {
    let mut groups = Vec::new();
    let mut start = 0usize;
    for boundary in boundaries {
        groups.push(entries[start..*boundary].to_vec());
        start = *boundary;
    }
    groups.push(entries[start..].to_vec());
    groups
}

fn sort_bboxes_by_output_order(groups: Vec<Vec<PageBoxEntry>>) -> Vec<BoundingBox> {
    let mut grouped = groups
        .into_iter()
        .map(|group| {
            let first_output = group
                .iter()
                .map(|entry| entry.output_start)
                .min()
                .unwrap_or(usize::MAX);
            (first_output, union_page_entries(&group))
        })
        .collect::<Vec<_>>();
    grouped.sort_by_key(|(first_output, _)| *first_output);
    grouped.into_iter().map(|(_, bbox)| bbox).collect()
}

fn union_page_entries(entries: &[PageBoxEntry]) -> BoundingBox {
    let mut iter = entries.iter();
    let first = iter.next().expect("non-empty page bbox entries");
    iter.fold(first.bbox.clone(), |acc, entry| {
        bbox_union(&acc, &entry.bbox)
    })
}

fn bbox_union(a: &BoundingBox, b: &BoundingBox) -> BoundingBox {
    let min_x = a.x.min(b.x);
    let min_y = a.y.min(b.y);
    let max_x = (a.x + a.width).max(b.x + b.width);
    let max_y = (a.y + a.height).max(b.y + b.height);
    BoundingBox {
        x: min_x,
        y: min_y,
        width: (max_x - min_x).max(0.01),
        height: (max_y - min_y).max(0.01),
    }
}

fn bbox_area(bbox: &BoundingBox) -> f64 {
    bbox.width.max(0.0) * bbox.height.max(0.0)
}

fn vertical_gap(a: &BoundingBox, b: &BoundingBox) -> f64 {
    let a_min = a.y;
    let a_max = a.y + a.height;
    let b_min = b.y;
    let b_max = b.y + b.height;
    if a_max < b_min {
        b_min - a_max
    } else if b_max < a_min {
        a_min - b_max
    } else {
        0.0
    }
}

fn vertical_overlap_ratio(a: &BoundingBox, b: &BoundingBox) -> f64 {
    let overlap = ((a.y + a.height).min(b.y + b.height) - a.y.max(b.y)).max(0.0);
    let denom = a.height.min(b.height).max(1.0);
    overlap / denom
}

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| cmp_f64(*a, *b));
    values[values.len() / 2]
}

fn cmp_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

fn valid_location_bbox(bbox: &BoundingBox) -> bool {
    bbox.x.is_finite()
        && bbox.y.is_finite()
        && bbox.width.is_finite()
        && bbox.height.is_finite()
        && bbox.width >= 0.0
        && bbox.height >= 0.0
}

fn normalize_bbox(bbox: &BoundingBox) -> BoundingBox {
    BoundingBox {
        x: bbox.x,
        y: bbox.y,
        width: bbox.width.max(0.01),
        height: bbox.height.max(0.01),
    }
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

    #[test]
    fn chunk_locations_fold_single_page_to_one_union_box() {
        let spans = vec![
            pdf_span(0, 5, 1, bbox(10.0, 20.0, 40.0, 10.0)),
            pdf_span(5, 10, 1, bbox(10.0, 35.0, 45.0, 10.0)),
        ];

        let locations = chunk_locations_from_output_spans(&spans);

        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].page, 1);
        assert_eq!(locations[0].bboxes.len(), 1);
        assert_bbox_close(&locations[0].bboxes[0], &bbox(10.0, 20.0, 45.0, 25.0));
    }

    #[test]
    fn chunk_locations_emit_pages_in_page_order() {
        let spans = vec![
            pdf_span(0, 5, 3, bbox(10.0, 20.0, 40.0, 10.0)),
            pdf_span(5, 10, 1, bbox(15.0, 30.0, 35.0, 10.0)),
            pdf_span(10, 15, 2, bbox(20.0, 40.0, 30.0, 10.0)),
        ];

        let pages = chunk_locations_from_output_spans(&spans)
            .into_iter()
            .map(|location| location.page)
            .collect::<Vec<_>>();

        assert_eq!(pages, vec![1, 2, 3]);
    }

    #[test]
    fn chunk_locations_split_columns_when_single_union_would_overcover() {
        let spans = vec![
            pdf_span(0, 5, 1, bbox(40.0, 100.0, 100.0, 12.0)),
            pdf_span(5, 10, 1, bbox(40.0, 118.0, 100.0, 12.0)),
            pdf_span(10, 15, 1, bbox(340.0, 100.0, 100.0, 12.0)),
            pdf_span(15, 20, 1, bbox(340.0, 118.0, 100.0, 12.0)),
        ];

        let locations = chunk_locations_from_output_spans(&spans);

        assert_eq!(locations.len(), 1);
        assert!(locations[0].bboxes.len() > 1);
        assert!(locations[0].bboxes.len() <= 3);
    }

    #[test]
    fn chunk_locations_are_empty_for_synthetic_only_spans() {
        let spans = vec![synthetic_span(
            0,
            3,
            SyntheticKind::ContextHeading,
            vec![SourceRef {
                page: 1,
                char_start: 10,
                char_end: 20,
            }],
        )];

        let locations = chunk_locations_from_output_spans(&spans);

        assert!(locations.is_empty());
    }

    #[test]
    fn chunk_locations_ignore_synthetic_spans_in_mixed_chunks() {
        let pdf_bbox = bbox(10.0, 20.0, 30.0, 10.0);
        let spans = vec![
            synthetic_span(0, 2, SyntheticKind::HeadingMarker, Vec::new()),
            pdf_span(2, 7, 1, pdf_bbox.clone()),
            synthetic_span(7, 8, SyntheticKind::InsertedWhitespace, Vec::new()),
        ];

        let locations = chunk_locations_from_output_spans(&spans);

        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].bboxes.len(), 1);
        assert_bbox_close(&locations[0].bboxes[0], &pdf_bbox);
    }

    #[test]
    fn chunk_location_boxes_contain_all_contributing_pdf_boxes() {
        let contributing = vec![
            bbox(40.0, 100.0, 100.0, 12.0),
            bbox(40.0, 118.0, 100.0, 12.0),
            bbox(340.0, 100.0, 100.0, 12.0),
            bbox(340.0, 118.0, 100.0, 12.0),
        ];
        let spans = contributing
            .iter()
            .enumerate()
            .map(|(idx, bbox)| pdf_span(idx * 5, idx * 5 + 5, 1, bbox.clone()))
            .collect::<Vec<_>>();

        let locations = chunk_locations_from_output_spans(&spans);
        let emitted = &locations[0].bboxes;

        for source_bbox in contributing {
            assert!(
                emitted
                    .iter()
                    .any(|candidate| contains_bbox(candidate, &source_bbox)),
                "no emitted bbox contained source bbox: {:?}",
                source_bbox
            );
        }
    }

    fn pdf_span(
        output_start: usize,
        output_end: usize,
        page: u32,
        bbox: BoundingBox,
    ) -> OutputSpan {
        OutputSpan {
            output_start,
            output_end,
            source: SpanSource::Pdf {
                page,
                char_start: output_start,
                char_end: output_end,
                bbox,
            },
        }
    }

    fn synthetic_span(
        output_start: usize,
        output_end: usize,
        kind: SyntheticKind,
        parent_refs: Vec<SourceRef>,
    ) -> OutputSpan {
        OutputSpan {
            output_start,
            output_end,
            source: SpanSource::Synthetic { kind, parent_refs },
        }
    }

    fn bbox(x: f64, y: f64, width: f64, height: f64) -> BoundingBox {
        BoundingBox {
            x,
            y,
            width,
            height,
        }
    }

    fn contains_bbox(outer: &BoundingBox, inner: &BoundingBox) -> bool {
        const EPSILON: f64 = 1e-6;
        outer.x <= inner.x + EPSILON
            && outer.y <= inner.y + EPSILON
            && outer.x + outer.width + EPSILON >= inner.x + inner.width
            && outer.y + outer.height + EPSILON >= inner.y + inner.height
    }

    fn assert_bbox_close(actual: &BoundingBox, expected: &BoundingBox) {
        const EPSILON: f64 = 1e-6;
        assert!((actual.x - expected.x).abs() <= EPSILON, "x: {:?}", actual);
        assert!((actual.y - expected.y).abs() <= EPSILON, "y: {:?}", actual);
        assert!(
            (actual.width - expected.width).abs() <= EPSILON,
            "width: {:?}",
            actual
        );
        assert!(
            (actual.height - expected.height).abs() <= EPSILON,
            "height: {:?}",
            actual
        );
    }
}
