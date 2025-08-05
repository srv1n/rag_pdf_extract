use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use rayon::prelude::*;
use lopdf::{Document, Dictionary};
use log::{debug, error};
use crate::{
    TextSegment, MediaBox, Processor, get_inherited, get_page_rotation, OcrHandler,
    BoundingBox, PagePosition, ExtractionResult, 
    create_content_core, create_content_ext, create_pdf_location_from_positions,
};
use crate::chunk_accumulator::{ChunkAccumulator, count_words as unicode_count_words, split_long_sentence, contains_sentence_end};
use crate::heading_hierarchy::HeaderHierarchy;
use crate::form::form_fields;
use super::{
    HeaderFooterDetector, TextLevel,
    analysis::is_heading,
    stats::calculate_document_stats
};

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

pub fn output_doc(
    doc: &Document,
    ocr_handler: Option<&OcrHandler>,
    max_tokens: Option<usize>,
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

    // Process pages in parallel
    // Change from flat_map to map to preserve page boundaries
    let page_results: Vec<(u32, Vec<TextSegment>)> = pages
        .par_iter()
        .map(|dict| {
            let page_num = dict.0;
            let page_dict = doc.get_object(*dict.1).unwrap().as_dict().unwrap();
            let resources = get_inherited(doc, page_dict, b"Resources").unwrap_or(empty_resources);

            let media_box: Vec<f64> = get_inherited(doc, page_dict, b"MediaBox").expect("MediaBox");
            let media_box = MediaBox {
                llx: media_box[0],
                lly: media_box[1],
                urx: media_box[2],
                ury: media_box[3],
            };
            
            // Get page rotation (Layer 1)
            let page_rotate = get_page_rotation(page_dict, &doc);
            debug!("Page {} has rotation: {}°", page_num, page_rotate);

            let mut p = Processor::new();
            let mut page_segments = Vec::new();

            p.process_stream(
                &doc,
                ocr_handler,
                doc.get_page_content(*dict.1).unwrap(),
                resources,
                &media_box,
                *page_num,
                &mut page_segments,
                page_rotate,
            )
            .unwrap();

            (*dict.0, page_segments) // Return tuple of (page_num, segments)
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
        // Get the page height from the media box (this is approximate)
        let page_height = 800.0; // Default page height, could be extracted from media box
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

    // Filter out headers and footers from text segments
    let text_segments: Vec<TextSegment> = text_segments
        .into_iter()
        .filter(|seg| !headers_footers.contains(&seg.content))
        .collect();

    // The rest of the function remains sequential to ensure that the document structure is created in the correct order
    // The rest of the function remains sequential to ensure that the document structure is created in the correct order
    // let doc_stats = calculate_document_stats(text_segments.clone());

    let mut header_hierarchy = HeaderHierarchy::new();
    
    // Initialize ChunkAccumulator
    let tokenizer = Arc::new(tiktoken_rs::get_bpe_from_model("gpt-4o").unwrap());
    let mut accumulator = ChunkAccumulator::new(max_tokens.unwrap_or(usize::MAX), tokenizer.clone());
    
    let doc_stats = calculate_document_stats(&text_segments.clone());

    // Process segments with ChunkAccumulator
    for segment in text_segments {
        let level = is_heading(&segment, &doc_stats);
        
        match level {
            TextLevel::H1 | TextLevel::H2 | TextLevel::H3 | 
            TextLevel::H4 | TextLevel::H5 | TextLevel::H6 => {
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
                    if accumulator.should_flush() {
                        // At sentence boundary - safe to flush
                        document_structure.push(accumulator.create_output());
                        accumulator.reset();
                        accumulator.set_headings(header_hierarchy.get_headers());
                        accumulator.add_segment(segment);
                    } else if contains_sentence_end(&segment.content) && 
                              accumulator.can_force_add_for_sentence(&segment) {
                        // Segment completes a sentence - force add it
                        accumulator.add_segment(segment);
                        document_structure.push(accumulator.create_output());
                        accumulator.reset();
                        accumulator.set_headings(header_hierarchy.get_headers());
                    } else {
                        // Must flush even though not at sentence boundary
                        document_structure.push(accumulator.create_output_with_warning());
                        accumulator.reset();
                        accumulator.set_headings(header_hierarchy.get_headers());
                        
                        // Check if segment itself is too large
                        if segment.word_count > 0 {
                            let test_tokens = tokenizer.encode_ordinary(&segment.content).len();
                            if test_tokens > max_tokens.unwrap_or(usize::MAX) {
                                // Split the large segment
                                let chunks = split_long_sentence(
                                    &segment.content, 
                                    max_tokens.unwrap_or(usize::MAX), 
                                    &tokenizer
                                );
                                
                                for (_idx, chunk_text) in chunks.into_iter().enumerate() {
                                    let mut partial_segment = segment.clone();
                                    partial_segment.content = chunk_text;
                                    partial_segment.word_count = unicode_count_words(&partial_segment.content);
                                    
                                    accumulator.add_segment(partial_segment);
                                    document_structure.push(accumulator.create_output());
                                    accumulator.reset();
                                    accumulator.set_headings(header_hierarchy.get_headers());
                                }
                            } else {
                                accumulator.add_segment(segment);
                            }
                        }
                    }
                } else {
                    // Normal case - just add the segment
                    accumulator.add_segment(segment);
                    
                    // Check if we should proactively flush
                    if accumulator.should_flush() {
                        document_structure.push(accumulator.create_output());
                        accumulator.reset();
                        accumulator.set_headings(header_hierarchy.get_headers());
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
    Ok(document_structure)
}

/// New output_doc function that directly returns the new schema
pub fn output_doc_new_schema(
    doc: &Document,
    ocr_handler: Option<&OcrHandler>,
    max_tokens: Option<usize>,
    source_id: i64,
    source_type: &str,
) -> Result<Vec<ExtractionResult>, Box<dyn std::error::Error>> {
    // Reuse most of the existing output_doc logic but modify the return format
    let content_outputs = output_doc(doc, ocr_handler, max_tokens)?;

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
) -> Result<Vec<ExtractionResult>, Box<dyn std::error::Error>> {
    let doc = Document::load(file_path)?;
    
    // Initialize OCR handler if config is provided
    let ocr_handler = if let Some(config) = ocr_config {
        Some(crate::OcrHandler::new(&config)?)
    } else {
        None
    };
    
    output_doc_new_schema(&doc, ocr_handler.as_ref(), max_tokens, source_id, source_type)
}