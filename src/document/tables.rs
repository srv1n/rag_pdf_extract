//! Table detection for PDF documents
//!
//! Detects tabular structures based on text alignment patterns
//! and formats them as markdown tables.

use crate::TextSegment;
use std::collections::HashMap;

/// Minimum number of columns to be considered a table
const MIN_TABLE_COLUMNS: usize = 2;

/// Minimum number of rows to be considered a table
const MIN_TABLE_ROWS: usize = 2;

/// Tolerance for X-alignment (in points)
const X_ALIGNMENT_TOLERANCE: f64 = 8.0;

/// Tolerance for Y-alignment (same row) (in points)
const Y_ALIGNMENT_TOLERANCE: f64 = 5.0;

/// Maximum words per cell for table detection (tables have short cells)
const MAX_WORDS_PER_CELL: usize = 15;

/// Minimum score for a region to be considered a table
const MIN_TABLE_SCORE: i32 = 5;

/// A detected table
#[derive(Debug, Clone)]
pub struct DetectedTable {
    pub rows: Vec<TableRow>,
    pub column_count: usize,
    pub bbox: TableBBox,
    pub confidence: f32,
}

#[derive(Debug, Clone)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
    pub y: f64,
}

#[derive(Debug, Clone)]
pub struct TableCell {
    pub content: String,
    pub column_index: usize,
    pub x: f64,
    pub width: f64,
}

#[derive(Debug, Clone)]
pub struct TableBBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Result of table detection
#[derive(Debug)]
pub struct TableDetectionResult {
    pub tables: Vec<DetectedTable>,
    pub non_table_segments: Vec<TextSegment>,
}

/// Column anchor - a repeated X position indicating a table column
#[derive(Debug, Clone)]
struct ColumnAnchor {
    x: f64,
    count: usize,
    segments: Vec<usize>, // indices of segments at this x
}

/// Row group - segments that appear on the same Y level
#[derive(Debug)]
struct RowGroup {
    y: f64,
    segments: Vec<usize>,
}

/// Detect tables in a list of segments
pub(crate) fn detect_tables(segments: &[TextSegment]) -> TableDetectionResult {
    if segments.len() < MIN_TABLE_COLUMNS * MIN_TABLE_ROWS {
        return TableDetectionResult {
            tables: Vec::new(),
            non_table_segments: segments.to_vec(),
        };
    }

    // Find column anchors (repeated X positions)
    let anchors = find_column_anchors(segments);

    if anchors.len() < MIN_TABLE_COLUMNS {
        return TableDetectionResult {
            tables: Vec::new(),
            non_table_segments: segments.to_vec(),
        };
    }

    // Find row groups (segments at same Y level)
    let row_groups = find_row_groups(segments);

    if row_groups.len() < MIN_TABLE_ROWS {
        return TableDetectionResult {
            tables: Vec::new(),
            non_table_segments: segments.to_vec(),
        };
    }

    // Find table candidates - regions with aligned content
    let candidates = find_table_candidates(segments, &anchors, &row_groups);

    // Score and filter candidates
    let mut tables = Vec::new();
    let mut table_segment_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for candidate in candidates {
        let score = score_table_candidate(segments, &candidate);

        if score >= MIN_TABLE_SCORE {
            if let Some(table) = build_table(segments, &candidate) {
                // Mark segments as belonging to table
                for idx in &candidate.segment_indices {
                    table_segment_indices.insert(*idx);
                }
                tables.push(table);
            }
        }
    }

    // Collect non-table segments
    let non_table_segments: Vec<TextSegment> = segments
        .iter()
        .enumerate()
        .filter(|(idx, _)| !table_segment_indices.contains(idx))
        .map(|(_, seg)| seg.clone())
        .collect();

    TableDetectionResult {
        tables,
        non_table_segments,
    }
}

/// Find column anchors (X positions that appear multiple times)
fn find_column_anchors(segments: &[TextSegment]) -> Vec<ColumnAnchor> {
    // Round X positions and count frequencies
    let mut x_counts: HashMap<i32, Vec<usize>> = HashMap::new();

    for (idx, segment) in segments.iter().enumerate() {
        // Round to nearest alignment tolerance
        let rounded_x = (segment.x / X_ALIGNMENT_TOLERANCE).round() as i32;
        x_counts.entry(rounded_x).or_default().push(idx);
    }

    // Filter to positions with multiple segments
    let mut anchors: Vec<ColumnAnchor> = x_counts
        .into_iter()
        .filter(|(_, indices)| indices.len() >= MIN_TABLE_ROWS)
        .map(|(rounded_x, indices)| {
            let actual_x = indices
                .iter()
                .map(|&i| segments[i].x)
                .sum::<f64>() / indices.len() as f64;

            ColumnAnchor {
                x: actual_x,
                count: indices.len(),
                segments: indices,
            }
        })
        .collect();

    // Sort by X position
    anchors.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());

    anchors
}

/// Find row groups (Y positions that have multiple segments)
fn find_row_groups(segments: &[TextSegment]) -> Vec<RowGroup> {
    // Round Y positions and group
    let mut y_groups: HashMap<i32, Vec<usize>> = HashMap::new();

    for (idx, segment) in segments.iter().enumerate() {
        let rounded_y = (segment.y / Y_ALIGNMENT_TOLERANCE).round() as i32;
        y_groups.entry(rounded_y).or_default().push(idx);
    }

    // Filter to rows with multiple segments (potential table rows)
    let mut groups: Vec<RowGroup> = y_groups
        .into_iter()
        .filter(|(_, indices)| indices.len() >= MIN_TABLE_COLUMNS)
        .map(|(rounded_y, indices)| {
            let actual_y = indices
                .iter()
                .map(|&i| segments[i].y)
                .sum::<f64>() / indices.len() as f64;

            RowGroup {
                y: actual_y,
                segments: indices,
            }
        })
        .collect();

    // Sort by Y position
    groups.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap());

    groups
}

/// A table candidate region
#[derive(Debug)]
struct TableCandidate {
    segment_indices: Vec<usize>,
    column_xs: Vec<f64>,
    row_ys: Vec<f64>,
    bbox: TableBBox,
}

/// Find table candidates - rectangular regions with aligned content
fn find_table_candidates(
    segments: &[TextSegment],
    anchors: &[ColumnAnchor],
    row_groups: &[RowGroup],
) -> Vec<TableCandidate> {
    if anchors.len() < MIN_TABLE_COLUMNS || row_groups.len() < MIN_TABLE_ROWS {
        return Vec::new();
    }

    // For now, use a simple approach: try to find the largest rectangular region
    // where multiple anchors and row groups intersect

    let mut candidates = Vec::new();

    // Collect all segment indices that participate in aligned structures
    let mut aligned_segments: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for anchor in anchors {
        for idx in &anchor.segments {
            aligned_segments.insert(*idx);
        }
    }

    // Find contiguous regions of aligned segments
    let aligned_indices: Vec<usize> = aligned_segments.into_iter().collect();

    if aligned_indices.len() < MIN_TABLE_COLUMNS * MIN_TABLE_ROWS {
        return candidates;
    }

    // Calculate bounding box of aligned segments
    let min_x = aligned_indices
        .iter()
        .map(|&i| segments[i].x)
        .fold(f64::INFINITY, f64::min);
    let max_x = aligned_indices
        .iter()
        .map(|&i| segments[i].x + segments[i].width)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = aligned_indices
        .iter()
        .map(|&i| segments[i].y)
        .fold(f64::INFINITY, f64::min);
    let max_y = aligned_indices
        .iter()
        .map(|&i| segments[i].y + segments[i].height)
        .fold(f64::NEG_INFINITY, f64::max);

    // Filter anchors to those within this region
    let region_anchors: Vec<f64> = anchors
        .iter()
        .filter(|a| a.x >= min_x - X_ALIGNMENT_TOLERANCE && a.x <= max_x + X_ALIGNMENT_TOLERANCE)
        .map(|a| a.x)
        .collect();

    // Filter row groups to those within this region
    let region_rows: Vec<f64> = row_groups
        .iter()
        .filter(|r| r.y >= min_y - Y_ALIGNMENT_TOLERANCE && r.y <= max_y + Y_ALIGNMENT_TOLERANCE)
        .map(|r| r.y)
        .collect();

    if region_anchors.len() >= MIN_TABLE_COLUMNS && region_rows.len() >= MIN_TABLE_ROWS {
        candidates.push(TableCandidate {
            segment_indices: aligned_indices,
            column_xs: region_anchors,
            row_ys: region_rows,
            bbox: TableBBox {
                x: min_x,
                y: min_y,
                width: max_x - min_x,
                height: max_y - min_y,
            },
        });
    }

    candidates
}

/// Score a table candidate
fn score_table_candidate(segments: &[TextSegment], candidate: &TableCandidate) -> i32 {
    let mut score: i32 = 0;

    // +2 for each column beyond minimum
    let extra_columns = candidate.column_xs.len().saturating_sub(MIN_TABLE_COLUMNS);
    score += (extra_columns * 2) as i32;

    // +1 for each row beyond minimum
    let extra_rows = candidate.row_ys.len().saturating_sub(MIN_TABLE_ROWS);
    score += extra_rows as i32;

    // Check cell content length
    let mut long_cells = 0;
    let mut short_cells = 0;

    for &idx in &candidate.segment_indices {
        let word_count = segments[idx].content.split_whitespace().count();
        if word_count <= MAX_WORDS_PER_CELL {
            short_cells += 1;
        } else {
            long_cells += 1;
        }
    }

    // +2 if most cells are short
    if short_cells > long_cells * 2 {
        score += 2;
    }

    // -3 if many cells are long (probably paragraphs, not table)
    if long_cells > short_cells {
        score -= 3;
    }

    // +2 for consistent column spacing
    if candidate.column_xs.len() >= 2 {
        let spacings: Vec<f64> = candidate.column_xs
            .windows(2)
            .map(|w| w[1] - w[0])
            .collect();

        if spacings.len() >= 2 {
            let avg_spacing = spacings.iter().sum::<f64>() / spacings.len() as f64;
            let variance: f64 = spacings
                .iter()
                .map(|s| (s - avg_spacing).powi(2))
                .sum::<f64>() / spacings.len() as f64;

            // Low variance = consistent spacing
            if variance < avg_spacing * 0.3 {
                score += 2;
            }
        }
    }

    // +2 for consistent row spacing
    if candidate.row_ys.len() >= 2 {
        let spacings: Vec<f64> = candidate.row_ys
            .windows(2)
            .map(|w| w[1] - w[0])
            .collect();

        if spacings.len() >= 2 {
            let avg_spacing = spacings.iter().sum::<f64>() / spacings.len() as f64;
            let variance: f64 = spacings
                .iter()
                .map(|s| (s - avg_spacing).powi(2))
                .sum::<f64>() / spacings.len() as f64;

            if variance < avg_spacing * 0.3 {
                score += 2;
            }
        }
    }

    score
}

/// Build a table structure from a candidate
fn build_table(segments: &[TextSegment], candidate: &TableCandidate) -> Option<DetectedTable> {
    if candidate.column_xs.is_empty() || candidate.row_ys.is_empty() {
        return None;
    }

    // Determine column boundaries (midpoints between anchors)
    let mut column_boundaries: Vec<f64> = vec![candidate.bbox.x];
    for window in candidate.column_xs.windows(2) {
        column_boundaries.push((window[0] + window[1]) / 2.0);
    }
    column_boundaries.push(candidate.bbox.x + candidate.bbox.width);

    // Determine row boundaries (midpoints between row Ys)
    let mut row_boundaries: Vec<f64> = vec![candidate.bbox.y];
    for window in candidate.row_ys.windows(2) {
        row_boundaries.push((window[0] + window[1]) / 2.0);
    }
    row_boundaries.push(candidate.bbox.y + candidate.bbox.height);

    // Assign segments to cells
    let mut rows: Vec<TableRow> = Vec::new();

    for (row_idx, row_y) in candidate.row_ys.iter().enumerate() {
        let row_start = row_boundaries[row_idx];
        let row_end = row_boundaries[row_idx + 1];

        let mut cells: Vec<TableCell> = Vec::new();

        // Find segments in this row
        let row_segments: Vec<&TextSegment> = candidate
            .segment_indices
            .iter()
            .map(|&i| &segments[i])
            .filter(|s| s.y >= row_start - Y_ALIGNMENT_TOLERANCE && s.y <= row_end + Y_ALIGNMENT_TOLERANCE)
            .collect();

        // Assign to columns
        for (col_idx, _) in candidate.column_xs.iter().enumerate() {
            let col_start = column_boundaries[col_idx];
            let col_end = column_boundaries[col_idx + 1];

            // Find segments in this cell
            let cell_segments: Vec<&&TextSegment> = row_segments
                .iter()
                .filter(|s| {
                    let seg_center = s.x + s.width / 2.0;
                    seg_center >= col_start && seg_center <= col_end
                })
                .collect();

            let content = cell_segments
                .iter()
                .map(|s| s.content.trim())
                .collect::<Vec<_>>()
                .join(" ");

            cells.push(TableCell {
                content,
                column_index: col_idx,
                x: col_start,
                width: col_end - col_start,
            });
        }

        rows.push(TableRow {
            cells,
            y: *row_y,
        });
    }

    Some(DetectedTable {
        column_count: candidate.column_xs.len(),
        rows,
        bbox: candidate.bbox.clone(),
        confidence: 0.8, // TODO: calculate from score
    })
}

/// Format a detected table as markdown
pub(crate) fn table_to_markdown(table: &DetectedTable) -> String {
    if table.rows.is_empty() {
        return String::new();
    }

    let mut md = String::new();

    // Header row
    if let Some(header_row) = table.rows.first() {
        md.push('|');
        for cell in &header_row.cells {
            md.push_str(&format!(" {} |", cell.content.trim()));
        }
        md.push('\n');

        // Separator row
        md.push('|');
        for _ in &header_row.cells {
            md.push_str("---|");
        }
        md.push('\n');
    }

    // Data rows
    for row in table.rows.iter().skip(1) {
        md.push('|');
        for cell in &row.cells {
            md.push_str(&format!(" {} |", cell.content.trim()));
        }
        md.push('\n');
    }

    md
}

/// Check if a region likely contains a table
pub(crate) fn region_likely_table(segments: &[TextSegment]) -> bool {
    let result = detect_tables(segments);
    !result.tables.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_segment(x: f64, y: f64, width: f64, content: &str) -> TextSegment {
        TextSegment {
            x,
            y,
            width,
            height: 12.0,
            content: content.to_string(),
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
            char_end: content.len(),
            word_count: content.split_whitespace().count(),
        }
    }

    #[test]
    fn test_simple_table_detection() {
        // Create a 3x3 table-like structure
        let segments = vec![
            // Row 1
            make_segment(72.0, 100.0, 80.0, "Name"),
            make_segment(200.0, 100.0, 80.0, "Amount"),
            make_segment(350.0, 100.0, 80.0, "Date"),
            // Row 2
            make_segment(72.0, 120.0, 80.0, "John"),
            make_segment(200.0, 120.0, 80.0, "$500"),
            make_segment(350.0, 120.0, 80.0, "2024-01"),
            // Row 3
            make_segment(72.0, 140.0, 80.0, "Jane"),
            make_segment(200.0, 140.0, 80.0, "$750"),
            make_segment(350.0, 140.0, 80.0, "2024-02"),
        ];

        let result = detect_tables(&segments);
        assert!(!result.tables.is_empty(), "Should detect a table");

        if let Some(table) = result.tables.first() {
            assert_eq!(table.column_count, 3);
            assert_eq!(table.rows.len(), 3);
        }
    }

    #[test]
    fn test_no_table_paragraph() {
        // Create paragraph-like text (not a table)
        let segments = vec![
            make_segment(72.0, 100.0, 400.0, "This is a long paragraph of text that should not be detected as a table."),
            make_segment(72.0, 120.0, 380.0, "Another line of paragraph text continuing the thought from above."),
            make_segment(72.0, 140.0, 390.0, "And yet another line to make sure we have enough content here."),
        ];

        let result = detect_tables(&segments);
        assert!(result.tables.is_empty(), "Should not detect a table in paragraph text");
    }
}
