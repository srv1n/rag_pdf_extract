use crate::TextSegment;
use log::debug;
use ordered_float::OrderedFloat;
use std::collections::HashMap;

#[derive(Debug)]
pub struct FontStats {
    pub sizes: HashMap<OrderedFloat<f64>, usize>,
    pub transformed_sizes: HashMap<OrderedFloat<f64>, usize>,
    pub size_to_transformed: HashMap<OrderedFloat<f64>, f64>,
    pub total_chars: usize,
}

/// Represents a visual line on the page (segments with same Y coordinate)
#[derive(Debug, Clone)]
pub struct VisualLine {
    pub segments: Vec<LineSegmentInfo>,
    pub y: f64,
    pub min_x: f64,
    pub max_x: f64,
    pub max_height: f64,
    pub total_width: f64,
    pub page_num: u32,
}

#[derive(Debug, Clone)]
pub struct LineSegmentInfo {
    pub content: String,
    pub x: f64,
    pub width: f64,
    pub height: f64,
    pub font_size: f64,
    pub is_bold: bool,
    pub font_name: String,
}

impl VisualLine {
    /// Check if all segments in this line have consistent font sizes (within tolerance)
    pub fn has_consistent_font_size(&self, tolerance: f64) -> bool {
        if self.segments.is_empty() {
            return true;
        }
        let first_size = self.segments[0].font_size;
        self.segments.iter().all(|s| (s.font_size - first_size).abs() < tolerance)
    }

    /// Check if all segments in this line have consistent heights (within tolerance)
    pub fn has_consistent_height(&self, tolerance: f64) -> bool {
        if self.segments.is_empty() {
            return true;
        }
        let first_height = self.segments[0].height;
        self.segments.iter().all(|s| (s.height - first_height).abs() < tolerance)
    }

    /// Get total word count across all segments
    pub fn word_count(&self) -> usize {
        self.segments.iter()
            .map(|s| s.content.split_whitespace().count())
            .sum()
    }

    /// Get combined text content
    pub fn text(&self) -> String {
        self.segments.iter()
            .map(|s| s.content.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Check if any segment is bold
    pub fn has_bold(&self) -> bool {
        self.segments.iter().any(|s| s.is_bold)
    }
}

#[derive(Debug)]
pub struct DocumentStats {
    pub font_stats: HashMap<String, FontStats>,
    pub body_font: String,
    pub body_size: f64,
    pub body_transformed_size: f64,
    pub transformed_thresholds: Vec<f64>,
    pub font_heading_thresholds: HashMap<String, Vec<f64>>,
    // New geometric stats
    pub body_line_height: f64,      // Median bbox height of body text lines
    pub body_line_width: f64,       // Typical paragraph width
    pub left_margin: f64,           // Leftmost X where body text starts
    pub right_margin: f64,          // Rightmost X where body text ends
    pub line_height_tolerance: f64, // Tolerance for same-line detection
}

/// Group segments into visual lines based on Y-coordinate proximity
pub fn group_into_visual_lines(segments: &[TextSegment], y_tolerance: f64) -> Vec<VisualLine> {
    if segments.is_empty() {
        return Vec::new();
    }

    // Sort segments by page, then Y (descending for top-to-bottom), then X
    let mut sorted_segments: Vec<&TextSegment> = segments.iter().collect();
    sorted_segments.sort_by(|a, b| {
        a.page_num.cmp(&b.page_num)
            .then(b.y.partial_cmp(&a.y).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut visual_lines: Vec<VisualLine> = Vec::new();
    let mut current_line_segments: Vec<&TextSegment> = Vec::new();
    let mut current_y: f64 = sorted_segments[0].y;
    let mut current_page: u32 = sorted_segments[0].page_num;

    for seg in sorted_segments {
        // New page or Y-coordinate jump beyond tolerance = new line
        if seg.page_num != current_page || (current_y - seg.y).abs() > y_tolerance {
            // Finalize current line if not empty
            if !current_line_segments.is_empty() {
                visual_lines.push(create_visual_line(&current_line_segments, current_page));
            }
            current_line_segments = vec![seg];
            current_y = seg.y;
            current_page = seg.page_num;
        } else {
            current_line_segments.push(seg);
        }
    }

    // Don't forget the last line
    if !current_line_segments.is_empty() {
        visual_lines.push(create_visual_line(&current_line_segments, current_page));
    }

    visual_lines
}

fn create_visual_line(segments: &[&TextSegment], page_num: u32) -> VisualLine {
    let line_segments: Vec<LineSegmentInfo> = segments.iter().map(|s| LineSegmentInfo {
        content: s.content.clone(),
        x: s.x,
        width: s.width,
        height: s.height,
        font_size: s.font_size,
        is_bold: s.is_bold,
        font_name: s.font_name.clone(),
    }).collect();

    let min_x = segments.iter().map(|s| s.x).fold(f64::INFINITY, f64::min);
    let max_x = segments.iter().map(|s| s.x + s.width).fold(f64::NEG_INFINITY, f64::max);
    let max_height = segments.iter().map(|s| s.height).fold(0.0_f64, f64::max);
    let y = segments.iter().map(|s| s.y).sum::<f64>() / segments.len() as f64;

    VisualLine {
        segments: line_segments,
        y,
        min_x,
        max_x,
        max_height,
        total_width: max_x - min_x,
        page_num,
    }
}

pub fn calculate_document_stats(segments: &[TextSegment]) -> DocumentStats {
    let mut font_stats: HashMap<String, FontStats> = HashMap::new();

    // Collect font statistics
    for segment in segments {
        let font_stat = font_stats
            .entry(segment.font_name.clone())
            .or_insert_with(|| FontStats {
                sizes: HashMap::new(),
                transformed_sizes: HashMap::new(),
                size_to_transformed: HashMap::new(),
                total_chars: 0,
            });

        *font_stat
            .sizes
            .entry(OrderedFloat(segment.font_size))
            .or_insert(0) += segment.content.len();
        *font_stat
            .transformed_sizes
            .entry(OrderedFloat(segment.transformed_font_size))
            .or_insert(0) += segment.content.len();
    }

    // Collect geometric statistics: heights, widths, positions
    let mut all_heights: Vec<f64> = segments.iter()
        .filter(|s| s.height > 0.0)
        .map(|s| s.height)
        .collect();
    let mut all_x_positions: Vec<f64> = segments.iter()
        .filter(|s| s.width > 0.0)
        .map(|s| s.x)
        .collect();
    let mut all_right_positions: Vec<f64> = segments.iter()
        .filter(|s| s.width > 0.0)
        .map(|s| s.x + s.width)
        .collect();

    // Sort for percentile calculations
    all_heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    all_x_positions.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    all_right_positions.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Body line height = mode (most common height), since body text is most frequent
    // Group heights by rounding to 0.5 precision
    let body_line_height = if !all_heights.is_empty() {
        let mut height_counts: HashMap<i64, usize> = HashMap::new();
        for &h in &all_heights {
            let key = (h * 2.0).round() as i64; // 0.5 precision
            *height_counts.entry(key).or_insert(0) += 1;
        }
        let mode_key = height_counts.into_iter()
            .max_by_key(|&(_, count)| count)
            .map(|(key, _)| key)
            .unwrap_or(24); // default 12.0 * 2
        mode_key as f64 / 2.0
    } else {
        12.0 // Default fallback
    };

    // Left margin = 10th percentile of X positions (to ignore outliers)
    let left_margin = if !all_x_positions.is_empty() {
        let idx = (all_x_positions.len() as f64 * 0.10) as usize;
        all_x_positions[idx.min(all_x_positions.len() - 1)]
    } else {
        0.0
    };

    // Right margin = 90th percentile of right edge positions
    let right_margin = if !all_right_positions.is_empty() {
        let idx = (all_right_positions.len() as f64 * 0.90) as usize;
        all_right_positions[idx.min(all_right_positions.len() - 1)]
    } else {
        612.0 // Default US Letter width
    };

    // Body line width = typical paragraph width (right - left margin)
    let body_line_width = (right_margin - left_margin).max(100.0);

    // Line height tolerance for same-line detection
    // Should be small enough to not group different lines together
    // Typically 20-30% of body line height works well
    let line_height_tolerance = body_line_height * 0.25;

    debug!("Geometric stats: body_line_height={:.2}, left_margin={:.2}, right_margin={:.2}, body_line_width={:.2}",
           body_line_height, left_margin, right_margin, body_line_width);
    let (body_font, body_size, body_transformed_size) = font_stats
        .iter()
        .flat_map(|(font_name, stats)| {
            stats
                .transformed_sizes
                .iter()
                .map(move |(&size, &count)| (font_name, size, count))
        })
        .max_by_key(|&(_, _, count)| count)
        .map(|(font, size, _)| (font.clone(), size, size))
        .unwrap_or((
            "".to_string(),
            ordered_float::OrderedFloat(0.0),
            ordered_float::OrderedFloat(0.0),
        ));

    // Find the body size (most common size across all fonts)

    debug!("body_transformed_size: {}", body_transformed_size);
    // Calculate six threshold levels based on transformed sizes
    let all_transformed_sizes: Vec<f64> = font_stats
        .values()
        .flat_map(|stats| stats.transformed_sizes.keys().cloned())
        .map(|ordered_float| ordered_float.into_inner())
        .collect();
    let transformed_thresholds =
        calculate_heading_thresholds(&all_transformed_sizes, *body_transformed_size);
    debug!("transformed_thresholds: {:?}", transformed_thresholds);

    // Calculate font-specific thresholds
    let mut font_heading_thresholds = HashMap::new();
    for (font_name, stats) in font_stats.iter() {
        let font_thresholds: Vec<f64> = transformed_thresholds
            .iter()
            .filter_map(|&transformed_size| {
                stats
                    .size_to_transformed
                    .iter()
                    .find(|&(_, &t)| (t - transformed_size).abs() < 0.01)
                    .map(|(&original_size, _)| original_size.into_inner())
            })
            .collect();
        font_heading_thresholds.insert(font_name.clone(), font_thresholds);
    }

    DocumentStats {
        font_stats,
        body_font,
        body_size: *body_size,
        body_transformed_size: *body_transformed_size,
        transformed_thresholds,
        font_heading_thresholds,
        // New geometric stats
        body_line_height,
        body_line_width,
        left_margin,
        right_margin,
        line_height_tolerance,
    }
}

pub fn calculate_mode(numbers: &[f64]) -> f64 {
    let mut counts = HashMap::new();
    for &num in numbers {
        *counts.entry((num * 100.0).round() as i64).or_insert(0) += 1;
    }
    let mode = counts
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .unwrap()
        .0;
    mode as f64 / 100.0
}

pub fn calculate_heading_thresholds(sizes: &[f64], body_size: f64) -> Vec<f64> {
    // Require 25% larger than body to be considered heading (was 10%)
    // This reduces false positives in documents with many font sizes
    let multiplier = std::env::var("PDF_EXTRACT_HEADING_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(1.25);
    let mut thresholds: Vec<f64> = sizes
        .iter()
        .filter(|&&size| size > body_size * multiplier)
        .cloned()
        .collect();

    thresholds.sort_by(|a, b| b.partial_cmp(a).unwrap()); // Sort in descending order
    thresholds.dedup(); // Remove duplicates

    if thresholds.len() <= 6 {
        thresholds
    } else {
        // Group into 6 levels
        let step = thresholds.len() / 6;
        let mut result: Vec<f64> = (0..6).map(|i| thresholds[i * step]).collect();
        result.dedup(); // Remove any potential duplicates after grouping
        result
    }
}
