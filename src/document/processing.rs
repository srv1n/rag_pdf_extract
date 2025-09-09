use super::{
    analysis::is_heading, stats::calculate_document_stats, HeaderFooterDetector, TextLevel,
};
use crate::chunk_accumulator::{
    contains_sentence_end, count_words as unicode_count_words, split_long_sentence, ChunkAccumulator,
};
use crate::form::form_fields;
use crate::heading_hierarchy::HeaderHierarchy;
use crate::{
    create_content_core, create_content_ext, create_pdf_location_from_positions, get_inherited,
    get_page_rotation, BoundingBox, ExtractionResult, MediaBox, OcrHandler, PagePosition,
    Processor, TextSegment,
};
use log::{debug, error};
use lopdf::{Dictionary, Document};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

// Helper: ASCII whitespace detection
fn is_ascii_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

// Find earliest safe forward boundary with conservative guards
fn find_forward_boundary(text: &str) -> Option<usize> {
    if text.is_empty() { return None; }
    if let Some(pos) = text.find("\n\n") { return Some(pos + 2); }

    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut best: Option<usize> = None;

    let mut consider = |i: usize, mut j: usize| {
        // Skip closing quotes/brackets
        while j < n {
            let c = bytes[j];
            if c == b'\'' || c == b'\"' || c == b')' || c == b']' { j += 1; } else { break; }
        }
        if j < n && !is_ascii_ws(bytes[j]) { return; }
        if best.is_none() || j < best.unwrap() { best = Some(j); }
    };

    let mut i = 0usize;
    while i < n {
        let b = bytes[i];
        if b == b'.' || b == b'!' || b == b'?' {
            consider(i, i + 1);
            i += 1; continue;
        }
        if b == b';' {
            consider(i, i + 1);
            i += 1; continue;
        }
        if b == b':' {
            let prev_digit = i > 0 && bytes[i - 1].is_ascii_digit();
            let next_digit = i + 1 < n && bytes[i + 1].is_ascii_digit();
            if !(prev_digit && next_digit) { consider(i, i + 1); }
            i += 1; continue;
        }
        if b == 0xE2 && i + 2 < n {
            let b1 = bytes[i + 1];
            let b2 = bytes[i + 2];
            if b1 == 0x80 && (b2 == 0x94 || b2 == 0x93) {
                let j = i + 3;
                if j >= n || is_ascii_ws(bytes[j]) { consider(i, j); }
                i += 3; continue;
            }
        }
        i += 1;
    }
    best
}

#[derive(Clone, Debug)]
pub struct PageText {
    pub segments: Vec<TextSegment>,
    pub page_num: u32,
    pub media_box: MediaBox,
}

pub struct PostProcessor {
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
                    let at_top_of_page =
                        segment.y < page.media_box.ury * (1.0 - self.continuation_threshold);
                    // Check if previous segment was at BOTTOM of previous page (large Y value)
                    let prev_at_bottom =
                        prev_segment.y > prev_page.media_box.ury * self.continuation_threshold;

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
        page.segments
            .iter()
            .find(|seg| seg.y > page.media_box.ury * self.header_threshold)
            .map(|seg| seg.content.clone())
    }

    fn find_footer_candidate(&self, page: &PageText) -> Option<String> {
        page.segments
            .iter()
            .find(|seg| seg.y < page.media_box.lly * self.footer_threshold)
            .map(|seg| seg.content.clone())
    }
}

fn ends_with_terminal_punctuation(s: &str) -> bool {
    s.trim()
        .ends_with(|c: char| c == '.' || c == '!' || c == '?')
}

// Temporary ContentOutput structure for backward compatibility during transition
#[derive(Debug)]
pub struct ContentOutput {
    pub headings: Vec<String>,
    pub paragraph: String,
    pub page: u32,
    pub end_page: Option<u32>,
    pub page_char_start: Option<usize>,
    pub page_char_end: Option<usize>,
    pub bbox: Option<BoundingBox>,
    pub page_positions: Vec<PagePosition>,
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
                if c == b'\'' || c == b'\"' || c == b')' || c == b']' { j += 1; } else { break; }
            }
            if j == n || is_ascii_ws(bytes[j]) { candidates.push(j); }
            continue;
        }
        if b == b';' {
            let j = i + 1;
            if j == n || is_ascii_ws(bytes[j]) { candidates.push(j); }
            continue;
        }
        if b == b':' {
            let prev_digit = i > 0 && bytes[i - 1].is_ascii_digit();
            let next_digit = i + 1 < n && bytes[i + 1].is_ascii_digit();
            let j = i + 1;
            if (j == n || is_ascii_ws(bytes[j])) && !(prev_digit && next_digit) { candidates.push(j); }
            continue;
        }
        if b == 0xE2 && i + 2 < n {
            let b1 = bytes[i + 1];
            let b2 = bytes[i + 2];
            if b1 == 0x80 && (b2 == 0x94 || b2 == 0x93) {
                let j = i + 3;
                if j == n || is_ascii_ws(bytes[j]) { candidates.push(j); }
            }
            continue;
        }
    }
    // Deduplicate and sort descending to try farthest boundary first
    candidates.sort_unstable();
    candidates.dedup();
    candidates.reverse();

    for idx in candidates.iter().cloned() {
        if idx > text.len() { continue; }
        let head = &text[..idx];
        let prefix = if sep.is_empty() { head.to_string() } else { format!("{}{}", sep, head) };
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
            if used_tokens + space_tokens + w_tokens > capacity_tokens { break; }
            used_tokens += space_tokens + w_tokens;
            cursor = end;
            cut_idx_word = Some(cursor);
        } else { break; }
    }
    let cut_idx = cut_idx_word.unwrap_or(0);
    if cut_idx == 0 { return (String::new(), text.to_string()); }
    let head = text[..cut_idx].trim().to_string();
    let tail = text[cut_idx..].trim_start().to_string();
    (head, tail)
}

pub fn output_doc(
    doc: &Document,
    ocr_handler: Option<&OcrHandler>,
    max_tokens: Option<usize>,
    laparams: Option<&crate::LAParams>,
) -> Result<Vec<ContentOutput>, Box<dyn std::error::Error>> {
    let mut document_structure: Vec<ContentOutput> = Vec::new();

    // debug!("Shaata");
    if doc.is_encrypted() {
        error!("Encrypted documents must be decrypted with a password using {{extract_text|extract_text_from_mem|output_doc}}_encrypted");
    }
    let empty_resources = &Dictionary::new();

    let pages = doc.get_pages();
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
    let page_results: Vec<(u32, Vec<TextSegment>)> = pages
        .par_iter()
        .filter_map(|dict| {
            let page_num = dict.0;
            
            // Try to get page dictionary
            let page_dict = match doc.get_object(*dict.1) {
                Ok(obj) => match obj.as_dict() {
                    Ok(dict) => dict,
                    Err(e) => {
                        error!("Failed to get page {} dictionary: {:?}", page_num, e);
                        return Some((*page_num, Vec::new())); // Return empty segments for this page
                    }
                },
                Err(e) => {
                    error!("Failed to get page {} object: {:?}", page_num, e);
                    return Some((*page_num, Vec::new())); // Return empty segments for this page
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
                    match p.process_stream(
                        &doc,
                        ocr_handler,
                        content,
                        resources,
                        &media_box,
                        *page_num,
                        &mut page_segments,
                        page_rotate,
                        laparams,
                    ) {
                        Ok(_) => {
                            let char_count: usize = page_segments.iter().map(|s| s.content.len()).sum();
                            debug!(
                                "Successfully processed page {} with {} segments ({} chars)",
                                page_num,
                                page_segments.len(),
                                char_count
                            );
                        }
                        Err(e) => {
                            error!("Error processing page {} content: {:?}. Returning partial results.", page_num, e);
                            // page_segments may contain partial results
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to get content for page {}: {:?}", page_num, e);
                    // Return empty segments for this page
                }
            }

            Some((*dict.0, page_segments)) // Return tuple of (page_num, segments)
        })
        .collect();

    // Sort by page number and flatten while maintaining order
    let mut page_results_vec: Vec<_> = page_results.into_iter().collect();
    page_results_vec.sort_by_key(|(page_num, _)| *page_num);
    let text_segments: Vec<TextSegment> = page_results_vec
        .into_iter()
        .flat_map(|(_, segments)| segments)
        .collect();

    // Create header/footer detector and analyze the document
    let mut header_footer_detector = HeaderFooterDetector::new(pages.len());

    // Feed all text segments to the detector
    for segment in &text_segments {
        // Approximate page height if unknown; segments carry page_num, not media box
        let page_height = 792.0; // US Letter default; detector primarily relies on normalization + repetition
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
    // The rest of the function remains sequential to ensure that the document structure is created in the correct order
    // let doc_stats = calculate_document_stats(text_segments.clone());

    let mut header_hierarchy = HeaderHierarchy::new();

    // Initialize ChunkAccumulator
    let tokenizer = Arc::new(tiktoken_rs::get_bpe_from_model("gpt-4o").unwrap());
    let mut accumulator =
        ChunkAccumulator::new(max_tokens.unwrap_or(usize::MAX), tokenizer.clone());

    let doc_stats = calculate_document_stats(&text_segments.clone());

    // Process segments with ChunkAccumulator
    for segment in text_segments {
        let level = is_heading(&segment, &doc_stats);

        match level {
            TextLevel::H1
            | TextLevel::H2
            | TextLevel::H3
            | TextLevel::H4
            | TextLevel::H5
            | TextLevel::H6 => {
                // Heading detected - flush current chunk if it has content
                if !accumulator.is_empty() {
                    document_structure.push(accumulator.create_output());
                    accumulator.reset();
                }

                // Update header hierarchy
                header_hierarchy.push(level, segment.content.trim().to_string());
                accumulator.set_headings(header_hierarchy.get_headers());
            }

            TextLevel::Body | TextLevel::SubBody => {
                // Don't flush just because of page boundaries - respect sentence boundaries instead

                // Check if we can add this segment
                if !accumulator.can_add_segment(&segment) {
                    // Can't add - need to handle overflow
                    // First try backtracking to the last wide boundary under the cap
                    if let Some(output) = accumulator.flush_at_last_boundary() {
                        document_structure.push(output);
                        accumulator.set_headings(header_hierarchy.get_headers());
                        // After flushing at a clean boundary, try adding again
                        if accumulator.can_add_segment(&segment) {
                            accumulator.add_segment(segment);
                            continue;
                        }
                    }
                    if accumulator.should_flush() {
                        // At sentence boundary - safe to flush
                        document_structure.push(accumulator.create_output());
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

                            for chunk_text in chunks.into_iter() {
                                let mut partial_segment = segment.clone();
                                partial_segment.content = chunk_text;
                                partial_segment.word_count =
                                    unicode_count_words(&partial_segment.content);
                                // Adjust bbox approximately by proportion of chars
                                let orig_chars = segment.content.chars().count().max(1);
                                let part_chars = partial_segment.content.chars().count();
                                let ratio = (part_chars as f64) / (orig_chars as f64);
                                partial_segment.width = (segment.width * ratio).max(0.0);
                                // Keep x for the first piece in this loop; subsequent pieces will be split by separate flushes

                                accumulator.add_segment(partial_segment);
                                document_structure.push(accumulator.create_output());
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
                            document_structure.push(accumulator.create_output());
                            accumulator.reset();
                            accumulator.set_headings(header_hierarchy.get_headers());

                            let chunks = split_long_sentence(
                                &segment.content,
                                max_tokens.unwrap_or(usize::MAX),
                                &tokenizer,
                            );

                            for chunk_text in chunks.into_iter() {
                                let mut partial_segment = segment.clone();
                                partial_segment.content = chunk_text;
                                partial_segment.word_count =
                                    unicode_count_words(&partial_segment.content);
                                let orig_chars = segment.content.chars().count().max(1);
                                let part_chars = partial_segment.content.chars().count();
                                let ratio = (part_chars as f64) / (orig_chars as f64);
                                partial_segment.width = (segment.width * ratio).max(0.0);

                                accumulator.add_segment(partial_segment);
                                document_structure.push(accumulator.create_output());
                                accumulator.reset();
                                accumulator.set_headings(header_hierarchy.get_headers());
                            }
                        } else {
                            // Force add it since it completes a sentence and fits
                            accumulator.add_segment(segment);
                            document_structure.push(accumulator.create_output());
                            accumulator.reset();
                            accumulator.set_headings(header_hierarchy.get_headers());
                        }
                    } else {
                        // Try to carve a clean prefix to fit current chunk before hard flush
                        let remaining = max_tokens
                            .unwrap_or(usize::MAX)
                            .saturating_sub(accumulator.get_token_count().unwrap_or(0));
                        let (prefix, rest) = split_to_fit(&segment.content, " ", remaining, &tokenizer);
                        if !prefix.is_empty() {
                            let mut seg1 = segment.clone();
                            seg1.content = prefix;
                            seg1.word_count = unicode_count_words(&seg1.content);
                            // Adjust char ranges to reflect the prefix length on this page
                            let prefix_chars = seg1.content.chars().count();
                            let original_start = seg1.char_start;
                            seg1.char_end = original_start + prefix_chars;
                            // Approximate bbox split left-to-right
                            let total_chars = segment.content.chars().count().max(1);
                            let ratio = (prefix_chars as f64) / (total_chars as f64);
                            let w1 = (segment.width * ratio).max(0.0);
                            seg1.width = w1;
                            accumulator.add_segment(seg1);
                            document_structure.push(accumulator.create_output());
                            accumulator.reset();
                            accumulator.set_headings(header_hierarchy.get_headers());

                            if !rest.is_empty() {
                                // Handle remainder like a normal segment
                                let mut seg2 = segment.clone();
                                seg2.content = rest;
                                seg2.word_count = unicode_count_words(&seg2.content);
                                // Remainder starts where prefix ended; end at original end
                                let rest_chars = seg2.content.chars().count();
                                seg2.char_start = original_start + prefix_chars;
                                seg2.char_end = seg2.char_start + rest_chars;
                                // Adjust bbox for remainder
                                let w2 = (segment.width - ratio * segment.width).max(0.0);
                                seg2.x = segment.x + (segment.width - w2);
                                seg2.width = w2;
                                let test_tokens = tokenizer.encode_ordinary(&seg2.content).len();
                                if test_tokens > max_tokens.unwrap_or(usize::MAX) {
                                let chunks = split_long_sentence(
                                        &seg2.content,
                                        max_tokens.unwrap_or(usize::MAX),
                                        &tokenizer,
                                    );
                                    for chunk_text in chunks.into_iter() {
                                        let mut partial = seg2.clone();
                                        partial.content = chunk_text;
                                        partial.word_count =
                                            unicode_count_words(&partial.content);
                                        let c = partial.content.chars().count();
                                        partial.char_end = partial.char_start + c;
                                        // Proportionally adjust width within the remainder
                                        let rem_total = seg2.content.chars().count().max(1);
                                        let part_ratio = (c as f64) / (rem_total as f64);
                                        partial.width = (seg2.width * part_ratio).max(0.0);
                                        // Keep x for first piece inside remainder; subsequent pieces are emitted across separate flushes
                                        accumulator.add_segment(partial);
                                        document_structure.push(accumulator.create_output());
                                        accumulator.reset();
                                        accumulator.set_headings(
                                            header_hierarchy.get_headers(),
                                        );
                                    }
                                } else {
                                    accumulator.add_segment(seg2);
                                }
                            }
                        } else {
                            // Hard flush, then handle the segment as usual
                            document_structure.push(accumulator.create_output_with_warning());
                            accumulator.reset();
                            accumulator.set_headings(header_hierarchy.get_headers());

                            // Check if segment itself is too large
                            if segment.word_count > 0 {
                                let test_tokens = tokenizer.encode_ordinary(&segment.content).len();
                                debug!("Segment has {} tokens (max: {:?})", test_tokens, max_tokens);
                                if test_tokens > max_tokens.unwrap_or(usize::MAX) {
                                    debug!("Splitting large segment with {} tokens", test_tokens);
                                    // Split the large segment
                                    let chunks = split_long_sentence(
                                        &segment.content,
                                        max_tokens.unwrap_or(usize::MAX),
                                        &tokenizer,
                                    );

                                    for (_idx, chunk_text) in chunks.into_iter().enumerate() {
                                        let mut partial_segment = segment.clone();
                                        partial_segment.content = chunk_text;
                                        partial_segment.word_count =
                                            unicode_count_words(&partial_segment.content);

                                        accumulator.add_segment(partial_segment);
                                        document_structure.push(accumulator.create_output());
                                        accumulator.reset();
                                        accumulator.set_headings(
                                            header_hierarchy.get_headers(),
                                        );
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
                    if let (Some(max_toks), Some(current_tokens)) = (max_tokens, accumulator.get_token_count()) {
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
                                    let needed = sep_tokens + tokenizer.encode_ordinary(prefix).len();
                                    if needed <= lookahead_n && current_tokens + needed <= max_toks {
                                        // Split incoming segment into prefix + remainder
                                        let mut seg1 = segment.clone();
                                        seg1.content = prefix.to_string();
                                        seg1.word_count = unicode_count_words(&seg1.content);
                                        let prefix_chars = seg1.content.chars().count();
                                        let original_start = seg1.char_start;
                                        seg1.char_end = original_start + prefix_chars;
                                        // Proportional bbox split
                                        let total_chars = segment.content.chars().count().max(1);
                                        let ratio = (prefix_chars as f64) / (total_chars as f64);
                                        let w1 = (segment.width * ratio).max(0.0);
                                        seg1.width = w1;

                                        // Add small prefix, then flush
                                        accumulator.add_segment(seg1);
                                        document_structure.push(accumulator.create_output());
                                        accumulator.reset();
                                        accumulator.set_headings(header_hierarchy.get_headers());

                                        // Handle remainder in the fresh chunk
                                        let rest = &segment.content[k..];
                                        if !rest.is_empty() {
                                            let mut seg2 = segment.clone();
                                            seg2.content = rest.to_string();
                                            seg2.word_count = unicode_count_words(&seg2.content);
                                            let rest_chars = seg2.content.chars().count();
                                            seg2.char_start = original_start + prefix_chars;
                                            seg2.char_end = seg2.char_start + rest_chars;
                                            // Adjust bbox for remainder
                                            seg2.x = segment.x + w1;
                                            seg2.width = (segment.width - w1).max(0.0);
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
                                document_structure.push(accumulator.create_output());
                                accumulator.reset();
                                accumulator.set_headings(header_hierarchy.get_headers());
                            } else if tokens > (max_toks * 9 / 10) && accumulator.should_flush() {
                                document_structure.push(accumulator.create_output());
                                accumulator.reset();
                                accumulator.set_headings(header_hierarchy.get_headers());
                            } else if tokens > (max_toks * 85 / 100) {
                                // Prefer flushing at the last wide boundary to avoid mid-sentence near cap
                                if let Some(output) = accumulator.flush_at_last_boundary() {
                                    document_structure.push(output);
                                    accumulator.set_headings(header_hierarchy.get_headers());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Flush any remaining content
    if !accumulator.is_empty() {
        document_structure.push(accumulator.create_output());
    }

    // Return the document structure directly - no post-processing needed
    // Dump font decode summary if enabled
    crate::dump_font_decode_summary();
    Ok(document_structure)
}

/// New output_doc function that directly returns the new schema
pub fn output_doc_new_schema(
    doc: &Document,
    ocr_handler: Option<&OcrHandler>,
    max_tokens: Option<usize>,
    source_id: i64,
    source_type: &str,
    laparams: Option<&crate::LAParams>,
) -> Result<Vec<ExtractionResult>, Box<dyn std::error::Error>> {
    // Reuse most of the existing output_doc logic but modify the return format
    let content_outputs = output_doc(doc, ocr_handler, max_tokens, laparams)?;

    let mut results = Vec::new();

    for output in content_outputs {
        // Create ContentCore
        let content_core =
            create_content_core(&output.paragraph, &output.headings, source_id, source_type);

        // Create PDF location metadata from page positions
        let format_location = create_pdf_location_from_positions(
            output.page,
            output.end_page,
            &output.page_positions,
            output.paragraph.len(),
        );

        // Create ContentExt with compressed metadata
        let content_ext = create_content_ext(
            &content_core.chunk_id,
            &format_location,
            output.page_char_start,
            output.page_char_end,
            output.bbox.as_ref(),
        )?;

        results.push(ExtractionResult {
            content_core,
            content_ext,
        });
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
) -> Result<Vec<ExtractionResult>, Box<dyn std::error::Error>> {
    let doc = Document::load(file_path)?;

    // Initialize OCR handler if config is provided
    let ocr_handler = if let Some(config) = ocr_config {
        Some(crate::OcrHandler::new(&config)?)
    } else {
        None
    };

    output_doc_new_schema(
        &doc,
        ocr_handler.as_ref(),
        max_tokens,
        source_id,
        source_type,
        laparams.as_ref(),
    )
}
