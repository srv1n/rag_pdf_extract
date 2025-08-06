pub mod analysis;
pub mod header_footer;
pub mod processing;
pub mod stats;

// Re-export public types and functions
pub use analysis::{TextLevel, is_heading};
pub use header_footer::{HeaderFooterDetector, HeaderFooterPattern, HeaderFooterType, PageOccurrence};  
pub use processing::{output_doc, output_doc_new_schema, parse_pdf, PostProcessor, PageText};
pub use stats::{DocumentStats, FontStats, calculate_document_stats, calculate_mode, calculate_heading_thresholds};