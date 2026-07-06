pub mod analysis;
pub(crate) mod columns;
pub mod header_footer;
pub(crate) mod hyphenation;
pub(crate) mod lists;
pub mod processing;
pub mod span_map;
pub mod stats;
pub(crate) mod tables;
pub mod visibility;

// Re-export public types and functions
pub use analysis::{classify_all_lines, classify_line, NextLineContext, TextLevel};
pub use header_footer::{
    HeaderFooterDetector, HeaderFooterPattern, HeaderFooterType, PageBand, PageOccurrence,
};
pub use processing::{clean_text_for_indexing, output_doc, output_doc_new_schema, parse_pdf};
pub use span_map::{
    compact_output_spans, LocatedText, OutputSpan, SourceRef, SpanSource, SyntheticKind,
};
pub use stats::{
    calculate_heading_thresholds, calculate_mode, DocumentStats, FontStats, LineSegmentInfo,
    VisualLine,
};
pub use visibility::{
    PageTextLayerStats, TextVisibilityInput, TextVisibilityPolicy, VisibilityDecision,
    VisibilityDropReason,
};
