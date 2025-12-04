pub mod analysis;
pub(crate) mod columns;
pub mod header_footer;
pub(crate) mod hyphenation;
pub(crate) mod lists;
pub mod processing;
pub mod stats;
pub(crate) mod tables;

// Re-export public types and functions
pub use analysis::{classify_all_lines, classify_line, is_heading, NextLineContext, TextLevel};
pub use header_footer::{
    HeaderFooterDetector, HeaderFooterPattern, HeaderFooterType, PageOccurrence,
};
pub use processing::{output_doc, output_doc_new_schema, parse_pdf, clean_text_for_indexing, PageText, PostProcessor};
pub use stats::{
    calculate_document_stats, calculate_heading_thresholds, calculate_mode,
    group_into_visual_lines, DocumentStats, FontStats, LineSegmentInfo, VisualLine,
};
