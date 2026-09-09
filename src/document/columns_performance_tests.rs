//! Equivalence and allocation-identity checks for the column hot paths.
//! The reference functions below preserve the pre-optimization algorithms.

use super::*;
use crate::document::{LocatedText, OutputSpan, SpanSource};
use crate::{BoundingBox, FontWeight};
use std::hint::black_box;
use std::time::{Duration, Instant};

fn make_segment(id: usize, x: f64, y: f64, width: f64, height: f64) -> TextSegment {
    let content = format!("segment {id}: legal text, 日本語, café, 🦀");
    let char_len = content.chars().count();
    let char_start = id * 100;
    let page_num = (id % 3 + 1) as u32;
    let located_text = LocatedText {
        text: content.clone(),
        spans: vec![OutputSpan {
            output_start: 0,
            output_end: char_len,
            source: SpanSource::Pdf {
                page: page_num,
                char_start,
                char_end: char_start + char_len,
                bbox: BoundingBox { x, y, width, height },
            },
        }],
    };
    TextSegment {
        x,
        y,
        width,
        height,
        content,
        font_size: 12.0,
        transformed_font_size: 12.0,
        font_name: format!("Font{}", id % 2),
        is_bold: id % 2 == 0,
        font_weight: FontWeight::Regular,
        is_italic: id % 3 == 0,
        page_num,
        cutat: format!("boundary-{id}"),
        fill_color: None,
        stroke_color: None,
        char_start,
        char_end: char_start + char_len,
        word_count: 6,
        located_text: Some(located_text),
    }
}

fn layout() -> ColumnLayout {
    ColumnLayout {
        columns: vec![
            Column { x_start: 0.0, x_end: 200.0, index: 0 },
            Column { x_start: 220.0, x_end: 420.0, index: 1 },
        ],
        is_multi_column: true,
        page_width: 612.0,
    }
}

fn assert_same_segments(actual: &[TextSegment], expected: &[TextSegment]) {
    // TextSegment intentionally has no PartialEq; Debug includes every field,
    // including nested source spans, character ranges, and bounding boxes.
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
}

#[test]
fn coverage_matches_reference_for_edge_cases() {
    let cases = vec![
        vec![],
        vec![(0.0, 0.0)],
        vec![(-0.0, -0.0)],
        vec![(5.0, 10.0)],
        vec![(10.0, 20.0), (0.0, 10.0), (20.0, 30.0)],
        vec![(0.0, 10.0), (0.0, 2.0), (1.0, 3.0), (12.0, 18.0)],
        vec![(-5.0, -1.0), (-10.0, -6.0), (0.0, 2.0)],
        vec![(5.0, 3.0), (7.0, 9.0)],
    ];
    for mut ranges in cases {
        let expected = reference_y_coverage(&ranges);
        assert_eq!(calculate_y_coverage(&mut ranges).to_bits(), expected.to_bits());
    }
}

fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
    *state >> 32
}

#[test]
fn coverage_matches_reference_for_generated_ranges() {
    let mut seed = 42;
    for len in 0..256 {
        let mut ranges = (0..len)
            .map(|_| {
                let start = (next_random(&mut seed) % 200) as f64 / 4.0 - 25.0;
                let end = start + (next_random(&mut seed) % 40) as f64 / 4.0;
                (start, end)
            })
            .collect::<Vec<_>>();
        let expected = reference_y_coverage(&ranges);
        assert_eq!(calculate_y_coverage(&mut ranges).to_bits(), expected.to_bits());
    }
}

#[test]
fn gutter_height_matches_reference_for_generated_pages() {
    let mut seed = 73;
    for len in 0..128 {
        let segments = (0..len)
            .map(|id| {
                let x = (next_random(&mut seed) % 500) as f64;
                let y = (next_random(&mut seed) % 800) as f64 - 100.0;
                let width = (next_random(&mut seed) % 100) as f64;
                let height = (next_random(&mut seed) % 30) as f64;
                make_segment(id, x, y, width, height)
            })
            .collect::<Vec<_>>();
        for (start, end) in [(0.0, 20.0), (150.0, 250.0), (500.0, 520.0)] {
            assert_eq!(
                calculate_gutter_height(&segments, start, end).to_bits(),
                reference_gutter_height(&segments, start, end).to_bits(),
            );
        }
    }
}

#[test]
fn gutter_height_preserves_inclusive_tolerance_boundaries() {
    // The first two segments sit exactly on the two +/- 5 point boundaries.
    let segments = vec![
        make_segment(0, 100.0, 10.0, 55.0, 30.0),
        make_segment(1, 245.0, 20.0, 50.0, 30.0),
    ];
    assert_eq!(calculate_gutter_height(&segments, 150.0, 250.0), 20.0);
    assert_eq!(calculate_gutter_height(&segments[..1], 150.0, 250.0), 0.0);
}

fn check_permutations(segments: &mut [TextSegment], pos: usize, layout: &ColumnLayout) {
    if pos == segments.len() {
        let mut expected = segments.to_vec();
        let mut actual = segments.to_vec();
        reference_reorder(&mut expected, layout);
        reorder_by_columns(&mut actual, layout);
        assert_same_segments(&actual, &expected);
        return;
    }
    for idx in pos..segments.len() {
        segments.swap(pos, idx);
        check_permutations(segments, pos + 1, layout);
        segments.swap(pos, idx);
    }
}

#[test]
fn reorder_matches_reference_for_all_small_permutations() {
    // Includes equal-Y ties in both columns and an equal-distance column tie.
    let mut segments = vec![
        make_segment(0, 30.0, 10.0, 20.0, 12.0),
        make_segment(1, 250.0, 20.0, 20.0, 12.0),
        make_segment(2, 30.0, 10.0, 20.0, 12.0),
        make_segment(3, 250.0, 20.0, 20.0, 12.0),
        make_segment(4, 200.0, 30.0, 20.0, 12.0),
        make_segment(5, 30.0, 40.0, 20.0, 12.0),
    ];
    check_permutations(&mut segments, 0, &layout());
}

#[test]
fn reorder_matches_reference_for_empty_single_and_fallback_layouts() {
    let mut single = layout();
    single.is_multi_column = false;
    let mut no_columns = layout();
    no_columns.columns.clear();
    for layout in [layout(), single, no_columns] {
        for len in [0, 1, 2, 32] {
            let mut actual = (0..len)
                .map(|id| make_segment(id, (id % 2 * 250) as f64, (len - id) as f64, 20.0, 12.0))
                .collect::<Vec<_>>();
            let mut expected = actual.clone();
            reference_reorder(&mut expected, &layout);
            reorder_by_columns(&mut actual, &layout);
            assert_same_segments(&actual, &expected);
        }
    }
}

fn payload_allocations(segments: &[TextSegment]) -> Vec<(usize, [usize; 5])> {
    let mut pointers = segments.iter().map(|segment| {
        let located = segment.located_text.as_ref().unwrap();
        (segment.char_start, [
            segment.content.as_ptr() as usize,
            segment.font_name.as_ptr() as usize,
            segment.cutat.as_ptr() as usize,
            located.text.as_ptr() as usize,
            located.spans.as_ptr() as usize,
        ])
    }).collect::<Vec<_>>();
    pointers.sort_unstable_by_key(|entry| entry.0);
    pointers
}

#[test]
fn reorder_preserves_text_and_source_span_allocations() {
    let mut segments = (0..64)
        .map(|id| make_segment(id, (id % 2 * 250) as f64, (64 - id) as f64, 20.0, 12.0))
        .collect::<Vec<_>>();
    let before = payload_allocations(&segments);
    let mut expected = segments.clone();
    reference_reorder(&mut expected, &layout());
    reorder_by_columns(&mut segments, &layout());
    assert_same_segments(&segments, &expected);
    assert_eq!(payload_allocations(&segments), before);
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

#[test]
#[ignore = "manual release-mode microbenchmark; not an end-to-end PDF benchmark"]
fn benchmark_column_hot_paths() {
    let layout = layout();
    let mut source = (0..2_000)
        .map(|id| make_segment(id, (id % 2 * 250) as f64, (2_000 - id) as f64, 180.0, 12.0))
        .collect::<Vec<_>>();
    for segment in &mut source {
        // Model long segments with substantial nested text and span payloads.
        segment.content = segment.content.repeat(16);
        let located = segment.located_text.as_mut().unwrap();
        located.text = segment.content.clone();
        located.spans = (0..16).map(|_| located.spans[0].clone()).collect();
    }
    let mut old_samples = Vec::new();
    let mut new_samples = Vec::new();
    for sample in 0..9 {
        // Alternate order; input preparation and result destruction are not timed.
        for old in [sample % 2 == 0, sample % 2 != 0] {
            let mut segments = source.clone();
            let start = Instant::now();
            if old {
                reference_reorder(black_box(&mut segments), black_box(&layout));
                old_samples.push(start.elapsed());
            } else {
                reorder_by_columns(black_box(&mut segments), black_box(&layout));
                new_samples.push(start.elapsed());
            }
            black_box(&segments);
        }
    }
    let old = median(old_samples);
    let new = median(new_samples);
    println!("column reorder, 2000 segments: old={old:?}, new={new:?}, speedup={:.3}x", old.as_secs_f64() / new.as_secs_f64());

    let ranges = (0..2_000).rev().map(|i| (i as f64 * 2.0, i as f64 * 2.0 + 1.0)).collect::<Vec<_>>();
    let mut old_samples = Vec::new();
    let mut new_samples = Vec::new();
    for sample in 0..9 {
        for old in [sample % 2 == 0, sample % 2 != 0] {
            let mut input = ranges.clone();
            let start = Instant::now();
            let result = if old {
                reference_y_coverage(black_box(&input))
            } else {
                calculate_y_coverage(black_box(&mut input))
            };
            let elapsed = start.elapsed();
            black_box(result);
            if old { old_samples.push(elapsed); } else { new_samples.push(elapsed); }
        }
    }
    let old = median(old_samples);
    let new = median(new_samples);
    println!("column coverage, 2000 intervals: old={old:?}, new={new:?}, speedup={:.3}x", old.as_secs_f64() / new.as_secs_f64());
}

// Frozen pre-optimization reference implementations, scoped to this test module.
fn reference_y_coverage(ranges: &[(f64, f64)]) -> f64 {
    if ranges.is_empty() {
        return 0.0;
    }

    // Merge overlapping ranges and calculate total coverage
    let mut sorted_ranges: Vec<(f64, f64)> = ranges.to_vec();
    sorted_ranges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (start, end) in sorted_ranges {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        } else {
            merged.push((start, end));
        }
    }

    merged.iter().map(|(s, e)| e - s).sum()
}

fn reference_gutter_height(
    segments: &[TextSegment],
    gutter_x_start: f64,
    gutter_x_end: f64,
) -> f64 {
    // Find segments on either side of the gutter
    let left_segments: Vec<&TextSegment> = segments
        .iter()
        .filter(|s| s.x + s.width <= gutter_x_start + 5.0)
        .collect();

    let right_segments: Vec<&TextSegment> = segments
        .iter()
        .filter(|s| s.x >= gutter_x_end - 5.0)
        .collect();

    if left_segments.is_empty() || right_segments.is_empty() {
        return 0.0;
    }

    // Find the y-overlap between left and right content
    let left_min_y = left_segments
        .iter()
        .map(|s| s.y)
        .fold(f64::INFINITY, f64::min);
    let left_max_y = left_segments
        .iter()
        .map(|s| s.y + s.height)
        .fold(f64::NEG_INFINITY, f64::max);

    let right_min_y = right_segments
        .iter()
        .map(|s| s.y)
        .fold(f64::INFINITY, f64::min);
    let right_max_y = right_segments
        .iter()
        .map(|s| s.y + s.height)
        .fold(f64::NEG_INFINITY, f64::max);

    // The gutter height is the overlap of left and right content ranges
    let overlap_start = left_min_y.max(right_min_y);
    let overlap_end = left_max_y.min(right_max_y);

    if overlap_end > overlap_start {
        overlap_end - overlap_start
    } else {
        0.0
    }
}

fn reference_reorder(segments: &mut [TextSegment], layout: &ColumnLayout) {
    if !layout.is_multi_column {
        // Single column: just sort by Y (top to bottom)
        segments.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap());
        return;
    }

    // Assign each segment to a column
    let mut column_assignments: Vec<(usize, usize)> = segments
        .iter()
        .enumerate()
        .map(|(idx, seg)| {
            let seg_center = seg.x + seg.width / 2.0;
            let col_idx = layout
                .columns
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    let dist_a = (a.center() - seg_center).abs();
                    let dist_b = (b.center() - seg_center).abs();
                    dist_a.partial_cmp(&dist_b).unwrap()
                })
                .map(|(i, _)| i)
                .unwrap_or(0);
            (idx, col_idx)
        })
        .collect();

    // Sort by: column index first, then Y position within column
    let y_positions: Vec<f64> = segments.iter().map(|s| s.y).collect();
    column_assignments.sort_by(|(idx_a, col_a), (idx_b, col_b)| match col_a.cmp(col_b) {
        std::cmp::Ordering::Equal => y_positions[*idx_a]
            .partial_cmp(&y_positions[*idx_b])
            .unwrap(),
        other => other,
    });

    // Reorder segments based on sorted indices
    let ordered_indices: Vec<usize> = column_assignments.iter().map(|(idx, _)| *idx).collect();
    let original_segments: Vec<TextSegment> = segments.to_vec();

    for (new_pos, old_idx) in ordered_indices.iter().enumerate() {
        segments[new_pos] = original_segments[*old_idx].clone();
    }
}

