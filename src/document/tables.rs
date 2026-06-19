//! Table detection for PDF documents
//!
//! Detects tabular structures based on text alignment patterns
//! and formats them as markdown tables.

use crate::document::{LocatedText, OutputSpan, SourceRef, SpanSource, SyntheticKind};
use crate::{BoundingBox, TextSegment};
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

/// Minimum fraction of cells that should contain content
const MIN_TABLE_DENSITY: f64 = 0.6;

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
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub char_start: Option<usize>,
    pub char_end: Option<usize>,
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
    let mut table_segment_indices: std::collections::HashSet<usize> =
        std::collections::HashSet::new();

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
            let actual_x =
                indices.iter().map(|&i| segments[i].x).sum::<f64>() / indices.len() as f64;

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
            let actual_y =
                indices.iter().map(|&i| segments[i].y).sum::<f64>() / indices.len() as f64;

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

fn segment_matches_anchor(segment: &TextSegment, anchor_x: f64) -> bool {
    (segment.x - anchor_x).abs() <= X_ALIGNMENT_TOLERANCE * 1.5
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

    let mut candidates = Vec::new();
    let anchor_xs: Vec<f64> = anchors.iter().map(|a| a.x).collect();

    #[derive(Clone)]
    struct RowCandidate {
        y: f64,
        segment_indices: Vec<usize>,
        column_indices: Vec<usize>,
    }

    let mut row_candidates = Vec::new();
    for row in row_groups {
        let mut column_indices = Vec::new();
        let mut segment_indices = Vec::new();

        for &seg_idx in &row.segments {
            let seg = &segments[seg_idx];
            let mut matched_col = None;

            for (col_idx, &anchor_x) in anchor_xs.iter().enumerate() {
                if segment_matches_anchor(seg, anchor_x) {
                    matched_col = Some(col_idx);
                    break;
                }
            }

            if let Some(col_idx) = matched_col {
                if !column_indices.contains(&col_idx) {
                    column_indices.push(col_idx);
                }
                segment_indices.push(seg_idx);
            }
        }

        if column_indices.len() >= MIN_TABLE_COLUMNS {
            column_indices.sort_unstable();
            row_candidates.push(RowCandidate {
                y: row.y,
                segment_indices,
                column_indices,
            });
        }
    }

    if row_candidates.len() < MIN_TABLE_ROWS {
        return candidates;
    }

    let mut band_start = 0usize;
    while band_start < row_candidates.len() {
        let mut band_rows = vec![row_candidates[band_start].clone()];
        let mut shared_columns = band_rows[0].column_indices.clone();
        let mut band_end = band_start + 1;

        while band_end < row_candidates.len() {
            let row_gap = row_candidates[band_end].y - row_candidates[band_end - 1].y;
            if row_gap > Y_ALIGNMENT_TOLERANCE * 6.0 {
                break;
            }

            shared_columns.retain(|col| row_candidates[band_end].column_indices.contains(col));
            if shared_columns.len() < MIN_TABLE_COLUMNS {
                break;
            }

            band_rows.push(row_candidates[band_end].clone());
            band_end += 1;
        }

        if band_rows.len() >= MIN_TABLE_ROWS && shared_columns.len() >= MIN_TABLE_COLUMNS {
            let segment_indices: Vec<usize> = band_rows
                .iter()
                .flat_map(|row| row.segment_indices.iter().copied())
                .filter(|idx| {
                    let seg = &segments[*idx];
                    shared_columns
                        .iter()
                        .any(|col_idx| segment_matches_anchor(seg, anchor_xs[*col_idx]))
                })
                .collect();

            if segment_indices.len() >= MIN_TABLE_COLUMNS * MIN_TABLE_ROWS {
                let min_x = segment_indices
                    .iter()
                    .map(|&i| segments[i].x)
                    .fold(f64::INFINITY, f64::min);
                let max_x = segment_indices
                    .iter()
                    .map(|&i| segments[i].x + segments[i].width)
                    .fold(f64::NEG_INFINITY, f64::max);
                let min_y = segment_indices
                    .iter()
                    .map(|&i| segments[i].y)
                    .fold(f64::INFINITY, f64::min);
                let max_y = segment_indices
                    .iter()
                    .map(|&i| segments[i].y + segments[i].height)
                    .fold(f64::NEG_INFINITY, f64::max);

                candidates.push(TableCandidate {
                    segment_indices,
                    column_xs: shared_columns.iter().map(|idx| anchor_xs[*idx]).collect(),
                    row_ys: band_rows.iter().map(|row| row.y).collect(),
                    bbox: TableBBox {
                        x: min_x,
                        y: min_y,
                        width: max_x - min_x,
                        height: max_y - min_y,
                    },
                });
            }
        }

        band_start = if band_end > band_start + 1 {
            band_end
        } else {
            band_start + 1
        };
    }

    candidates
}

/// Score a table candidate
fn score_table_candidate(segments: &[TextSegment], candidate: &TableCandidate) -> i32 {
    let mut score: i32 = 0;
    let total_cells = candidate.column_xs.len() * candidate.row_ys.len();
    if total_cells == 0 {
        return 0;
    }

    // +2 for each column beyond minimum
    let extra_columns = candidate.column_xs.len().saturating_sub(MIN_TABLE_COLUMNS);
    score += (extra_columns * 2) as i32;

    // +1 for each row beyond minimum
    let extra_rows = candidate.row_ys.len().saturating_sub(MIN_TABLE_ROWS);
    score += extra_rows as i32;

    let density = candidate.segment_indices.len() as f64 / total_cells as f64;
    if density >= MIN_TABLE_DENSITY {
        score += 3;
    } else {
        score -= 6;
    }

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
        let spacings: Vec<f64> = candidate
            .column_xs
            .windows(2)
            .map(|w| w[1] - w[0])
            .collect();

        if spacings.len() >= 2 {
            let avg_spacing = spacings.iter().sum::<f64>() / spacings.len() as f64;
            let variance: f64 = spacings
                .iter()
                .map(|s| (s - avg_spacing).powi(2))
                .sum::<f64>()
                / spacings.len() as f64;

            // Low variance = consistent spacing
            if variance < avg_spacing * 0.3 {
                score += 2;
            }
        }
    }

    // +2 for consistent row spacing
    if candidate.row_ys.len() >= 2 {
        let spacings: Vec<f64> = candidate.row_ys.windows(2).map(|w| w[1] - w[0]).collect();

        if spacings.len() >= 2 {
            let avg_spacing = spacings.iter().sum::<f64>() / spacings.len() as f64;
            let variance: f64 = spacings
                .iter()
                .map(|s| (s - avg_spacing).powi(2))
                .sum::<f64>()
                / spacings.len() as f64;

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
            .filter(|s| {
                s.y >= row_start - Y_ALIGNMENT_TOLERANCE && s.y <= row_end + Y_ALIGNMENT_TOLERANCE
            })
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
            let char_start = cell_segments.iter().map(|s| s.char_start).min();
            let char_end = cell_segments.iter().map(|s| s.char_end).max();

            cells.push(TableCell {
                content,
                column_index: col_idx,
                x: col_start,
                y: row_start,
                width: col_end - col_start,
                height: (row_end - row_start).abs().max(1.0),
                char_start,
                char_end,
            });
        }

        rows.push(TableRow { cells, y: *row_y });
    }

    let total_cells = rows.iter().map(|row| row.cells.len()).sum::<usize>();
    let populated_cells = rows
        .iter()
        .flat_map(|row| row.cells.iter())
        .filter(|cell| !cell.content.trim().is_empty())
        .count();
    let density = if total_cells == 0 {
        0.0
    } else {
        populated_cells as f64 / total_cells as f64
    };

    if density < MIN_TABLE_DENSITY {
        return None;
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
            md.push_str(&format!(" {} |", escape_markdown_cell(&cell.content)));
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
            md.push_str(&format!(" {} |", escape_markdown_cell(&cell.content)));
        }
        md.push('\n');
    }

    md
}

/// Format a detected table as markdown with cell-level source locations.
pub(crate) fn table_to_located_markdown(table: &DetectedTable, page_num: u32) -> LocatedText {
    if table.rows.is_empty() {
        return LocatedText::empty(String::new());
    }

    let mut builder = LocatedTableBuilder::default();
    let table_refs = table_source_refs(table, page_num);

    if let Some(header_row) = table.rows.first() {
        push_markdown_row(&mut builder, header_row, page_num, &table.bbox);
        builder.push_synthetic("|", SyntheticKind::TableMarkdown, table_refs.clone());
        for _ in &header_row.cells {
            builder.push_synthetic("---|", SyntheticKind::TableMarkdown, table_refs.clone());
        }
        builder.push_synthetic("\n", SyntheticKind::TableMarkdown, table_refs.clone());
    }

    for row in table.rows.iter().skip(1) {
        push_markdown_row(&mut builder, row, page_num, &table.bbox);
    }

    LocatedText {
        text: builder.text,
        spans: builder.spans,
    }
}

#[derive(Default)]
struct LocatedTableBuilder {
    text: String,
    spans: Vec<OutputSpan>,
}

impl LocatedTableBuilder {
    fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    fn push_synthetic(&mut self, text: &str, kind: SyntheticKind, parent_refs: Vec<SourceRef>) {
        if text.is_empty() {
            return;
        }
        let start = self.char_len();
        self.text.push_str(text);
        let end = self.char_len();
        self.spans.push(OutputSpan {
            output_start: start,
            output_end: end,
            source: SpanSource::Synthetic { kind, parent_refs },
        });
    }

    fn push_pdf_char(
        &mut self,
        ch: char,
        page_num: u32,
        source_char: usize,
        max_source_char: usize,
        bbox: BoundingBox,
    ) {
        let start = self.char_len();
        self.text.push(ch);
        let end = self.char_len();
        self.spans.push(OutputSpan {
            output_start: start,
            output_end: end,
            source: SpanSource::Pdf {
                page: page_num,
                char_start: source_char.min(max_source_char),
                char_end: source_char.saturating_add(1).min(max_source_char),
                bbox,
            },
        });
    }
}

fn push_markdown_row(
    builder: &mut LocatedTableBuilder,
    row: &TableRow,
    page_num: u32,
    table_bbox: &TableBBox,
) {
    let row_refs = row_source_refs(row, page_num);
    builder.push_synthetic("|", SyntheticKind::TableMarkdown, row_refs.clone());
    for cell in &row.cells {
        let cell_refs = cell_source_refs(cell, page_num);
        builder.push_synthetic(" ", SyntheticKind::TableMarkdown, cell_refs.clone());
        push_cell_text(builder, cell, page_num, table_bbox);
        builder.push_synthetic(" |", SyntheticKind::TableMarkdown, cell_refs);
    }
    builder.push_synthetic("\n", SyntheticKind::TableMarkdown, row_refs);
}

fn push_cell_text(
    builder: &mut LocatedTableBuilder,
    cell: &TableCell,
    page_num: u32,
    table_bbox: &TableBBox,
) {
    let Some(source_start) = cell.char_start else {
        return;
    };
    let source_end = cell
        .char_end
        .unwrap_or_else(|| source_start + cell.content.chars().count())
        .max(source_start);
    let parent_refs = cell_source_refs(cell, page_num);
    let bbox = cell_bbox(cell, table_bbox);
    let mut source_offset = 0usize;
    let mut pending_space = false;
    let mut emitted_content = false;

    for ch in cell.content.chars() {
        if ch.is_whitespace() {
            if emitted_content {
                pending_space = true;
            }
            source_offset = source_offset.saturating_add(1);
            continue;
        }

        if pending_space {
            builder.push_synthetic(
                " ",
                SyntheticKind::NormalizationReplacement,
                parent_refs.clone(),
            );
            pending_space = false;
        }

        if ch == '|' {
            builder.push_synthetic("\\", SyntheticKind::TableMarkdown, parent_refs.clone());
        }
        builder.push_pdf_char(
            ch,
            page_num,
            source_start.saturating_add(source_offset),
            source_end,
            bbox.clone(),
        );
        source_offset = source_offset.saturating_add(1);
        emitted_content = true;
    }
}

fn cell_bbox(cell: &TableCell, table_bbox: &TableBBox) -> BoundingBox {
    let x = if cell.width.is_finite() && cell.width > 0.0 {
        cell.x
    } else {
        table_bbox.x
    };
    let y = if cell.height.is_finite() && cell.height > 0.0 {
        cell.y
    } else {
        table_bbox.y
    };
    let width = if cell.width.is_finite() && cell.width > 0.0 {
        cell.width
    } else {
        table_bbox.width
    };
    let height = if cell.height.is_finite() && cell.height > 0.0 {
        cell.height
    } else {
        table_bbox.height
    };
    BoundingBox {
        x,
        y,
        width,
        height,
    }
}

fn table_source_refs(table: &DetectedTable, page_num: u32) -> Vec<SourceRef> {
    table
        .rows
        .iter()
        .flat_map(|row| row_source_refs(row, page_num))
        .collect()
}

fn row_source_refs(row: &TableRow, page_num: u32) -> Vec<SourceRef> {
    row.cells
        .iter()
        .flat_map(|cell| cell_source_refs(cell, page_num))
        .collect()
}

fn cell_source_refs(cell: &TableCell, page_num: u32) -> Vec<SourceRef> {
    match (cell.char_start, cell.char_end) {
        (Some(char_start), Some(char_end)) if char_end > char_start => vec![SourceRef {
            page: page_num,
            char_start,
            char_end,
        }],
        _ => Vec::new(),
    }
}

fn escape_markdown_cell(text: &str) -> String {
    text.replace('|', r"\|")
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
            located_text: None,
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
            make_segment(
                72.0,
                100.0,
                400.0,
                "This is a long paragraph of text that should not be detected as a table.",
            ),
            make_segment(
                72.0,
                120.0,
                380.0,
                "Another line of paragraph text continuing the thought from above.",
            ),
            make_segment(
                72.0,
                140.0,
                390.0,
                "And yet another line to make sure we have enough content here.",
            ),
        ];

        let result = detect_tables(&segments);
        assert!(
            result.tables.is_empty(),
            "Should not detect a table in paragraph text"
        );
    }

    #[test]
    fn test_sparse_grid_is_not_table() {
        let segments = vec![
            make_segment(72.0, 100.0, 50.0, "A"),
            make_segment(220.0, 100.0, 50.0, "B"),
            make_segment(72.0, 120.0, 50.0, "C"),
            make_segment(
                360.0,
                140.0,
                220.0,
                "Long prose segment that should break table density",
            ),
        ];

        let result = detect_tables(&segments);
        assert!(
            result.tables.is_empty(),
            "Sparse aligned prose should not be promoted to a markdown table"
        );
    }

    #[test]
    fn table_markdown_escapes_pipes_and_newlines() {
        let table = DetectedTable {
            column_count: 2,
            rows: vec![
                TableRow {
                    y: 10.0,
                    cells: vec![
                        TableCell {
                            content: "A|B".to_string(),
                            column_index: 0,
                            x: 0.0,
                            y: 10.0,
                            width: 10.0,
                            height: 10.0,
                            char_start: Some(0),
                            char_end: Some(3),
                        },
                        TableCell {
                            content: "C\nD".to_string(),
                            column_index: 1,
                            x: 20.0,
                            y: 10.0,
                            width: 10.0,
                            height: 10.0,
                            char_start: Some(4),
                            char_end: Some(7),
                        },
                    ],
                },
                TableRow {
                    y: 20.0,
                    cells: vec![
                        TableCell {
                            content: "1|2".to_string(),
                            column_index: 0,
                            x: 0.0,
                            y: 20.0,
                            width: 10.0,
                            height: 10.0,
                            char_start: Some(8),
                            char_end: Some(11),
                        },
                        TableCell {
                            content: "3\n4".to_string(),
                            column_index: 1,
                            x: 20.0,
                            y: 20.0,
                            width: 10.0,
                            height: 10.0,
                            char_start: Some(12),
                            char_end: Some(15),
                        },
                    ],
                },
            ],
            bbox: TableBBox {
                x: 0.0,
                y: 10.0,
                width: 30.0,
                height: 20.0,
            },
            confidence: 1.0,
        };

        let md = table_to_markdown(&table);
        assert!(md.contains(r"A\|B"));
        assert!(md.contains("C D"));
        assert!(!md.contains("C\nD"));

        let located = table_to_located_markdown(&table, 1);
        assert_eq!(located.text, md);
        assert!(located.spans.iter().any(|span| matches!(
            &span.source,
            SpanSource::Synthetic {
                kind: SyntheticKind::TableMarkdown,
                ..
            }
        )));
        assert!(located.spans.iter().any(|span| matches!(
            &span.source,
            SpanSource::Synthetic {
                kind: SyntheticKind::NormalizationReplacement,
                ..
            }
        )));
        assert!(located.spans.iter().any(|span| match &span.source {
            SpanSource::Pdf {
                page,
                char_start,
                char_end,
                bbox,
                ..
            } => {
                *page == 1
                    && *char_start == 0
                    && *char_end == 1
                    && (bbox.x - 0.0).abs() < 0.01
                    && (bbox.width - 10.0).abs() < 0.01
            }
            _ => false,
        }));
    }
}
