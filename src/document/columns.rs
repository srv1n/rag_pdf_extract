//! Column detection for multi-column PDF layouts
//!
//! Detects vertical gutters (whitespace gaps) to identify column boundaries,
//! then reorders text segments for correct reading order.

use crate::TextSegment;

/// Minimum gutter width to be considered a column separator (in points)
const MIN_GUTTER_WIDTH: f64 = 20.0;

/// Minimum height ratio a gutter must span to be valid (0.3 = 30% of content height)
const MIN_GUTTER_HEIGHT_RATIO: f64 = 0.25;

/// Tolerance for assigning segments to columns (in points)
const COLUMN_TOLERANCE: f64 = 10.0;

/// A detected column region
#[derive(Debug, Clone)]
pub struct Column {
    pub x_start: f64,
    pub x_end: f64,
    pub index: usize,
}

impl Column {
    pub fn contains_x(&self, x: f64) -> bool {
        x >= self.x_start - COLUMN_TOLERANCE && x <= self.x_end + COLUMN_TOLERANCE
    }

    pub fn center(&self) -> f64 {
        (self.x_start + self.x_end) / 2.0
    }
}

/// Result of column detection
#[derive(Debug)]
pub struct ColumnLayout {
    pub columns: Vec<Column>,
    pub is_multi_column: bool,
    pub page_width: f64,
}

/// Detect columns on a page by finding vertical gutters
pub(crate) fn detect_columns(segments: &[TextSegment], page_width: f64) -> ColumnLayout {
    if segments.is_empty() {
        return ColumnLayout {
            columns: vec![Column {
                x_start: 0.0,
                x_end: page_width,
                index: 0,
            }],
            is_multi_column: false,
            page_width,
        };
    }

    // Find the content bounds
    let min_x = segments.iter().map(|s| s.x).fold(f64::INFINITY, f64::min);
    let max_x = segments
        .iter()
        .map(|s| s.x + s.width)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = segments.iter().map(|s| s.y).fold(f64::INFINITY, f64::min);
    let max_y = segments
        .iter()
        .map(|s| s.y + s.height)
        .fold(f64::NEG_INFINITY, f64::max);

    let content_height = max_y - min_y;
    let content_width = max_x - min_x;

    // If content is too narrow, it's single column
    if content_width < page_width * 0.5 {
        return ColumnLayout {
            columns: vec![Column {
                x_start: min_x,
                x_end: max_x,
                index: 0,
            }],
            is_multi_column: false,
            page_width,
        };
    }

    // Build coverage map: for each x position, track which y ranges are covered
    let resolution = 2.0; // 2-point resolution
    let num_buckets = ((max_x - min_x) / resolution).ceil() as usize + 1;

    // For each x bucket, store the y-ranges that have content
    let mut coverage: Vec<Vec<(f64, f64)>> = vec![Vec::new(); num_buckets];

    for segment in segments {
        let start_bucket = ((segment.x - min_x) / resolution).floor() as usize;
        let end_bucket = ((segment.x + segment.width - min_x) / resolution).ceil() as usize;

        for bucket in start_bucket..=end_bucket.min(num_buckets - 1) {
            coverage[bucket].push((segment.y, segment.y + segment.height));
        }
    }

    // Find gutters: continuous x-ranges with no content (or very little)
    let mut gutters: Vec<(f64, f64)> = Vec::new(); // (start_x, end_x)
    let mut gutter_start: Option<usize> = None;

    for (i, ranges) in coverage.iter_mut().enumerate() {
        // Calculate total y-coverage for this x position
        let y_coverage = calculate_y_coverage(ranges);
        let coverage_ratio = y_coverage / content_height;

        // If less than 5% coverage, this x position is "empty"
        let is_empty = coverage_ratio < 0.05;

        match (is_empty, gutter_start) {
            (true, None) => {
                gutter_start = Some(i);
            }
            (false, Some(start)) => {
                let gutter_width = (i - start) as f64 * resolution;
                if gutter_width >= MIN_GUTTER_WIDTH {
                    let start_x = min_x + start as f64 * resolution;
                    let end_x = min_x + i as f64 * resolution;
                    gutters.push((start_x, end_x));
                }
                gutter_start = None;
            }
            _ => {}
        }
    }

    // Handle gutter at the end
    if let Some(start) = gutter_start {
        let gutter_width = (num_buckets - start) as f64 * resolution;
        if gutter_width >= MIN_GUTTER_WIDTH {
            let start_x = min_x + start as f64 * resolution;
            gutters.push((start_x, max_x));
        }
    }

    // Filter gutters by height coverage
    let valid_gutters: Vec<(f64, f64)> = gutters
        .into_iter()
        .filter(|(gx_start, gx_end)| {
            // Check if this gutter spans enough of the page height
            let gutter_height = calculate_gutter_height(segments, *gx_start, *gx_end);
            gutter_height / content_height >= MIN_GUTTER_HEIGHT_RATIO
        })
        .collect();

    // Build columns from gutters
    if valid_gutters.is_empty() {
        return ColumnLayout {
            columns: vec![Column {
                x_start: min_x,
                x_end: max_x,
                index: 0,
            }],
            is_multi_column: false,
            page_width,
        };
    }

    let mut columns = Vec::new();
    let mut current_start = min_x;

    for (i, (gutter_start, gutter_end)) in valid_gutters.iter().enumerate() {
        // Column before this gutter
        if *gutter_start > current_start + 10.0 {
            columns.push(Column {
                x_start: current_start,
                x_end: *gutter_start,
                index: columns.len(),
            });
        }
        current_start = *gutter_end;
    }

    // Final column after last gutter
    if current_start < max_x - 10.0 {
        columns.push(Column {
            x_start: current_start,
            x_end: max_x,
            index: columns.len(),
        });
    }

    // Validate: need at least 2 columns for multi-column
    let is_multi_column = columns.len() >= 2;

    if !is_multi_column {
        return ColumnLayout {
            columns: vec![Column {
                x_start: min_x,
                x_end: max_x,
                index: 0,
            }],
            is_multi_column: false,
            page_width,
        };
    }

    ColumnLayout {
        columns,
        is_multi_column,
        page_width,
    }
}

/// Calculate total y-coverage from a list of y-ranges
fn calculate_y_coverage(ranges: &mut [(f64, f64)]) -> f64 {
    if ranges.is_empty() {
        return 0.0;
    }

    // The coverage bucket is no longer needed by the caller. Sort and compact
    // it in place instead of allocating sorted and merged copies per bucket.
    // Keep the stable sort and summation order used by the original algorithm.
    ranges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut merged_len = 1;
    for idx in 1..ranges.len() {
        let (start, end) = ranges[idx];
        let last = &mut ranges[merged_len - 1];
        if start <= last.1 {
            last.1 = last.1.max(end);
        } else {
            ranges[merged_len] = (start, end);
            merged_len += 1;
        }
    }

    ranges[..merged_len].iter().map(|(s, e)| e - s).sum()
}

/// Calculate the height extent of a gutter (how much vertical space it spans)
fn calculate_gutter_height(
    segments: &[TextSegment],
    gutter_x_start: f64,
    gutter_x_end: f64,
) -> f64 {
    // Only the bounds are needed; do not collect two temporary segment lists.
    let mut left_min_y = f64::INFINITY;
    let mut left_max_y = f64::NEG_INFINITY;
    let mut right_min_y = f64::INFINITY;
    let mut right_max_y = f64::NEG_INFINITY;
    let mut has_left = false;
    let mut has_right = false;

    for segment in segments {
        if segment.x + segment.width <= gutter_x_start + 5.0 {
            has_left = true;
            left_min_y = left_min_y.min(segment.y);
            left_max_y = left_max_y.max(segment.y + segment.height);
        }
        if segment.x >= gutter_x_end - 5.0 {
            has_right = true;
            right_min_y = right_min_y.min(segment.y);
            right_max_y = right_max_y.max(segment.y + segment.height);
        }
    }

    if !has_left || !has_right {
        return 0.0;
    }

    // The gutter height is the overlap of left and right content ranges
    let overlap_start = left_min_y.max(right_min_y);
    let overlap_end = left_max_y.min(right_max_y);

    if overlap_end > overlap_start {
        overlap_end - overlap_start
    } else {
        0.0
    }
}

/// Reorder segments based on column layout (read column by column, top to bottom)
pub(crate) fn reorder_by_columns(segments: &mut [TextSegment], layout: &ColumnLayout) {
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
    column_assignments.sort_by(|(idx_a, col_a), (idx_b, col_b)| match col_a.cmp(col_b) {
        std::cmp::Ordering::Equal => segments[*idx_a]
            .y
            .partial_cmp(&segments[*idx_b].y)
            .unwrap(),
        other => other,
    });

    // Invert the sorted indices into a destination for each original segment.
    // Swapping the permutation alongside the segments resolves each cycle in
    // linear time, without cloning text, font names, or detailed source spans.
    let mut destinations = vec![0; segments.len()];
    for (new_pos, &(old_pos, _)) in column_assignments.iter().enumerate() {
        destinations[old_pos] = new_pos;
    }
    for pos in 0..segments.len() {
        while destinations[pos] != pos {
            let dest = destinations[pos];
            segments.swap(pos, dest);
            destinations.swap(pos, dest);
        }
    }
}

/// Check if a segment spans multiple columns (likely a header)
pub(crate) fn is_spanning_segment(segment: &TextSegment, layout: &ColumnLayout) -> bool {
    if !layout.is_multi_column || layout.columns.len() < 2 {
        return false;
    }

    let seg_start = segment.x;
    let seg_end = segment.x + segment.width;

    // Count how many columns this segment overlaps
    let overlapping_columns = layout
        .columns
        .iter()
        .filter(|col| {
            // Check if segment overlaps this column
            seg_start < col.x_end && seg_end > col.x_start
        })
        .count();

    overlapping_columns > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_segment(x: f64, y: f64, width: f64, height: f64) -> TextSegment {
        TextSegment {
            x,
            y,
            width,
            height,
            content: "test".to_string(),
            font_size: 12.0,
            transformed_font_size: 12.0,
            font_name: "Test".to_string(),
            is_bold: false,
            font_weight: crate::FontWeight::Regular,
            is_italic: false,
            page_num: 1,
            cutat: String::new(),
            fill_color: None,
            stroke_color: None,
            char_start: 0,
            char_end: 4,
            word_count: 1,
            located_text: None,
        }
    }

    #[test]
    fn test_single_column() {
        let segments = vec![
            make_segment(72.0, 100.0, 400.0, 12.0),
            make_segment(72.0, 120.0, 380.0, 12.0),
            make_segment(72.0, 140.0, 390.0, 12.0),
        ];

        let layout = detect_columns(&segments, 612.0);
        assert!(!layout.is_multi_column);
        assert_eq!(layout.columns.len(), 1);
    }

    #[test]
    fn test_two_columns() {
        // Left column segments
        let mut segments = vec![
            make_segment(72.0, 100.0, 200.0, 12.0),
            make_segment(72.0, 120.0, 200.0, 12.0),
            make_segment(72.0, 140.0, 200.0, 12.0),
            make_segment(72.0, 160.0, 200.0, 12.0),
            make_segment(72.0, 180.0, 200.0, 12.0),
        ];

        // Right column segments (with gap)
        segments.extend(vec![
            make_segment(340.0, 100.0, 200.0, 12.0),
            make_segment(340.0, 120.0, 200.0, 12.0),
            make_segment(340.0, 140.0, 200.0, 12.0),
            make_segment(340.0, 160.0, 200.0, 12.0),
            make_segment(340.0, 180.0, 200.0, 12.0),
        ]);

        let layout = detect_columns(&segments, 612.0);
        assert!(layout.is_multi_column);
        assert_eq!(layout.columns.len(), 2);
    }
}

#[cfg(test)]
#[path = "columns_performance_tests.rs"]
mod performance_tests;
