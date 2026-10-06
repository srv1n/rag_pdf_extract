//! Document running header/footer filtering extracted from the layout pipeline.
use super::header_footer::HeaderFooterDetector;
use crate::TextSegment;
use std::collections::HashMap;

pub(crate) fn remove_running_matter(
    text_segments: Vec<TextSegment>,
    page_heights: &HashMap<u32, f64>,
    page_count: usize,
) -> Vec<TextSegment> {
    // Create header/footer detector and analyze the document
    let mut header_footer_detector = HeaderFooterDetector::new(page_count);

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
    if skip_hf {
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
    }
}
