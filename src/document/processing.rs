use super::{
    analysis::{classify_line, is_heading},
    columns::{detect_columns, reorder_by_columns},
    hyphenation::recover_hyphenation,
    stats::{calculate_document_stats, group_into_visual_lines},
    tables::{detect_tables, table_to_located_markdown},
    HeaderFooterDetector, TextLevel,
};
use crate::chunk_accumulator::{
    contains_sentence_end, count_words as unicode_count_words, split_long_sentence,
    split_text_hard_capped, ChunkAccumulator,
};
use crate::document::{
    compact_output_spans, LocatedText, OutputSpan, SourceRef, SpanSource, SyntheticKind,
};
use crate::form::form_fields;
use crate::heading_hierarchy::HeaderHierarchy;
use crate::{
    create_content_core_with_identity, create_content_ext_with_spans,
    create_pdf_location_from_output_spans, create_pdf_location_from_positions, get_inherited,
    get_page_rotation, BoundingBox, ExtractionOptions, ExtractionResult, MediaBox, OcrHandler,
    OcrImageTelemetry, OutputError, PagePosition, Processor, TextSegment,
};
use log::{debug, error, info, warn};
use lopdf::{Dictionary, Document};
use rayon::prelude::*;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::LazyLock;
use unicode_segmentation::UnicodeSegmentation;

static RE_MULTI_NEWLINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{2,}").unwrap());
static RE_MULTI_SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]{2,}").unwrap());
static RE_EXCESS_DOTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.{4,}").unwrap());
static RE_EXCESS_UNDERSCORES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_{4,}").unwrap());
static RE_EXCESS_DASHES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-{4,}").unwrap());
static RE_EXCESS_EQUALS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"={4,}").unwrap());
static RE_SPACE_BEFORE_NEWLINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" +\n").unwrap());
static RE_SPACE_AFTER_NEWLINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n +").unwrap());
static OUTPUT_TOKENIZER: LazyLock<Option<tiktoken_rs::CoreBPE>> =
    LazyLock::new(|| tiktoken_rs::get_bpe_from_model("gpt-4o").ok());

// Helper: ASCII whitespace detection
fn is_ascii_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// Clean text for indexing by normalizing whitespace and special characters
/// while preserving readability and paragraph structure.
///
/// - Multiple spaces → single space
/// - Multiple newlines → single newline (preserve paragraph breaks)
/// - Excessive dots/periods (4+) → ellipsis (...)
/// - Remove other excessive repeated punctuation
/// - Normalize unicode whitespace to ASCII
/// - Recover soft hyphens (word continuations across lines)
pub fn clean_text_for_indexing(text: &str) -> String {
    use super::hyphenation::clean_internal_hyphens;

    // First, clean internal soft hyphens (word continuations across lines)
    let text = clean_internal_hyphens(text);
    let text = strip_problematic_control_chars(&text);

    // Replace unicode whitespace with ASCII equivalents
    // \u{00A0} = non-breaking space, \u{2000}-\u{200B} = various spaces
    let text = text
        .replace('\u{00A0}', " ") // non-breaking space
        .replace('\u{2009}', " ") // thin space
        .replace('\u{200A}', " ") // hair space
        .replace('\u{202F}', " ") // narrow no-break space
        .replace('\u{205F}', " "); // medium mathematical space

    // Multiple newlines → single newline (preserve paragraph breaks)
    let text = RE_MULTI_NEWLINES.replace_all(&text, "\n").to_string();

    // Multiple spaces → single space (but preserve newlines)
    let text = RE_MULTI_SPACES.replace_all(&text, " ").to_string();

    // Excessive dots (4 or more) → ellipsis
    let text = RE_EXCESS_DOTS.replace_all(&text, "...").to_string();

    // Excessive underscores (4 or more) → three underscores
    let text = RE_EXCESS_UNDERSCORES.replace_all(&text, "___").to_string();

    // Excessive dashes (4 or more) → three dashes
    let text = RE_EXCESS_DASHES.replace_all(&text, "---").to_string();

    // Excessive equals signs (4 or more) → three equals
    let text = RE_EXCESS_EQUALS.replace_all(&text, "===").to_string();

    // Clean up spaces around newlines
    let text = RE_SPACE_BEFORE_NEWLINE.replace_all(&text, "\n").to_string();
    let text = RE_SPACE_AFTER_NEWLINE.replace_all(&text, "\n").to_string();

    text
}

fn strip_problematic_control_chars(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

// Find earliest safe forward boundary with conservative guards
fn find_forward_boundary(text: &str) -> Option<usize> {
    if text.is_empty() {
        return None;
    }
    if let Some(pos) = text.find("\n\n") {
        return Some(pos + 2);
    }

    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut best: Option<usize> = None;

    let mut consider = |i: usize, mut j: usize| {
        // Skip closing quotes/brackets
        while j < n {
            let c = bytes[j];
            if c == b'\'' || c == b'\"' || c == b')' || c == b']' {
                j += 1;
            } else {
                break;
            }
        }
        if j < n && !is_ascii_ws(bytes[j]) {
            return;
        }
        if best.is_none() || j < best.unwrap() {
            best = Some(j);
        }
    };

    let mut i = 0usize;
    while i < n {
        let b = bytes[i];
        if b == b'.' || b == b'!' || b == b'?' {
            consider(i, i + 1);
            i += 1;
            continue;
        }
        if b == b';' {
            consider(i, i + 1);
            i += 1;
            continue;
        }
        if b == b':' {
            let prev_digit = i > 0 && bytes[i - 1].is_ascii_digit();
            let next_digit = i + 1 < n && bytes[i + 1].is_ascii_digit();
            if !(prev_digit && next_digit) {
                consider(i, i + 1);
            }
            i += 1;
            continue;
        }
        if b == 0xE2 && i + 2 < n {
            let b1 = bytes[i + 1];
            let b2 = bytes[i + 2];
            if b1 == 0x80 && (b2 == 0x94 || b2 == 0x93) {
                let j = i + 3;
                if j >= n || is_ascii_ws(bytes[j]) {
                    consider(i, j);
                }
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    best
}

#[derive(Clone, Debug)]
pub(crate) struct PageText {
    pub segments: Vec<TextSegment>,
    pub page_num: u32,
    pub media_box: MediaBox,
}

/// Merge continuation segments: segments ending with space (continuation marker from layout analysis)
/// should be joined with the following segment if it doesn't start a new paragraph
/// AND they are at similar Y positions (same visual line).
fn merge_continuation_segments(segments: Vec<TextSegment>) -> Vec<TextSegment> {
    if segments.is_empty() {
        return segments;
    }

    let mut result: Vec<TextSegment> = Vec::new();
    let mut current: Option<TextSegment> = None;

    for seg in segments {
        match current.take() {
            None => {
                current = Some(seg);
            }
            Some(mut prev) => {
                // Check if prev ends with space (continuation marker) and not newline
                let prev_content = &prev.content;
                let ends_with_space =
                    prev_content.ends_with(' ') && !prev_content.trim_end().ends_with('\n');

                // Must be on same page
                let same_page = prev.page_num == seg.page_num;

                // Check Y proximity - only merge if segments are on the same visual line
                // Use font size as reference for line height
                let y_tolerance = prev.font_size.max(seg.font_size).max(12.0) * 0.5;
                let y_diff = (prev.y - seg.y).abs();
                let same_line = y_diff < y_tolerance;

                // Also check if the next segment starts a new paragraph
                let next_starts_para = seg.content.trim_start().chars().next().map_or(false, |c| {
                    c.is_ascii_digit() || c == '(' || c == '-' || c == '•'
                }) || seg.content.trim_start().starts_with("A.")
                    || seg.content.trim_start().starts_with("B.")
                    || seg.content.trim_start().starts_with("C.");

                let should_merge = ends_with_space && same_page && same_line && !next_starts_para;

                if should_merge {
                    // Merge: append seg's content to prev
                    prev.content.push_str(&seg.content);
                    // Keep prev's position, update end position
                    prev.char_end = seg.char_end;
                    current = Some(prev);
                } else {
                    // Don't merge, push prev and start fresh with seg
                    result.push(prev);
                    current = Some(seg);
                }
            }
        }
    }

    // Don't forget the last segment
    if let Some(last) = current {
        result.push(last);
    }

    result
}

/// Merge consecutive short ALL CAPS lines that are likely part of the same entity (e.g., party names).
/// Common pattern in legal documents: "COMPETITION COMMISSION" + "OF INDIA" should be one entity.
/// Limit to 2-3 lines max to avoid over-merging.
fn merge_title_block_entities(segments: Vec<TextSegment>) -> Vec<TextSegment> {
    if segments.is_empty() {
        return segments;
    }

    let mut result: Vec<TextSegment> = Vec::new();
    let mut current: Option<TextSegment> = None;
    let mut merge_count = 0;

    for seg in segments {
        match current.take() {
            None => {
                current = Some(seg);
                merge_count = 0;
            }
            Some(mut prev) => {
                let prev_trimmed = prev.content.trim();
                let seg_trimmed = seg.content.trim();

                // Check if both are ALL CAPS short lines (likely title block)
                let prev_all_caps = prev_trimmed
                    .chars()
                    .filter(|c| c.is_alphabetic())
                    .all(|c| c.is_uppercase());
                let seg_all_caps = seg_trimmed
                    .chars()
                    .filter(|c| c.is_alphabetic())
                    .all(|c| c.is_uppercase());

                // Both must be reasonably short and on the same page
                let prev_len = prev_trimmed.trim_start_matches('#').trim().len();
                let seg_len = seg_trimmed.trim_start_matches('#').trim().len();
                let same_page = prev.page_num == seg.page_num;

                // At least one must be very short (like "OF INDIA", "FEDERATION & ORS.")
                // This prevents merging long lines together
                let has_short = prev_len <= 20 || seg_len <= 20;
                let both_reasonable = prev_len < 40 && seg_len < 40;

                // Check if they're part of a title block pattern
                // Don't merge if prev ends with sentence punctuation or roles like "(S)"
                let prev_ends_terminal = prev_trimmed.ends_with('.')
                    || prev_trimmed.ends_with('!')
                    || prev_trimmed.ends_with('?')
                    || prev_trimmed.ends_with(')'); // Don't merge after "APPELLANT (S)"

                // Don't merge section markers (VERSUS, headings with ##)
                let is_section_marker = seg_trimmed.starts_with("##")
                    || prev_trimmed.contains("VERSUS")
                    || seg_trimmed.contains("VERSUS");

                // Limit to 2 merges (3 total lines) to avoid over-merging title blocks
                let under_limit = merge_count < 2;

                let should_merge = prev_all_caps
                    && seg_all_caps
                    && both_reasonable
                    && has_short
                    && same_page
                    && !prev_ends_terminal
                    && !is_section_marker
                    && under_limit;

                if should_merge {
                    // Merge with a space
                    if !prev.content.ends_with(' ') {
                        prev.content.push(' ');
                    }
                    prev.content.push_str(&seg.content);
                    prev.char_end = seg.char_end;
                    merge_count += 1;
                    current = Some(prev);
                } else {
                    result.push(prev);
                    current = Some(seg);
                    merge_count = 0;
                }
            }
        }
    }

    if let Some(last) = current {
        result.push(last);
    }

    result
}

fn dedup_overlapping_form_segments(segments: &mut Vec<TextSegment>) {
    let mut kept: Vec<TextSegment> = Vec::with_capacity(segments.len());
    'outer: for segment in segments.drain(..) {
        if segment.cutat.contains(":Form") {
            let norm = normalize_for_dedup(&segment.content);
            for existing in kept
                .iter()
                .filter(|existing| existing.cutat.contains(":Form"))
            {
                if existing.page_num == segment.page_num
                    && normalize_for_dedup(&existing.content) == norm
                    && bbox_iou_segments(existing, &segment) >= 0.90
                {
                    continue 'outer;
                }
            }
        }
        kept.push(segment);
    }
    *segments = kept;
}

fn normalize_for_dedup(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn bbox_iou_segments(a: &TextSegment, b: &TextSegment) -> f64 {
    let ax2 = a.x + a.width;
    let ay2 = a.y + a.height;
    let bx2 = b.x + b.width;
    let by2 = b.y + b.height;
    let ix = (ax2.min(bx2) - a.x.max(b.x)).max(0.0);
    let iy = (ay2.min(by2) - a.y.max(b.y)).max(0.0);
    let inter = ix * iy;
    let area_a = a.width.max(0.0) * a.height.max(0.0);
    let area_b = b.width.max(0.0) * b.height.max(0.0);
    let denom = area_a + area_b - inter;
    if denom <= 0.0 {
        0.0
    } else {
        inter / denom
    }
}

pub(crate) struct PostProcessor {
    header_threshold: f64,
    footer_threshold: f64,
    continuation_threshold: f64,
    header_footer_detector: Option<HeaderFooterDetector>,
}

impl PostProcessor {
    pub fn new() -> Self {
        PostProcessor {
            header_threshold: 0.85,      // Top 15% of page
            footer_threshold: 0.15,      // Bottom 15% of page
            continuation_threshold: 0.9, // Top/Bottom 10% of page for continuation detection
            header_footer_detector: None,
        }
    }

    pub fn with_header_footer_detector(mut self, detector: HeaderFooterDetector) -> Self {
        self.header_footer_detector = Some(detector);
        self
    }

    pub fn process(&self, pages: Vec<PageText>) -> Vec<ContentOutput> {
        // Sort pages and their segments
        let sorted_pages = self.sort_and_filter(pages);

        // Filter out detected headers/footers if detector is available
        let sorted_pages = if let Some(detector) = &self.header_footer_detector {
            let headers_footers = detector.analyze();
            self.filter_headers_footers(sorted_pages, headers_footers)
        } else {
            sorted_pages
        };

        let mut document_structure = Vec::new();
        let mut current_chunk = String::new();
        let header_hierarchy_alt = HeaderHierarchy::new();
        // Initialize with first page number if available
        let mut current_page_start = sorted_pages.first().map(|p| p.page_num).unwrap_or(1);
        let mut last_y = f64::MAX;

        // Track position data for multi-page sections
        let mut section_segments: Vec<TextSegment> = Vec::new();
        let mut section_start_char_pos: Option<usize> = None;
        let mut section_end_char_pos: Option<usize> = None;

        for (idx, page) in sorted_pages.clone().iter().enumerate() {
            for segment in &page.segments {
                // Detect continued text
                let is_continuation = if idx > 0 && !sorted_pages[idx - 1].segments.is_empty() {
                    let prev_page = &sorted_pages[idx - 1];
                    let prev_segment = prev_page.segments.last().unwrap();

                    // Note: Y coordinates are flipped (0 at top, increases downward)
                    // Check if current segment is at TOP of page (small Y value)
                    let page_height = page.media_box.ury - page.media_box.lly;
                    let prev_page_height = prev_page.media_box.ury - prev_page.media_box.lly;
                    let at_top_of_page =
                        segment.y < page_height * (1.0 - self.continuation_threshold);
                    // Check if previous segment was at BOTTOM of previous page (large Y value)
                    let prev_at_bottom =
                        prev_segment.y > prev_page_height * self.continuation_threshold;

                    at_top_of_page
                        && prev_at_bottom
                        && !ends_with_terminal_punctuation(&prev_segment.content)
                        && !segment.content.starts_with(|c: char| c.is_uppercase())
                } else {
                    false
                };

                if is_continuation {
                    current_chunk.push_str(" ");
                    current_chunk.push_str(&segment.content);
                    section_segments.push(segment.clone());
                    if section_end_char_pos.is_some() || segment.char_end > 0 {
                        section_end_char_pos = Some(segment.char_end);
                    }
                } else {
                    if !current_chunk.is_empty() {
                        // Calculate bounding box from segments on the starting page only
                        let bbox = if !section_segments.is_empty() {
                            // Filter segments to only include those from the starting page
                            let start_page_segments: Vec<&TextSegment> = section_segments
                                .iter()
                                .filter(|s| s.page_num == current_page_start)
                                .collect();

                            if !start_page_segments.is_empty() {
                                let min_x = start_page_segments
                                    .iter()
                                    .map(|s| s.x)
                                    .fold(f64::INFINITY, f64::min);
                                let max_x = start_page_segments
                                    .iter()
                                    .map(|s| s.x + s.width)
                                    .fold(f64::NEG_INFINITY, f64::max);
                                let min_y = start_page_segments
                                    .iter()
                                    .map(|s| s.y)
                                    .fold(f64::INFINITY, f64::min);
                                let max_y = start_page_segments
                                    .iter()
                                    .map(|s| s.y + s.height)
                                    .fold(f64::NEG_INFINITY, f64::max);

                                Some(BoundingBox {
                                    x: min_x,
                                    y: min_y,
                                    width: max_x - min_x,
                                    height: max_y - min_y,
                                })
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        document_structure.push(ContentOutput {
                            headings: header_hierarchy_alt.get_headers(),
                            paragraph: current_chunk.trim().to_string(),
                            page: current_page_start,
                            end_page: if sorted_pages[idx - 1].page_num != current_page_start {
                                Some(sorted_pages[idx - 1].page_num)
                            } else {
                                None
                            },
                            page_char_start: section_start_char_pos,
                            page_char_end: section_end_char_pos,
                            bbox,
                            page_positions: vec![], // TODO: Implement for PostProcessor
                            located_text: None,
                        });
                        current_chunk.clear();
                        section_segments.clear();
                    }
                    current_chunk = segment.content.clone();
                    current_page_start = page.page_num;
                    section_segments = vec![segment.clone()];
                    section_start_char_pos = Some(segment.char_start);
                    section_end_char_pos = Some(segment.char_end);
                }
                last_y = segment.y;
            }
        }

        // Add final chunk
        if !current_chunk.is_empty() {
            // Calculate bounding box from segments on the starting page only
            let bbox = if !section_segments.is_empty() {
                // Filter segments to only include those from the starting page
                let start_page_segments: Vec<&TextSegment> = section_segments
                    .iter()
                    .filter(|s| s.page_num == current_page_start)
                    .collect();

                if !start_page_segments.is_empty() {
                    let min_x = start_page_segments
                        .iter()
                        .map(|s| s.x)
                        .fold(f64::INFINITY, f64::min);
                    let max_x = start_page_segments
                        .iter()
                        .map(|s| s.x + s.width)
                        .fold(f64::NEG_INFINITY, f64::max);
                    let min_y = start_page_segments
                        .iter()
                        .map(|s| s.y)
                        .fold(f64::INFINITY, f64::min);
                    let max_y = start_page_segments
                        .iter()
                        .map(|s| s.y + s.height)
                        .fold(f64::NEG_INFINITY, f64::max);

                    Some(BoundingBox {
                        x: min_x,
                        y: min_y,
                        width: max_x - min_x,
                        height: max_y - min_y,
                    })
                } else {
                    None
                }
            } else {
                None
            };

            let last_page = sorted_pages.last().unwrap().page_num;
            document_structure.push(ContentOutput {
                headings: header_hierarchy_alt.get_headers(),
                paragraph: current_chunk.trim().to_string(),
                page: current_page_start,
                end_page: if last_page != current_page_start {
                    Some(last_page)
                } else {
                    None
                },
                page_char_start: section_start_char_pos,
                page_char_end: section_end_char_pos,
                bbox,
                page_positions: vec![], // TODO: Implement for PostProcessor
                located_text: None,
            });
        }

        document_structure
    }

    fn sort_and_filter(&self, mut pages: Vec<PageText>) -> Vec<PageText> {
        // Sort pages by page number
        pages.sort_by_key(|p| p.page_num);

        // First pass to detect common headers/footers
        let mut header_counts = HashMap::new();
        let mut footer_counts = HashMap::new();

        for page in &pages {
            if let Some(header) = self.find_header_candidate(page) {
                *header_counts.entry(header).or_insert(0) += 1;
            }
            if let Some(footer) = self.find_footer_candidate(page) {
                *footer_counts.entry(footer).or_insert(0) += 1;
            }
        }

        // Find headers/footers that appear on >50% of pages
        let total_pages = pages.len() as f64;
        let common_headers: HashSet<_> = header_counts
            .iter()
            .filter(|(_, &count)| count as f64 / total_pages > 0.5)
            .map(|(k, _)| k.clone())
            .collect();

        let common_footers: HashSet<_> = footer_counts
            .iter()
            .filter(|(_, &count)| count as f64 / total_pages > 0.5)
            .map(|(k, _)| k.clone())
            .collect();

        // Filter out headers/footers from all pages
        for page in &mut pages {
            page.segments.retain(|seg| {
                !common_headers.contains(&seg.content) && !common_footers.contains(&seg.content)
            });
        }

        pages
    }

    fn filter_headers_footers(
        &self,
        mut pages: Vec<PageText>,
        headers_footers: HashSet<String>,
    ) -> Vec<PageText> {
        for page in &mut pages {
            page.segments
                .retain(|seg| !headers_footers.contains(&seg.content));
        }
        pages
    }

    fn find_header_candidate(&self, page: &PageText) -> Option<String> {
        let page_height = page.media_box.ury - page.media_box.lly;
        page.segments
            .iter()
            .find(|seg| seg.y > page_height * self.header_threshold)
            .map(|seg| seg.content.clone())
    }

    fn find_footer_candidate(&self, page: &PageText) -> Option<String> {
        let page_height = page.media_box.ury - page.media_box.lly;
        page.segments
            .iter()
            .find(|seg| seg.y < page_height * self.footer_threshold)
            .map(|seg| seg.content.clone())
    }
}

/// Translate a page's extracted geometry into a nonnegative page-relative
/// coordinate space. The parser may receive text/form geometry whose local
/// origin is outside the declared page box. Moving the whole page by its
/// negative minimum preserves every distance and size; clamping individual
/// boxes would distort the geometry and break highlighting.
fn page_relative_origin(segments: &[TextSegment]) -> (f64, f64) {
    let mut origin_x = 0.0_f64;
    let mut origin_y = 0.0_f64;

    for segment in segments.iter() {
        origin_x = origin_x.min(segment.x);
        origin_y = origin_y.min(segment.y);
        if let Some(located) = &segment.located_text {
            for span in &located.spans {
                if let SpanSource::Pdf { bbox, .. } = &span.source {
                    origin_x = origin_x.min(bbox.x);
                    origin_y = origin_y.min(bbox.y);
                }
            }
        }
    }

    (origin_x, origin_y)
}

fn normalize_output_coordinates(
    outputs: &mut [ContentOutput],
    page_origins: &HashMap<u32, (f64, f64)>,
) {
    for output in outputs {
        for position in &mut output.page_positions {
            if let Some((origin_x, origin_y)) = page_origins.get(&position.page) {
                position.bbox.x -= origin_x;
                position.bbox.y -= origin_y;
            }
        }

        if let Some(located) = &mut output.located_text {
            for span in &mut located.spans {
                if let SpanSource::Pdf { page, bbox, .. } = &mut span.source {
                    if let Some((origin_x, origin_y)) = page_origins.get(page) {
                        bbox.x -= origin_x;
                        bbox.y -= origin_y;
                    }
                }
            }
        }

        output.bbox = bbox_union_from_positions(&output.page_positions);
    }
}

fn ends_with_terminal_punctuation(s: &str) -> bool {
    s.trim()
        .ends_with(|c: char| c == '.' || c == '!' || c == '?')
}

// Temporary ContentOutput structure for backward compatibility during transition
#[derive(Clone, Debug)]
pub struct ContentOutput {
    pub headings: Vec<String>,
    pub paragraph: String,
    pub page: u32,
    pub end_page: Option<u32>,
    pub page_char_start: Option<usize>,
    pub page_char_end: Option<usize>,
    pub bbox: Option<BoundingBox>,
    pub page_positions: Vec<PagePosition>,
    pub located_text: Option<LocatedText>,
}

fn extend_with_capped_output(
    outputs: &mut Vec<ContentOutput>,
    output: ContentOutput,
    max_tokens: Option<usize>,
) {
    let Some(limit) = max_tokens else {
        outputs.push(output);
        return;
    };

    let Some(tokenizer) = OUTPUT_TOKENIZER.as_ref() else {
        outputs.push(output);
        return;
    };

    if tokenizer.encode_ordinary(&output.paragraph).len() <= limit {
        outputs.push(output);
        return;
    }

    let parts = split_text_hard_capped(&output.paragraph, limit, tokenizer);
    if parts.is_empty() {
        outputs.push(output);
        return;
    }

    let split_total_chars = output.paragraph.chars().count().max(1);
    let mut split_char_cursor = 0usize;
    let mut split_search_byte = 0usize;
    for paragraph in parts {
        let paragraph_chars = paragraph.chars().count();
        let split_start_chars = locate_split_start_chars(
            &output.paragraph,
            &paragraph,
            &mut split_search_byte,
            split_char_cursor,
        );
        let page_positions = approximate_split_positions(
            &output.page_positions,
            split_start_chars,
            paragraph_chars,
            split_total_chars,
        );
        let mut part = output.clone();
        part.paragraph = paragraph;
        part.located_text = output.located_text.as_ref().map(|located| {
            located.slice_chars(split_start_chars, paragraph_chars, part.paragraph.clone())
        });
        apply_split_location_metadata(&mut part, page_positions);
        split_char_cursor = split_start_chars + paragraph_chars;
        outputs.push(part);
    }
}

fn locate_split_start_chars(
    source: &str,
    part: &str,
    search_byte: &mut usize,
    fallback_chars: usize,
) -> usize {
    if part.is_empty() || *search_byte >= source.len() {
        return fallback_chars;
    }

    if let Some(rel) = source[*search_byte..].find(part) {
        let byte_start = *search_byte + rel;
        *search_byte = byte_start + part.len();
        source[..byte_start].chars().count()
    } else {
        fallback_chars
    }
}

// Split a text so that the head fits within the given token capacity.
// Preference order: last sentence boundary within capacity, else last whitespace
fn split_to_fit(
    text: &str,
    sep: &str,
    capacity_tokens: usize,
    tokenizer: &tiktoken_rs::CoreBPE,
) -> (String, String) {
    if capacity_tokens == 0 {
        return (String::new(), text.to_string());
    }
    let total = tokenizer.encode_ordinary(text).len();
    if total <= capacity_tokens {
        return (text.to_string(), String::new());
    }
    // Prefer wide boundaries first: double newline, or sentence/clause punctuation with guards.
    let mut candidates: Vec<usize> = Vec::new();
    // Double newline candidates
    let mut start = 0usize;
    while let Some(pos) = text[start..].find("\n\n") {
        let idx = start + pos + 2; // cut after the two newlines
        candidates.push(idx);
        start = start + pos + 2;
    }
    // Sentence punctuation and strong clause candidates with guards
    let bytes = text.as_bytes();
    let n = bytes.len();
    for i in 0..n {
        let b = bytes[i];
        if b == b'.' || b == b'!' || b == b'?' {
            let mut j = i + 1;
            while j < n {
                let c = bytes[j];
                if c == b'\'' || c == b'\"' || c == b')' || c == b']' {
                    j += 1;
                } else {
                    break;
                }
            }
            if j == n || is_ascii_ws(bytes[j]) {
                candidates.push(j);
            }
            continue;
        }
        if b == b';' {
            let j = i + 1;
            if j == n || is_ascii_ws(bytes[j]) {
                candidates.push(j);
            }
            continue;
        }
        if b == b':' {
            let prev_digit = i > 0 && bytes[i - 1].is_ascii_digit();
            let next_digit = i + 1 < n && bytes[i + 1].is_ascii_digit();
            let j = i + 1;
            if (j == n || is_ascii_ws(bytes[j])) && !(prev_digit && next_digit) {
                candidates.push(j);
            }
            continue;
        }
        if b == 0xE2 && i + 2 < n {
            let b1 = bytes[i + 1];
            let b2 = bytes[i + 2];
            if b1 == 0x80 && (b2 == 0x94 || b2 == 0x93) {
                let j = i + 3;
                if j == n || is_ascii_ws(bytes[j]) {
                    candidates.push(j);
                }
            }
            continue;
        }
    }
    // Deduplicate and sort descending to try farthest boundary first
    candidates.sort_unstable();
    candidates.dedup();
    candidates.reverse();

    for idx in candidates.iter().cloned() {
        if idx > text.len() {
            continue;
        }
        let head = &text[..idx];
        let prefix = if sep.is_empty() {
            head.to_string()
        } else {
            format!("{}{}", sep, head)
        };
        let needed = tokenizer.encode_ordinary(&prefix).len();
        if needed <= capacity_tokens {
            let h = head.trim().to_string();
            let t = text[idx..].trim_start().to_string();
            return (h, t);
        }
    }

    // Fallback: grow by words until we hit capacity
    let mut used_tokens = 0usize;
    let mut cut_idx_word: Option<usize> = None;
    let mut cursor = 0usize;
    for word in text.unicode_words() {
        if let Some(rel) = text[cursor..].find(word) {
            let start = cursor + rel;
            let end = start + word.len();
            let w_tokens = tokenizer.encode_ordinary(word).len();
            let space_tokens = if used_tokens == 0 { 0 } else { 1 };
            if used_tokens + space_tokens + w_tokens > capacity_tokens {
                break;
            }
            used_tokens += space_tokens + w_tokens;
            cursor = end;
            cut_idx_word = Some(cursor);
        } else {
            break;
        }
    }
    let cut_idx = cut_idx_word.unwrap_or(0);
    if cut_idx == 0 {
        return (String::new(), text.to_string());
    }
    let head = text[..cut_idx].trim().to_string();
    let tail = text[cut_idx..].trim_start().to_string();
    (head, tail)
}

fn positioned_split_segments(segment: &TextSegment, chunks: Vec<String>) -> Vec<TextSegment> {
    let total_chars = segment.content.chars().count().max(1);
    let mut split_search_byte = 0usize;
    let mut split_char_cursor = 0usize;

    chunks
        .into_iter()
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| {
            let chunk_chars = chunk.chars().count();
            let start_chars = locate_split_start_chars(
                &segment.content,
                &chunk,
                &mut split_search_byte,
                split_char_cursor,
            )
            .min(total_chars);
            let end_chars = start_chars.saturating_add(chunk_chars).min(total_chars);
            split_char_cursor = end_chars;

            slice_text_segment_by_chars(
                segment,
                start_chars,
                end_chars.saturating_sub(start_chars),
                chunk,
            )
        })
        .collect()
}

fn slice_text_segment_by_chars(
    segment: &TextSegment,
    start_chars: usize,
    len_chars: usize,
    new_content: String,
) -> TextSegment {
    let total_chars = segment.content.chars().count().max(1);
    let start_chars = start_chars.min(total_chars);
    let end_chars = start_chars.saturating_add(len_chars).min(total_chars);
    let start_ratio = start_chars as f64 / total_chars as f64;
    let end_ratio = end_chars.max(start_chars + 1) as f64 / total_chars as f64;

    let mut partial = segment.clone();
    partial.content = new_content;
    partial.word_count = unicode_count_words(&partial.content);
    partial.char_start = segment.char_start.saturating_add(start_chars);
    partial.char_end = segment.char_start.saturating_add(end_chars);
    partial.x = segment.x + segment.width * start_ratio;
    partial.width = (segment.width * (end_ratio - start_ratio).max(0.0)).max(0.0);
    partial.located_text = segment.located_text.as_ref().map(|located| {
        located.slice_chars(
            start_chars,
            end_chars - start_chars,
            partial.content.clone(),
        )
    });
    partial
}

pub fn output_doc(
    doc: &Document,
    ocr_handler: Option<&OcrHandler>,
    max_tokens: Option<usize>,
    laparams: Option<&crate::LAParams>,
) -> Result<Vec<ContentOutput>, OutputError> {
    let ocr_assessments = ocr_assessments_for_extraction(doc, ocr_handler.is_some());
    output_doc_with_ocr_telemetry(
        doc,
        ocr_handler,
        max_tokens,
        laparams,
        None,
        ExtractionOptions::default(),
        ocr_assessments.as_ref(),
        None,
    )
    .map_err(OutputError::boxed_error)
}

/// Extract only the requested pages for bounded attribution work such as the
/// classifier. The full extraction path remains unchanged for public callers.
pub(crate) fn output_doc_for_pages(
    doc: &Document,
    pages: &[u32],
) -> Result<Vec<ContentOutput>, OutputError> {
    let page_filter = pages.iter().copied().collect::<HashSet<_>>();
    output_doc_with_ocr_telemetry(
        doc,
        None,
        None,
        None,
        None,
        ExtractionOptions::default(),
        None,
        Some(&page_filter),
    )
    .map_err(OutputError::boxed_error)
}

fn ocr_assessments_for_extraction(
    doc: &Document,
    enabled: bool,
) -> Option<HashMap<u32, crate::PageAssessment>> {
    if !enabled {
        return None;
    }
    match catch_unwind(AssertUnwindSafe(|| {
        crate::classifier::full_page_assessments(doc)
    })) {
        Ok(Ok(assessments)) => Some(assessments),
        Ok(Err(error)) => {
            warn!(
                "PDF classifier page assessment failed; continuing extraction without OCR routing hints: {:?}",
                error
            );
            None
        }
        Err(_) => {
            warn!(
                "PDF classifier page assessment panicked; continuing extraction without OCR routing hints"
            );
            None
        }
    }
}

pub(crate) fn output_doc_with_ocr_telemetry(
    doc: &Document,
    ocr_handler: Option<&OcrHandler>,
    max_tokens: Option<usize>,
    laparams: Option<&crate::LAParams>,
    ocr_telemetry: Option<&OcrImageTelemetry>,
    options: ExtractionOptions,
    ocr_assessments: Option<&HashMap<u32, crate::PageAssessment>>,
    page_filter: Option<&HashSet<u32>>,
) -> Result<Vec<ContentOutput>, Box<dyn std::error::Error>> {
    let mut document_structure: Vec<ContentOutput> = Vec::new();

    // debug!("Shaata");
    if doc.is_encrypted() {
        error!("Encrypted documents must be decrypted with a password using {{extract_text|extract_text_from_mem|output_doc}}_encrypted");
        return Err(Box::new(OutputError::Encrypted {
            reason: crate::EncryptionFailure::PasswordRequired,
            context: crate::ErrorContext {
                phase: Some("extraction".to_string()),
                ..crate::ErrorContext::default()
            },
        }));
    }
    let empty_resources = &Dictionary::new();

    let pages = doc.get_pages();
    let pages = match page_filter {
        Some(page_filter) => pages
            .into_iter()
            .filter(|(page, _)| page_filter.contains(page))
            .collect(),
        None => pages,
    };
    // trim to only first page
    // let pages: BTreeMap<u32, (u32, u16)> = pages
    //     .iter()
    //     .filter(|(&k, _)| k >= 1 && k <= 3)
    //     .map(|(&k, &v)| (k, v))
    //     .collect();
    // let toc = doc.get_toc();

    let _form = match form_fields(&doc, &mut document_structure) {
        Ok(form) => form,
        Err(e) => {
            // Silently ignore form errors - many PDFs don't have forms
            // Only print in debug builds if needed
            #[cfg(debug_assertions)]
            if !format!("{:?}", e).contains("AcroForm") {
                debug!("Form processing error: {:#?}", e);
            }
        }
    };

    // Process pages in parallel with error handling
    // Change from flat_map to map to preserve page boundaries
    let page_results: Result<Vec<(u32, f64, Vec<TextSegment>)>, OutputError> = pages
        .par_iter()
        .map(|dict| {
            let page_num = dict.0;

            // Try to get page dictionary
            let page_dict = match doc.get_object(*dict.1) {
                Ok(obj) => match obj.as_dict() {
                    Ok(dict) => dict,
                    Err(e) => {
                        error!("Failed to get page {} dictionary: {:?}", page_num, e);
                        return Ok((*page_num, 792.0, Vec::new())); // Return empty segments for this page
                    }
                },
                Err(e) => {
                    error!("Failed to get page {} object: {:?}", page_num, e);
                    return Ok((*page_num, 792.0, Vec::new())); // Return empty segments for this page
                }
            };

            let resources = get_inherited(doc, page_dict, b"Resources").unwrap_or(empty_resources);

            // Try to get media box
            let media_box = match get_inherited(doc, page_dict, b"MediaBox") {
                Some(mb) => {
                    let mb_vec: Vec<f64> = mb;
                    MediaBox {
                        llx: mb_vec[0],
                        lly: mb_vec[1],
                        urx: mb_vec[2],
                        ury: mb_vec[3],
                    }
                }
                None => {
                    error!("Page {} missing MediaBox, using default", page_num);
                    MediaBox {
                        llx: 0.0,
                        lly: 0.0,
                        urx: 612.0,  // Default US Letter width
                        ury: 792.0,  // Default US Letter height
                    }
                }
            };

            // Get page rotation (Layer 1)
            let page_rotate = get_page_rotation(page_dict, &doc);
            debug!("Page {} has rotation: {}°", page_num, page_rotate);

            let mut p = Processor::new();
            let mut page_segments = Vec::new();

            // Try to process page content
            match doc.get_page_content(*dict.1) {
                Ok(content) => {
                    let process_result = catch_unwind(AssertUnwindSafe(|| {
                        p.process_stream(
                        &doc,
                        ocr_handler,
                        ocr_telemetry,
                        ocr_route_for_page(
                            ocr_handler.is_some(),
                            ocr_assessments.and_then(|pages| pages.get(page_num)),
                        ),
                        content,
                        resources,
                        &media_box,
                        *page_num,
                        &mut page_segments,
                        page_rotate,
                        laparams,
                        None,
                        crate::StreamContext::page_with_limit(
                            options.max_recursion_depth.unwrap_or(8),
                        ),
                        )
                    }));
                    match process_result {
                        Ok(Ok(_)) => {
                            let char_count: usize = page_segments.iter().map(|s| s.content.len()).sum();
                            debug!(
                                "Successfully processed page {} with {} segments ({} chars)",
                                page_num,
                                page_segments.len(),
                                char_count
                            );
                        }
                        Ok(Err(e)) => {
                            if matches!(e, OutputError::ResourceLimit { .. }) {
                                return Err(e);
                            }
                            error!("Error processing page {} content: {:?}. Returning partial results.", page_num, e);
                            // page_segments may contain partial results
                        }
                        Err(_) => {
                            error!(
                                "Panic while processing page {} content. Returning partial results.",
                                page_num
                            );
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to get content for page {}: {:?}", page_num, e);
                    // Return empty segments for this page
                }
            }

            dedup_overlapping_form_segments(&mut page_segments);

            // Detect and handle columns for this page
            let page_width = media_box.urx - media_box.llx;
            let column_layout = detect_columns(&page_segments, page_width);

            if column_layout.is_multi_column {
                debug!(
                    "Page {} has {} columns detected",
                    page_num, column_layout.columns.len()
                );
                // Reorder segments to read column by column
                reorder_by_columns(&mut page_segments, &column_layout);
            }

            Ok((*dict.0, media_box.ury - media_box.lly, page_segments))
        })
        .collect();

    let page_results =
        page_results.map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;

    // Sort by page number and flatten while maintaining order
    let mut page_results_vec: Vec<_> = page_results.into_iter().collect();
    page_results_vec.sort_by_key(|(page_num, _, _)| *page_num);

    let page_origins: HashMap<u32, (f64, f64)> = page_results_vec
        .iter()
        .map(|(page_num, _, segments)| (*page_num, page_relative_origin(segments)))
        .collect();

    let page_heights: HashMap<u32, f64> = page_results_vec
        .iter()
        .map(|(page_num, height, _)| (*page_num, *height))
        .collect();

    let text_segments: Vec<TextSegment> = page_results_vec
        .into_iter()
        .flat_map(|(_, _, segments)| segments)
        .collect();

    // Merge continuation segments: if a segment ends with space (continuation marker)
    // and the next segment on the same page doesn't start a new paragraph, join them
    let text_segments = merge_continuation_segments(text_segments);

    // Recover hyphenated words split across lines/segments
    let text_segments = recover_hyphenation(text_segments);

    // Merge title block entities: consecutive short ALL CAPS lines (typical in legal docs)
    let text_segments = merge_title_block_entities(text_segments);

    // Create header/footer detector and analyze the document
    let mut header_footer_detector = HeaderFooterDetector::new(pages.len());

    for segment in &text_segments {
        let page_height = page_heights
            .get(&segment.page_num)
            .copied()
            .unwrap_or(792.0);
        header_footer_detector.add_occurrence(
            &segment.content,
            segment.page_num,
            segment.y,
            segment.font_size,
            page_height,
        );
    }

    // Get the detected headers and footers
    let headers_footers = header_footer_detector.analyze();

    // Optionally skip header/footer filtering for debugging
    let skip_hf = std::env::var("PDF_EXTRACT_SKIP_HEADER_FOOTER").is_ok();

    // Filter out headers and footers from text segments (unless skipped)
    let text_segments: Vec<TextSegment> = if skip_hf {
        text_segments
    } else {
        text_segments
            .into_iter()
            .filter(|seg| {
                let norm = crate::document::header_footer::HeaderFooterDetector::normalize_text(
                    &seg.content,
                );
                !headers_footers.contains(&norm)
            })
            .collect()
    };

    // The rest of the function remains sequential to ensure that the document structure is created in the correct order

    // Detect tables and format them as markdown
    // Tables are detected per-page to avoid cross-page false positives
    let text_segments = {
        let mut all_segments: Vec<TextSegment> = Vec::new();

        // Group segments by page
        let mut page_groups: HashMap<u32, Vec<TextSegment>> = HashMap::new();
        for seg in text_segments {
            page_groups.entry(seg.page_num).or_default().push(seg);
        }

        // Process each page for tables
        let mut page_nums: Vec<u32> = page_groups.keys().copied().collect();
        page_nums.sort();

        for page_num in page_nums {
            let page_segments = page_groups.remove(&page_num).unwrap_or_default();

            // Detect tables on this page
            let table_result = detect_tables(&page_segments);

            if !table_result.tables.is_empty() {
                debug!(
                    "Page {} has {} table(s) detected",
                    page_num,
                    table_result.tables.len()
                );

                // Add non-table segments
                all_segments.extend(table_result.non_table_segments);

                // For each detected table, create a markdown segment
                for table in table_result.tables {
                    let located = table_to_located_markdown(&table, page_num);
                    let md = located.text.clone();
                    if !md.is_empty() {
                        let char_start = table
                            .rows
                            .iter()
                            .flat_map(|row| row.cells.iter())
                            .filter_map(|cell| cell.char_start)
                            .min()
                            .unwrap_or(0);
                        let char_end = table
                            .rows
                            .iter()
                            .flat_map(|row| row.cells.iter())
                            .filter_map(|cell| cell.char_end)
                            .max()
                            .unwrap_or(char_start + md.chars().count());
                        let word_count = unicode_count_words(&md);
                        all_segments.push(TextSegment {
                            content: md,
                            font_size: 12.0,
                            transformed_font_size: 12.0,
                            x: table.bbox.x,
                            y: table.bbox.y,
                            is_bold: false,
                            font_name: "Table".to_string(),
                            font_weight: crate::FontWeight::Regular,
                            is_italic: false,
                            page_num,
                            cutat: String::new(),
                            fill_color: None,
                            stroke_color: None,
                            char_start,
                            char_end,
                            width: table.bbox.width,
                            height: table.bbox.height,
                            word_count,
                            located_text: Some(located),
                        });
                    }
                }
            } else {
                // No tables, keep all segments
                all_segments.extend(page_segments);
            }
        }

        // Re-sort by page and y position to maintain reading order
        all_segments.sort_by(|a, b| match a.page_num.cmp(&b.page_num) {
            std::cmp::Ordering::Equal => {
                let y_cmp = a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal);
                if y_cmp == std::cmp::Ordering::Equal {
                    a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal)
                } else {
                    y_cmp
                }
            }
            other => other,
        });

        all_segments
    };

    let mut header_hierarchy = HeaderHierarchy::new();

    // Initialize ChunkAccumulator
    let tokenizer = Arc::new(tiktoken_rs::get_bpe_from_model("gpt-4o").unwrap());
    let mut accumulator =
        ChunkAccumulator::new(max_tokens.unwrap_or(usize::MAX), tokenizer.clone());

    let doc_stats = calculate_document_stats(&text_segments);

    // Group segments into visual lines for better heading detection
    let visual_lines = group_into_visual_lines(&text_segments, doc_stats.line_height_tolerance);

    // Classify all lines and create a mapping from (page_num, y) -> TextLevel
    // We use a HashMap with rounded Y values for lookup
    let mut line_classifications: HashMap<(u32, i64), TextLevel> = HashMap::new();
    for (i, line) in visual_lines.iter().enumerate() {
        let next_line = visual_lines.get(i + 1);
        let level = classify_line(line, next_line, &doc_stats);
        // Round Y to nearest integer for lookup (segments on same line have similar Y)
        let y_key = (line.y * 10.0).round() as i64; // 0.1 precision
        line_classifications.insert((line.page_num, y_key), level);
    }

    debug!(
        "Classified {} visual lines, {} unique classifications",
        visual_lines.len(),
        line_classifications.len()
    );

    // Track consecutive headings to group them together
    let mut pending_headings: Vec<(TextLevel, TextSegment)> = Vec::new();

    // Track title block region at the top of the document.
    // Title blocks often have small non-heading elements interspersed (page numbers, dates),
    // which breaks the "3+ consecutive headings" heuristic. Instead, we track ALL headings
    // seen before substantial body content, treating them collectively as title block.
    let mut past_title_block_region = false;
    let mut title_block_headings_count: usize = 0;
    const SUBSTANTIAL_BODY_WORD_THRESHOLD: usize = 5;

    // Process segments
    for segment in text_segments {
        // Look up the line classification for this segment
        let y_key = (segment.y * 10.0).round() as i64;
        let level = line_classifications
            .get(&(segment.page_num, y_key))
            .copied()
            // Fallback to segment-based classification if not found
            .unwrap_or_else(|| is_heading(&segment, &doc_stats));

        match level {
            TextLevel::H1
            | TextLevel::H2
            | TextLevel::H3
            | TextLevel::H4
            | TextLevel::H5
            | TextLevel::H6 => {
                // Heading detected
                // If we have body content in accumulator, flush before starting new heading sequence
                if pending_headings.is_empty() && !accumulator.is_empty() {
                    extend_with_capped_output(
                        &mut document_structure,
                        accumulator.create_output(),
                        max_tokens,
                    );
                    accumulator.reset();
                }

                // Accumulate this heading (consecutive headings stay together)
                pending_headings.push((level, segment.clone()));
            }

            TextLevel::Body | TextLevel::SubBody => {
                // Skip empty/whitespace-only body segments - don't flush headings yet
                let content_trimmed = segment.content.trim();
                if content_trimmed.is_empty() && !pending_headings.is_empty() {
                    // Just add the whitespace segment, don't process headings yet
                    accumulator.add_segment(segment);
                    continue;
                }

                // Check if this is substantial body content (marks end of title block region)
                let word_count = content_trimmed.split_whitespace().count();
                let is_substantial_body = word_count >= SUBSTANTIAL_BODY_WORD_THRESHOLD;

                // Process any pending headings first (before updating title block region)
                if !pending_headings.is_empty() {
                    // Track headings seen before title block region ends
                    let was_in_title_block_region = !past_title_block_region;
                    if was_in_title_block_region {
                        title_block_headings_count += pending_headings.len();
                    }

                    // Determine if this is part of title block:
                    // - Traditional: 3+ consecutive heading-like lines
                    // - Enhanced: if in title block region AND have accumulated 3+ headings total
                    let is_title_block = pending_headings.len() >= 3
                        || (was_in_title_block_region && title_block_headings_count >= 3);

                    for (h_level, h_segment) in &pending_headings {
                        let h_text = h_segment.content.trim();
                        let heading_content = if is_title_block {
                            // Title block: no markdown heading markers, just plain text
                            format!("{}\n", h_text)
                        } else {
                            // Actual heading: add markdown markers
                            let prefix = match h_level {
                                TextLevel::H1 => "# ",
                                TextLevel::H2 => "## ",
                                TextLevel::H3 => "### ",
                                TextLevel::H4 => "#### ",
                                TextLevel::H5 => "##### ",
                                TextLevel::H6 => "###### ",
                                _ => "",
                            };
                            format!("{}{}\n", prefix, h_text)
                        };
                        let mut heading_seg = h_segment.clone();
                        heading_seg.content = heading_content;
                        heading_seg.word_count = unicode_count_words(&heading_seg.content);
                        let char_len = heading_seg.content.chars().count();
                        heading_seg.char_end = heading_seg.char_start + char_len;
                        accumulator.add_segment(heading_seg);
                    }
                    // Update hierarchy with last heading (for context tracking)
                    // Only for actual headings, not title block elements
                    if !is_title_block {
                        for (h_level, h_segment) in &pending_headings {
                            header_hierarchy.push(*h_level, h_segment.content.trim().to_string());
                        }
                        accumulator.set_headings(header_hierarchy.get_headers());
                    }
                    pending_headings.clear();
                }

                // Update title block region AFTER processing headings
                if !past_title_block_region && (is_substantial_body || segment.page_num > 1) {
                    past_title_block_region = true;
                }

                // Check if we can add this segment
                if !accumulator.can_add_segment(&segment) {
                    // Can't add - need to handle overflow
                    // First try backtracking to the last wide boundary under the cap
                    if let Some(output) = accumulator.flush_at_last_boundary() {
                        extend_with_capped_output(&mut document_structure, output, max_tokens);
                        accumulator.set_headings(header_hierarchy.get_headers());
                        // After flushing at a clean boundary, try adding again
                        if accumulator.can_add_segment(&segment) {
                            accumulator.add_segment(segment);
                            continue;
                        }
                    }
                    if accumulator.should_flush() {
                        // At sentence boundary - safe to flush
                        extend_with_capped_output(
                            &mut document_structure,
                            accumulator.create_output(),
                            max_tokens,
                        );
                        accumulator.reset();
                        accumulator.set_headings(header_hierarchy.get_headers());

                        // Check if the segment itself exceeds limits before adding
                        let segment_tokens = tokenizer.encode_ordinary(&segment.content).len();
                        if segment_tokens > max_tokens.unwrap_or(usize::MAX) {
                            // Split the large segment
                            let chunks = split_long_sentence(
                                &segment.content,
                                max_tokens.unwrap_or(usize::MAX),
                                &tokenizer,
                            );

                            for partial_segment in positioned_split_segments(&segment, chunks) {
                                accumulator.add_segment(partial_segment);
                                extend_with_capped_output(
                                    &mut document_structure,
                                    accumulator.create_output(),
                                    max_tokens,
                                );
                                accumulator.reset();
                                accumulator.set_headings(header_hierarchy.get_headers());
                            }
                        } else {
                            accumulator.add_segment(segment);
                        }
                    } else if contains_sentence_end(&segment.content)
                        && accumulator.can_force_add_for_sentence(&segment)
                    {
                        // Segment completes a sentence - check if we can force add it
                        let segment_tokens = tokenizer.encode_ordinary(&segment.content).len();
                        if segment_tokens > max_tokens.unwrap_or(usize::MAX) {
                            // Even though it completes a sentence, it's too large - must split
                            extend_with_capped_output(
                                &mut document_structure,
                                accumulator.create_output(),
                                max_tokens,
                            );
                            accumulator.reset();
                            accumulator.set_headings(header_hierarchy.get_headers());

                            let chunks = split_long_sentence(
                                &segment.content,
                                max_tokens.unwrap_or(usize::MAX),
                                &tokenizer,
                            );

                            for partial_segment in positioned_split_segments(&segment, chunks) {
                                accumulator.add_segment(partial_segment);
                                extend_with_capped_output(
                                    &mut document_structure,
                                    accumulator.create_output(),
                                    max_tokens,
                                );
                                accumulator.reset();
                                accumulator.set_headings(header_hierarchy.get_headers());
                            }
                        } else {
                            // Force add it since it completes a sentence and fits
                            accumulator.add_segment(segment);
                            extend_with_capped_output(
                                &mut document_structure,
                                accumulator.create_output(),
                                max_tokens,
                            );
                            accumulator.reset();
                            accumulator.set_headings(header_hierarchy.get_headers());
                        }
                    } else {
                        // Try to carve a clean prefix to fit current chunk before hard flush
                        let remaining = max_tokens
                            .unwrap_or(usize::MAX)
                            .saturating_sub(accumulator.get_token_count().unwrap_or(0));
                        let (prefix, rest) =
                            split_to_fit(&segment.content, " ", remaining, &tokenizer);
                        if !prefix.is_empty() {
                            let mut split_search_byte = 0usize;
                            let prefix_chars = prefix.chars().count();
                            let prefix_start_chars = locate_split_start_chars(
                                &segment.content,
                                &prefix,
                                &mut split_search_byte,
                                0,
                            );
                            let seg1 = slice_text_segment_by_chars(
                                &segment,
                                prefix_start_chars,
                                prefix_chars,
                                prefix,
                            );
                            accumulator.add_segment(seg1);
                            extend_with_capped_output(
                                &mut document_structure,
                                accumulator.create_output(),
                                max_tokens,
                            );
                            accumulator.reset();
                            accumulator.set_headings(header_hierarchy.get_headers());

                            if !rest.is_empty() {
                                // Handle remainder like a normal segment
                                let rest_chars = rest.chars().count();
                                let rest_start_chars = locate_split_start_chars(
                                    &segment.content,
                                    &rest,
                                    &mut split_search_byte,
                                    prefix_start_chars + prefix_chars,
                                );
                                let seg2 = slice_text_segment_by_chars(
                                    &segment,
                                    rest_start_chars,
                                    rest_chars,
                                    rest,
                                );
                                let test_tokens = tokenizer.encode_ordinary(&seg2.content).len();
                                if test_tokens > max_tokens.unwrap_or(usize::MAX) {
                                    let chunks = split_long_sentence(
                                        &seg2.content,
                                        max_tokens.unwrap_or(usize::MAX),
                                        &tokenizer,
                                    );
                                    for partial in positioned_split_segments(&seg2, chunks) {
                                        accumulator.add_segment(partial);
                                        extend_with_capped_output(
                                            &mut document_structure,
                                            accumulator.create_output(),
                                            max_tokens,
                                        );
                                        accumulator.reset();
                                        accumulator.set_headings(header_hierarchy.get_headers());
                                    }
                                } else {
                                    accumulator.add_segment(seg2);
                                }
                            }
                        } else {
                            // Hard flush, then handle the segment as usual
                            extend_with_capped_output(
                                &mut document_structure,
                                accumulator.create_output_with_warning(),
                                max_tokens,
                            );
                            accumulator.reset();
                            accumulator.set_headings(header_hierarchy.get_headers());

                            // Check if segment itself is too large
                            if segment.word_count > 0 {
                                let test_tokens = tokenizer.encode_ordinary(&segment.content).len();
                                debug!(
                                    "Segment has {} tokens (max: {:?})",
                                    test_tokens, max_tokens
                                );
                                if test_tokens > max_tokens.unwrap_or(usize::MAX) {
                                    debug!("Splitting large segment with {} tokens", test_tokens);
                                    // Split the large segment
                                    let chunks = split_long_sentence(
                                        &segment.content,
                                        max_tokens.unwrap_or(usize::MAX),
                                        &tokenizer,
                                    );

                                    for partial_segment in
                                        positioned_split_segments(&segment, chunks)
                                    {
                                        accumulator.add_segment(partial_segment);
                                        extend_with_capped_output(
                                            &mut document_structure,
                                            accumulator.create_output(),
                                            max_tokens,
                                        );
                                        accumulator.reset();
                                        accumulator.set_headings(header_hierarchy.get_headers());
                                    }
                                } else {
                                    accumulator.add_segment(segment);
                                }
                            }
                        }
                    }
                } else {
                    // Normal case - consider look-ahead ending before adding full segment
                    let mut handled = false;
                    if let (Some(max_toks), Some(current_tokens)) =
                        (max_tokens, accumulator.get_token_count())
                    {
                        // Trigger window when we're near capacity
                        if current_tokens > (max_toks * 85 / 100) {
                            // Configurable small token window
                            let lookahead_n: usize = std::env::var("PDF_EXTRACT_LOOKAHEAD_TOKENS")
                                .ok()
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(12);
                            if let Some(k) = find_forward_boundary(&segment.content) {
                                if k > 0 {
                                    let prefix = &segment.content[..k];
                                    // Conservative space token: assume a join space if accumulator non-empty
                                    let sep_tokens = if accumulator.is_empty() { 0 } else { 1 };
                                    let needed =
                                        sep_tokens + tokenizer.encode_ordinary(prefix).len();
                                    if needed <= lookahead_n && current_tokens + needed <= max_toks
                                    {
                                        // Split incoming segment into prefix + remainder
                                        let prefix_chars = prefix.chars().count();
                                        let seg1 = slice_text_segment_by_chars(
                                            &segment,
                                            0,
                                            prefix_chars,
                                            prefix.to_string(),
                                        );

                                        // Add small prefix, then flush
                                        accumulator.add_segment(seg1);
                                        extend_with_capped_output(
                                            &mut document_structure,
                                            accumulator.create_output(),
                                            max_tokens,
                                        );
                                        accumulator.reset();
                                        accumulator.set_headings(header_hierarchy.get_headers());

                                        // Handle remainder in the fresh chunk
                                        let rest = &segment.content[k..];
                                        if !rest.is_empty() {
                                            let rest_chars = rest.chars().count();
                                            let seg2 = slice_text_segment_by_chars(
                                                &segment,
                                                prefix_chars,
                                                rest_chars,
                                                rest.to_string(),
                                            );
                                            accumulator.add_segment(seg2);
                                        }
                                        handled = true;
                                    }
                                }
                            }
                        }
                    }
                    if !handled {
                        // Just add the segment (allow multi-page chunks)
                        accumulator.add_segment(segment);
                    }

                    // Proactive flush logic: enforce strict cap, then boundary-based flush
                    if let Some(tokens) = accumulator.get_token_count() {
                        if let Some(max_toks) = max_tokens {
                            if tokens >= max_toks {
                                // Strict cap: flush immediately
                                extend_with_capped_output(
                                    &mut document_structure,
                                    accumulator.create_output(),
                                    max_tokens,
                                );
                                accumulator.reset();
                                accumulator.set_headings(header_hierarchy.get_headers());
                            } else if tokens > (max_toks * 9 / 10) && accumulator.should_flush() {
                                extend_with_capped_output(
                                    &mut document_structure,
                                    accumulator.create_output(),
                                    max_tokens,
                                );
                                accumulator.reset();
                                accumulator.set_headings(header_hierarchy.get_headers());
                            } else if tokens > (max_toks * 85 / 100) {
                                // Prefer flushing at the last wide boundary to avoid mid-sentence near cap
                                if let Some(output) = accumulator.flush_at_last_boundary() {
                                    extend_with_capped_output(
                                        &mut document_structure,
                                        output,
                                        max_tokens,
                                    );
                                    accumulator.set_headings(header_hierarchy.get_headers());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Handle any remaining pending headings
    if !pending_headings.is_empty() {
        // Apply same title block logic as in main loop
        let was_in_title_block_region = !past_title_block_region;
        if was_in_title_block_region {
            title_block_headings_count += pending_headings.len();
        }
        let is_title_block = pending_headings.len() >= 3
            || (was_in_title_block_region && title_block_headings_count >= 3);

        for (h_level, h_segment) in &pending_headings {
            let h_text = h_segment.content.trim();
            let heading_content = if is_title_block {
                // Title block: no markdown heading markers, just plain text
                format!("{}\n", h_text)
            } else {
                // Actual heading: add markdown markers
                let prefix = match h_level {
                    TextLevel::H1 => "# ",
                    TextLevel::H2 => "## ",
                    TextLevel::H3 => "### ",
                    TextLevel::H4 => "#### ",
                    TextLevel::H5 => "##### ",
                    TextLevel::H6 => "###### ",
                    _ => "",
                };
                format!("{}{}\n", prefix, h_text)
            };
            let mut heading_seg = h_segment.clone();
            heading_seg.content = heading_content;
            heading_seg.word_count = unicode_count_words(&heading_seg.content);
            let char_len = heading_seg.content.chars().count();
            heading_seg.char_end = heading_seg.char_start + char_len;
            accumulator.add_segment(heading_seg);
        }
        // Only update hierarchy for actual headings, not title block
        if !is_title_block {
            for (h_level, h_segment) in &pending_headings {
                header_hierarchy.push(*h_level, h_segment.content.trim().to_string());
            }
            accumulator.set_headings(header_hierarchy.get_headers());
        }
    }

    // Flush any remaining content
    if !accumulator.is_empty() {
        extend_with_capped_output(
            &mut document_structure,
            accumulator.create_output(),
            max_tokens,
        );
    }

    normalize_output_coordinates(&mut document_structure, &page_origins);

    // Return the document structure directly - no post-processing needed
    // Dump font decode summary if enabled
    crate::dump_font_decode_summary();
    Ok(document_structure)
}

fn ocr_route_for_page(
    ocr_handler_present: bool,
    assessment: Option<&crate::PageAssessment>,
) -> bool {
    ocr_handler_present && assessment.map_or(true, crate::PageAssessment::should_route_ocr)
}

fn approximate_split_positions(
    source_positions: &[PagePosition],
    split_start_chars: usize,
    split_len_chars: usize,
    total_chars: usize,
) -> Vec<PagePosition> {
    if source_positions.is_empty() || split_len_chars == 0 {
        return Vec::new();
    }

    let split_end_chars = split_start_chars + split_len_chars;
    let mut out = Vec::new();

    for pos in source_positions {
        let source_len = pos.char_end.saturating_sub(pos.char_start).max(1);
        let source_global_start = pos.char_start;
        let source_global_end = pos.char_end.max(source_global_start + source_len);
        let abs_start =
            source_global_start + split_start_chars.min(total_chars) * source_len / total_chars;
        let abs_end =
            source_global_start + split_end_chars.min(total_chars) * source_len / total_chars;
        let mut local_start = abs_start.clamp(source_global_start, source_global_end);
        if local_start >= source_global_end {
            local_start = source_global_end.saturating_sub(1).max(source_global_start);
        }
        let local_end = abs_end.clamp(local_start + 1, source_global_end);

        let start_ratio =
            (local_start.saturating_sub(source_global_start) as f64) / source_len as f64;
        let end_ratio = (local_end.saturating_sub(source_global_start) as f64) / source_len as f64;
        let width = (pos.bbox.width * (end_ratio - start_ratio).max(0.01)).max(0.01);
        let x = pos.bbox.x + pos.bbox.width * start_ratio;

        out.push(PagePosition {
            page: pos.page,
            char_start: local_start,
            char_end: local_end,
            bbox: BoundingBox {
                x,
                y: pos.bbox.y,
                width,
                height: pos.bbox.height.max(0.01),
            },
        });
    }

    out
}

fn apply_split_location_metadata(output: &mut ContentOutput, page_positions: Vec<PagePosition>) {
    if page_positions.is_empty() {
        return;
    }

    output.page = page_positions
        .iter()
        .map(|pos| pos.page)
        .min()
        .unwrap_or(output.page);
    let end_page = page_positions
        .iter()
        .map(|pos| pos.page)
        .max()
        .unwrap_or(output.page);
    output.end_page = (end_page != output.page).then_some(end_page);
    output.page_char_start = page_positions.iter().map(|pos| pos.char_start).min();
    output.page_char_end = page_positions.iter().map(|pos| pos.char_end).max();
    output.bbox = bbox_union_from_positions(&page_positions);
    output.page_positions = page_positions;
}

fn bbox_union_from_positions(page_positions: &[PagePosition]) -> Option<BoundingBox> {
    let mut iter = page_positions.iter();
    let first = iter.next()?;
    let mut min_x = first.bbox.x;
    let mut min_y = first.bbox.y;
    let mut max_x = first.bbox.x + first.bbox.width;
    let mut max_y = first.bbox.y + first.bbox.height;

    for pos in iter {
        min_x = min_x.min(pos.bbox.x);
        min_y = min_y.min(pos.bbox.y);
        max_x = max_x.max(pos.bbox.x + pos.bbox.width);
        max_y = max_y.max(pos.bbox.y + pos.bbox.height);
    }

    Some(BoundingBox {
        x: min_x,
        y: min_y,
        width: (max_x - min_x).max(0.01),
        height: (max_y - min_y).max(0.01),
    })
}

fn clean_located_text_for_indexing(located: &LocatedText) -> LocatedText {
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

fn collect_source_refs(located: &LocatedText, idx: usize, refs: &mut Vec<SourceRef>) {
    if let Some(source) = source_for_char(located, idx, ' ') {
        match source {
            SpanSource::Pdf {
                page,
                char_start,
                char_end,
                ..
            } => refs.push(SourceRef {
                page,
                char_start,
                char_end,
            }),
            SpanSource::Synthetic { parent_refs, .. } => refs.extend(parent_refs),
        }
    }
}

fn source_for_char(located: &LocatedText, idx: usize, text_char: char) -> Option<SpanSource> {
    let sliced = located.slice_chars(idx, 1, text_char.to_string());
    sliced.spans.into_iter().next().map(|span| span.source)
}

fn push_clean_span(
    spans: &mut Vec<OutputSpan>,
    output_start: usize,
    output_end: usize,
    source: SpanSource,
) {
    if let Some(last) = spans.last_mut() {
        if last.output_end == output_start && can_merge_clean_span(&last.source, &source) {
            last.output_end = output_end;
            match (&mut last.source, source) {
                (
                    SpanSource::Synthetic {
                        parent_refs: last_refs,
                        ..
                    },
                    SpanSource::Synthetic { parent_refs, .. },
                ) => last_refs.extend(parent_refs),
                _ => {}
            }
            return;
        }
    }
    spans.push(OutputSpan {
        output_start,
        output_end,
        source,
    });
}

fn can_merge_clean_span(a: &SpanSource, b: &SpanSource) -> bool {
    match (a, b) {
        (SpanSource::Synthetic { kind: ak, .. }, SpanSource::Synthetic { kind: bk, .. }) => {
            ak == bk
        }
        _ => false,
    }
}

/// New output_doc function that directly returns the new schema
pub fn output_doc_new_schema(
    doc: &Document,
    ocr_handler: Option<&OcrHandler>,
    max_tokens: Option<usize>,
    source_id: i64,
    source_type: &str,
    laparams: Option<&crate::LAParams>,
    clean_text: bool, // Whether to clean text for indexing
    options: ExtractionOptions,
) -> Result<Vec<ExtractionResult>, OutputError> {
    crate::validate_document_budgets(doc, &options)?;
    let ocr_assessments = ocr_assessments_for_extraction(doc, ocr_handler.is_some());
    output_doc_new_schema_with_ocr_telemetry(
        doc,
        ocr_handler,
        max_tokens,
        source_id,
        source_type,
        laparams,
        clean_text,
        options,
        None,
        ocr_assessments.as_ref(),
    )
    .map_err(OutputError::boxed_error)
}

pub(crate) fn output_doc_new_schema_with_ocr_telemetry(
    doc: &Document,
    ocr_handler: Option<&OcrHandler>,
    max_tokens: Option<usize>,
    source_id: i64,
    source_type: &str,
    laparams: Option<&crate::LAParams>,
    clean_text: bool,
    options: ExtractionOptions,
    ocr_telemetry: Option<&OcrImageTelemetry>,
    ocr_assessments: Option<&HashMap<u32, crate::PageAssessment>>,
) -> Result<Vec<ExtractionResult>, Box<dyn std::error::Error>> {
    // Reuse most of the existing output_doc logic but modify the return format
    let content_outputs = output_doc_with_ocr_telemetry(
        doc,
        ocr_handler,
        max_tokens,
        laparams,
        ocr_telemetry,
        options.clone(),
        ocr_assessments,
        None,
    )?;
    content_outputs_to_results(
        content_outputs,
        max_tokens,
        source_id,
        source_type,
        clean_text,
        options,
    )
}

fn content_outputs_to_results(
    content_outputs: Vec<ContentOutput>,
    max_tokens: Option<usize>,
    source_id: i64,
    source_type: &str,
    clean_text: bool,
    options: ExtractionOptions,
) -> Result<Vec<ExtractionResult>, Box<dyn std::error::Error>> {
    let mut results = Vec::new();
    let mut output_bytes = 0usize;

    for (output_idx, output) in content_outputs.into_iter().enumerate() {
        let located_base = if clean_text {
            output
                .located_text
                .as_ref()
                .map(clean_located_text_for_indexing)
        } else {
            output.located_text.clone()
        };
        let cleaned_paragraph = located_base
            .as_ref()
            .map(|located| located.text.clone())
            .unwrap_or_else(|| {
                if clean_text {
                    clean_text_for_indexing(&output.paragraph)
                } else {
                    output.paragraph.clone()
                }
            });
        output_bytes = output_bytes.saturating_add(cleaned_paragraph.len());
        if let Some(limit) = options.max_output_bytes {
            if output_bytes > limit {
                return Err(Box::new(OutputError::resource_limit(
                    crate::ResourceLimitKind::OutputBytes,
                    limit,
                    Some(output_bytes),
                )));
            }
        }
        let split_total_chars = cleaned_paragraph.chars().count().max(1);
        let paragraphs =
            if let (Some(limit), Some(tokenizer)) = (max_tokens, OUTPUT_TOKENIZER.as_ref()) {
                if tokenizer.encode_ordinary(&cleaned_paragraph).len() > limit {
                    split_text_hard_capped(&cleaned_paragraph, limit, tokenizer)
                } else {
                    vec![cleaned_paragraph.clone()]
                }
            } else {
                vec![cleaned_paragraph.clone()]
            };

        let mut split_char_cursor = 0usize;
        let mut split_search_byte = 0usize;
        let split_count = paragraphs.len();

        for (split_idx, paragraph) in paragraphs.into_iter().enumerate() {
            if paragraph.trim().is_empty() {
                continue;
            }
            let paragraph_chars = paragraph.chars().count();
            let split_start_chars = locate_split_start_chars(
                &cleaned_paragraph,
                &paragraph,
                &mut split_search_byte,
                split_char_cursor,
            );
            let page_positions = if split_count > 1 {
                approximate_split_positions(
                    &output.page_positions,
                    split_start_chars,
                    paragraph_chars,
                    split_total_chars,
                )
            } else {
                output.page_positions.clone()
            };
            split_char_cursor = split_start_chars + paragraph_chars;
            let mut located_output = output.clone();
            located_output.located_text = located_base.as_ref().map(|located| {
                located.slice_chars(split_start_chars, paragraph_chars, paragraph.clone())
            });
            apply_split_location_metadata(&mut located_output, page_positions.clone());

            let content_core = create_content_core_with_identity(
                &paragraph,
                &located_output.headings,
                source_id,
                source_type,
                Some(located_output.page),
                located_output.page_char_start,
                output_idx * 10_000 + split_idx,
            );

            let format_location = located_output
                .located_text
                .as_ref()
                .map(|located| create_pdf_location_from_output_spans(located, &page_positions))
                .unwrap_or_else(|| {
                    create_pdf_location_from_positions(
                        located_output.page,
                        located_output.end_page,
                        &page_positions,
                        paragraph.len(),
                    )
                });

            let content_ext = create_content_ext_with_spans(
                &content_core.chunk_id,
                &format_location,
                located_output.page_char_start,
                located_output.page_char_end,
                located_output.bbox.as_ref(),
                located_output.located_text.as_ref(),
                options.clone(),
            )?;

            results.push(ExtractionResult {
                content_core,
                content_ext,
                repairs: Vec::new(),
            });
        }
    }

    Ok(results)
}

/// Convenience function to parse a PDF file
pub fn parse_pdf(
    file_path: &str,
    source_id: i64,
    source_type: &str,
    ocr_config: Option<crate::OcrConfig>,
    ocr_cache: Option<&str>,
    _resume: Option<bool>, // Kept for compatibility
    max_tokens: Option<usize>,
    laparams: Option<crate::LAParams>,
    clean_text: Option<bool>, // Clean text for indexing (default: true)
    options: ExtractionOptions,
) -> Result<Vec<ExtractionResult>, OutputError> {
    let _ = ocr_cache;
    let loaded = crate::load_pdf_from_path(file_path, &options)?;
    let doc = loaded.document;
    let repairs = loaded.repairs;
    let layout_policy = laparams
        .as_ref()
        .map(|params| params.layout_fallback_policy)
        .unwrap_or(crate::LayoutFallbackPolicy::Disabled);
    let layout_requested = laparams.is_some();
    let all_texts_enabled = laparams
        .as_ref()
        .map(|params| params.all_texts)
        .unwrap_or(false);
    let ocr_telemetry = crate::OcrImageTelemetry::default();

    // Initialize OCR handler if config is provided
    let ocr_handler = if let Some(config) = ocr_config {
        Some(crate::OcrHandler::new(&config).map_err(OutputError::boxed_error)?)
    } else {
        None
    };
    let ocr_assessments = ocr_assessments_for_extraction(&doc, ocr_handler.is_some());

    let mut layout_results = output_doc_new_schema_with_ocr_telemetry(
        &doc,
        ocr_handler.as_ref(),
        max_tokens,
        source_id,
        source_type,
        laparams.as_ref(),
        clean_text.unwrap_or(true), // Default to cleaning enabled
        options.clone(),
        Some(&ocr_telemetry),
        ocr_assessments.as_ref(),
    )
    .map_err(OutputError::boxed_error)?;
    for result in &mut layout_results {
        result.repairs = repairs.clone();
    }
    let layout_chars = normalized_extraction_chars(&layout_results);

    if layout_requested && layout_policy != crate::LayoutFallbackPolicy::Disabled {
        let mut no_layout_results = output_doc_new_schema_with_ocr_telemetry(
            &doc,
            ocr_handler.as_ref(),
            max_tokens,
            source_id,
            source_type,
            None,
            clean_text.unwrap_or(true),
            options.clone(),
            Some(&ocr_telemetry),
            ocr_assessments.as_ref(),
        )
        .map_err(OutputError::boxed_error)?;
        for result in &mut no_layout_results {
            result.repairs = repairs.clone();
        }
        let no_layout_chars = normalized_extraction_chars(&no_layout_results);
        let layout_duplicate_ratio = duplicate_line_ratio_from_results(&layout_results);
        let use_fallback = match layout_policy {
            crate::LayoutFallbackPolicy::Disabled => false,
            crate::LayoutFallbackPolicy::OnTextLoss => {
                should_use_no_layout_fallback(layout_chars, no_layout_chars)
            }
            crate::LayoutFallbackPolicy::OnSuspiciousVolume => {
                should_use_no_layout_fallback(layout_chars, no_layout_chars)
                    || should_use_no_layout_inflation_fallback(
                        layout_chars,
                        no_layout_chars,
                        layout_duplicate_ratio,
                        laparams
                            .as_ref()
                            .map(|params| params.all_texts)
                            .unwrap_or(false),
                    )
            }
        };
        if use_fallback {
            log::warn!(
                "layout extraction for {} kept {} normalized chars vs {} without layout; using no-layout fallback",
                file_path,
                layout_chars,
                no_layout_chars
            );
            log_production_telemetry(
                file_path,
                &doc,
                &no_layout_results,
                max_tokens,
                layout_requested,
                all_texts_enabled,
                true,
                Some(layout_chars),
                Some(no_layout_chars),
                &ocr_telemetry,
            );
            return Ok(no_layout_results);
        }

        log_production_telemetry(
            file_path,
            &doc,
            &layout_results,
            max_tokens,
            layout_requested,
            all_texts_enabled,
            false,
            Some(layout_chars),
            Some(no_layout_chars),
            &ocr_telemetry,
        );
        return Ok(layout_results);
    }

    log_production_telemetry(
        file_path,
        &doc,
        &layout_results,
        max_tokens,
        layout_requested,
        all_texts_enabled,
        false,
        Some(layout_chars),
        None,
        &ocr_telemetry,
    );
    Ok(layout_results)
}

fn log_production_telemetry(
    file_path: &str,
    doc: &Document,
    results: &[ExtractionResult],
    max_tokens: Option<usize>,
    layout_enabled: bool,
    all_texts_enabled: bool,
    layout_fallback_used: bool,
    layout_normalized_chars: Option<usize>,
    no_layout_normalized_chars: Option<usize>,
    ocr_telemetry: &crate::OcrImageTelemetry,
) {
    let page_dimensions = crate::pdf_page_dimensions(doc);
    let telemetry = crate::production_telemetry_for_results(
        results,
        max_tokens,
        layout_enabled,
        all_texts_enabled,
        layout_fallback_used,
        layout_normalized_chars,
        no_layout_normalized_chars,
        &page_dimensions,
        &ocr_telemetry.snapshot(),
    );
    let payload = serde_json::json!({
        "event": "pdf_extraction_telemetry",
        "file_path": file_path,
        "metrics": telemetry,
    });
    info!("{}", payload);
}

fn normalized_extraction_chars(results: &[ExtractionResult]) -> usize {
    results
        .iter()
        .flat_map(|doc| doc.content_core.content.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .count()
}

fn should_use_no_layout_fallback(layout_chars: usize, no_layout_chars: usize) -> bool {
    const MIN_ABSOLUTE_LOSS: usize = 512;
    const MIN_LAYOUT_TO_NO_LAYOUT_RATIO: f64 = 0.95;

    if no_layout_chars == 0 || no_layout_chars <= layout_chars {
        return false;
    }
    let lost_chars = no_layout_chars - layout_chars;
    if lost_chars < MIN_ABSOLUTE_LOSS {
        return false;
    }
    (layout_chars as f64 / no_layout_chars as f64) < MIN_LAYOUT_TO_NO_LAYOUT_RATIO
}

fn should_use_no_layout_inflation_fallback(
    layout_chars: usize,
    no_layout_chars: usize,
    duplicate_line_ratio: f64,
    all_texts: bool,
) -> bool {
    if no_layout_chars == 0 || layout_chars <= no_layout_chars {
        return false;
    }
    let ratio = layout_chars as f64 / no_layout_chars as f64;
    if !all_texts {
        return ratio > 1.25;
    }
    ratio > 1.25 && duplicate_line_ratio >= 0.20
}

fn duplicate_line_ratio_from_results(results: &[ExtractionResult]) -> f64 {
    let lines: Vec<String> = results
        .iter()
        .flat_map(|doc| doc.content_core.content.lines())
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return 0.0;
    }
    let unique = lines.iter().collect::<std::collections::HashSet<_>>().len();
    1.0 - unique as f64 / lines.len() as f64
}

#[cfg(test)]
mod tests {
    use super::{
        apply_split_location_metadata, approximate_split_positions,
        clean_located_text_for_indexing, clean_text_for_indexing,
        duplicate_line_ratio_from_results, normalized_extraction_chars, ocr_route_for_page,
        positioned_split_segments, should_use_no_layout_fallback,
        should_use_no_layout_inflation_fallback, ContentOutput,
    };
    use crate::document::{LocatedText, OutputSpan, SpanSource, SyntheticKind};
    use crate::{
        BoundingBox, ContentExt, ExtractionResult, FontWeight, PageAssessment, PagePosition,
        TextSegment,
    };

    #[test]
    fn missing_ocr_assessment_fails_open_to_ocr_routing() {
        assert!(ocr_route_for_page(true, None));
        assert!(!ocr_route_for_page(false, None));

        let text_page = PageAssessment {
            page: 1,
            sampled: true,
            text_operator_count: 4,
            text_char_count: 40,
            path_operator_count: 0,
            image_operator_count: 0,
            image_area: 0,
            needs_ocr: false,
            ocr_reason: None,
            garble_signals: Vec::new(),
        };
        assert!(!ocr_route_for_page(true, Some(&text_page)));
    }

    #[test]
    fn clean_text_for_indexing_strips_controls() {
        let cleaned = clean_text_for_indexing("A \0\0\0 B");
        assert_eq!(cleaned, "A B");
    }

    #[test]
    fn clean_located_text_keeps_letters_source_backed_and_synthetic_spaces() {
        let source_bbox = BoundingBox {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 12.0,
        };
        let input = LocatedText {
            text: "regu-\nlation\0  x".to_string(),
            spans: vec![OutputSpan {
                output_start: 0,
                output_end: "regu-\nlation\0  x".chars().count(),
                source: SpanSource::Pdf {
                    page: 1,
                    char_start: 100,
                    char_end: 117,
                    bbox: source_bbox,
                },
            }],
        };

        let cleaned = clean_located_text_for_indexing(&input);

        assert_eq!(cleaned.text, "regulation x");
        assert!(cleaned.spans.iter().any(|span| matches!(
            span.source,
            SpanSource::Synthetic {
                kind: SyntheticKind::InsertedWhitespace,
                ..
            }
        )));
        assert!(
            cleaned
                .spans
                .iter()
                .filter(|span| matches!(span.source, SpanSource::Pdf { .. }))
                .map(|span| span.output_end - span.output_start)
                .sum::<usize>()
                >= "regulationx".chars().count()
        );
    }

    #[test]
    fn clean_located_text_does_not_collapse_pdf_bboxes() {
        let first_bbox = BoundingBox {
            x: 10.0,
            y: 20.0,
            width: 50.0,
            height: 10.0,
        };
        let second_bbox = BoundingBox {
            x: 10.0,
            y: 40.0,
            width: 50.0,
            height: 10.0,
        };
        let input = LocatedText {
            text: "ab".to_string(),
            spans: vec![
                OutputSpan {
                    output_start: 0,
                    output_end: 1,
                    source: SpanSource::Pdf {
                        page: 1,
                        char_start: 10,
                        char_end: 11,
                        bbox: first_bbox.clone(),
                    },
                },
                OutputSpan {
                    output_start: 1,
                    output_end: 2,
                    source: SpanSource::Pdf {
                        page: 1,
                        char_start: 11,
                        char_end: 12,
                        bbox: second_bbox.clone(),
                    },
                },
            ],
        };

        let cleaned = clean_located_text_for_indexing(&input);
        let pdf_bboxes = cleaned
            .spans
            .iter()
            .filter_map(|span| match &span.source {
                SpanSource::Pdf { bbox, .. } => Some(bbox),
                SpanSource::Synthetic { .. } => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(pdf_bboxes.len(), 2);
        assert_eq!(pdf_bboxes[0].y, first_bbox.y);
        assert_eq!(pdf_bboxes[1].y, second_bbox.y);
    }

    #[test]
    fn layout_fallback_requires_material_loss() {
        assert_eq!(
            crate::LAParams::default().layout_fallback_policy,
            crate::LayoutFallbackPolicy::OnSuspiciousVolume
        );
        assert_eq!(
            crate::LAParams::diagnostic_layout().layout_fallback_policy,
            crate::LayoutFallbackPolicy::Disabled
        );
        assert!(should_use_no_layout_fallback(16, 80_932));
        assert!(should_use_no_layout_fallback(36_556, 40_263));
        assert!(!should_use_no_layout_fallback(5_086, 5_134));
        assert!(!should_use_no_layout_fallback(15, 24));
        assert!(should_use_no_layout_inflation_fallback(
            130, 100, 0.0, false
        ));
        assert!(!should_use_no_layout_inflation_fallback(
            130, 100, 0.0, true
        ));
        assert!(should_use_no_layout_inflation_fallback(
            130, 100, 0.25, true
        ));
    }

    #[test]
    fn normalized_extraction_chars_collapses_whitespace() {
        let result = ExtractionResult {
            content_core: crate::create_content_core_with_identity(
                "alpha\n\n beta",
                &[],
                1,
                "file",
                Some(1),
                Some(0),
                0,
            ),
            content_ext: ContentExt {
                chunk_id: String::new(),
                ext_json: Vec::new(),
            },
            repairs: Vec::new(),
        };

        assert_eq!(
            normalized_extraction_chars(&[result]),
            "alpha beta".chars().count()
        );
    }

    #[test]
    fn split_location_metadata_uses_child_range_and_bbox() {
        let output = ContentOutput {
            headings: Vec::new(),
            paragraph: "abcdefghij".to_string(),
            page: 1,
            end_page: None,
            page_char_start: Some(10),
            page_char_end: Some(20),
            bbox: Some(BoundingBox {
                x: 0.0,
                y: 10.0,
                width: 100.0,
                height: 10.0,
            }),
            page_positions: vec![PagePosition {
                page: 1,
                char_start: 10,
                char_end: 20,
                bbox: BoundingBox {
                    x: 0.0,
                    y: 10.0,
                    width: 100.0,
                    height: 10.0,
                },
            }],
            located_text: None,
        };

        let first_positions = approximate_split_positions(&output.page_positions, 0, 5, 10);
        let second_positions = approximate_split_positions(&output.page_positions, 5, 5, 10);
        let mut first = output.clone();
        let mut second = output;

        apply_split_location_metadata(&mut first, first_positions);
        apply_split_location_metadata(&mut second, second_positions);

        assert_eq!(first.page_char_start, Some(10));
        assert_eq!(first.page_char_end, Some(15));
        assert_eq!(second.page_char_start, Some(15));
        assert_eq!(second.page_char_end, Some(20));
        assert_ne!(
            first.bbox.as_ref().unwrap().x,
            second.bbox.as_ref().unwrap().x
        );
    }

    #[test]
    fn positioned_large_segment_splits_advance_source_ranges_and_bbox() {
        let segment = TextSegment {
            content: "alpha beta gamma delta".to_string(),
            font_size: 10.0,
            transformed_font_size: 10.0,
            x: 100.0,
            y: 200.0,
            is_bold: false,
            font_name: "Helvetica".to_string(),
            font_weight: FontWeight::Regular,
            is_italic: false,
            page_num: 1,
            cutat: "test".to_string(),
            fill_color: None,
            stroke_color: None,
            char_start: 50,
            char_end: 72,
            width: 220.0,
            height: 10.0,
            word_count: 4,
            located_text: None,
        };

        let parts = positioned_split_segments(
            &segment,
            vec!["alpha beta".to_string(), "gamma delta".to_string()],
        );

        assert_eq!(parts.len(), 2);
        assert_eq!((parts[0].char_start, parts[0].char_end), (50, 60));
        assert_eq!((parts[1].char_start, parts[1].char_end), (61, 72));
        assert!(parts[1].x > parts[0].x);
        assert!(parts[0].width > 0.0);
        assert!(parts[1].width > 0.0);
    }

    #[test]
    fn positioned_large_segment_splits_slice_located_text() {
        let content = "alpha beta gamma delta";
        let segment = TextSegment {
            content: content.to_string(),
            font_size: 10.0,
            transformed_font_size: 10.0,
            x: 100.0,
            y: 200.0,
            is_bold: false,
            font_name: "Helvetica".to_string(),
            font_weight: FontWeight::Regular,
            is_italic: false,
            page_num: 1,
            cutat: "test".to_string(),
            fill_color: None,
            stroke_color: None,
            char_start: 50,
            char_end: 72,
            width: 220.0,
            height: 10.0,
            word_count: 4,
            located_text: Some(LocatedText {
                text: content.to_string(),
                spans: vec![OutputSpan {
                    output_start: 0,
                    output_end: content.chars().count(),
                    source: SpanSource::Pdf {
                        page: 1,
                        char_start: 50,
                        char_end: 72,
                        bbox: BoundingBox {
                            x: 100.0,
                            y: 200.0,
                            width: 220.0,
                            height: 10.0,
                        },
                    },
                }],
            }),
        };

        let parts = positioned_split_segments(
            &segment,
            vec!["alpha beta".to_string(), "gamma delta".to_string()],
        );

        assert_eq!(
            parts[0].located_text.as_ref().expect("first located").text,
            "alpha beta"
        );
        assert_eq!(
            parts[1].located_text.as_ref().expect("second located").text,
            "gamma delta"
        );
        assert_ne!(
            parts[0].located_text.as_ref().unwrap().spans[0].output_end,
            parts[1].located_text.as_ref().unwrap().spans[0].output_end
        );
        assert_eq!(
            parts[1].located_text.as_ref().unwrap().spans[0].output_start,
            0
        );
    }
}
