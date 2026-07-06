use super::stats::{DocumentStats, VisualLine};
use crate::TextSegment;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum TextLevel {
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Body,
    SubBody,
}

impl TextLevel {
    pub fn is_heading(&self) -> bool {
        matches!(
            self,
            TextLevel::H1
                | TextLevel::H2
                | TextLevel::H3
                | TextLevel::H4
                | TextLevel::H5
                | TextLevel::H6
        )
    }
}

/// Context about the next line, used for standalone detection
#[derive(Debug, Clone)]
pub struct NextLineContext {
    pub starts_at_left_margin: bool,
    pub y_gap: f64,
    pub exists: bool,
}

impl Default for NextLineContext {
    fn default() -> Self {
        Self {
            starts_at_left_margin: true,
            y_gap: 0.0,
            exists: false,
        }
    }
}

/// Classify a visual line as heading or body text.
///
/// Uses geometric/visual features rather than just font properties:
/// - Rendered height (bounding box) compared to body text
/// - Line width compared to typical paragraph width
/// - Whether the line is standalone (next line starts at left margin)
/// - Whether the line has consistent styling (not mixed inline formatting)
/// - Bold/emphasis detection
pub fn classify_line(
    line: &VisualLine,
    next_line: Option<&VisualLine>,
    doc_stats: &DocumentStats,
) -> TextLevel {
    let text = line.text();
    let trimmed = text.trim();
    let word_count = line.word_count();

    // === BASIC GUARDS ===

    // Empty content
    if trimmed.is_empty() {
        return TextLevel::Body;
    }

    // Starts with lowercase = continuation, not heading
    if trimmed
        .chars()
        .find(|c| c.is_alphabetic())
        .map_or(false, |c| c.is_lowercase())
    {
        return TextLevel::Body;
    }

    // Too long to be a heading (>12 words)
    if word_count > 12 {
        return TextLevel::Body;
    }

    // Paragraph/list numbers are never headings (e.g., "1.", "2.", "10.")
    let is_number_only = trimmed
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == ')' || c == '-' || c.is_whitespace());
    if is_number_only && trimmed.len() <= 5 {
        return TextLevel::Body;
    }

    // Text ending with sentence-continuing punctuation
    if trimmed.ends_with(',') || trimmed.ends_with('-') || trimmed.ends_with(':') {
        return TextLevel::Body;
    }

    // === GEOMETRIC CHECKS ===

    // Check for mixed font sizes in line (indicates inline formatting, not heading)
    // Tolerance of 1.0 point for font size variations
    if !line.has_consistent_font_size(1.0) {
        return TextLevel::Body;
    }

    // Height ratio: compare rendered height to body line height
    let height_ratio = line.max_height / doc_stats.body_line_height.max(1.0);

    // Width ratio: compare line width to typical paragraph width
    let width_ratio = line.total_width / doc_stats.body_line_width.max(1.0);

    // Is this line "short" (less than 70% of typical paragraph width)?
    let is_short = width_ratio < 0.70;

    // Is this line visually taller (rendered larger)?
    let is_taller = height_ratio > 1.08;

    // Is any segment bold?
    let is_bold = line.has_bold();

    // Is the line ALL CAPS? (common heading style, especially in legal docs)
    let alpha_chars: String = trimmed.chars().filter(|c| c.is_alphabetic()).collect();
    let is_all_caps = !alpha_chars.is_empty() && alpha_chars.chars().all(|c| c.is_uppercase());

    // Margin tolerance for detecting left-aligned text
    let margin_tolerance = doc_stats.body_line_width * 0.15; // 15% tolerance

    // Is this a standalone line? (next line starts at left margin, below)
    let is_standalone = match next_line {
        Some(next) => {
            let at_margin = (next.min_x - doc_stats.left_margin).abs() < margin_tolerance;
            let y_gap = (line.y - next.y).abs();
            // Next line starts at margin = current line is standalone
            // Also consider larger Y gaps as section breaks
            at_margin || y_gap > doc_stats.body_line_height * 1.5
        }
        None => true, // Last line is considered standalone
    };

    // === HEADING CLASSIFICATION ===

    // Smaller than body text = not a heading
    if height_ratio < 0.95 {
        return TextLevel::SubBody;
    }

    // Primary signal: visually taller AND short AND standalone
    if is_taller && is_short && is_standalone {
        if height_ratio > 1.20 {
            return TextLevel::H1;
        } else {
            return TextLevel::H2;
        }
    }

    // Secondary signal: bold AND short AND standalone (even if same height as body)
    if is_bold && is_short && is_standalone && word_count <= 10 {
        return TextLevel::H2;
    }

    // Tertiary signal: ALL CAPS AND short (common in legal docs)
    // Being short + ALL CAPS is a strong signal even without strict standalone check
    // because short lines can't be flowing paragraphs
    if is_all_caps && is_short && word_count <= 10 {
        return TextLevel::H2;
    }

    // Quaternary signal: significantly taller even if not perfectly short
    // (for centered headings that might span more width)
    if height_ratio > 1.25 && is_standalone && word_count <= 10 {
        return TextLevel::H1;
    }

    TextLevel::Body
}

/// Legacy function for backward compatibility.
/// Classifies a single segment based on font properties.
/// For better accuracy, use classify_line() with visual line context.
pub(crate) fn is_heading(segment: &TextSegment, doc_stats: &DocumentStats) -> TextLevel {
    let trimmed = segment.content.trim();
    let word_count = trimmed.split_whitespace().count();

    // === BASIC GUARDS ===

    // Empty content
    if trimmed.is_empty() {
        return TextLevel::Body;
    }

    // Starts with lowercase = continuation, not heading
    if trimmed
        .chars()
        .find(|c| c.is_alphabetic())
        .map_or(false, |c| c.is_lowercase())
    {
        return TextLevel::Body;
    }

    // Too long to be a heading (>12 words)
    if word_count > 12 {
        return TextLevel::Body;
    }

    // Paragraph/list numbers are never headings (e.g., "1.", "2.", "10.")
    let is_number_only = trimmed
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == ')' || c == '-');
    if is_number_only && trimmed.len() <= 5 {
        return TextLevel::Body;
    }

    // Text ending with hyphen is a word continuation
    if trimmed.ends_with('-') {
        return TextLevel::Body;
    }

    // Text ending with comma is mid-sentence
    if trimmed.ends_with(',') {
        return TextLevel::Body;
    }

    // === GEOMETRIC-BASED DETECTION ===

    // Use bounding box height instead of font size
    let height_ratio = segment.height / doc_stats.body_line_height.max(1.0);

    // Width ratio for "short" detection
    let width_ratio = segment.width / doc_stats.body_line_width.max(1.0);
    let is_short = width_ratio < 0.70;

    // Smaller than body text = not a heading
    if height_ratio < 0.95 {
        return TextLevel::SubBody;
    }

    // Same height as body (within 8%) - only heading if bold AND short
    if height_ratio < 1.08 {
        if segment.is_bold && is_short && word_count <= 10 {
            return TextLevel::H2;
        }
        return TextLevel::Body;
    }

    // Taller than body - potential heading if reasonably short
    if !is_short && word_count > 8 {
        return TextLevel::Body;
    }

    // Two-level heading hierarchy based on height
    if height_ratio > 1.20 {
        return TextLevel::H1;
    } else if height_ratio > 1.08 {
        return TextLevel::H2;
    }

    TextLevel::Body
}

/// Classify all lines in a document and return a mapping from line index to TextLevel
pub fn classify_all_lines(lines: &[VisualLine], doc_stats: &DocumentStats) -> Vec<TextLevel> {
    let mut classifications = Vec::with_capacity(lines.len());

    for (i, line) in lines.iter().enumerate() {
        let next_line = lines.get(i + 1);
        let level = classify_line(line, next_line, doc_stats);
        classifications.push(level);
    }

    classifications
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::stats::LineSegmentInfo;
    use std::collections::HashMap;

    fn doc_stats() -> DocumentStats {
        DocumentStats {
            font_stats: HashMap::new(),
            body_font: "Test".to_string(),
            body_size: 10.0,
            body_transformed_size: 10.0,
            transformed_thresholds: Vec::new(),
            font_heading_thresholds: HashMap::<String, Vec<f64>>::new(),
            body_line_height: 10.0,
            body_line_width: 400.0,
            left_margin: 50.0,
            right_margin: 450.0,
            line_height_tolerance: 2.0,
        }
    }

    fn line(text: &str, height: f64, width: f64, y: f64, is_bold: bool) -> VisualLine {
        VisualLine {
            segments: vec![LineSegmentInfo {
                content: text.to_string(),
                x: 50.0,
                width,
                height,
                font_size: height,
                is_bold,
                font_name: "Test".to_string(),
            }],
            y,
            min_x: 50.0,
            max_x: 50.0 + width,
            max_height: height,
            total_width: width,
            page_num: 1,
        }
    }

    fn mixed_font_line(text: &str) -> VisualLine {
        VisualLine {
            segments: vec![
                LineSegmentInfo {
                    content: text.to_string(),
                    x: 50.0,
                    width: 70.0,
                    height: 10.0,
                    font_size: 10.0,
                    is_bold: false,
                    font_name: "Test".to_string(),
                },
                LineSegmentInfo {
                    content: "inline".to_string(),
                    x: 125.0,
                    width: 40.0,
                    height: 10.0,
                    font_size: 12.0,
                    is_bold: true,
                    font_name: "Test-Bold".to_string(),
                },
            ],
            y: 100.0,
            min_x: 50.0,
            max_x: 165.0,
            max_height: 10.0,
            total_width: 115.0,
            page_num: 1,
        }
    }

    #[test]
    fn classify_line_uses_geometric_heading_signals() {
        let stats = doc_stats();
        let next = line("The body starts here", 10.0, 360.0, 84.0, false);

        let cases = [
            (
                line("JUDGMENT", 13.0, 120.0, 100.0, false),
                Some(&next),
                TextLevel::H1,
            ),
            (
                line("Brief reasons", 11.0, 160.0, 100.0, false),
                Some(&next),
                TextLevel::H2,
            ),
            (
                line("Issues", 10.0, 100.0, 100.0, true),
                Some(&next),
                TextLevel::H2,
            ),
            (
                line("INTERIM ORDER", 10.0, 170.0, 100.0, false),
                None,
                TextLevel::H2,
            ),
        ];

        for (line, next_line, expected) in cases {
            assert_eq!(classify_line(&line, next_line, &stats), expected);
        }
    }

    #[test]
    fn classify_line_rejects_body_and_numeric_false_positives() {
        let stats = doc_stats();
        let next = line("continues away from margin", 10.0, 360.0, 84.0, false);

        let long_heading_like =
            "This Heading Has Far Too Many Words To Be A Safe Heading Candidate";
        let cases = [
            (
                line("lowercase continuation", 13.0, 120.0, 100.0, true),
                TextLevel::Body,
            ),
            (
                line(long_heading_like, 13.0, 260.0, 100.0, true),
                TextLevel::Body,
            ),
            (line("1.", 13.0, 40.0, 100.0, true), TextLevel::Body),
            (line("Clause:", 13.0, 80.0, 100.0, true), TextLevel::Body),
            (mixed_font_line("Mixed"), TextLevel::Body),
        ];

        for (line, expected) in cases {
            assert_eq!(classify_line(&line, Some(&next), &stats), expected);
        }
    }
}
