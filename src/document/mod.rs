pub mod analysis;
pub mod header_footer;
pub mod processing;
pub mod stats;

// Re-export public types and functions
pub use analysis::{is_heading, TextLevel};
pub use header_footer::{
    HeaderFooterDetector, HeaderFooterPattern, HeaderFooterType, PageOccurrence,
};
pub use processing::{output_doc, output_doc_new_schema, parse_pdf, PageText, PostProcessor};
pub use stats::{
    calculate_document_stats, calculate_heading_thresholds, calculate_mode, DocumentStats,
    FontStats,
};
