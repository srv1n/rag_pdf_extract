#![allow(
    dead_code,
    unused_assignments,
    unused_imports,
    unused_mut,
    unused_variables,
    mismatched_lifetime_syntaxes
)]

use cmap::{ByteMapping, CIDRange, CodeRange};
use encoding_rs::UTF_16BE;
use euclid::*;
use log::{debug, error, info, trace, warn};
use lopdf::content::Content;
use lopdf::encryption::DecryptionError;
use lopdf::*;

use std::fmt::{Debug, Formatter};

use euclid::vec2;
use lazy_static::lazy_static;
use rayon::prelude::*;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::fmt;
use std::fs::File;
use std::marker::PhantomData;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;
use std::result::Result;
use std::slice::Iter;
use std::str;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use unicode_normalization::UnicodeNormalization;

// Import text splitting utilities
use crate::text_splitting::{
    count_words as count_words_simple, estimate_tokens_from_words, get_text_splitter_shared,
};

mod core_fonts;
mod encodings;

mod chunk_accumulator;
pub mod classifier;
mod cmap;
pub mod document;
mod form;
pub mod founder_regression;
mod glyphnames;
mod heading_hierarchy;
mod layout_params;
mod ocrs;
mod pdf_image;
pub mod quality;
pub mod text_splitting;

pub use classifier::{
    classify_pdf, classify_pdf_from_mem, ClassifierOptions, DocumentClassification, DocumentType,
    OcrReason, PageAssessment,
};
mod zapfglyphnames;

// Re-export OCR types
pub use ocrs::{OcrConfig, OcrHandler};
// Use the transform function internally
use ocrs::apply_transform_to_image;

// Re-export document processing types and functions
pub use document::{
    calculate_heading_thresholds, calculate_mode, output_doc, output_doc_new_schema, parse_pdf,
    DocumentStats, FontStats, HeaderFooterDetector, HeaderFooterPattern, HeaderFooterType,
    PageBand, PageOccurrence, TextLevel,
};

// Re-export LAParams configuration
pub use layout_params::{LAParams, LayoutFallbackPolicy, TokenCountMode};
pub use quality::{
    assess_decode_quality, assess_parse_quality, DecodeQualityMetrics, ParseQualityMetrics,
    ParseQualityStatus, RepeatedLine,
};

use crate::chunk_accumulator::count_words as unicode_count_words;
use crate::document::visibility::{PageTextLayerStats, TextVisibilityInput, TextVisibilityPolicy};
use crate::text_splitting::count_words;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tiktoken_rs::get_bpe_from_model;

// New schema structures for content extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentCore {
    pub chunk_id: String,     // stable source/range/ordinal identity
    pub content_hash: String, // blake3(content)
    pub source_id: i64,       // files.id
    pub source_type: String,  // 'file' | 'web' | 'api'
    pub content: String,
    pub token_count: i32,
    pub headings_json: Option<String>,
    pub status: String,
    pub schema_version: i32,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentExt {
    pub chunk_id: String,
    pub ext_json: Vec<u8>, // zstd compressed
}

// Format-specific location enums
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "format")]
pub enum FormatLocation {
    Pdf(PdfLocation),
    Docx(DocxLocation),
    Epub(EpubLocation),
    Markdown(MarkdownLocation),
    PlainText(PlainTextLocation),
    Code(CodeLocation),
    Html(HtmlLocation),
}

// PDF specific structures - simplified
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfLocation {
    pub fragments: Vec<PageFragment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageFragment {
    pub page: u32,
    /// Character offsets in the emitted chunk text. When output spans are
    /// opted in, source-PDF offsets are stored in
    /// `ContentExt.output_spans[].source.Pdf`.
    pub char_range: CharRange,
    pub bbox: BoundingBox, // Renamed from bbox_union for clarity
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharRange {
    pub start: usize,
    pub end: usize,
}

// Removed Quad and TextFlow - not needed for basic highlighting

// Placeholder structs for other formats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocxLocation {
    pub paragraph_id: String,
    pub section_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubLocation {
    pub chapter_id: String,
    pub paragraph_offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkdownLocation {
    pub heading_path: Vec<String>,
    pub line_range: CharRange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlainTextLocation {
    pub line_range: CharRange,
    pub char_range: CharRange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeLocation {
    pub function_name: Option<String>,
    pub class_name: Option<String>,
    pub line_range: CharRange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HtmlLocation {
    pub element_path: Vec<String>,
    pub element_id: Option<String>,
    pub char_range: CharRange,
}

/// Statistics for PDF extraction process
#[derive(Debug, Clone, Default)]
pub struct ExtractionStats {
    pub total_pages: usize,
    pub successful_pages: usize,
    pub partial_pages: usize,
    pub failed_pages: usize,
    pub total_chunks: usize,
    pub ocr_pages: usize,
    pub total_segments: usize,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OcrImageTelemetrySnapshot {
    pub images_seen: usize,
    pub images_converted: usize,
    pub images_skipped: usize,
    pub images_with_text: usize,
    pub text_chars_emitted: usize,
}

#[derive(Debug, Default)]
pub(crate) struct OcrImageTelemetry {
    images_seen: AtomicUsize,
    images_converted: AtomicUsize,
    images_skipped: AtomicUsize,
    images_with_text: AtomicUsize,
    text_chars_emitted: AtomicUsize,
}

impl OcrImageTelemetry {
    pub(crate) fn image_seen(&self) {
        self.images_seen.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn image_converted(&self) {
        self.images_converted.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn image_skipped(&self) {
        self.images_skipped.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn image_text_emitted(&self, chars: usize) {
        self.images_with_text.fetch_add(1, Ordering::Relaxed);
        self.text_chars_emitted.fetch_add(chars, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> OcrImageTelemetrySnapshot {
        OcrImageTelemetrySnapshot {
            images_seen: self.images_seen.load(Ordering::Relaxed),
            images_converted: self.images_converted.load(Ordering::Relaxed),
            images_skipped: self.images_skipped.load(Ordering::Relaxed),
            images_with_text: self.images_with_text.load(Ordering::Relaxed),
            text_chars_emitted: self.text_chars_emitted.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProductionTelemetry {
    pub layout_enabled: bool,
    pub all_texts_enabled: bool,
    pub layout_fallback_used: bool,
    pub layout_normalized_chars: Option<usize>,
    pub no_layout_normalized_chars: Option<usize>,
    pub layout_to_no_layout_normalized_char_ratio: Option<f64>,
    pub chunk_count: usize,
    pub extracted_chars: usize,
    pub normalized_chars: usize,
    pub max_chunk_tokens: usize,
    pub over_cap_chunks: usize,
    pub chunks_without_location: usize,
    pub repeated_line_ratio: f64,
    pub unique_token_ratio: f64,
    pub parse_quality_status: String,
    pub parse_quality_score: f64,
    pub decode_confidence: f64,
    pub mojibake_char_ratio: f64,
    pub symbol_char_ratio: f64,
    pub non_ascii_symbol_char_ratio: f64,
    pub suspicious_chunk_ratio: f64,
    pub chars_without_span: usize,
    pub chars_with_overlapping_spans: usize,
    pub pdf_backed_nonsynthetic_char_ratio: f64,
    pub synthetic_char_ratio: f64,
    pub invalid_bbox_count: usize,
    pub out_of_page_bbox_count: usize,
    pub content_ext_max_compressed_bytes: usize,
    pub content_ext_max_uncompressed_bytes: usize,
    pub content_ext_total_compressed_bytes: usize,
    pub content_ext_total_uncompressed_bytes: usize,
    pub ocr_images_seen: usize,
    pub ocr_images_converted: usize,
    pub ocr_images_skipped: usize,
    pub ocr_images_with_text: usize,
    pub ocr_text_chars_emitted: usize,
}

impl ExtractionStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_error(&mut self, page: u32, error: String) {
        self.errors.push(format!("Page {}: {}", page, error));
    }

    pub fn add_warning(&mut self, page: u32, warning: String) {
        self.warnings.push(format!("Page {}: {}", page, warning));
    }

    pub fn success_rate(&self) -> f64 {
        if self.total_pages == 0 {
            0.0
        } else {
            (self.successful_pages as f64) / (self.total_pages as f64)
        }
    }
}

lazy_static! {
    // Regex for date patterns to exclude from numbered headings
    static ref DATE_PATTERN: Regex = Regex::new(r"^\d{1,2}\.\d{1,2}\.\d{4}").unwrap();

    static ref NUMBERED_HEADING: Regex = Regex::new(
        r"(?x)
        ^
        (?P<number>
            (?:\d{1,2}\.){1,3}\d{1,2} | # Matches 1.1, 2.3.4, etc.
            [IVXLCDM]+\.          | # Matches VII.
            (?:Section|Article|Chapter)\s+[A-Z0-9]+ | # Matches Section 2, Article B, etc.
            \d{1,3}\.\s+[A-Z]     # Matches '1. A' or '12. Text' (numbered paragraphs)
        )

        "
    ).unwrap();
}

lazy_static! {
    static ref CONTENT_CORE_TOKENIZER: Option<tiktoken_rs::CoreBPE> =
        get_bpe_from_model("gpt-4o").ok();
}

pub struct Space;
pub type Transform = Transform2D<f64, Space, Space>;

// Constants for text extraction
const SPACE_THRESHOLD_RATIO: f64 = 0.25; // Typical space is 25% of font size
const MIN_SPACE_GAP: f64 = 0.1; // Minimum 10% of font size to be considered a gap
const PARAGRAPH_GAP_RATIO: f64 = 1.5; // Paragraph break if gap is 150% of font size
const LINE_HEIGHT_RATIO: f64 = 0.8; // Detect new line if Y gap > 80% of font size
const SAME_LINE_THRESHOLD: f64 = 0.3; // Consider same line if Y difference < 30% font size (more sensitive to line breaks)

// Helper function to determine if there's a space between characters
fn should_insert_space(current_x: f64, last_end: f64, font_size: f64) -> bool {
    let gap = current_x - last_end;
    // Use adaptive threshold based on font size
    // Smaller fonts tend to have tighter spacing
    let threshold = if font_size < 10.0 {
        font_size * MIN_SPACE_GAP
    } else {
        font_size * SPACE_THRESHOLD_RATIO
    };
    gap > threshold
}

// Helper function to determine if there's a paragraph break
fn is_paragraph_break(current_y: f64, last_y: f64, font_size: f64) -> bool {
    let y_gap = (current_y - last_y).abs();
    y_gap > font_size * PARAGRAPH_GAP_RATIO
}

// Helper function to determine if there's a column break (large horizontal gap)
fn is_column_break(current_x: f64, last_end: f64, font_size: f64) -> bool {
    let gap = current_x - last_end;
    // If gap is more than 4x font size, treat as column break
    // Lowered from 8x to catch more column breaks
    gap > font_size * 4.0
}

// Helper function to determine if we've moved to a new line
fn is_new_line(current_x: f64, last_end: f64, current_y: f64, last_y: f64, font_size: f64) -> bool {
    let y_diff = (current_y - last_y).abs();

    // Case 1: Significant Y movement (more than line height) - always a new line
    // This catches most line breaks regardless of X position
    if y_diff > font_size * LINE_HEIGHT_RATIO {
        return true;
    }

    // Case 2: Y moved and X reset to left (original logic, but less strict)
    // New line if we've moved down/up significantly and moved back to the left
    if y_diff > font_size * SAME_LINE_THRESHOLD && current_x < last_end {
        return true;
    }

    // Case 3: Y moved significantly and there's a gap (not continuous text)
    // This catches centered text where X doesn't necessarily go back
    if y_diff > font_size * SAME_LINE_THRESHOLD {
        let x_gap = (current_x - last_end).abs();
        // If there's any significant gap, it's probably a new line
        if x_gap > font_size * 2.0 {
            return true;
        }
    }

    false
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorContext {
    pub phase: Option<String>,
    pub page: Option<u32>,
    pub repair_attempted: bool,
    pub estimated_pages: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceLimitKind {
    Pages,
    Objects,
    RecursionDepth,
    DecompressedStreamBytes,
    OutputBytes,
}

impl std::fmt::Display for ResourceLimitKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        let name = match self {
            Self::Pages => "max_pages",
            Self::Objects => "max_objects",
            Self::RecursionDepth => "max_recursion_depth",
            Self::DecompressedStreamBytes => "max_decompressed_stream_bytes",
            Self::OutputBytes => "max_output_bytes",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncryptionFailure {
    PasswordRequired,
    IncorrectPassword,
    UnsupportedHandler,
    InvalidDictionary,
}

#[derive(Debug, Clone)]
pub enum OutputError {
    NotAPdf {
        context: ErrorContext,
    },
    Encrypted {
        reason: EncryptionFailure,
        context: ErrorContext,
    },
    InvalidStructure {
        message: String,
        context: ErrorContext,
    },
    ResourceLimit {
        kind: ResourceLimitKind,
        limit: usize,
        observed: Option<usize>,
        context: ErrorContext,
    },
    Parse {
        message: String,
        context: ErrorContext,
    },
    Io {
        message: String,
        context: ErrorContext,
    },
    Format {
        message: String,
        context: ErrorContext,
    },
}

impl OutputError {
    /// Stable machine-readable reason suitable for worker protocols and job reports.
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::NotAPdf { .. } => "pdf_not_a_pdf",
            Self::Encrypted { reason, .. } => match reason {
                EncryptionFailure::PasswordRequired => "pdf_password_required",
                EncryptionFailure::IncorrectPassword => "pdf_incorrect_password",
                EncryptionFailure::UnsupportedHandler => "pdf_unsupported_encryption",
                EncryptionFailure::InvalidDictionary => "pdf_invalid_encryption_dictionary",
            },
            Self::InvalidStructure { .. } => "pdf_invalid_structure",
            Self::ResourceLimit { kind, .. } => match kind {
                ResourceLimitKind::Pages => "pdf_resource_limit_pages",
                ResourceLimitKind::Objects => "pdf_resource_limit_objects",
                ResourceLimitKind::RecursionDepth => "pdf_resource_limit_recursion_depth",
                ResourceLimitKind::DecompressedStreamBytes => {
                    "pdf_resource_limit_decompressed_stream_bytes"
                }
                ResourceLimitKind::OutputBytes => "pdf_resource_limit_output_bytes",
            },
            Self::Parse { .. } => "pdf_parse_error",
            Self::Io { .. } => "pdf_io_error",
            Self::Format { .. } => "pdf_output_format_error",
        }
    }

    pub fn context(&self) -> &ErrorContext {
        match self {
            Self::NotAPdf { context }
            | Self::Encrypted { context, .. }
            | Self::InvalidStructure { context, .. }
            | Self::ResourceLimit { context, .. }
            | Self::Parse { context, .. }
            | Self::Io { context, .. }
            | Self::Format { context, .. } => context,
        }
    }

    pub fn resource_limit(kind: ResourceLimitKind, limit: usize, observed: Option<usize>) -> Self {
        Self::ResourceLimit {
            kind,
            limit,
            observed,
            context: ErrorContext {
                phase: Some("preflight".to_string()),
                ..ErrorContext::default()
            },
        }
    }

    fn parse_message(message: impl Into<String>) -> Self {
        Self::Parse {
            message: message.into(),
            context: ErrorContext::default(),
        }
    }

    fn boxed_error(error: Box<dyn std::error::Error>) -> Self {
        if let Some(error) = error.downcast_ref::<Self>() {
            return error.clone();
        }
        Self::parse_message(error.to_string())
    }

    fn with_repair_attempted(self, repair_attempted: bool) -> Self {
        if !repair_attempted {
            return self;
        }
        match self {
            Self::NotAPdf { mut context } => {
                context.repair_attempted = true;
                Self::NotAPdf { context }
            }
            Self::Encrypted {
                reason,
                mut context,
            } => {
                context.repair_attempted = true;
                Self::Encrypted { reason, context }
            }
            Self::InvalidStructure {
                message,
                mut context,
            } => {
                context.repair_attempted = true;
                Self::InvalidStructure { message, context }
            }
            Self::ResourceLimit {
                kind,
                limit,
                observed,
                mut context,
            } => {
                context.repair_attempted = true;
                Self::ResourceLimit {
                    kind,
                    limit,
                    observed,
                    context,
                }
            }
            Self::Parse {
                message,
                mut context,
            } => {
                context.repair_attempted = true;
                Self::Parse { message, context }
            }
            Self::Io {
                message,
                mut context,
            } => {
                context.repair_attempted = true;
                Self::Io { message, context }
            }
            Self::Format {
                message,
                mut context,
            } => {
                context.repair_attempted = true;
                Self::Format { message, context }
            }
        }
    }

    fn with_estimated_pages(self, estimated_pages: usize) -> Self {
        let mut context = self.context().clone();
        context.estimated_pages = Some(estimated_pages);
        match self {
            Self::NotAPdf { .. } => Self::NotAPdf { context },
            Self::Encrypted { reason, .. } => Self::Encrypted { reason, context },
            Self::InvalidStructure { message, .. } => Self::InvalidStructure { message, context },
            Self::ResourceLimit {
                kind,
                limit,
                observed,
                ..
            } => Self::ResourceLimit {
                kind,
                limit,
                observed,
                context,
            },
            Self::Parse { message, .. } => Self::Parse { message, context },
            Self::Io { message, .. } => Self::Io { message, context },
            Self::Format { message, .. } => Self::Format { message, context },
        }
    }
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            Self::NotAPdf { context } => {
                write!(f, "input is not a PDF")?;
                write_estimate(f, context)
            }
            Self::Encrypted { reason, context } => {
                write!(f, "encrypted PDF: {:?}", reason)?;
                write_estimate(f, context)
            }
            Self::InvalidStructure { message, context } => {
                write!(f, "invalid PDF structure: {message}")?;
                write_estimate(f, context)
            }
            Self::ResourceLimit {
                kind,
                limit,
                observed,
                context,
            } => {
                write!(
                    f,
                    "PDF resource limit {kind} exceeded (limit={limit}, observed={:?})",
                    observed
                )?;
                write_estimate(f, context)
            }
            Self::Parse { message, context } => {
                write!(f, "PDF parse error: {message}")?;
                write_estimate(f, context)
            }
            Self::Io { message, context } => {
                write!(f, "PDF I/O error: {message}")?;
                write_estimate(f, context)
            }
            Self::Format { message, context } => {
                write!(f, "PDF output formatting error: {message}")?;
                write_estimate(f, context)
            }
        }
    }
}

fn write_estimate(f: &mut Formatter<'_>, context: &ErrorContext) -> Result<(), std::fmt::Error> {
    if let Some(pages) = context.estimated_pages {
        write!(f, " (estimated_pages={pages})")?;
    }
    Ok(())
}

impl std::error::Error for OutputError {}

impl From<std::fmt::Error> for OutputError {
    fn from(error: std::fmt::Error) -> Self {
        Self::Format {
            message: error.to_string(),
            context: ErrorContext::default(),
        }
    }
}

impl From<std::io::Error> for OutputError {
    fn from(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
            context: ErrorContext::default(),
        }
    }
}

impl From<lopdf::Error> for OutputError {
    fn from(error: lopdf::Error) -> Self {
        match error {
            lopdf::Error::IO(error) => Self::Io {
                message: error.to_string(),
                context: ErrorContext::default(),
            },
            lopdf::Error::Parse(error) if error.to_string() == "invalid file header" => {
                Self::NotAPdf {
                    context: ErrorContext {
                        phase: Some("header".to_string()),
                        ..ErrorContext::default()
                    },
                }
            }
            lopdf::Error::Parse(error) => Self::InvalidStructure {
                message: error.to_string(),
                context: ErrorContext::default(),
            },
            lopdf::Error::InvalidPassword => Self::Encrypted {
                reason: EncryptionFailure::IncorrectPassword,
                context: ErrorContext {
                    phase: Some("decrypt".to_string()),
                    ..ErrorContext::default()
                },
            },
            lopdf::Error::Decryption(error) => Self::Encrypted {
                reason: match error {
                    DecryptionError::IncorrectPassword => EncryptionFailure::IncorrectPassword,
                    DecryptionError::UnsupportedEncryption
                    | DecryptionError::UnsupportedVersion
                    | DecryptionError::UnsupportedRevision => EncryptionFailure::UnsupportedHandler,
                    _ => EncryptionFailure::InvalidDictionary,
                },
                context: ErrorContext::default(),
            },
            lopdf::Error::UnsupportedSecurityHandler(_) => Self::Encrypted {
                reason: EncryptionFailure::UnsupportedHandler,
                context: ErrorContext::default(),
            },
            error => Self::Parse {
                message: error.to_string(),
                context: ErrorContext::default(),
            },
        }
    }
}

macro_rules! dlog {
    ($($e:expr),*) => { {$(let _ = $e;)*} }
    //($($t:tt)*) => { println!($($t)*) }
}

// Instrumentation: per-font decode stats (enabled when PDF_EXTRACT_LOG_FONTS is set)
lazy_static! {
    static ref FONT_DECODE_COUNTS: Mutex<std::collections::HashMap<String, (u64, u64)>> =
        Mutex::new(std::collections::HashMap::new()); // font_key -> (total, empty)
    static ref FONT_SEEN: Mutex<std::collections::HashSet<String>> =
        Mutex::new(std::collections::HashSet::new());
    static ref FONT_APPEND_COUNTS: Mutex<std::collections::HashMap<String, u64>> =
        Mutex::new(std::collections::HashMap::new()); // font_id -> appended chars
}
static FONT_LOG_SAMPLES: AtomicUsize = AtomicUsize::new(0);

fn strip_non_printing_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

fn populate_core_font_widths(
    base_name: &str,
    fallback_name: &str,
    encoding_table: &mut Option<Vec<u16>>,
    width_map: &mut HashMap<CharCode, f64>,
) {
    for font_metrics in core_fonts::metrics().iter() {
        if font_metrics.0 != fallback_name {
            continue;
        }

        if let Some(ref encoding) = encoding_table {
            dlog!("has encoding");
            for w in font_metrics.2 {
                let c = glyphnames::name_to_unicode(w.2).unwrap();
                for (i, &codepoint) in encoding.iter().enumerate() {
                    if codepoint == c {
                        width_map.insert(i as CharCode, w.1 as f64);
                    }
                }
            }
        } else {
            let mut table = vec![0; 256];
            for w in font_metrics.2 {
                dlog!("{} {}", w.0, w.2);
                if w.0 != -1 {
                    table[w.0 as usize] = if fallback_name == "ZapfDingbats" {
                        zapfglyphnames::zapfdigbats_names_to_unicode(w.2)
                            .unwrap_or_else(|| panic!("bad name {:?}", w))
                    } else {
                        glyphnames::name_to_unicode(w.2).unwrap()
                    };
                }
            }

            for w in font_metrics.2 {
                width_map.insert(w.0 as CharCode, w.1 as f64);
            }
            *encoding_table = Some(table);
        }

        if fallback_name != base_name {
            warn!(
                "missing Widths for non-core font '{}'; falling back to core metrics '{}'",
                base_name, fallback_name
            );
        }

        return;
    }
}

fn looks_like_symbol_glyph_soup(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.chars().count() < 24 {
        return false;
    }

    let total = trimmed.chars().count().max(1);
    let alpha = trimmed.chars().filter(|c| c.is_alphabetic()).count();
    let digits = trimmed.chars().filter(|c| c.is_ascii_digit()).count();
    let whitespace = trimmed.chars().filter(|c| c.is_whitespace()).count();
    let symbols = trimmed
        .chars()
        .filter(|c| !c.is_alphanumeric() && !c.is_whitespace())
        .count();

    let alpha_ratio = alpha as f64 / total as f64;
    let digit_ratio = digits as f64 / total as f64;
    let symbol_ratio = symbols as f64 / total as f64;
    let whitespace_ratio = whitespace as f64 / total as f64;

    alpha_ratio < 0.08 && digit_ratio < 0.20 && symbol_ratio > 0.55 && whitespace_ratio < 0.20
}

fn record_font_decode(font_key: &str, decoded_nonempty: bool) {
    if std::env::var("PDF_EXTRACT_LOG_FONTS").is_err() {
        return;
    }
    if let Ok(mut map) = FONT_DECODE_COUNTS.lock() {
        let entry = map.entry(font_key.to_string()).or_insert((0, 0));
        entry.0 += 1;
        if !decoded_nonempty {
            entry.1 += 1;
        }
    }
}

fn log_font_once(prefix: &str, font_key: &str, details: &str) {
    if std::env::var("PDF_EXTRACT_LOG_FONTS").is_err() {
        return;
    }
    if let Ok(mut seen) = FONT_SEEN.lock() {
        if !seen.contains(font_key) {
            seen.insert(font_key.to_string());
            // Cap logs to avoid spam
            if FONT_LOG_SAMPLES.fetch_add(1, Ordering::Relaxed) < 50 {
                log::debug!("{} {} {}", prefix, font_key, details);
            }
        }
    }
}

pub fn dump_font_decode_summary() {
    if std::env::var("PDF_EXTRACT_LOG_FONTS").is_err() {
        return;
    }
    if let Ok(map) = FONT_DECODE_COUNTS.lock() {
        for (k, (total, empty)) in map.iter() {
            log::debug!(
                "fontstats: {} total={} empty={} empty_ratio={:.2}",
                k,
                total,
                empty,
                (*empty as f64) / (*total as f64 + 1e-9)
            );
        }
    }
    if let Ok(map) = FONT_APPEND_COUNTS.lock() {
        for (k, appended) in map.iter() {
            log::debug!("fontstats: append {} appended={}", k, appended);
        }
    }
}

fn record_font_append(font_id: &str) {
    if std::env::var("PDF_EXTRACT_LOG_FONTS").is_err() {
        return;
    }
    if let Ok(mut map) = FONT_APPEND_COUNTS.lock() {
        *map.entry(font_id.to_string()).or_insert(0) += 1;
    }
}

fn get_info(doc: &Document) -> Option<&Dictionary> {
    match doc.trailer.get(b"Info") {
        Ok(&Object::Reference(ref id)) => match doc.get_object(*id) {
            Ok(&Object::Dictionary(ref info)) => {
                return Some(info);
            }
            _ => {}
        },
        _ => {}
    }
    None
}

fn get_catalog(doc: &Document) -> &Dictionary {
    match doc.trailer.get(b"Root").unwrap() {
        &Object::Reference(ref id) => match doc.get_object(*id) {
            Ok(&Object::Dictionary(ref catalog)) => {
                return catalog;
            }
            _ => {}
        },
        _ => {}
    }
    panic!();
}

fn get_pages(doc: &Document) -> &Dictionary {
    let catalog = get_catalog(doc);
    match catalog.get(b"Pages").unwrap() {
        &Object::Reference(ref id) => match doc.get_object(*id) {
            Ok(&Object::Dictionary(ref pages)) => {
                return pages;
            }
            other => {
                dlog!("pages: {:?}", other)
            }
        },
        other => {
            dlog!("pages: {:?}", other)
        }
    }
    dlog!("catalog {:?}", catalog);
    panic!();
}

#[allow(non_upper_case_globals)]
const PDFDocEncoding: &'static [u16] = &[
    0x0000, 0x0001, 0x0002, 0x0003, 0x0004, 0x0005, 0x0006, 0x0007, 0x0008, 0x0009, 0x000a, 0x000b,
    0x000c, 0x000d, 0x000e, 0x000f, 0x0010, 0x0011, 0x0012, 0x0013, 0x0014, 0x0015, 0x0016, 0x0017,
    0x02d8, 0x02c7, 0x02c6, 0x02d9, 0x02dd, 0x02db, 0x02da, 0x02dc, 0x0020, 0x0021, 0x0022, 0x0023,
    0x0024, 0x0025, 0x0026, 0x0027, 0x0028, 0x0029, 0x002a, 0x002b, 0x002c, 0x002d, 0x002e, 0x002f,
    0x0030, 0x0031, 0x0032, 0x0033, 0x0034, 0x0035, 0x0036, 0x0037, 0x0038, 0x0039, 0x003a, 0x003b,
    0x003c, 0x003d, 0x003e, 0x003f, 0x0040, 0x0041, 0x0042, 0x0043, 0x0044, 0x0045, 0x0046, 0x0047,
    0x0048, 0x0049, 0x004a, 0x004b, 0x004c, 0x004d, 0x004e, 0x004f, 0x0050, 0x0051, 0x0052, 0x0053,
    0x0054, 0x0055, 0x0056, 0x0057, 0x0058, 0x0059, 0x005a, 0x005b, 0x005c, 0x005d, 0x005e, 0x005f,
    0x0060, 0x0061, 0x0062, 0x0063, 0x0064, 0x0065, 0x0066, 0x0067, 0x0068, 0x0069, 0x006a, 0x006b,
    0x006c, 0x006d, 0x006e, 0x006f, 0x0070, 0x0071, 0x0072, 0x0073, 0x0074, 0x0075, 0x0076, 0x0077,
    0x0078, 0x0079, 0x007a, 0x007b, 0x007c, 0x007d, 0x007e, 0x0000, 0x2022, 0x2020, 0x2021, 0x2026,
    0x2014, 0x2013, 0x0192, 0x2044, 0x2039, 0x203a, 0x2212, 0x2030, 0x201e, 0x201c, 0x201d, 0x2018,
    0x2019, 0x201a, 0x2122, 0xfb01, 0xfb02, 0x0141, 0x0152, 0x0160, 0x0178, 0x017d, 0x0131, 0x0142,
    0x0153, 0x0161, 0x017e, 0x0000, 0x20ac, 0x00a1, 0x00a2, 0x00a3, 0x00a4, 0x00a5, 0x00a6, 0x00a7,
    0x00a8, 0x00a9, 0x00aa, 0x00ab, 0x00ac, 0x0000, 0x00ae, 0x00af, 0x00b0, 0x00b1, 0x00b2, 0x00b3,
    0x00b4, 0x00b5, 0x00b6, 0x00b7, 0x00b8, 0x00b9, 0x00ba, 0x00bb, 0x00bc, 0x00bd, 0x00be, 0x00bf,
    0x00c0, 0x00c1, 0x00c2, 0x00c3, 0x00c4, 0x00c5, 0x00c6, 0x00c7, 0x00c8, 0x00c9, 0x00ca, 0x00cb,
    0x00cc, 0x00cd, 0x00ce, 0x00cf, 0x00d0, 0x00d1, 0x00d2, 0x00d3, 0x00d4, 0x00d5, 0x00d6, 0x00d7,
    0x00d8, 0x00d9, 0x00da, 0x00db, 0x00dc, 0x00dd, 0x00de, 0x00df, 0x00e0, 0x00e1, 0x00e2, 0x00e3,
    0x00e4, 0x00e5, 0x00e6, 0x00e7, 0x00e8, 0x00e9, 0x00ea, 0x00eb, 0x00ec, 0x00ed, 0x00ee, 0x00ef,
    0x00f0, 0x00f1, 0x00f2, 0x00f3, 0x00f4, 0x00f5, 0x00f6, 0x00f7, 0x00f8, 0x00f9, 0x00fa, 0x00fb,
    0x00fc, 0x00fd, 0x00fe, 0x00ff,
];

fn pdf_to_utf8(s: &[u8]) -> String {
    if s.len() > 2 && s[0] == 0xfe && s[1] == 0xff {
        return UTF_16BE
            .decode_without_bom_handling_and_without_replacement(&s[2..])
            .unwrap()
            .to_string();
    } else {
        let r: Vec<u8> = s
            .iter()
            .map(|x| *x)
            .flat_map(|x| {
                let k = PDFDocEncoding[x as usize];
                vec![(k >> 8) as u8, k as u8].into_iter()
            })
            .collect();
        return UTF_16BE
            .decode_without_bom_handling_and_without_replacement(&r)
            .unwrap()
            .to_string();
    }
}

fn to_utf8(encoding: &[u16], s: &[u8]) -> String {
    if s.len() > 2 && s[0] == 0xfe && s[1] == 0xff {
        return UTF_16BE
            .decode_without_bom_handling_and_without_replacement(&s[2..])
            .unwrap()
            .to_string();
    } else {
        let r: Vec<u8> = s
            .iter()
            .map(|x| *x)
            .flat_map(|x| {
                let k = encoding[x as usize];
                vec![(k >> 8) as u8, k as u8].into_iter()
            })
            .collect();
        return UTF_16BE
            .decode_without_bom_handling_and_without_replacement(&r)
            .unwrap()
            .to_string();
    }
}

fn maybe_deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    match o {
        &Object::Reference(r) => doc.get_object(r).expect("missing object reference"),
        _ => o,
    }
}

fn maybe_get_obj<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    dict.get(key).map(|o| maybe_deref(doc, o)).ok()
}

// an intermediate trait that can be used to chain conversions that may have failed
trait FromOptObj<'a> {
    fn from_opt_obj(doc: &'a Document, obj: Option<&'a Object>, key: &[u8]) -> Self;
}

// conditionally convert to Self returns None if the conversion failed
trait FromObj<'a>
where
    Self: std::marker::Sized,
{
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<Self>;
}

impl<'a, T: FromObj<'a>> FromOptObj<'a> for Option<T> {
    fn from_opt_obj(doc: &'a Document, obj: Option<&'a Object>, _key: &[u8]) -> Self {
        obj.and_then(|x| T::from_obj(doc, x))
    }
}

impl<'a, T: FromObj<'a>> FromOptObj<'a> for T {
    fn from_opt_obj(doc: &'a Document, obj: Option<&'a Object>, key: &[u8]) -> Self {
        T::from_obj(doc, obj.expect(&String::from_utf8_lossy(key))).expect("wrong type")
    }
}

// Follow common PDF renderer conventions for when to support indirect objects:
// on arrays, streams and dicts
impl<'a, T: FromObj<'a>> FromObj<'a> for Vec<T> {
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<Self> {
        maybe_deref(doc, obj)
            .as_array()
            .map(|x| {
                x.iter()
                    .map(|x| T::from_obj(doc, x).expect("wrong type"))
                    .collect()
            })
            .ok()
    }
}

// XXX: These will panic if we don't have the right number of items
// we don't want to do that
impl<'a, T: FromObj<'a>> FromObj<'a> for [T; 4] {
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<Self> {
        maybe_deref(doc, obj)
            .as_array()
            .map(|x| {
                let mut all = x.iter().map(|x| T::from_obj(doc, x).expect("wrong type"));
                [
                    all.next().unwrap(),
                    all.next().unwrap(),
                    all.next().unwrap(),
                    all.next().unwrap(),
                ]
            })
            .ok()
    }
}

impl<'a, T: FromObj<'a>> FromObj<'a> for [T; 3] {
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<Self> {
        maybe_deref(doc, obj)
            .as_array()
            .map(|x| {
                let mut all = x.iter().map(|x| T::from_obj(doc, x).expect("wrong type"));
                [
                    all.next().unwrap(),
                    all.next().unwrap(),
                    all.next().unwrap(),
                ]
            })
            .ok()
    }
}

impl<'a> FromObj<'a> for f64 {
    fn from_obj(_doc: &Document, obj: &Object) -> Option<Self> {
        match obj {
            &Object::Integer(i) => Some(i as f64),
            &Object::Real(f) => Some(f.into()),
            _ => None,
        }
    }
}

impl<'a> FromObj<'a> for i64 {
    fn from_obj(_doc: &Document, obj: &Object) -> Option<Self> {
        match obj {
            &Object::Integer(i) => Some(i),
            _ => None,
        }
    }
}

impl<'a> FromObj<'a> for &'a Dictionary {
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
        maybe_deref(doc, obj).as_dict().ok()
    }
}

impl<'a> FromObj<'a> for &'a Stream {
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<&'a Stream> {
        maybe_deref(doc, obj).as_stream().ok()
    }
}

impl<'a> FromObj<'a> for &'a Object {
    fn from_obj(doc: &'a Document, obj: &'a Object) -> Option<&'a Object> {
        Some(maybe_deref(doc, obj))
    }
}

fn get<'a, T: FromOptObj<'a>>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> T {
    T::from_opt_obj(doc, dict.get(key).ok(), key)
}

fn maybe_get<'a, T: FromObj<'a>>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<T> {
    maybe_get_obj(doc, dict, key).and_then(|o| T::from_obj(doc, o))
}

fn get_name_string<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> String {
    pdf_to_utf8(
        dict.get(key)
            .map(|o| maybe_deref(doc, o))
            .unwrap_or_else(|_| panic!("deref"))
            .as_name()
            .expect("name"),
    )
}

#[allow(dead_code)]
fn maybe_get_name_string<'a>(
    doc: &'a Document,
    dict: &'a Dictionary,
    key: &[u8],
) -> Option<String> {
    maybe_get_obj(doc, dict, key)
        .and_then(|n| n.as_name().ok())
        .map(|n| pdf_to_utf8(n))
}

fn font_name_component<'a>(
    doc: &'a Document,
    dict: &'a Dictionary,
    key: &[u8],
    fallback: &str,
) -> String {
    maybe_get_name_string(doc, dict, key).unwrap_or_else(|| fallback.to_string())
}

fn maybe_get_name<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a [u8]> {
    maybe_get_obj(doc, dict, key).and_then(|n| n.as_name().ok())
}
fn maybe_get_array<'a>(
    doc: &'a Document,
    dict: &'a Dictionary,
    key: &[u8],
) -> Option<&'a Vec<Object>> {
    maybe_get_obj(doc, dict, key).and_then(|n| n.as_array().ok())
}

#[derive(Clone)]
struct PdfSimpleFont<'a> {
    font: &'a Dictionary,
    doc: &'a Document,
    encoding: Option<Vec<u16>>,
    unicode_map: Option<HashMap<u32, String>>,
    widths: HashMap<CharCode, f64>, // should probably just use i32 here
    missing_width: f64,
}

#[derive(Clone)]
struct PdfType3Font<'a> {
    font: &'a Dictionary,
    doc: &'a Document,
    encoding: Option<Vec<u16>>,
    unicode_map: Option<HashMap<u32, String>>,
    widths: HashMap<CharCode, f64>, // should probably just use i32 here
}

fn make_font<'a>(doc: &'a Document, font: &'a Dictionary) -> Rc<dyn PdfFont + 'a> {
    let subtype = get_name_string(doc, font, b"Subtype");
    dlog!("MakeFont({})", subtype);
    if subtype == "Type0" {
        Rc::new(PdfCIDFont::new(doc, font))
    } else if subtype == "Type3" {
        Rc::new(PdfType3Font::new(doc, font))
    } else {
        Rc::new(PdfSimpleFont::new(doc, font))
    }
}

fn is_core_font(name: &str) -> bool {
    match name {
        "Courier-Bold"
        | "Courier-BoldOblique"
        | "Courier-Oblique"
        | "Courier"
        | "Helvetica-Bold"
        | "Helvetica-BoldOblique"
        | "Helvetica-Oblique"
        | "Helvetica"
        | "Symbol"
        | "Times-Bold"
        | "Times-BoldItalic"
        | "Times-Italic"
        | "Times-Roman"
        | "ZapfDingbats" => true,
        _ => false,
    }
}

fn encoding_to_unicode_table(name: &[u8]) -> Vec<u16> {
    let encoding = match &name[..] {
        b"MacRomanEncoding" => encodings::MAC_ROMAN_ENCODING,
        b"MacExpertEncoding" => encodings::MAC_EXPERT_ENCODING,
        b"WinAnsiEncoding" => encodings::WIN_ANSI_ENCODING,
        _ => panic!("unexpected encoding {:?}", pdf_to_utf8(name)),
    };
    let encoding_table = encoding
        .iter()
        .map(|x| {
            if let &Some(x) = x {
                glyphnames::name_to_unicode(x).unwrap()
            } else {
                0
            }
        })
        .collect();
    encoding_table
}

/* "Glyphs in the font are selected by single-byte character codes obtained from a string that
    is shown by the text-showing operators. Logically, these codes index into a table of 256
    glyphs; the mapping from codes to glyphs is called the font's encoding. Each font program
    has a built-in encoding. Under some circumstances, the encoding can be altered by means
    described in Section 5.5.5, "Character Encoding."
*/
impl<'a> PdfSimpleFont<'a> {
    fn new(doc: &'a Document, font: &'a Dictionary) -> PdfSimpleFont<'a> {
        let base_name = get_name_string(doc, font, b"BaseFont");
        let subtype = get_name_string(doc, font, b"Subtype");

        let encoding: Option<&Object> = get(doc, font, b"Encoding");
        dlog!(
            "base_name {} {} enc:{:?} {:?}",
            base_name,
            subtype,
            encoding,
            font
        );
        let descriptor: Option<&Dictionary> = get(doc, font, b"FontDescriptor");
        let mut type1_encoding = None;
        let mut embedded_unicode_map: Option<HashMap<u32, String>> = None;
        if let Some(descriptor) = descriptor {
            dlog!("descriptor {:?}", descriptor);
            if subtype == "Type1" {
                let file = maybe_get_obj(doc, descriptor, b"FontFile");
                match file {
                    Some(&Object::Stream(ref s)) => {
                        let s = get_contents(s);
                        //dlog!("font contents {:?}", pdf_to_utf8(&s));
                        type1_encoding =
                            Some(type1_encoding_parser::get_encoding_map(&s).expect("encoding"));
                    }
                    _ => {
                        dlog!("font file {:?}", file)
                    }
                }
            } else if subtype == "TrueType" {
                let file = maybe_get_obj(doc, descriptor, b"FontFile2");
                match file {
                    Some(&Object::Stream(ref s)) => {
                        let _s = get_contents(s);
                        //File::create(format!("/tmp/{}", base_name)).unwrap().write_all(&s);
                    }
                    _ => {
                        dlog!("font file {:?}", file)
                    }
                }
            }

            let font_file3 = get::<Option<&Object>>(doc, descriptor, b"FontFile3");
            match font_file3 {
                Some(&Object::Stream(ref s)) => {
                    let subtype = get_name_string(doc, &s.dict, b"Subtype");
                    dlog!("font file {}, {:?}", subtype, s);
                    if subtype == "Type1C" {
                        let bytes = get_contents(s);
                        match cff_parser::Table::parse(&bytes) {
                            Some(table) => {
                                let charset = table.charset.get_table();
                                let encoding = table.encoding.get_table();
                                let mut mapping = HashMap::new();
                                let limit = encoding.len().min(charset.len());
                                if encoding.len() != charset.len() {
                                    warn!(
                                        "Type1C charset/encoding length mismatch for font {}: encoding={} charset={}",
                                        base_name,
                                        encoding.len(),
                                        charset.len()
                                    );
                                }
                                for i in 0..limit {
                                    let cid = encoding[i];
                                    let sid = charset[i];
                                    let Some(name) = cff_parser::string_by_id(&table, sid) else {
                                        continue;
                                    };
                                    let unicode =
                                        glyphnames::name_to_unicode(&name).or_else(|| {
                                            zapfglyphnames::zapfdigbats_names_to_unicode(&name)
                                        });
                                    if let Some(unicode) = unicode {
                                        if let Ok(text) = String::from_utf16(&[unicode]) {
                                            mapping.insert(cid as u32, text);
                                        }
                                    }
                                }
                                if !mapping.is_empty() {
                                    embedded_unicode_map = Some(mapping);
                                }
                            }
                            None => {
                                warn!("failed to parse embedded Type1C font {}", base_name);
                            }
                        }
                    }
                }
                None => {}
                _ => {
                    dlog!("unexpected")
                }
            }

            let charset = maybe_get_obj(doc, descriptor, b"CharSet");
            let _charset = match charset {
                Some(&Object::String(ref s, _)) => Some(pdf_to_utf8(&s)),
                _ => None,
            };
            //dlog!("charset {:?}", charset);
        }

        let mut unicode_map: Option<HashMap<u32, String>> =
            match (embedded_unicode_map, get_unicode_map(doc, font)) {
                (Some(mut embedded), Some(explicit)) => {
                    embedded.extend(explicit);
                    Some(embedded)
                }
                (Some(embedded), None) => Some(embedded),
                (None, explicit) => explicit,
            };

        let mut encoding_table = None;
        match encoding {
            Some(&Object::Name(ref encoding_name)) => {
                dlog!("encoding {:?}", pdf_to_utf8(encoding_name));
                encoding_table = Some(encoding_to_unicode_table(encoding_name));
            }
            Some(&Object::Dictionary(ref encoding)) => {
                //dlog!("Encoding {:?}", encoding);
                let mut table =
                    if let Some(base_encoding) = maybe_get_name(doc, encoding, b"BaseEncoding") {
                        dlog!("BaseEncoding {:?}", base_encoding);
                        encoding_to_unicode_table(base_encoding)
                    } else {
                        Vec::from(PDFDocEncoding)
                    };
                let differences = maybe_get_array(doc, encoding, b"Differences");
                if let Some(differences) = differences {
                    dlog!("Differences");
                    let mut code = 0;
                    for o in differences {
                        let o = maybe_deref(doc, o);
                        match o {
                            &Object::Integer(i) => {
                                code = i;
                            }
                            &Object::Name(ref n) => {
                                let name = pdf_to_utf8(&n);
                                // XXX: names of Type1 fonts can map to arbitrary strings instead of real
                                // unicode names, so we should probably handle this differently
                                let unicode = glyphnames::name_to_unicode(&name);
                                if let Some(unicode) = unicode {
                                    table[code as usize] = unicode;
                                    if let Some(ref mut unicode_map) = unicode_map {
                                        let be = [unicode];
                                        match unicode_map.entry(code as u32) {
                                            // If there's a unicode table entry missing use one based on the name
                                            Entry::Vacant(v) => {
                                                v.insert(String::from_utf16(&be).unwrap());
                                            }
                                            Entry::Occupied(e) => {
                                                if e.get() != &String::from_utf16(&be).unwrap() {
                                                    let normal_match =
                                                        e.get().nfkc().eq(String::from_utf16(&be)
                                                            .unwrap()
                                                            .nfkc());
                                                    // println!(
                                                    //     "Unicode mismatch {} {} {:?} {:?} {:?}",
                                                    //     normal_match,
                                                    //     name,
                                                    //     e.get(),
                                                    //     String::from_utf16(&be),
                                                    //     be
                                                    // );
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    match unicode_map {
                                        Some(ref mut unicode_map)
                                            if base_name.contains("FontAwesome") =>
                                        {
                                            // the fontawesome tex package will use glyph names that don't have a corresponding unicode
                                            // code point, so we'll use an empty string instead. See issue #76
                                            match unicode_map.entry(code as u32) {
                                                Entry::Vacant(v) => {
                                                    v.insert("".to_owned());
                                                }
                                                Entry::Occupied(e) => {
                                                    panic!("unexpected entry in unicode map")
                                                }
                                            }
                                        }
                                        _ => {
                                            warn!(
                                                "unknown glyph name '{}' for font {}",
                                                name, base_name
                                            );
                                        }
                                    }
                                }
                                dlog!("{} = {} ({:?})", code, name, unicode);
                                if let Some(ref mut unicode_map) = unicode_map {
                                    // The unicode map might not have the code in it, but the code might
                                    // not be used so we don't want to panic here.
                                    // An example of this is the 'suppress' character in the TeX Latin Modern font.
                                    // This shows up in https://arxiv.org/pdf/2405.01295v1.pdf
                                    dlog!("{} {:?}", code, unicode_map.get(&(code as u32)));
                                }
                                code += 1;
                            }
                            _ => {
                                panic!("wrong type {:?}", o);
                            }
                        }
                    }
                }
                let name = encoding
                    .get(b"Type")
                    .and_then(|x| x.as_name())
                    .and_then(|x| Ok(pdf_to_utf8(x)));
                dlog!("name: {}", name);

                encoding_table = Some(table);
            }
            None => {
                if let Some(type1_encoding) = type1_encoding {
                    let mut table = Vec::from(PDFDocEncoding);
                    dlog!("type1encoding");
                    for (code, name) in type1_encoding {
                        let unicode = glyphnames::name_to_unicode(&pdf_to_utf8(&name));
                        if let Some(unicode) = unicode {
                            table[code as usize] = unicode;
                        } else {
                            dlog!("unknown character {}", pdf_to_utf8(&name));
                        }
                    }
                    encoding_table = Some(table)
                } else if subtype == "TrueType" {
                    encoding_table = Some(
                        encodings::WIN_ANSI_ENCODING
                            .iter()
                            .map(|x| {
                                if let &Some(x) = x {
                                    glyphnames::name_to_unicode(x).unwrap()
                                } else {
                                    0
                                }
                            })
                            .collect(),
                    );
                }
            }
            _ => {
                dlog!("unknown encoding {:?}", encoding);
            }
        }

        let mut width_map = HashMap::new();
        /* "Ordinarily, a font dictionary that refers to one of the standard fonts
        should omit the FirstChar, LastChar, Widths, and FontDescriptor entries.
        However, it is permissible to override a standard font by including these
        entries and embedding the font program in the PDF file."

        Note: some PDFs include a descriptor but still don't include these entries */

        // If we have widths prefer them over the core font widths. Needed for https://dkp.de/wp-content/uploads/parteitage/Sozialismusvorstellungen-der-DKP.pdf
        if let (Some(first_char), Some(last_char), Some(widths)) = (
            maybe_get::<i64>(doc, font, b"FirstChar"),
            maybe_get::<i64>(doc, font, b"LastChar"),
            maybe_get::<Vec<f64>>(doc, font, b"Widths"),
        ) {
            // Some PDF's don't have these like fips-197.pdf
            let mut i: i64 = 0;
            dlog!(
                "first_char {:?}, last_char: {:?}, widths: {} {:?}",
                first_char,
                last_char,
                widths.len(),
                widths
            );

            for w in widths {
                width_map.insert((first_char + i) as CharCode, w);
                i += 1;
            }
            assert_eq!(first_char + i - 1, last_char);
        } else if is_core_font(&base_name) {
            populate_core_font_widths(&base_name, &base_name, &mut encoding_table, &mut width_map);
        } else {
            populate_core_font_widths(&base_name, "Helvetica", &mut encoding_table, &mut width_map);
        }

        let missing_width = get::<Option<f64>>(doc, font, b"MissingWidth").unwrap_or(0.);
        PdfSimpleFont {
            doc,
            font,
            widths: width_map,
            encoding: encoding_table,
            missing_width,
            unicode_map,
        }
    }

    #[allow(dead_code)]
    fn get_type(&self) -> String {
        get_name_string(self.doc, self.font, b"Type")
    }
    #[allow(dead_code)]
    fn get_basefont(&self) -> String {
        get_name_string(self.doc, self.font, b"BaseFont")
    }
    #[allow(dead_code)]
    fn get_subtype(&self) -> String {
        get_name_string(self.doc, self.font, b"Subtype")
    }
    #[allow(dead_code)]
    fn get_widths(&self) -> Option<&Vec<Object>> {
        maybe_get_obj(self.doc, self.font, b"Widths")
            .map(|widths| widths.as_array().expect("Widths should be an array"))
    }
    /* For type1: This entry is obsolescent and its use is no longer recommended. (See
     * implementation note 42 in Appendix H.) */
    #[allow(dead_code)]
    fn get_name(&self) -> Option<String> {
        maybe_get_name_string(self.doc, self.font, b"Name")
    }

    #[allow(dead_code)]
    fn get_descriptor(&self) -> Option<PdfFontDescriptor> {
        maybe_get_obj(self.doc, self.font, b"FontDescriptor")
            .and_then(|desc| desc.as_dict().ok())
            .map(|desc| PdfFontDescriptor {
                desc: desc,
                doc: self.doc,
            })
    }
}
impl<'a> PdfType3Font<'a> {
    fn new(doc: &'a Document, font: &'a Dictionary) -> PdfType3Font<'a> {
        // Attempt to read ToUnicode map if present
        let unicode_map = get_unicode_map(doc, font);
        let encoding: Option<&Object> = get(doc, font, b"Encoding");

        let encoding_table;
        match encoding {
            Some(&Object::Name(ref encoding_name)) => {
                dlog!("encoding {:?}", pdf_to_utf8(encoding_name));
                encoding_table = Some(encoding_to_unicode_table(encoding_name));
            }
            Some(&Object::Dictionary(ref encoding)) => {
                //dlog!("Encoding {:?}", encoding);
                let mut table =
                    if let Some(base_encoding) = maybe_get_name(doc, encoding, b"BaseEncoding") {
                        dlog!("BaseEncoding {:?}", base_encoding);
                        encoding_to_unicode_table(base_encoding)
                    } else {
                        Vec::from(PDFDocEncoding)
                    };
                let differences = maybe_get_array(doc, encoding, b"Differences");
                if let Some(differences) = differences {
                    dlog!("Differences");
                    let mut code = 0;
                    for o in differences {
                        match o {
                            &Object::Integer(i) => {
                                code = i;
                            }
                            &Object::Name(ref n) => {
                                let name = pdf_to_utf8(&n);
                                // XXX: names of Type1 fonts can map to arbitrary strings instead of real
                                // unicode names, so we should probably handle this differently
                                let unicode = glyphnames::name_to_unicode(&name);
                                if let Some(unicode) = unicode {
                                    table[code as usize] = unicode;
                                }
                                dlog!("{} = {} ({:?})", code, name, unicode);
                                if let Some(ref unicode_map) = unicode_map {
                                    dlog!("{} {:?}", code, unicode_map.get(&(code as u32)));
                                }
                                code += 1;
                            }
                            _ => {
                                panic!("wrong type");
                            }
                        }
                    }
                }
                let name_encoded = encoding.get(b"Type");
                if let Ok(Object::Name(name)) = name_encoded {
                    dlog!("name: {}", pdf_to_utf8(name));
                } else {
                    dlog!("name not found");
                }

                encoding_table = Some(table);
            }
            _ => {
                panic!()
            }
        }

        let first_char: i64 = get(doc, font, b"FirstChar");
        let last_char: i64 = get(doc, font, b"LastChar");
        let widths: Vec<f64> = get(doc, font, b"Widths");

        let mut width_map = HashMap::new();

        let mut i = 0;
        dlog!(
            "first_char {:?}, last_char: {:?}, widths: {} {:?}",
            first_char,
            last_char,
            widths.len(),
            widths
        );

        for w in widths {
            width_map.insert((first_char + i) as CharCode, w);
            i += 1;
        }
        assert_eq!(first_char + i - 1, last_char);
        PdfType3Font {
            doc,
            font,
            widths: width_map,
            encoding: encoding_table,
            unicode_map,
        }
    }
}

type CharCode = u32;

struct PdfFontIter<'a> {
    i: Iter<'a, u8>,
    font: &'a dyn PdfFont,
}

impl<'a> Iterator for PdfFontIter<'a> {
    type Item = (CharCode, u8);
    fn next(&mut self) -> Option<(CharCode, u8)> {
        self.font.next_char(&mut self.i)
    }
}

trait PdfFont: Debug {
    fn get_width(&self, id: CharCode) -> f64;
    fn next_char(&self, iter: &mut Iter<u8>) -> Option<(CharCode, u8)>;
    fn decode_char(&self, char: CharCode) -> String;
}

impl<'a> dyn PdfFont + 'a {
    fn char_codes(&'a self, chars: &'a [u8]) -> PdfFontIter<'a> {
        PdfFontIter {
            i: chars.iter(),
            font: self,
        }
    }
    fn decode(&self, chars: &[u8]) -> String {
        let strings = self
            .char_codes(chars)
            .map(|x| self.decode_char(x.0))
            .collect::<Vec<_>>();
        strings.join("")
    }
}

impl<'a> PdfFont for PdfSimpleFont<'a> {
    fn get_width(&self, id: CharCode) -> f64 {
        let width = self.widths.get(&id);
        if let Some(width) = width {
            return *width;
        } else {
            let mut widths = self.widths.iter().collect::<Vec<_>>();
            widths.sort_by_key(|x| x.0);
            dlog!(
                "missing width for {} len(widths) = {}, {:?} falling back to missing_width {:?}",
                id,
                self.widths.len(),
                widths,
                self.font
            );
            return self.missing_width;
        }
    }

    fn next_char(&self, iter: &mut Iter<u8>) -> Option<(CharCode, u8)> {
        iter.next().map(|x| (*x as CharCode, 1))
    }
    fn decode_char(&self, char: CharCode) -> String {
        let base_name = font_name_component(self.doc, self.font, b"BaseFont", "<missing-basefont>");
        let subtype = font_name_component(self.doc, self.font, b"Subtype", "<missing-subtype>");
        let font_key = format!("{}:{}:Simple", base_name, subtype);
        log_font_once(
            "font-init:",
            &font_key,
            &format!(
                "to_unicode={} encoding_table={}",
                self.unicode_map.as_ref().map(|m| m.len()).unwrap_or(0),
                self.encoding.as_ref().map(|v| v.len()).unwrap_or(0)
            ),
        );
        let slice = [char as u8];
        if let Some(ref unicode_map) = self.unicode_map {
            let s = unicode_map.get(&char);
            let s = match s {
                None => {
                    log_font_once(
                        "font-missing:",
                        &format!("{}:missing-simple:{}", font_key, char),
                        &format!("char={} font={:?}", char, self.font),
                    );
                    // some pdf's like http://arxiv.org/pdf/2312.00064v1 are missing entries in their unicode map but do have
                    // entries in the encoding.
                    let encoding = self
                        .encoding
                        .as_ref()
                        .map(|x| &x[..])
                        .expect("missing unicode map and encoding");
                    let s = to_utf8(encoding, &slice);
                    trace!("falling back to encoding {} -> {:?}", char, s);
                    s
                }
                Some(s) => s.clone(),
            };
            record_font_decode(&font_key, !s.is_empty());
            return strip_non_printing_text(&s);
        }
        let encoding = self
            .encoding
            .as_ref()
            .map(|x| &x[..])
            .unwrap_or(&PDFDocEncoding);
        //dlog!("char_code {:?} {:?}", char, self.encoding);
        let s = to_utf8(encoding, &slice);
        record_font_decode(&font_key, !s.is_empty());
        strip_non_printing_text(&s)
    }
}

impl<'a> fmt::Debug for PdfSimpleFont<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.font.fmt(f)
    }
}

impl<'a> PdfFont for PdfType3Font<'a> {
    fn get_width(&self, id: CharCode) -> f64 {
        let width = self.widths.get(&id);
        if let Some(width) = width {
            return *width;
        } else {
            warn!(
                "missing Type3 width for char {}; falling back to default width",
                id
            );
            1000.0
        }
    }

    fn next_char(&self, iter: &mut Iter<u8>) -> Option<(CharCode, u8)> {
        iter.next().map(|x| (*x as CharCode, 1))
    }
    fn decode_char(&self, char: CharCode) -> String {
        let base_name = font_name_component(self.doc, self.font, b"BaseFont", "<missing-basefont>");
        let subtype = font_name_component(self.doc, self.font, b"Subtype", "<missing-subtype>");
        let font_key = format!("{}:{}:Type3", base_name, subtype);
        log_font_once(
            "font-init:",
            &font_key,
            &format!(
                "to_unicode={} encoding_table={}",
                self.unicode_map.as_ref().map(|m| m.len()).unwrap_or(0),
                self.encoding.as_ref().map(|v| v.len()).unwrap_or(0)
            ),
        );
        let slice = [char as u8];
        if let Some(ref unicode_map) = self.unicode_map {
            let s = unicode_map.get(&char);
            let s = match s {
                None => {
                    log_font_once(
                        "font-missing:",
                        &format!("{}:missing-type3:{}", font_key, char),
                        &format!("char={} font={:?}", char, self.font),
                    );
                    let encoding = self
                        .encoding
                        .as_ref()
                        .map(|x| &x[..])
                        .expect("missing unicode map and encoding");
                    let s = to_utf8(encoding, &slice);
                    trace!("falling back to type3 encoding {} -> {:?}", char, s);
                    s
                }
                Some(s) => s.clone(),
            };
            record_font_decode(&font_key, !s.is_empty());
            return strip_non_printing_text(&s);
        }
        let encoding = self
            .encoding
            .as_ref()
            .map(|x| &x[..])
            .unwrap_or(&PDFDocEncoding);
        //dlog!("char_code {:?} {:?}", char, self.encoding);
        let s = to_utf8(encoding, &slice);
        record_font_decode(&font_key, !s.is_empty());
        strip_non_printing_text(&s)
    }
}

impl<'a> fmt::Debug for PdfType3Font<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.font.fmt(f)
    }
}

struct PdfCIDFont<'a> {
    font: &'a Dictionary,
    #[allow(dead_code)]
    doc: &'a Document,
    #[allow(dead_code)]
    encoding: ByteMapping,
    to_unicode: Option<HashMap<u32, String>>,
    widths: HashMap<CharCode, f64>, // should probably just use i32 here
    default_width: Option<f64>, // only used for CID fonts and we should probably brake out the different font types
}

fn get_unicode_map<'a>(doc: &'a Document, font: &'a Dictionary) -> Option<HashMap<u32, String>> {
    let to_unicode = maybe_get_obj(doc, font, b"ToUnicode");
    dlog!("ToUnicode: {:?}", to_unicode);
    let mut unicode_map = None;
    match to_unicode {
        Some(&Object::Stream(ref stream)) => {
            let contents = get_contents(stream);
            dlog!("Stream: {}", String::from_utf8(contents.clone()).unwrap());

            let cmap = crate::cmap::get_unicode_map(&contents).unwrap();
            let mut unicode = HashMap::new();
            // "It must use the beginbfchar, endbfchar, beginbfrange, and endbfrange operators to
            // define the mapping from character codes to Unicode character sequences expressed in
            // UTF-16BE encoding."
            for (&k, v) in cmap.iter() {
                let mut be: Vec<u16> = Vec::new();
                let mut i = 0;
                assert!(v.len() % 2 == 0);
                while i < v.len() {
                    be.push(((v[i] as u16) << 8) | v[i + 1] as u16);
                    i += 2;
                }
                match &be[..] {
                    [0xd800..=0xdfff] => {
                        // this range is not specified as not being encoded
                        // we ignore them so we don't an error from from_utt16
                        continue;
                    }
                    _ => {}
                }
                let s = String::from_utf16(&be).unwrap();

                unicode.insert(k, s);
            }
            unicode_map = Some(unicode);

            dlog!("map: {:?}", unicode_map);
        }
        None => {}
        Some(&Object::Name(ref name)) => {
            let name = pdf_to_utf8(name);
            if name != "Identity-H" && name != "Identity-V" {
                todo!("unsupported ToUnicode name: {:?}", name);
            }
        }
        _ => {
            panic!("unsupported cmap {:?}", to_unicode)
        }
    }
    unicode_map
}

impl<'a> PdfCIDFont<'a> {
    fn new(doc: &'a Document, font: &'a Dictionary) -> PdfCIDFont<'a> {
        let base_name = get_name_string(doc, font, b"BaseFont");
        let descendants =
            maybe_get_array(doc, font, b"DescendantFonts").expect("Descendant fonts required");
        let ciddict = maybe_deref(doc, &descendants[0])
            .as_dict()
            .expect("should be CID dict");
        let encoding =
            maybe_get_obj(doc, font, b"Encoding").expect("Encoding required in type0 fonts");
        dlog!("base_name {} {:?}", base_name, font);

        let encoding = match encoding {
            &Object::Name(ref name) => {
                let name = pdf_to_utf8(name);
                dlog!("encoding {:?}", name);
                if name == "Identity-H" || name == "Identity-V" {
                    ByteMapping {
                        codespace: vec![CodeRange {
                            width: 2,
                            start: 0,
                            end: 0xffff,
                        }],
                        cid: vec![CIDRange {
                            src_code_lo: 0,
                            src_code_hi: 0xffff,
                            dst_CID_lo: 0,
                        }],
                    }
                } else {
                    panic!("unsupported encoding {}", name);
                }
            }
            &Object::Stream(ref stream) => {
                let contents = get_contents(stream);
                dlog!("Stream: {}", String::from_utf8(contents.clone()).unwrap());
                crate::cmap::get_byte_mapping(&contents).unwrap()
            }
            _ => {
                panic!("unsupported encoding {:?}", encoding)
            }
        };

        // Sometimes a Type0 font might refer to the same underlying data as regular font. In this case we may be able to extract some encoding
        // data. We try ToUnicode on the Type0 font; if missing, also try on the descendant CID font.
        let mut unicode_map = get_unicode_map(doc, font);
        if unicode_map.is_none() {
            unicode_map = get_unicode_map(doc, ciddict);
        }
        let map_len = unicode_map.as_ref().map(|m| m.len()).unwrap_or(0);
        debug!("CID font {} ToUnicode entries: {}", base_name, map_len);

        dlog!("descendents {:?} {:?}", descendants, ciddict);

        let font_dict = maybe_get_obj(doc, ciddict, b"FontDescriptor").expect("required");
        dlog!("{:?}", font_dict);
        let _f = font_dict.as_dict().expect("must be dict");
        let default_width = get::<Option<i64>>(doc, ciddict, b"DW").unwrap_or(1000);
        let w: Option<Vec<&Object>> = get(doc, ciddict, b"W");
        dlog!("widths {:?}", w);
        let mut widths = HashMap::new();
        let mut i = 0;
        if let Some(w) = w {
            while i < w.len() {
                let Some(next) = w.get(i + 1) else {
                    warn!(
                        "malformed CID width array for {}: missing entry after index {}",
                        base_name, i
                    );
                    break;
                };

                if let Object::Array(wa) = *next {
                    let cid = w[i].as_i64().expect("id should be num");
                    dlog!("wa: {:?} -> {:?}", cid, wa);
                    for (j, width) in wa.iter().enumerate() {
                        widths.insert((cid + j as i64) as CharCode, as_num(width));
                    }
                    i += 2;
                    continue;
                }

                let (Some(first), Some(last), Some(width)) = (w.get(i), w.get(i + 1), w.get(i + 2))
                else {
                    warn!(
                        "malformed CID width range for {}: truncated triplet at index {}",
                        base_name, i
                    );
                    break;
                };
                let c_first = first.as_i64().expect("first should be num");
                let c_last = last.as_i64().expect("last should be num");
                let c_width = as_num(width);
                for id in c_first..=c_last {
                    widths.insert(id as CharCode, c_width);
                }
                i += 3;
            }
        }
        PdfCIDFont {
            doc,
            font,
            widths,
            to_unicode: unicode_map,
            encoding,
            default_width: Some(default_width as f64),
        }
    }
}

impl<'a> PdfFont for PdfCIDFont<'a> {
    fn get_width(&self, id: CharCode) -> f64 {
        // For CID fonts, `id` should represent the original character code from the stream.
        // Map char code -> CID using the encoding, then look up CID width in `self.widths`.
        let mut cid: Option<CharCode> = None;
        for range in &self.encoding.cid {
            if id as u32 >= range.src_code_lo && id as u32 <= range.src_code_hi {
                cid = Some(((id as u32 - range.src_code_lo) + range.dst_CID_lo) as CharCode);
                break;
            }
        }

        let key = cid.unwrap_or(id);
        if let Some(width) = self.widths.get(&key) {
            dlog!("GetWidth (char={}) CID={} -> {}", id, key, *width);
            *width
        } else {
            dlog!(
                "missing width for char={} (CID={}), falling back to default_width",
                id,
                key
            );
            self.default_width.unwrap()
        }
    } /*
      fn decode(&self, chars: &[u8]) -> String {
          self.char_codes(chars);


          let encoding = self.encoding.as_ref().map(|x| &x[..]).unwrap_or(&PDFDocEncoding);
          to_utf8(encoding, chars)
      }*/

    fn next_char(&self, iter: &mut Iter<u8>) -> Option<(CharCode, u8)> {
        let mut c = *iter.next()? as u32;
        let mut code = None;
        'outer: for width in 1..=4 {
            for range in &self.encoding.codespace {
                if c as u32 >= range.start && c as u32 <= range.end && range.width == width {
                    code = Some((c as u32, width));
                    break 'outer;
                }
            }
            let next = *iter.next()?;
            c = ((c as u32) << 8) | next as u32;
        }
        let code = code?;
        // Return the original character code and width (bytes consumed).
        // Width lookup will map to CID internally in get_width().
        Some((code.0 as CharCode, code.1 as u8))
    }
    fn decode_char(&self, char: CharCode) -> String {
        // `char` is the original character code from the content stream.
        // Prefer direct ToUnicode mapping using that code.
        let base_name = font_name_component(self.doc, self.font, b"BaseFont", "<missing-basefont>");
        let font_key = format!("{}:{}:CID", base_name, "Type0");
        if let Some(map) = self.to_unicode.as_ref() {
            if let Some(s) = map.get(&(char as u32)) {
                record_font_decode(&font_key, !s.is_empty());
                return s.clone();
            }
            // Fallback: some ToUnicode maps are keyed by CID; map char code -> CID and try again
            for range in &self.encoding.cid {
                if char as u32 >= range.src_code_lo && char as u32 <= range.src_code_hi {
                    let cid = (char as u32 - range.src_code_lo) + range.dst_CID_lo;
                    if let Some(s) = map.get(&cid) {
                        record_font_decode(&font_key, !s.is_empty());
                        return s.clone();
                    }
                    break;
                }
            }
        }
        // Last-resort fallback:
        // Some PDFs (Identity-H) effectively carry UTF-16BE values directly in char codes.
        // Try interpreting the 2-byte code as UTF-16 before falling back to PDFDocEncoding per byte.
        let mut out = String::new();
        if char > 0xFF {
            let u = char as u16;
            if let Ok(s) = String::from_utf16(&[u]) {
                if !s.is_empty() {
                    out.push_str(&s);
                }
            }
        }
        if out.is_empty() {
            let encoding = &PDFDocEncoding;
            if char <= 0xFF {
                let slice = [char as u8];
                out.push_str(&to_utf8(encoding, &slice));
            } else {
                let hi = ((char >> 8) & 0xFF) as u8;
                let lo = (char & 0xFF) as u8;
                out.push_str(&to_utf8(encoding, &[hi]));
                out.push_str(&to_utf8(encoding, &[lo]));
            }
        }
        record_font_decode(&font_key, !out.is_empty());
        strip_non_printing_text(&out)
    }
}

impl<'a> fmt::Debug for PdfCIDFont<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.font.fmt(f)
    }
}

#[derive(Copy, Clone)]
struct PdfFontDescriptor<'a> {
    desc: &'a Dictionary,
    doc: &'a Document,
}

impl<'a> PdfFontDescriptor<'a> {
    #[allow(dead_code)]
    fn get_file(&self) -> Option<&'a Object> {
        maybe_get_obj(self.doc, self.desc, b"FontFile")
    }
}

impl<'a> fmt::Debug for PdfFontDescriptor<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.desc.fmt(f)
    }
}

#[derive(Clone, Debug)]
struct Type0Func {
    domain: Vec<f64>,
    range: Vec<f64>,
    contents: Vec<u8>,
    size: Vec<i64>,
    bits_per_sample: i64,
    encode: Vec<f64>,
    decode: Vec<f64>,
}

#[allow(dead_code)]
fn interpolate(x: f64, x_min: f64, _x_max: f64, y_min: f64, y_max: f64) -> f64 {
    let divisor = x - x_min;
    if divisor != 0. {
        y_min + (x - x_min) * ((y_max - y_min) / divisor)
    } else {
        // (x - x_min) will be 0 which means we want to discard the interpolation
        // and arbitrarily choose y_min.
        y_min
    }
}

impl Type0Func {
    #[allow(dead_code)]
    fn eval(&self, _input: &[f64], _output: &mut [f64]) {
        let _n_inputs = self.domain.len() / 2;
        let _n_ouputs = self.range.len() / 2;
    }
}

#[derive(Clone, Debug)]
struct Type2Func {
    c0: Option<Vec<f64>>,
    c1: Option<Vec<f64>>,
    n: f64,
}

#[derive(Clone, Debug)]
enum Function {
    Type0(Type0Func),
    Type2(Type2Func),
    #[allow(dead_code)]
    Type3,
    #[allow(dead_code)]
    Type4,
}

impl Function {
    fn new(doc: &Document, obj: &Object) -> Function {
        let dict = match obj {
            &Object::Dictionary(ref dict) => dict,
            &Object::Stream(ref stream) => &stream.dict,
            _ => panic!(),
        };
        let function_type: i64 = get(doc, dict, b"FunctionType");
        let f = match function_type {
            0 => {
                let stream = match obj {
                    &Object::Stream(ref stream) => stream,
                    _ => panic!(),
                };
                let range: Vec<f64> = get(doc, dict, b"Range");
                let domain: Vec<f64> = get(doc, dict, b"Domain");
                let contents = get_contents(stream);
                let size: Vec<i64> = get(doc, dict, b"Size");
                let bits_per_sample = get(doc, dict, b"BitsPerSample");
                // We ignore 'Order' like other PDF renderers do.

                let encode = get::<Option<Vec<f64>>>(doc, dict, b"Encode");
                // maybe there's some better way to write this.
                let encode = encode.unwrap_or_else(|| {
                    let mut default = Vec::new();
                    for i in &size {
                        default.extend([0., (i - 1) as f64].iter());
                    }
                    default
                });
                let decode =
                    get::<Option<Vec<f64>>>(doc, dict, b"Decode").unwrap_or_else(|| range.clone());

                Function::Type0(Type0Func {
                    domain,
                    range,
                    size,
                    contents,
                    bits_per_sample,
                    encode,
                    decode,
                })
            }
            2 => {
                let c0 = get::<Option<Vec<f64>>>(doc, dict, b"C0");
                let c1 = get::<Option<Vec<f64>>>(doc, dict, b"C1");
                let n = get::<f64>(doc, dict, b"N");
                Function::Type2(Type2Func { c0, c1, n })
            }
            _ => {
                panic!("unhandled function type {}", function_type)
            }
        };
        f
    }
}

fn as_num(o: &Object) -> f64 {
    match o {
        &Object::Integer(i) => i as f64,
        &Object::Real(f) => f.into(),
        _ => {
            panic!("not a number")
        }
    }
}

fn median_f64(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

fn normalize_display_spaced_text(text: &str) -> String {
    fn flush_run(out: &mut Vec<String>, run: &mut Vec<String>) {
        if run.is_empty() {
            return;
        }

        let all_upper = run.iter().all(|token| {
            token
                .chars()
                .next()
                .map(|ch| ch.is_uppercase())
                .unwrap_or(false)
        });
        let join_threshold = if all_upper { 5 } else { 7 };

        if run.len() >= join_threshold {
            out.push(run.join(""));
        } else {
            out.extend(run.drain(..));
            return;
        }

        run.clear();
    }

    text.lines()
        .map(|line| {
            let mut out = Vec::new();
            let mut run = Vec::new();

            for token in line.split_whitespace() {
                let is_single_alpha = token.chars().count() == 1
                    && token
                        .chars()
                        .next()
                        .map(|ch| ch.is_alphabetic())
                        .unwrap_or(false);

                if is_single_alpha {
                    run.push(token.to_string());
                } else {
                    flush_run(&mut out, &mut run);
                    out.push(token.to_string());
                }
            }

            flush_run(&mut out, &mut run);
            out.join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone)]
struct TextState<'a> {
    font: Option<Rc<dyn PdfFont + 'a>>,
    font_size: f64,
    character_spacing: f64,
    word_spacing: f64,
    horizontal_scaling: f64,
    leading: f64,
    rise: f64,
    tm: Transform,
    // PDF text rendering mode (Tr): 0=fill,1=stroke,2=fill+stroke,3=invisible, etc.
    rendering_mode: i32,
}

// XXX: We'd ideally implement this without having to copy the uncompressed data
fn get_contents(contents: &Stream) -> Vec<u8> {
    if contents.filters().is_ok() {
        contents
            .decompressed_content()
            .unwrap_or_else(|_| contents.content.clone())
    } else {
        contents.content.clone()
    }
}

#[derive(Debug, Clone)]
struct PdfImage<'a> {
    pub id: ObjectId,
    pub width: i64,
    pub height: i64,
    pub color_space: Option<String>,
    pub filters: Option<Vec<String>>,
    pub bits_per_component: Option<i64>,
    pub content: &'a [u8],
    pub origin_dict: &'a Dictionary,
    pub components: Option<usize>, // 1 for gray, 3 for RGB, 4 for CMYK
    pub decode_predictor: Option<i32>, // From /DecodeParms /Predictor
    /// Upper bound for decoded bitmap bytes and the RGB conversion buffer.
    pub decoded_byte_limit: usize,
}

#[derive(Debug, Clone, Copy)]
struct ImageMemoryBounds {
    decoded_bytes: usize,
    working_set_bytes: usize,
}

fn image_memory_bounds(
    width: usize,
    height: usize,
    components: usize,
    bits_per_component: usize,
    predictor: bool,
    stored_bytes: usize,
) -> Option<ImageMemoryBounds> {
    let row_bits = width
        .checked_mul(components)?
        .checked_mul(bits_per_component)?;
    let row_bytes = row_bits.checked_add(7)?.checked_div(8)?;
    let raw_bytes = row_bytes.checked_mul(height)?;
    let predictor_bytes = if predictor {
        raw_bytes.checked_add(height)?
    } else {
        raw_bytes
    };
    let rgb_bytes = width.checked_mul(height)?.checked_mul(3)?;
    let inflate_peak = stored_bytes.checked_add(predictor_bytes)?;
    let predictor_peak = if predictor {
        predictor_bytes.checked_add(raw_bytes)?
    } else {
        predictor_bytes
    };
    // The decoded bitmap remains live while RGB conversion runs, and a flip,
    // rotation, or downscale can briefly hold both RGB buffers.
    let render_peak = raw_bytes.checked_add(rgb_bytes.checked_mul(2)?)?;
    Some(ImageMemoryBounds {
        decoded_bytes: predictor_bytes,
        working_set_bytes: inflate_peak.max(predictor_peak).max(render_peak),
    })
}

/// Get the page rotation from the page dictionary (inheritable)
fn get_page_rotation(page_dict: &Dictionary, doc: &Document) -> i32 {
    // /Rotate is inheritable – walk up the page tree
    get_inherited(doc, page_dict, b"Rotate")
        .and_then(|o: &Object| o.as_i64().ok())
        .unwrap_or(0) as i32
}

/// Build the initial CTM with page rotation only.
///
/// Viewer-space Y conversion is applied exactly once at text/bbox emission.
fn build_initial_ctm(_media_box: &[f64], page_rotate: i32) -> Transform {
    let mut init = Transform::identity();

    init = match page_rotate {
        90 => init.pre_rotate(euclid::Angle::radians(std::f64::consts::FRAC_PI_2)),
        180 => init.pre_rotate(euclid::Angle::radians(std::f64::consts::PI)),
        270 => init.pre_rotate(euclid::Angle::radians(-std::f64::consts::FRAC_PI_2)),
        _ => init,
    };

    init
}

/// Build the initial CTM for images (only page rotation, no Y-flip)
fn build_image_ctm(page_rotate: i32) -> Transform {
    let mut init = Transform::identity();

    // Apply page rotation for images, NOT the viewer Y-flip
    init = match page_rotate {
        90 => init.pre_rotate(euclid::Angle::radians(std::f64::consts::FRAC_PI_2)),
        180 => init.pre_rotate(euclid::Angle::radians(std::f64::consts::PI)),
        270 => init.pre_rotate(euclid::Angle::radians(-std::f64::consts::FRAC_PI_2)),
        _ => init,
    };

    init
}

/// Decode PDF stream data, handling compression and PNG predictors
fn decode_stream(img: &PdfImage) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut data = img.content.to_vec();

    // 1. Handle compression filters
    if let Some(filters) = &img.filters {
        for filter in filters {
            match filter.as_str() {
                "FlateDecode" => {
                    debug!("Decompressing FlateDecode filter");
                    data = bounded_flate_decode(&data, img.decoded_byte_limit)?;
                }
                "LZWDecode" => {
                    // For now, treat LZW same as Flate (many PDFs mislabel)
                    debug!("Decompressing LZWDecode filter (treating as FlateDecode)");
                    match bounded_flate_decode(&data, img.decoded_byte_limit) {
                        Ok(decompressed) => data = decompressed,
                        Err(_) => {
                            warn!("LZWDecode failed, keeping original data");
                        }
                    }
                }
                _ => {
                    debug!("Unhandled filter: {}", filter);
                }
            }
        }
    }

    // 2. Handle PNG predictor
    if let Some(predictor) = img.decode_predictor {
        debug!("Applying PNG predictor: {}", predictor);

        if predictor >= 10 && predictor <= 15 {
            // PNG predictors
            let components = img.components.unwrap_or(3);
            let bits_per_component = img.bits_per_component.unwrap_or(8) as usize;
            let bytes_per_pixel = (bits_per_component * components + 7) / 8;
            let row_bytes = img.width as usize * bytes_per_pixel;
            let mut output = Vec::with_capacity(row_bytes * img.height as usize);

            // Each row has a predictor byte prefix
            for (row_idx, chunk) in data.chunks(row_bytes + 1).enumerate() {
                if chunk.len() < 2 {
                    warn!("Incomplete row at index {}", row_idx);
                    continue;
                }

                let (predictor_byte, row_data) = chunk.split_first().unwrap();

                match predictor_byte {
                    0 => {
                        // None predictor
                        output.extend_from_slice(row_data);
                    }
                    1 => {
                        // Sub predictor (horizontal differencing)
                        for (i, &byte) in row_data.iter().enumerate() {
                            let prior = if i >= bytes_per_pixel {
                                output[output.len() - bytes_per_pixel]
                            } else {
                                0
                            };
                            output.push(byte.wrapping_add(prior));
                        }
                    }
                    2 => {
                        // Up predictor (vertical differencing)
                        if row_idx == 0 {
                            // First row has no up reference
                            output.extend_from_slice(row_data);
                        } else {
                            for (i, &byte) in row_data.iter().enumerate() {
                                let prior = output[output.len() - row_bytes + i];
                                output.push(byte.wrapping_add(prior));
                            }
                        }
                    }
                    3 => {
                        // Average predictor
                        for (i, &byte) in row_data.iter().enumerate() {
                            let left = if i >= bytes_per_pixel {
                                output[output.len() - bytes_per_pixel] as u16
                            } else {
                                0
                            };
                            let up = if row_idx > 0 {
                                output[output.len() - row_bytes + i] as u16
                            } else {
                                0
                            };
                            let avg = ((left + up) / 2) as u8;
                            output.push(byte.wrapping_add(avg));
                        }
                    }
                    4 => {
                        // Paeth predictor
                        for (i, &byte) in row_data.iter().enumerate() {
                            let a = if i >= bytes_per_pixel {
                                output[output.len() - bytes_per_pixel] as i16
                            } else {
                                0
                            };
                            let b = if row_idx > 0 {
                                output[output.len() - row_bytes + i] as i16
                            } else {
                                0
                            };
                            let c = if row_idx > 0 && i >= bytes_per_pixel {
                                output[output.len() - row_bytes + i - bytes_per_pixel] as i16
                            } else {
                                0
                            };

                            // Paeth predictor algorithm
                            let p = a + b - c;
                            let pa = (p - a).abs();
                            let pb = (p - b).abs();
                            let pc = (p - c).abs();

                            let prediction = if pa <= pb && pa <= pc {
                                a as u8
                            } else if pb <= pc {
                                b as u8
                            } else {
                                c as u8
                            };

                            output.push(byte.wrapping_add(prediction));
                        }
                    }
                    _ => {
                        warn!("Unknown PNG predictor byte: {}", predictor_byte);
                        output.extend_from_slice(row_data);
                    }
                }
            }

            data = output;
        }
    }

    Ok(data)
}

/// Helper function to split OCR text into smaller segments respecting token limits
fn split_ocr_text_to_segments(
    text: &str,
    max_tokens: usize,
    page_num: u32,
    position: (f64, f64),
    size: (f64, f64),
    char_start: usize,
    current_font_size: f64,
    current_transformed_font_size: f64,
) -> Vec<TextSegment> {
    let mut segments = Vec::new();

    // Use text-splitter for proper tokenization
    match get_text_splitter_shared(max_tokens) {
        Ok(splitter) => {
            let chunks = splitter.chunks(text);
            let mut current_pos = char_start;

            for (idx, chunk) in chunks.enumerate() {
                let chunk_text = chunk.to_string();
                let chunk_len = chunk_text.chars().count();
                let word_count = unicode_count_words(&chunk_text);

                segments.push(TextSegment {
                    content: chunk_text,
                    font_size: current_font_size,
                    transformed_font_size: current_transformed_font_size,
                    x: position.0,
                    y: position.1 + (idx as f64 * current_font_size * 1.2), // Adjust Y for subsequent chunks
                    is_bold: false,
                    font_name: "OCR".to_string(),
                    font_weight: FontWeight::Regular,
                    is_italic: false,
                    page_num,
                    cutat: format!("OCR[{}]", idx),
                    fill_color: None,
                    stroke_color: None,
                    char_start: current_pos,
                    char_end: current_pos + chunk_len,
                    width: size.0,
                    height: size.1,
                    word_count,
                    located_text: None,
                });
                current_pos += chunk_len;
            }
        }
        Err(e) => {
            // Fallback: if text-splitter fails, create a single segment with warning
            warn!(
                "OCR text splitter failed: {}. Using fallback single segment.",
                e
            );
            let sanitized = strip_non_printing_text(text);
            let word_count = unicode_count_words(&sanitized);
            segments.push(TextSegment {
                content: sanitized,
                font_size: current_font_size,
                transformed_font_size: current_transformed_font_size,
                x: position.0,
                y: position.1,
                is_bold: false,
                font_name: "OCR".to_string(),
                font_weight: FontWeight::Regular,
                is_italic: false,
                page_num,
                cutat: "OCRFallback".to_string(),
                fill_color: None,
                stroke_color: None,
                char_start,
                char_end: char_start + text.chars().count(),
                width: size.0,
                height: size.1,
                word_count,
                located_text: None,
            });
        }
    }

    segments
}

// In your content processing function:
fn process_xobject(
    doc: &Document,
    resources: &Dictionary,
    xobject_name: &[u8],
    ocr_handler: Option<&OcrHandler>,
    ocr_telemetry: Option<&OcrImageTelemetry>,
    text_segments: &mut Vec<TextSegment>,
    position: (f64, f64),
    size: (f64, f64),
    page_num: u32,
    current_font_size: f64,
    current_transformed_font_size: f64,
    page_char_counter: &mut usize,
    current_transform: &Transform,
) -> Result<(), Box<dyn std::error::Error>> {
    debug!("process_xobject called for page {}", page_num);
    let xobject = doc.get_dict_in_dict(resources, b"XObject")?;
    debug!("Found XObject dictionary with {} entries", xobject.len());

    let xvalue = xobject.get(xobject_name)?;
    let id = xvalue.as_reference()?;
    let xvalue = doc.get_object(id)?;
    let xvalue = xvalue.as_stream()?;
    let dict = &xvalue.dict;

    // Only process the invoked image XObject. Form XObjects are handled by
    // recursive stream processing in the caller.
    let subtype = dict.get(b"Subtype")?.as_name()?;
    debug!("XObject subtype: {:?}", String::from_utf8_lossy(subtype));
    if subtype != b"Image" {
        return Ok(());
    }
    debug!("Found invoked Image XObject!");
    if let Some(stats) = ocr_telemetry {
        stats.image_seen();
    }

    // Extract image information
    let width = dict.get(b"Width")?.as_i64()?;
    let height = dict.get(b"Height")?.as_i64()?;
    info!("Found image: {}x{} pixels", width, height);

    // Skip extremely large images that might cause issues
    if width > 10000 || height > 10000 {
        warn!("Skipping extremely large image: {}x{}", width, height);
        if let Some(stats) = ocr_telemetry {
            stats.image_skipped();
        }
        return Ok(());
    }
    let color_space = match dict.get(b"ColorSpace") {
        Ok(cs) => match cs {
            Object::Array(array) => Some(String::from_utf8_lossy(array[0].as_name()?).to_string()),
            Object::Name(name) => Some(String::from_utf8_lossy(name).to_string()),
            _ => None,
        },
        Err(_) => None,
    };

    let bits_per_component = match dict.get(b"BitsPerComponent") {
        Ok(bpc) => Some(bpc.as_i64()?),
        Err(_) => None,
    };

    let mut filters = vec![];
    if let Ok(filter) = dict.get(b"Filter") {
        match filter {
            Object::Array(array) => {
                for obj in array.iter() {
                    let name = obj.as_name()?;
                    filters.push(String::from_utf8_lossy(name).to_string());
                }
            }
            Object::Name(name) => {
                filters.push(String::from_utf8_lossy(name).to_string());
            }
            _ => {}
        }
    };

    // Parse components from color space
    let components = match &color_space {
        Some(cs) => match cs.as_str() {
            "DeviceGray" => Some(1),
            "DeviceRGB" => Some(3),
            "DeviceCMYK" => Some(4),
            _ => None,
        },
        None => None,
    };

    // Parse decode parameters
    let decode_predictor = dict
        .get(b"DecodeParms")
        .ok()
        .and_then(|obj| obj.as_dict().ok())
        .and_then(|dict| dict.get(b"Predictor").ok())
        .and_then(|p| p.as_i64().ok())
        .map(|p| p as i32);

    let pdf_image = PdfImage {
        id,
        width,
        height,
        color_space,
        bits_per_component,
        filters: Some(filters),
        content: &xvalue.content,
        origin_dict: &xvalue.dict,
        components,
        decode_predictor,
        decoded_byte_limit: image_memory_bounds(
            width.try_into().unwrap_or(usize::MAX),
            height.try_into().unwrap_or(usize::MAX),
            components.unwrap_or(4),
            bits_per_component
                .unwrap_or(8)
                .try_into()
                .unwrap_or(usize::MAX),
            decode_predictor.is_some_and(|predictor| (10..=15).contains(&predictor)),
            xvalue.content.len(),
        )
        .map(|bounds| bounds.decoded_bytes)
        .unwrap_or(usize::MAX),
    };

    debug!("OCR handler check before");
    // Only attempt OCR if we have a handler
    if let Some(handler) = ocr_handler {
        debug!("OCR handler check after");
        debug!(
            "Attempting image conversion: {}x{} filters={:?}",
            pdf_image.width, pdf_image.height, pdf_image.filters
        );
        // Pass the current transform to properly orient the image
        let det = current_transform.m11 * current_transform.m22
            - current_transform.m12 * current_transform.m21;
        debug!(
            "Image transform CTM: [{:.3} {:.3} {:.3} {:.3} {:.1} {:.1}] det={:.3}",
            current_transform.m11,
            current_transform.m12,
            current_transform.m21,
            current_transform.m22,
            current_transform.m31,
            current_transform.m32,
            det
        );
        match pdf_image.to_rgb_image(Some(current_transform)) {
            Ok(img) => {
                if let Some(stats) = ocr_telemetry {
                    stats.image_converted();
                }
                debug!(
                    "Successful image conversion {}x{}",
                    img.dimensions().0,
                    img.dimensions().1
                );
                match handler.process_image(&img) {
                    Ok(ocr_text) => {
                        if ocr_text.is_empty() {
                            debug!(
                                "OCR: No text detected in image at page {} position ({:.1},{:.1})",
                                page_num, position.0, position.1
                            );
                        } else {
                            let ocr_chars = ocr_text.chars().count();
                            if let Some(stats) = ocr_telemetry {
                                stats.image_text_emitted(ocr_chars);
                            }
                            info!(
                                "OCR extracted {} chars from image (Page {}, Position {:.1},{:.1})",
                                ocr_chars, page_num, position.0, position.1
                            );

                            // Split OCR text into smaller segments respecting token limits
                            // Using very conservative limit: max 200 tokens per segment
                            // OCR text with unusual spacing can have high token density
                            let max_ocr_tokens = 200;
                            let ocr_segments = split_ocr_text_to_segments(
                                &ocr_text,
                                max_ocr_tokens,
                                page_num,
                                position,
                                size,
                                *page_char_counter,
                                current_font_size,
                                current_transformed_font_size,
                            );

                            info!("OCR text split into {} segments", ocr_segments.len());

                            for segment in ocr_segments {
                                let seg_len = segment.content.chars().count();
                                text_segments.push(segment);
                                *page_char_counter += seg_len;
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(stats) = ocr_telemetry {
                            stats.image_skipped();
                        }
                        error!(
                            "OCR error at page {} position ({:.1},{:.1}): {}",
                            page_num, position.0, position.1, e
                        );
                    }
                }
            }
            Err(e) => {
                if let Some(stats) = ocr_telemetry {
                    stats.image_skipped();
                }
                // Check if it's a CCITT format issue
                if pdf_image
                    .filters
                    .as_ref()
                    .and_then(|f| f.first())
                    .map(|s| s == "CCITTFaxDecode")
                    .unwrap_or(false)
                {
                    debug!(
                        "Skipping CCITTFaxDecode image ({}x{}) - format not fully supported",
                        pdf_image.width, pdf_image.height
                    );
                } else {
                    // Image conversion failed - this only affects OCR, not text extraction
                    debug!(
                            "Image conversion skipped (OCR unavailable for this image): {}x{} filters={:?}, reason: {}",
                            pdf_image.width, pdf_image.height, pdf_image.filters, e
                        );
                }
            }
        }
    }

    Ok(())
}

#[derive(Clone)]
struct GraphicsState<'a> {
    ctm: Transform,
    ts: TextState<'a>,
    smask: Option<Dictionary>,
    fill_colorspace: ColorSpace,
    fill_color: Vec<f64>,
    stroke_colorspace: ColorSpace,
    stroke_color: Vec<f64>,
    line_width: f64,
}

fn show_text(
    gs: &mut GraphicsState,
    s: &[u8],
    _tlm: &Transform,
    _flip_ctm: &Transform,
    output: &mut dyn OutputDev,
) -> Result<(), OutputError> {
    let ts = &mut gs.ts;
    let font = ts.font.as_ref().unwrap();
    dlog!("{:?}", font.decode(s));
    dlog!("{:?}", font.decode(s).as_bytes());
    dlog!("{:?}", s);
    output.begin_word()?;

    for (c, length) in font.char_codes(s) {
        // 5.3.3 Text Space Details
        let tsm = Transform2D::row_major(ts.horizontal_scaling, 0., 0., 1.0, 0., ts.rise);
        // Trm = Tsm × Tm × CTM
        let trm = tsm.post_transform(&ts.tm.post_transform(&gs.ctm));
        //dlog!("ctm: {:?} tm {:?}", gs.ctm, tm);
        //dlog!("current pos: {:?}", position);
        // 5.9 Extraction of Text Content

        //dlog!("w: {}", font.widths[&(*c as i64)]);
        let w0 = font.get_width(c) / 1000.;

        let mut spacing = ts.character_spacing;
        // "Word spacing is applied to every occurrence of the single-byte character code 32 in a
        //  string when using a simple font or a composite font that defines code 32 as a
        //  single-byte code. It does not apply to occurrences of the byte value 32 in
        //  multiple-byte codes."
        let is_space = c == 32 && length == 1;
        if is_space {
            spacing += ts.word_spacing
        }

        output.output_character(&trm, w0, spacing, ts.font_size, &font.decode_char(c))?;
        let tj = 0.;
        let ty = 0.;
        let tx = ts.horizontal_scaling * ((w0 - tj / 1000.) * ts.font_size + spacing);
        dlog!(
            "horizontal {} adjust {} {} {} {}",
            ts.horizontal_scaling,
            tx,
            w0,
            ts.font_size,
            spacing
        );
        // dlog!("w0: {}, tx: {}", w0, tx);
        ts.tm = ts
            .tm
            .pre_transform(&Transform2D::create_translation(tx, ty));
        let _trm = ts.tm.pre_transform(&gs.ctm);
        //dlog!("post pos: {:?}", trm);
    }
    output.end_word()?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub struct MediaBox {
    pub llx: f64,
    pub lly: f64,
    pub urx: f64,
    pub ury: f64,
}

fn apply_state(doc: &Document, gs: &mut GraphicsState, state: &Dictionary) {
    for (k, v) in state.iter() {
        let k: &[u8] = k.as_ref();
        match k {
            b"SMask" => match maybe_deref(doc, v) {
                &Object::Name(ref name) => {
                    if name == b"None" {
                        gs.smask = None;
                    } else {
                        panic!("unexpected smask name")
                    }
                }
                &Object::Dictionary(ref dict) => {
                    gs.smask = Some(dict.clone());
                }
                _ => {
                    panic!("unexpected smask type {:?}", v)
                }
            },
            b"Type" => match v {
                &Object::Name(ref name) => {
                    assert_eq!(name, b"ExtGState")
                }
                _ => {
                    panic!("unexpected type")
                }
            },
            _ => {
                dlog!("unapplied state: {:?} {:?}", k, v);
            }
        }
    }
}

#[derive(Debug)]
pub enum PathOp {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    // XXX: is it worth distinguishing the different kinds of curve ops?
    CurveTo(f64, f64, f64, f64, f64, f64),
    Rect(f64, f64, f64, f64),
    Close,
}

#[derive(Debug)]
pub struct Path {
    pub ops: Vec<PathOp>,
}

impl Path {
    fn new() -> Path {
        Path { ops: Vec::new() }
    }
    fn current_point(&self) -> (f64, f64) {
        match self.ops.last().unwrap() {
            &PathOp::MoveTo(x, y) => (x, y),
            &PathOp::LineTo(x, y) => (x, y),
            &PathOp::CurveTo(_, _, _, _, x, y) => (x, y),
            _ => {
                panic!()
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct CalGray {
    white_point: [f64; 3],
    black_point: Option<[f64; 3]>,
    gamma: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct CalRGB {
    white_point: [f64; 3],
    black_point: Option<[f64; 3]>,
    gamma: Option<[f64; 3]>,
    matrix: Option<Vec<f64>>,
}

#[derive(Clone, Debug)]
pub struct Lab {
    white_point: [f64; 3],
    black_point: Option<[f64; 3]>,
    range: Option<[f64; 4]>,
}

#[derive(Clone, Debug)]
pub enum AlternateColorSpace {
    DeviceGray,
    DeviceRGB,
    DeviceCMYK,
    CalRGB(CalRGB),
    CalGray(CalGray),
    Lab(Lab),
    ICCBased(Vec<u8>),
}

#[derive(Clone)]
pub struct Separation {
    name: String,
    alternate_space: AlternateColorSpace,
    tint_transform: Box<Function>,
}

#[derive(Clone)]
pub enum ColorSpace {
    DeviceGray,
    DeviceRGB,
    DeviceCMYK,
    DeviceN,
    Pattern,
    CalRGB(CalRGB),
    CalGray(CalGray),
    Lab(Lab),
    Separation(Separation),
    ICCBased(Vec<u8>),
}

fn make_colorspace<'a>(doc: &'a Document, name: &[u8], resources: &'a Dictionary) -> ColorSpace {
    match name {
        b"DeviceGray" => ColorSpace::DeviceGray,
        b"DeviceRGB" => ColorSpace::DeviceRGB,
        b"DeviceCMYK" => ColorSpace::DeviceCMYK,
        b"Pattern" => ColorSpace::Pattern,
        _ => {
            let colorspaces: &Dictionary = get(&doc, resources, b"ColorSpace");
            let cs: &Object = maybe_get_obj(doc, colorspaces, &name[..])
                .unwrap_or_else(|| panic!("missing colorspace {:?}", &name[..]));
            if let Ok(cs) = cs.as_array() {
                let cs_name = pdf_to_utf8(cs[0].as_name().expect("first arg must be a name"));
                match cs_name.as_ref() {
                    "Separation" => {
                        let name = pdf_to_utf8(cs[1].as_name().expect("second arg must be a name"));
                        let alternate_space = match &maybe_deref(doc, &cs[2]) {
                            Object::Name(name) => match &name[..] {
                                b"DeviceGray" => AlternateColorSpace::DeviceGray,
                                b"DeviceRGB" => AlternateColorSpace::DeviceRGB,
                                b"DeviceCMYK" => AlternateColorSpace::DeviceCMYK,
                                _ => panic!("unexpected color space name"),
                            },
                            Object::Array(cs) => {
                                let cs_name =
                                    pdf_to_utf8(cs[0].as_name().expect("first arg must be a name"));
                                match cs_name.as_ref() {
                                    "ICCBased" => {
                                        let stream = maybe_deref(doc, &cs[1]).as_stream().unwrap();
                                        dlog!("ICCBased {:?}", stream);
                                        // XXX: we're going to be continually decompressing everytime this object is referenced
                                        AlternateColorSpace::ICCBased(get_contents(stream))
                                    }
                                    "CalGray" => {
                                        let dict =
                                            cs[1].as_dict().expect("second arg must be a dict");
                                        AlternateColorSpace::CalGray(CalGray {
                                            white_point: get(&doc, dict, b"WhitePoint"),
                                            black_point: get(&doc, dict, b"BackPoint"),
                                            gamma: get(&doc, dict, b"Gamma"),
                                        })
                                    }
                                    "CalRGB" => {
                                        let dict =
                                            cs[1].as_dict().expect("second arg must be a dict");
                                        AlternateColorSpace::CalRGB(CalRGB {
                                            white_point: get(&doc, dict, b"WhitePoint"),
                                            black_point: get(&doc, dict, b"BackPoint"),
                                            gamma: get(&doc, dict, b"Gamma"),
                                            matrix: get(&doc, dict, b"Matrix"),
                                        })
                                    }
                                    "Lab" => {
                                        let dict =
                                            cs[1].as_dict().expect("second arg must be a dict");
                                        AlternateColorSpace::Lab(Lab {
                                            white_point: get(&doc, dict, b"WhitePoint"),
                                            black_point: get(&doc, dict, b"BackPoint"),
                                            range: get(&doc, dict, b"Range"),
                                        })
                                    }
                                    _ => panic!("Unexpected color space name"),
                                }
                            }
                            _ => panic!("Alternate space should be name or array {:?}", cs[2]),
                        };
                        let tint_transform = Box::new(Function::new(doc, maybe_deref(doc, &cs[3])));

                        dlog!("{:?} {:?} {:?}", name, alternate_space, tint_transform);
                        ColorSpace::Separation(Separation {
                            name,
                            alternate_space,
                            tint_transform,
                        })
                    }
                    "ICCBased" => {
                        let stream = maybe_deref(doc, &cs[1]).as_stream().unwrap();
                        dlog!("ICCBased {:?}", stream);
                        // XXX: we're going to be continually decompressing everytime this object is referenced
                        ColorSpace::ICCBased(get_contents(stream))
                    }
                    "CalGray" => {
                        let dict = cs[1].as_dict().expect("second arg must be a dict");
                        ColorSpace::CalGray(CalGray {
                            white_point: get(&doc, dict, b"WhitePoint"),
                            black_point: get(&doc, dict, b"BackPoint"),
                            gamma: get(&doc, dict, b"Gamma"),
                        })
                    }
                    "CalRGB" => {
                        let dict = cs[1].as_dict().expect("second arg must be a dict");
                        ColorSpace::CalRGB(CalRGB {
                            white_point: get(&doc, dict, b"WhitePoint"),
                            black_point: get(&doc, dict, b"BackPoint"),
                            gamma: get(&doc, dict, b"Gamma"),
                            matrix: get(&doc, dict, b"Matrix"),
                        })
                    }
                    "Lab" => {
                        let dict = cs[1].as_dict().expect("second arg must be a dict");
                        ColorSpace::Lab(Lab {
                            white_point: get(&doc, dict, b"WhitePoint"),
                            black_point: get(&doc, dict, b"BackPoint"),
                            range: get(&doc, dict, b"Range"),
                        })
                    }
                    "Pattern" => ColorSpace::Pattern,
                    "DeviceGray" => ColorSpace::DeviceGray,
                    "DeviceRGB" => ColorSpace::DeviceRGB,
                    "DeviceCMYK" => ColorSpace::DeviceCMYK,
                    "DeviceN" => ColorSpace::DeviceN,
                    _ => {
                        panic!("color_space {:?} {:?} {:?}", name, cs_name, cs)
                    }
                }
            } else if let Ok(cs) = cs.as_name() {
                match pdf_to_utf8(cs).as_ref() {
                    "DeviceRGB" => ColorSpace::DeviceRGB,
                    "DeviceGray" => ColorSpace::DeviceGray,
                    "DeviceN" => ColorSpace::DeviceN,
                    _ => panic!(),
                }
            } else {
                panic!();
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TextSegment {
    pub content: String,
    pub font_size: f64,
    pub transformed_font_size: f64,
    pub x: f64,
    pub y: f64,
    pub is_bold: bool,
    pub font_name: String,
    pub font_weight: FontWeight,
    pub is_italic: bool,
    pub page_num: u32,
    pub cutat: String,
    pub fill_color: Option<(u8, u8, u8)>,
    pub stroke_color: Option<(u8, u8, u8)>,
    // Position tracking
    pub char_start: usize,
    pub char_end: usize,
    pub width: f64,
    pub height: f64,
    pub word_count: usize,
    pub located_text: Option<crate::document::LocatedText>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FontCacheKey {
    resource_name: Vec<u8>,
    object_id: Option<ObjectId>,
    inline_dict_addr: usize,
}

struct Processor<'a> {
    font_table: HashMap<FontCacheKey, Rc<dyn PdfFont + 'a>>,
    /// Optional sink for the low-latency text contract. When enabled, the
    /// shared interpreter keeps its decoding/visibility semantics but emits
    /// text directly instead of allocating discarded `TextSegment` metadata.
    fast_text_output: Option<String>,
    _none: PhantomData<&'a ()>,
}

#[derive(Debug, Clone)]
pub(crate) enum StreamSourceKind {
    Page,
    FormXObject,
}

fn add_visibility_count(stats: &mut PageTextLayerStats, rendering_mode: i32, bytes: &[u8]) {
    let useful_chars = bytes
        .iter()
        .filter(|b| !b.is_ascii_whitespace() && **b != 0)
        .count();
    if useful_chars == 0 {
        return;
    }

    match rendering_mode {
        3 => stats.invisible_useful_chars += useful_chars,
        0 | 1 | 2 | 4 | 5 | 6 => stats.visible_useful_chars += useful_chars,
        _ => {}
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StreamContext {
    depth: usize,
    max_depth: usize,
    source_kind: StreamSourceKind,
    xobject_name: Option<String>,
    xobject_stack: Vec<String>,
}

impl StreamContext {
    pub(crate) fn page() -> Self {
        Self::page_with_limit(8)
    }

    pub(crate) fn page_with_limit(max_depth: usize) -> Self {
        Self {
            depth: 0,
            max_depth,
            source_kind: StreamSourceKind::Page,
            xobject_name: None,
            xobject_stack: Vec::new(),
        }
    }

    fn form_child(&self, name: String) -> Self {
        let mut xobject_stack = self.xobject_stack.clone();
        xobject_stack.push(name.clone());
        Self {
            depth: self.depth + 1,
            max_depth: self.max_depth,
            source_kind: StreamSourceKind::FormXObject,
            xobject_name: Some(name),
            xobject_stack,
        }
    }

    fn has_xobject_in_stack(&self, name: &str) -> bool {
        self.xobject_stack.iter().any(|existing| existing == name)
    }
}

impl<'a> Processor<'a> {
    fn new() -> Processor<'a> {
        Processor {
            font_table: HashMap::new(),
            fast_text_output: None,
            _none: PhantomData,
        }
    }

    fn new_fast_text() -> Processor<'a> {
        Processor {
            font_table: HashMap::new(),
            fast_text_output: Some(String::new()),
            _none: PhantomData,
        }
    }

    fn resolve_font(
        &mut self,
        doc: &'a Document,
        fonts: &'a Dictionary,
        name: &[u8],
    ) -> Rc<dyn PdfFont + 'a> {
        let font_obj = fonts
            .get(name)
            .unwrap_or_else(|_| panic!("missing font resource {:?}", pdf_to_utf8(name)));
        let object_id = match font_obj {
            Object::Reference(id) => Some(*id),
            _ => None,
        };
        let font_dict = maybe_deref(doc, font_obj).as_dict().expect("font dict");
        let key = FontCacheKey {
            resource_name: name.to_vec(),
            object_id,
            inline_dict_addr: if object_id.is_some() {
                0
            } else {
                font_dict as *const Dictionary as usize
            },
        };
        self.font_table
            .entry(key)
            .or_insert_with(|| make_font(doc, font_dict))
            .clone()
    }

    // Helper: previously added a trailing space; now only trims to avoid double spaces
    fn preserve_sentence_boundaries(content: &str) -> String {
        content.trim().to_string()
    }

    fn emit_fast_text(&mut self, content: &str) {
        let content = strip_non_printing_text(content);
        if content.trim().is_empty() || looks_like_symbol_glyph_soup(&content) {
            return;
        }
        let Some(output) = self.fast_text_output.as_mut() else {
            return;
        };
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(content.trim());
    }

    fn take_fast_text_output(&mut self) -> Option<String> {
        self.fast_text_output.take()
    }

    /// Create text segments that respect token limits
    /// If the content is too large, it gets split into multiple segments
    fn create_text_segments_with_limit(
        content: String,
        font_size: f64,
        transformed_font_size: f64,
        x: f64,
        y: f64,
        is_bold: bool,
        font_name: String,
        font_weight: FontWeight,
        is_italic: bool,
        page_num: u32,
        cutat: String,
        fill_color: Option<(f64, f64, f64)>,
        stroke_color: Option<(f64, f64, f64)>,
        char_start: usize,
        char_end: usize,
        max_segment_tokens: usize,
    ) -> Vec<TextSegment> {
        // Convert f64 colors to u8
        let fill_color_u8 =
            fill_color.map(|(r, g, b)| ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8));
        let stroke_color_u8 =
            stroke_color.map(|(r, g, b)| ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8));

        // Quick estimation: if content is definitely small, don't bother splitting
        let word_count = count_words_simple(&content);
        let estimated_tokens = estimate_tokens_from_words(word_count, 1.3);

        if estimated_tokens <= max_segment_tokens {
            // Content is small enough, return single segment
            return Self::create_text_segment(
                content,
                font_size,
                transformed_font_size,
                x,
                y,
                is_bold,
                font_name,
                font_weight,
                is_italic,
                page_num,
                cutat,
                fill_color_u8,
                stroke_color_u8,
                char_start,
                char_end,
            )
            .into_iter()
            .collect();
        }

        // Content is too large, need to split it
        debug!(
            "Splitting large text segment with {} estimated tokens (max: {})",
            estimated_tokens, max_segment_tokens
        );

        // Use text-splitter for smart splitting
        let mut segments = Vec::new();
        match get_text_splitter_shared(max_segment_tokens) {
            Ok(splitter) => {
                let chunks = splitter.chunks(&content);
                let mut current_char_pos = char_start;

                for (idx, chunk) in chunks.enumerate() {
                    let chunk_str = chunk.to_string();
                    let chunk_len = chunk_str.chars().count();

                    if let Some(segment) = Self::create_text_segment(
                        chunk_str,
                        font_size,
                        transformed_font_size,
                        x,
                        y + (idx as f64 * font_size * 1.2), // Adjust Y for subsequent chunks
                        is_bold,
                        font_name.clone(),
                        font_weight.clone(),
                        is_italic,
                        page_num,
                        format!("{}[{}]", cutat, idx), // Indicate this was split
                        fill_color_u8,
                        stroke_color_u8,
                        current_char_pos,
                        current_char_pos + chunk_len,
                    ) {
                        segments.push(segment);
                    }
                    current_char_pos += chunk_len;
                }

                debug!("Split into {} segments", segments.len());
            }
            Err(e) => {
                // Fallback: if text-splitter fails, use simple splitting
                warn!("Text splitter failed: {}. Using fallback splitting.", e);

                // Simple word-based splitting
                let words: Vec<&str> = content.split_whitespace().collect();
                let mut current_chunk = String::new();
                let mut current_words = 0;
                let mut current_char_pos = char_start;

                for word in words {
                    let new_words = current_words + 1;
                    let new_estimated = estimate_tokens_from_words(new_words, 1.3);

                    if new_estimated > max_segment_tokens && !current_chunk.is_empty() {
                        // Push current chunk
                        let chunk_len = current_chunk.chars().count();
                        if let Some(segment) = Self::create_text_segment(
                            current_chunk.clone(),
                            font_size,
                            transformed_font_size,
                            x,
                            y + (segments.len() as f64 * font_size * 1.2),
                            is_bold,
                            font_name.clone(),
                            font_weight.clone(),
                            is_italic,
                            page_num,
                            format!("{}[fallback{}]", cutat, segments.len()),
                            fill_color_u8,
                            stroke_color_u8,
                            current_char_pos,
                            current_char_pos + chunk_len,
                        ) {
                            segments.push(segment);
                        }
                        current_char_pos += chunk_len;
                        current_chunk.clear();
                        current_words = 0;
                    }

                    if !current_chunk.is_empty() {
                        current_chunk.push(' ');
                        current_char_pos += 1;
                    }
                    current_chunk.push_str(word);
                    current_words += 1;
                }

                // Push remaining
                if !current_chunk.is_empty() {
                    let chunk_len = current_chunk.chars().count();
                    if let Some(segment) = Self::create_text_segment(
                        current_chunk,
                        font_size,
                        transformed_font_size,
                        x,
                        y + (segments.len() as f64 * font_size * 1.2),
                        is_bold,
                        font_name,
                        font_weight,
                        is_italic,
                        page_num,
                        format!("{}[fallback{}]", cutat, segments.len()),
                        fill_color_u8,
                        stroke_color_u8,
                        current_char_pos,
                        current_char_pos + chunk_len,
                    ) {
                        segments.push(segment);
                    }
                }
            }
        }

        segments
    }

    fn create_text_segment(
        content: String,
        font_size: f64,
        transformed_font_size: f64,
        x: f64,
        y: f64,
        is_bold: bool,
        font_name: String,
        font_weight: FontWeight,
        is_italic: bool,
        page_num: u32,
        cutat: String,
        fill_color: Option<(u8, u8, u8)>,
        stroke_color: Option<(u8, u8, u8)>,
        char_start: usize,
        char_end: usize,
    ) -> Option<TextSegment> {
        let content = strip_non_printing_text(&content);
        if content.trim().is_empty() || looks_like_symbol_glyph_soup(&content) {
            return None;
        }
        let char_count = content.chars().count();
        let word_count = unicode_count_words(&content);
        // Calculate approximate width based on average character width
        // For multi-line segments, we need a better approximation
        let avg_chars_per_line = 80.0; // Typical line length
        let lines = (char_count as f64 / avg_chars_per_line).max(1.0);
        let approx_width = if lines > 1.0 {
            // For multi-line text, use average line width
            avg_chars_per_line * font_size * 0.6
        } else {
            // For single line, use actual character count
            char_count as f64 * font_size * 0.6
        };
        Some(TextSegment {
            content,
            font_size,
            transformed_font_size,
            x,
            y,
            is_bold,
            font_name,
            font_weight,
            is_italic,
            page_num,
            cutat,
            fill_color,
            stroke_color,
            char_start,
            char_end,
            width: approx_width,
            height: font_size,
            word_count,
            located_text: None,
        })
    }

    fn is_visible_text(
        &self,
        transform: &Transform,
        font_size: f64,
        color: &(f64, f64, f64),
        media_box: &MediaBox,
        rendering_mode: i32,
        page_stats: Option<PageTextLayerStats>,
    ) -> bool {
        TextVisibilityPolicy::default()
            .decide(TextVisibilityInput {
                transform,
                font_size,
                color: *color,
                media_box,
                rendering_mode,
                generated: None,
                unicode: None,
                page_stats,
            })
            .keep()
    }

    fn content_text_layer_stats(content: &Content) -> PageTextLayerStats {
        let mut stats = PageTextLayerStats::default();
        let mut rendering_mode = 0i32;

        for operation in &content.operations {
            match operation.operator.as_ref() {
                "Tr" => {
                    if let Some(value) = operation.operands.get(0) {
                        rendering_mode = as_num(value) as i32;
                    }
                }
                "Tj" | "'" => {
                    if let Some(Object::String(bytes, _)) = operation.operands.get(0) {
                        add_visibility_count(&mut stats, rendering_mode, bytes);
                    }
                }
                "\"" => {
                    if let Some(Object::String(bytes, _)) = operation.operands.get(2) {
                        add_visibility_count(&mut stats, rendering_mode, bytes);
                    }
                }
                "TJ" => {
                    if let Some(Object::Array(items)) = operation.operands.get(0) {
                        for item in items {
                            if let Object::String(bytes, _) = item {
                                add_visibility_count(&mut stats, rendering_mode, bytes);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        stats
    }

    fn visible_for_layout(
        &self,
        transform: &Transform,
        font_size: f64,
        color: &(f64, f64, f64),
        media_box: &MediaBox,
        rendering_mode: i32,
        page_stats: Option<PageTextLayerStats>,
    ) -> bool {
        if !self.is_visible_text(
            transform,
            font_size,
            color,
            media_box,
            rendering_mode,
            page_stats,
        ) {
            return false;
        }
        let page_h = media_box.ury - media_box.lly;
        let max_frac: f64 = std::env::var("PDF_EXTRACT_MAX_FONT_FRAC")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.35);
        font_size <= page_h * max_frac
    }

    fn process_stream(
        &mut self,
        doc: &'a Document,
        ocr_handler: Option<&OcrHandler>,
        ocr_telemetry: Option<&OcrImageTelemetry>,
        ocr_route: bool,
        content: Vec<u8>,
        resources: &'a Dictionary,
        media_box: &MediaBox,
        page_num: u32,
        text_segments: &mut Vec<TextSegment>,
        page_rotate: i32,
        laparams: Option<&crate::LAParams>,
        initial_ctm_override: Option<Transform>,
        stream_context: StreamContext,
    ) -> Result<(), OutputError> {
        if stream_context.depth > stream_context.max_depth {
            warn!(
                "Skipping nested PDF stream on page {} at depth {} ({:?})",
                page_num, stream_context.depth, stream_context.xobject_name
            );
            return Ok(());
        }
        let use_layout = laparams.is_some();
        let params = laparams.cloned();
        #[derive(Clone, Debug)]
        struct Glyph {
            ch: String,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
            page_char_start: usize,
            page_char_end: usize,
            source_text_object_id: u32,
            font_name: String,
            font_size: f64,
            transformed_font_size: f64,
            font_weight: FontWeight,
            is_italic: bool,
            fill_color: Option<(u8, u8, u8)>,
            render_mode: i32,
            source_kind: StreamSourceKind,
            xobject_name: Option<String>,
        }
        let mut glyphs: Vec<Glyph> = Vec::new();
        let mut current_text_obj_id: u32 = 0;
        let mut text = String::new();
        let mut current_line = String::new();
        let current_word = String::new();
        let mut last_end = 100000.0;
        let mut last_y = 0.0;
        let mut current_is_bold = false;
        let mut current_font_size = 0.0;
        let mut current_transformed_font_size = 0.0;
        let mut current_x = 0.0;
        let mut current_y = 0.0;
        let mut first_char = false;
        let mut current_font = String::new();
        let mut current_font_weight = FontWeight::Regular;
        let mut current_is_italic = false;
        let mut current_color = (0.0, 0.0, 0.0); // Default to black
        let is_clipping = false;
        let current_transform = Transform::default();
        let current_font_color = (0.0, 0.0, 0.0); // Default to black
        let mut page_char_counter = 0usize; // Track character position in page
        let mut current_segment_start = 0usize; // Start position of current segment

        let content = match Content::decode(&content) {
            Ok(content) => content,
            Err(e) => {
                warn!(
                    "Skipping page {} due to invalid content stream: {:?}",
                    page_num, e
                );
                // Return Ok(()) - empty result, segments list stays empty for this page
                return Ok(());
            }
        };
        let page_text_stats = Self::content_text_layer_stats(&content);

        // Build initial CTM with page rotation and viewer Y-flip
        let media_box_array = [media_box.llx, media_box.lly, media_box.urx, media_box.ury];
        let initial_ctm = initial_ctm_override
            .unwrap_or_else(|| build_initial_ctm(&media_box_array, page_rotate));

        // Store page rotation for image processing (images don't need Y-flip)
        let page_rotation_only = build_image_ctm(page_rotate);

        // Debug: Log both CTMs to understand the issue
        let det_initial = initial_ctm.m11 * initial_ctm.m22 - initial_ctm.m12 * initial_ctm.m21;
        let det_image = page_rotation_only.m11 * page_rotation_only.m22
            - page_rotation_only.m12 * page_rotation_only.m21;
        debug!(
            "Initial CTM (with Y-flip): det={:.3}, CTM=[{:.3} {:.3} {:.3} {:.3} {:.1} {:.1}]",
            det_initial,
            initial_ctm.m11,
            initial_ctm.m12,
            initial_ctm.m21,
            initial_ctm.m22,
            initial_ctm.m31,
            initial_ctm.m32
        );
        debug!(
            "Image CTM (no Y-flip): det={:.3}, CTM=[{:.3} {:.3} {:.3} {:.3} {:.1} {:.1}]",
            det_image,
            page_rotation_only.m11,
            page_rotation_only.m12,
            page_rotation_only.m21,
            page_rotation_only.m22,
            page_rotation_only.m31,
            page_rotation_only.m32
        );

        let mut gs: GraphicsState = GraphicsState {
            ts: TextState {
                font: None,
                font_size: std::f64::NAN,
                character_spacing: 0.,
                word_spacing: 0.,
                horizontal_scaling: 100. / 100.,
                leading: 0.,
                rise: 0.,
                tm: Transform2D::identity(),
                rendering_mode: 0,
            },
            fill_color: Vec::new(),
            fill_colorspace: ColorSpace::DeviceGray,
            stroke_color: Vec::new(),
            stroke_colorspace: ColorSpace::DeviceGray,
            line_width: 1.,
            ctm: initial_ctm, // Use initial CTM instead of identity
            smask: None,
        };

        let mut gs_stack = Vec::new();
        let mut mc_stack = Vec::new();
        let mut tlm = Transform2D::<f64, Space, Space>::identity();
        let mut path = Path::new();
        // Emit page-relative viewer coordinates. PDF user space may place the
        // MediaBox away from (0, 0); translating by its lower-left corner
        // preserves geometry while satisfying consumers' nonnegative page
        // coordinate contract.
        let flip_ctm = Transform2D::<f64, Space, Space>::row_major(
            1.,
            0.,
            0.,
            -1.,
            -media_box.llx,
            media_box.ury,
        );
        dlog!("MediaBox {:?}", media_box);
        let layout_analyzer = LayoutAnalyzer::new(media_box.ury - media_box.lly);

        // Add to Processor struct
        let last_vertical_gap: Option<f64> = None;

        for operation in &content.operations {
            match operation.operator.as_ref() {
                "BT" => {
                    tlm = Transform2D::identity();
                    gs.ts.tm = tlm;
                    // New text object - increment ID for tracking text block boundaries
                    current_text_obj_id += 1;
                }
                "ET" => {
                    tlm = Transform2D::identity();
                    gs.ts.tm = tlm;
                }
                "cm" => {
                    if operation.operands.len() != 6 {
                        warn!(
                            "Skipping malformed cm operator on page {}: expected 6 operands, got {}",
                            page_num,
                            operation.operands.len()
                        );
                        continue;
                    }
                    let m = Transform2D::row_major(
                        as_num(&operation.operands[0]),
                        as_num(&operation.operands[1]),
                        as_num(&operation.operands[2]),
                        as_num(&operation.operands[3]),
                        as_num(&operation.operands[4]),
                        as_num(&operation.operands[5]),
                    );
                    gs.ctm = gs.ctm.pre_transform(&m);
                    dlog!("matrix {:?}", gs.ctm);
                }
                "CS" => {
                    if let Some(name) = operation
                        .operands
                        .first()
                        .and_then(|obj| obj.as_name().ok())
                    {
                        gs.stroke_colorspace = make_colorspace(doc, name, resources);
                    } else {
                        warn!("Skipping malformed CS operator on page {}", page_num);
                    }
                }
                "cs" => {
                    if let Some(name) = operation
                        .operands
                        .first()
                        .and_then(|obj| obj.as_name().ok())
                    {
                        gs.fill_colorspace = make_colorspace(doc, name, resources);
                    } else {
                        warn!("Skipping malformed cs operator on page {}", page_num);
                    }
                }
                "SC" | "SCN" => {
                    gs.stroke_color = match gs.stroke_colorspace {
                        ColorSpace::Pattern => {
                            dlog!("unhandled pattern color");
                            Vec::new()
                        }
                        _ => operation.operands.iter().map(|x| as_num(x)).collect(),
                    };
                }
                "sc" | "scn" => {
                    gs.fill_color = match gs.fill_colorspace {
                        ColorSpace::Pattern => {
                            dlog!("unhandled pattern color");
                            Vec::new()
                        }
                        _ => operation.operands.iter().map(|x| as_num(x)).collect(),
                    };
                }
                "rg" | "g" => {
                    current_color = if operation.operator == "rg" {
                        (
                            as_num(&operation.operands[0]),
                            as_num(&operation.operands[1]),
                            as_num(&operation.operands[2]),
                        )
                    } else {
                        let gray = as_num(&operation.operands[0]);
                        (gray, gray, gray)
                    };
                }
                "G" | "RG" | "K" | "k" => {
                    dlog!("unhandled color operation {:?}", operation);
                }
                "Tf" => {
                    if operation.operands.len() < 2 {
                        warn!(
                            "Skipping malformed Tf operator on page {}: expected font and size",
                            page_num
                        );
                        continue;
                    }
                    let fonts: &Dictionary = get(&doc, resources, b"Font");
                    let Some(name) = operation.operands[0].as_name().ok() else {
                        warn!(
                            "Skipping Tf operator with non-name font on page {}",
                            page_num
                        );
                        continue;
                    };
                    let font = self.resolve_font(doc, fonts, name);

                    let new_font_size =
                        (as_num(&operation.operands[1]) * 100.0_f64).round() / 100.0;
                    let new_font_name = String::from_utf8_lossy(name).into_owned();
                    let font_info = FontInfo::from_font_name(&new_font_name);

                    // Enhanced bold detection: first try font name analysis, then fallback to font object debug
                    let mut new_is_bold = font_info.weight.is_bold();
                    if !new_is_bold {
                        // Fallback to checking the font object's debug representation
                        // This catches cases where bold info is in font descriptors but not in the name
                        let font_debug = format!("{:?}", font).to_lowercase();
                        new_is_bold = font_debug.contains("bold");

                        // Debug output to help diagnose bold detection issues
                        // if new_is_bold {
                        //     eprintln!("Bold detected via font debug for font: {} (weight: {:?})", new_font_name, font_info.weight);
                        // }
                    }

                    let new_font_weight = font_info.weight.clone();
                    let new_is_italic = font_info.is_italic;

                    //     self.is_visible_text(&tlm, current_font_size, &current_color, media_box);

                    if (new_is_bold != current_is_bold
                        || new_font_size != current_font_size
                        || new_font_name != current_font
                        || new_font_weight != current_font_weight
                        || new_is_italic != current_is_italic)
                        && !current_line.trim().is_empty()
                    {
                        // Only cut a new text segment if we are actually on a new line.
                        // Here we check if the vertical difference is significant compared to the font size.

                        if current_x > 0.0 && current_y > 0.0 {
                            // Process the fill color as before.

                            // Push the current line as a new TextSegment.
                            let content_str = Self::preserve_sentence_boundaries(&current_line);
                            if self.fast_text_output.is_some() {
                                self.emit_fast_text(&content_str);
                            } else {
                                let segments = Self::create_text_segments_with_limit(
                                    content_str,
                                    current_font_size,
                                    current_transformed_font_size,
                                    current_x,
                                    current_y,
                                    current_is_bold,
                                    current_font.clone(),
                                    current_font_weight.clone(),
                                    current_is_italic,
                                    page_num,
                                    "Tj".to_string(),
                                    None,
                                    None,
                                    current_segment_start,
                                    page_char_counter,
                                    5000, // Avoid pre-splitting here; let the chunker split
                                );
                                text_segments.extend(segments);
                            }
                            current_segment_start = page_char_counter;
                            current_line.clear();
                        }

                        // Whether or not we pushed a new segment, update our current font properties
                        // so that subsequent text uses the new style.
                        current_is_bold = new_is_bold;
                        current_font = new_font_name;
                        current_font_size = new_font_size;
                        current_font_weight = new_font_weight;
                        current_is_italic = new_is_italic;
                    }

                    gs.ts.font = Some(font.clone());

                    gs.ts.font_size = as_num(&operation.operands[1]);
                    // println!("font size: {}", gs.ts.font_size);

                    dlog!(
                        "font {} size: {} {:?}",
                        pdf_to_utf8(name),
                        current_font_size,
                        operation
                    );
                }
                "TJ" => match operation.operands.get(0) {
                    Some(Object::Array(ref array)) => {
                        let Some(font) = gs.ts.font.clone() else {
                            warn!("Skipping TJ text on page {} before any Tf font", page_num);
                            continue;
                        };
                        // current_line.push_str("*");
                        for e in array {
                            match e {
                                &Object::String(ref s, _) => {
                                    first_char = true;

                                    for (c, length) in font.char_codes(s) {
                                        let w0 = font.get_width(c) / 1000.;
                                        let mut spacing = gs.ts.character_spacing;
                                        let is_space = c == 32 && length == 1;
                                        if is_space {
                                            spacing += gs.ts.word_spacing;
                                        }

                                        let char = font.decode_char(c);

                                        let text_trm = gs.ctm.post_transform(&gs.ts.tm);
                                        let device_trm = text_trm.post_transform(&flip_ctm);
                                        let (x, y) = (device_trm.m31, device_trm.m32);
                                        let transformed_font_size_vec = text_trm.transform_vector(
                                            vec2(gs.ts.font_size, gs.ts.font_size),
                                        );
                                        let transformed_font_size =
                                            ((transformed_font_size_vec.x.abs()
                                                * transformed_font_size_vec.y.abs())
                                            .sqrt()
                                                * 100.0)
                                                .round()
                                                / 100.0;
                                        // Use full device-space transform: CTM x Tm x viewer Y-flip
                                        let trm_vis = device_trm;
                                        let is_add = if use_layout {
                                            self.visible_for_layout(
                                                &trm_vis,
                                                transformed_font_size,
                                                &current_color,
                                                media_box,
                                                gs.ts.rendering_mode,
                                                Some(page_text_stats),
                                            )
                                        } else {
                                            self.is_visible_text(
                                                &trm_vis,
                                                transformed_font_size,
                                                &current_color,
                                                media_box,
                                                gs.ts.rendering_mode,
                                                Some(page_text_stats),
                                            )
                                        };

                                        if transformed_font_size != current_transformed_font_size
                                            && !transformed_font_size.is_nan()
                                            && !current_transformed_font_size.is_nan()
                                        {
                                            if !current_line.trim().is_empty() {
                                                let content_str =
                                                    Self::preserve_sentence_boundaries(
                                                        &current_line,
                                                    );
                                                if self.fast_text_output.is_some() {
                                                    self.emit_fast_text(&content_str);
                                                } else {
                                                    let segments =
                                                        Self::create_text_segments_with_limit(
                                                            content_str,
                                                            current_font_size,
                                                            current_transformed_font_size,
                                                            current_x,
                                                            current_y,
                                                            current_is_bold,
                                                            current_font.clone(),
                                                            current_font_weight.clone(),
                                                            current_is_italic,
                                                            page_num,
                                                            "Tj".to_string(),
                                                            Some(current_font_color),
                                                            None,
                                                            current_segment_start,
                                                            page_char_counter,
                                                            5000, // Avoid pre-splitting here; let the chunker split
                                                        );
                                                    text_segments.extend(segments);
                                                }
                                                current_segment_start = page_char_counter;
                                                current_line.clear();
                                            }

                                            current_transformed_font_size = transformed_font_size;
                                        }

                                        if first_char {
                                            // Check for paragraph break (large vertical gap)
                                            if is_paragraph_break(y, last_y, transformed_font_size)
                                            {
                                                current_line.push('\n');
                                                page_char_counter += 1;
                                            }
                                            // Check for new line (moved down and back to left)
                                            else if is_new_line(
                                                x,
                                                last_end,
                                                y,
                                                last_y,
                                                transformed_font_size,
                                            ) {
                                                current_line.push('\n');
                                                page_char_counter += 1;
                                            }
                                            // Check for large horizontal gap (column break)
                                            // This handles two-column layouts where elements are at same Y
                                            else if is_column_break(
                                                x,
                                                last_end,
                                                transformed_font_size,
                                            ) {
                                                current_line.push('\n');
                                                page_char_counter += 1;
                                            }
                                            // Check for space between words on same line
                                            else if should_insert_space(
                                                x,
                                                last_end,
                                                transformed_font_size,
                                            ) {
                                                current_line.push(' ');
                                                page_char_counter += 1;
                                            }

                                            current_x = (x * 100.00).round() / 100.0;
                                            current_y = (y * 100.00).round() / 100.0;
                                        }

                                        let glyph_char_start = page_char_counter;
                                        let glyph_char_len = char.chars().count();
                                        if is_add {
                                            if use_layout {
                                                // Collect glyphs for layout grouping
                                                let g_height = transformed_font_size.max(0.0);
                                                let g_width = (w0 * transformed_font_size).max(0.0);
                                                glyphs.push(Glyph {
                                                    ch: char.clone(),
                                                    x,
                                                    y,
                                                    width: g_width,
                                                    height: g_height,
                                                    page_char_start: glyph_char_start,
                                                    page_char_end: glyph_char_start
                                                        + glyph_char_len,
                                                    source_text_object_id: current_text_obj_id,
                                                    font_name: current_font.clone(),
                                                    font_size: current_font_size,
                                                    transformed_font_size,
                                                    font_weight: current_font_weight.clone(),
                                                    is_italic: current_is_italic,
                                                    fill_color: Some((
                                                        (current_color.0 * 255.0).clamp(0.0, 255.0)
                                                            as u8,
                                                        (current_color.1 * 255.0).clamp(0.0, 255.0)
                                                            as u8,
                                                        (current_color.2 * 255.0).clamp(0.0, 255.0)
                                                            as u8,
                                                    )),
                                                    render_mode: gs.ts.rendering_mode,
                                                    source_kind: stream_context.source_kind.clone(),
                                                    xobject_name: stream_context
                                                        .xobject_name
                                                        .clone(),
                                                });
                                                page_char_counter += glyph_char_len;
                                                record_font_append(&current_font);
                                            } else {
                                                current_line.push_str(&char);
                                                page_char_counter += char.chars().count();
                                                record_font_append(&current_font);
                                            }
                                        }
                                        first_char = false;

                                        last_end = x + w0 * transformed_font_size;
                                        last_y = y;

                                        let tx = gs.ts.horizontal_scaling
                                            * ((w0 - 0. / 1000.) * gs.ts.font_size + spacing);
                                        gs.ts.tm = gs.ts.tm.pre_transform(
                                            &Transform2D::create_translation(tx, 0.),
                                        );
                                    }
                                }
                                &Object::Integer(i) => {
                                    let ts = &mut gs.ts;
                                    let w0 = 0.;
                                    let tj = i as f64;
                                    let ty = 0.;
                                    let tx =
                                        ts.horizontal_scaling * ((w0 - tj / 1000.) * ts.font_size);
                                    ts.tm = ts
                                        .tm
                                        .pre_transform(&Transform2D::create_translation(tx, ty));
                                    dlog!("adjust text by: {} {:?}", i, ts.tm);
                                }
                                &Object::Real(i) => {
                                    let ts = &mut gs.ts;
                                    let w0 = 0.;
                                    let tj = i as f64;
                                    let ty = 0.;
                                    let tx =
                                        ts.horizontal_scaling * ((w0 - tj / 1000.) * ts.font_size);
                                    ts.tm = ts
                                        .tm
                                        .pre_transform(&Transform2D::create_translation(tx, ty));
                                    dlog!("adjust text by: {} {:?}", i, ts.tm);
                                }
                                _ => {
                                    dlog!("kind of {:?}", e);
                                }
                            }
                        }
                    }
                    _ => {}
                },
                "Tj" => match operation.operands.get(0) {
                    Some(Object::String(ref s, _)) => {
                        let Some(font) = gs.ts.font.clone() else {
                            warn!("Skipping Tj text on page {} before any Tf font", page_num);
                            continue;
                        };
                        let ts = &mut gs.ts;
                        first_char = true;

                        for (c, length) in font.char_codes(s) {
                            let w0 = font.get_width(c) / 1000.;
                            let mut spacing = gs.ts.character_spacing;
                            let is_space = c == 32 && length == 1;
                            if is_space {
                                spacing += gs.ts.word_spacing;
                            }

                            let char = font.decode_char(c);

                            let text_trm = gs.ctm.post_transform(&gs.ts.tm);
                            let device_trm = text_trm.post_transform(&flip_ctm);
                            let (x, y) = (device_trm.m31, device_trm.m32);
                            let transformed_font_size_vec =
                                text_trm.transform_vector(vec2(gs.ts.font_size, gs.ts.font_size));
                            let transformed_font_size = ((transformed_font_size_vec.x.abs()
                                * transformed_font_size_vec.y.abs())
                            .sqrt()
                                * 100.0)
                                .round()
                                / 100.0;

                            // Use full device-space transform: CTM x Tm x viewer Y-flip
                            let trm_vis = device_trm;
                            let is_add = if use_layout {
                                self.visible_for_layout(
                                    &trm_vis,
                                    transformed_font_size,
                                    &current_color,
                                    media_box,
                                    gs.ts.rendering_mode,
                                    Some(page_text_stats),
                                )
                            } else {
                                self.is_visible_text(
                                    &trm_vis,
                                    transformed_font_size,
                                    &current_color,
                                    media_box,
                                    gs.ts.rendering_mode,
                                    Some(page_text_stats),
                                )
                            };
                            if transformed_font_size != current_transformed_font_size
                                && !transformed_font_size.is_nan()
                                && !current_transformed_font_size.is_nan()
                            {
                                if !current_line.trim().is_empty() {
                                    let content_str =
                                        Self::preserve_sentence_boundaries(&current_line);
                                    if self.fast_text_output.is_some() {
                                        self.emit_fast_text(&content_str);
                                    } else {
                                        let segments = Self::create_text_segments_with_limit(
                                            content_str,
                                            current_font_size,
                                            current_transformed_font_size,
                                            current_x,
                                            current_y,
                                            current_is_bold,
                                            current_font.clone(),
                                            current_font_weight.clone(),
                                            current_is_italic,
                                            page_num,
                                            "Tj".to_string(),
                                            Some(current_font_color),
                                            None,
                                            current_segment_start,
                                            page_char_counter,
                                            300, // Conservative limit
                                        );
                                        text_segments.extend(segments);
                                    }
                                    current_segment_start = page_char_counter;
                                    current_line.clear();
                                }

                                current_transformed_font_size = transformed_font_size;
                            }

                            if first_char {
                                // Check for paragraph break (large vertical gap)
                                if is_paragraph_break(y, last_y, transformed_font_size) {
                                    current_line.push('\n');
                                    page_char_counter += 1;
                                }
                                // Check for new line (moved down and back to left)
                                else if is_new_line(x, last_end, y, last_y, transformed_font_size)
                                {
                                    current_line.push('\n');
                                    page_char_counter += 1;
                                }
                                // Check for large horizontal gap (column break)
                                else if is_column_break(x, last_end, transformed_font_size) {
                                    current_line.push('\n');
                                    page_char_counter += 1;
                                }
                                // Check for space between words on same line
                                else if should_insert_space(x, last_end, transformed_font_size) {
                                    current_line.push(' ');
                                    page_char_counter += 1;
                                }

                                current_x = x;
                                current_y = y;
                            }
                            let glyph_char_start = page_char_counter;
                            let glyph_char_len = char.chars().count();
                            if is_add {
                                if use_layout {
                                    let g_height = transformed_font_size.max(0.0);
                                    let g_width = (w0 * transformed_font_size).max(0.0);
                                    glyphs.push(Glyph {
                                        ch: char.clone(),
                                        x,
                                        y,
                                        width: g_width,
                                        height: g_height,
                                        page_char_start: glyph_char_start,
                                        page_char_end: glyph_char_start + glyph_char_len,
                                        source_text_object_id: current_text_obj_id,
                                        font_name: current_font.clone(),
                                        font_size: current_font_size,
                                        transformed_font_size,
                                        font_weight: current_font_weight.clone(),
                                        is_italic: current_is_italic,
                                        fill_color: Some((
                                            (current_color.0 * 255.0).clamp(0.0, 255.0) as u8,
                                            (current_color.1 * 255.0).clamp(0.0, 255.0) as u8,
                                            (current_color.2 * 255.0).clamp(0.0, 255.0) as u8,
                                        )),
                                        render_mode: gs.ts.rendering_mode,
                                        source_kind: stream_context.source_kind.clone(),
                                        xobject_name: stream_context.xobject_name.clone(),
                                    });
                                    page_char_counter += glyph_char_len;
                                    record_font_append(&current_font);
                                } else {
                                    current_line.push_str(&char);
                                    page_char_counter += char.chars().count();
                                    record_font_append(&current_font);
                                }
                            }
                            first_char = false;

                            last_end = x + w0 * transformed_font_size;
                            last_y = y;

                            let tx = gs.ts.horizontal_scaling
                                * ((w0 - 0. / 1000.) * gs.ts.font_size + spacing);
                            gs.ts.tm = gs
                                .ts
                                .tm
                                .pre_transform(&Transform2D::create_translation(tx, 0.));
                        }
                    }
                    _ => {
                        warn!(
                            "Skipping malformed Tj operator on page {}: {:?}",
                            page_num, operation
                        );
                    }
                },
                "Tc" => {
                    gs.ts.character_spacing = as_num(&operation.operands[0]);
                }
                "Tw" => {
                    gs.ts.word_spacing = as_num(&operation.operands[0]);
                }
                "Tz" => {
                    gs.ts.horizontal_scaling = as_num(&operation.operands[0]) / 100.;
                }
                "TL" => {
                    gs.ts.leading = as_num(&operation.operands[0]);
                }
                "Ts" => {
                    gs.ts.rise = as_num(&operation.operands[0]);
                    text.push(' ');
                    current_line.push(' ');
                }
                "Tr" => {
                    // Text rendering mode: 0=fill,1=stroke,2=fill+stroke,3=invisible, etc.
                    if !operation.operands.is_empty() {
                        gs.ts.rendering_mode = as_num(&operation.operands[0]) as i32;
                    }
                }
                "Tm" => {
                    if operation.operands.len() != 6 {
                        warn!(
                            "Skipping malformed Tm operator on page {}: expected 6 operands, got {}",
                            page_num,
                            operation.operands.len()
                        );
                        continue;
                    }
                    tlm = Transform2D::row_major(
                        as_num(&operation.operands[0]),
                        as_num(&operation.operands[1]),
                        as_num(&operation.operands[2]),
                        as_num(&operation.operands[3]),
                        as_num(&operation.operands[4]),
                        as_num(&operation.operands[5]),
                    );
                    gs.ts.tm = tlm;
                    dlog!("Tm: matrix {:?}", gs.ts.tm);
                }
                "Td" => {
                    if operation.operands.len() != 2 {
                        warn!(
                            "Skipping malformed Td operator on page {}: expected 2 operands, got {}",
                            page_num,
                            operation.operands.len()
                        );
                        continue;
                    }
                    let tx = as_num(&operation.operands[0]);
                    let ty = as_num(&operation.operands[1]);
                    dlog!("translation: {} {}", tx, ty);

                    tlm = tlm.pre_transform(&Transform2D::create_translation(tx, ty));
                    gs.ts.tm = tlm;
                    dlog!("Td matrix {:?}", gs.ts.tm);
                }
                "TD" => {
                    if operation.operands.len() != 2 {
                        warn!(
                            "Skipping malformed TD operator on page {}: expected 2 operands, got {}",
                            page_num,
                            operation.operands.len()
                        );
                        continue;
                    }
                    let tx = as_num(&operation.operands[0]);
                    let ty = as_num(&operation.operands[1]);
                    dlog!("translation: {} {}", tx, ty);
                    gs.ts.leading = -ty;

                    tlm = tlm.pre_transform(&Transform2D::create_translation(tx, ty));
                    gs.ts.tm = tlm;
                    dlog!("TD matrix {:?}", gs.ts.tm);
                }
                "T*" => {
                    let tx = 0.0;
                    let ty = -gs.ts.leading;

                    tlm = tlm.pre_transform(&Transform2D::create_translation(tx, ty));
                    gs.ts.tm = tlm;
                    dlog!("T* matrix {:?}", gs.ts.tm);
                }
                "'" => {
                    // Equivalent to T* followed by Tj with the string operand
                    let tx = 0.0;
                    let ty = -gs.ts.leading;
                    tlm = tlm.pre_transform(&Transform2D::create_translation(tx, ty));
                    gs.ts.tm = tlm;
                    dlog!("' => T* then Tj; matrix {:?}", gs.ts.tm);

                    if let Some(Object::String(ref s, _)) = operation.operands.get(0) {
                        let Some(font) = gs.ts.font.clone() else {
                            warn!("Skipping ' text on page {} before any Tf font", page_num);
                            continue;
                        };
                        let ts = &mut gs.ts;
                        first_char = true;

                        for (c, length) in font.char_codes(s) {
                            let w0 = font.get_width(c) / 1000.;
                            let mut spacing = ts.character_spacing;
                            let is_space = c == 32 && length == 1;
                            if is_space {
                                spacing += ts.word_spacing;
                            }

                            let ch = font.decode_char(c);

                            let text_trm = gs.ctm.post_transform(&ts.tm);
                            let device_trm = text_trm.post_transform(&flip_ctm);
                            let (x, y) = (device_trm.m31, device_trm.m32);
                            let transformed_font_size_vec =
                                text_trm.transform_vector(vec2(ts.font_size, ts.font_size));
                            let transformed_font_size = ((transformed_font_size_vec.x.abs()
                                * transformed_font_size_vec.y.abs())
                            .sqrt()
                                * 100.0)
                                .round()
                                / 100.0;

                            let trm_vis = device_trm;
                            let is_add = if use_layout {
                                self.visible_for_layout(
                                    &trm_vis,
                                    transformed_font_size,
                                    &current_color,
                                    media_box,
                                    ts.rendering_mode,
                                    Some(page_text_stats),
                                )
                            } else {
                                self.is_visible_text(
                                    &trm_vis,
                                    transformed_font_size,
                                    &current_color,
                                    media_box,
                                    ts.rendering_mode,
                                    Some(page_text_stats),
                                )
                            };
                            let glyph_char_start = page_char_counter;
                            let glyph_char_len = ch.chars().count();
                            if is_add {
                                if use_layout {
                                    let g_height = transformed_font_size.max(0.0);
                                    let g_width = (w0 * transformed_font_size).max(0.0);
                                    glyphs.push(Glyph {
                                        ch: ch.clone(),
                                        x,
                                        y,
                                        width: g_width,
                                        height: g_height,
                                        page_char_start: glyph_char_start,
                                        page_char_end: glyph_char_start + glyph_char_len,
                                        source_text_object_id: current_text_obj_id,
                                        font_name: current_font.clone(),
                                        font_size: current_font_size,
                                        transformed_font_size,
                                        font_weight: current_font_weight.clone(),
                                        is_italic: current_is_italic,
                                        fill_color: Some((
                                            (current_color.0 * 255.0).clamp(0.0, 255.0) as u8,
                                            (current_color.1 * 255.0).clamp(0.0, 255.0) as u8,
                                            (current_color.2 * 255.0).clamp(0.0, 255.0) as u8,
                                        )),
                                        render_mode: ts.rendering_mode,
                                        source_kind: stream_context.source_kind.clone(),
                                        xobject_name: stream_context.xobject_name.clone(),
                                    });
                                    page_char_counter += glyph_char_len;
                                    record_font_append(&current_font);
                                } else {
                                    current_line.push_str(&ch);
                                    page_char_counter += ch.chars().count();
                                    record_font_append(&current_font);
                                }
                            }
                            first_char = false;
                            last_end = x + w0 * transformed_font_size;
                            last_y = y;
                            let tx = ts.horizontal_scaling
                                * ((w0 - 0. / 1000.) * ts.font_size + spacing);
                            ts.tm = ts
                                .tm
                                .pre_transform(&Transform2D::create_translation(tx, 0.));
                        }
                    }
                }
                "\"" => {
                    // Equivalent to setting Tw and Tc, then T*, then Tj
                    if operation.operands.len() >= 3 {
                        // Set word spacing and character spacing
                        gs.ts.word_spacing = as_num(&operation.operands[0]);
                        gs.ts.character_spacing = as_num(&operation.operands[1]);
                        // Move to next line (T*)
                        let tx = 0.0;
                        let ty = -gs.ts.leading;
                        tlm = tlm.pre_transform(&Transform2D::create_translation(tx, ty));
                        gs.ts.tm = tlm;
                        dlog!("\" => set Tw/Tc then T*; matrix {:?}", gs.ts.tm);

                        if let Object::String(ref s, _) = operation.operands[2] {
                            let Some(font) = gs.ts.font.clone() else {
                                warn!("Skipping \" text on page {} before any Tf font", page_num);
                                continue;
                            };
                            let ts = &mut gs.ts;
                            first_char = true;

                            for (c, length) in font.char_codes(s) {
                                let w0 = font.get_width(c) / 1000.;
                                let mut spacing = ts.character_spacing;
                                let is_space = c == 32 && length == 1;
                                if is_space {
                                    spacing += ts.word_spacing;
                                }

                                let ch = font.decode_char(c);

                                let text_trm = gs.ctm.post_transform(&ts.tm);
                                let device_trm = text_trm.post_transform(&flip_ctm);
                                let (x, y) = (device_trm.m31, device_trm.m32);
                                let transformed_font_size_vec =
                                    text_trm.transform_vector(vec2(ts.font_size, ts.font_size));
                                let transformed_font_size = ((transformed_font_size_vec.x.abs()
                                    * transformed_font_size_vec.y.abs())
                                .sqrt()
                                    * 100.0)
                                    .round()
                                    / 100.0;

                                let trm_vis = device_trm;
                                let is_add = if use_layout {
                                    self.visible_for_layout(
                                        &trm_vis,
                                        transformed_font_size,
                                        &current_color,
                                        media_box,
                                        ts.rendering_mode,
                                        Some(page_text_stats),
                                    )
                                } else {
                                    self.is_visible_text(
                                        &trm_vis,
                                        transformed_font_size,
                                        &current_color,
                                        media_box,
                                        ts.rendering_mode,
                                        Some(page_text_stats),
                                    )
                                };
                                let glyph_char_start = page_char_counter;
                                let glyph_char_len = ch.chars().count();
                                if is_add {
                                    if use_layout {
                                        let g_height = transformed_font_size.max(0.0);
                                        let g_width = (w0 * transformed_font_size).max(0.0);
                                        glyphs.push(Glyph {
                                            ch: ch.clone(),
                                            x,
                                            y,
                                            width: g_width,
                                            height: g_height,
                                            page_char_start: glyph_char_start,
                                            page_char_end: glyph_char_start + glyph_char_len,
                                            source_text_object_id: current_text_obj_id,
                                            font_name: current_font.clone(),
                                            font_size: current_font_size,
                                            transformed_font_size,
                                            font_weight: current_font_weight.clone(),
                                            is_italic: current_is_italic,
                                            fill_color: Some((
                                                (current_color.0 * 255.0).clamp(0.0, 255.0) as u8,
                                                (current_color.1 * 255.0).clamp(0.0, 255.0) as u8,
                                                (current_color.2 * 255.0).clamp(0.0, 255.0) as u8,
                                            )),
                                            render_mode: ts.rendering_mode,
                                            source_kind: stream_context.source_kind.clone(),
                                            xobject_name: stream_context.xobject_name.clone(),
                                        });
                                        page_char_counter += glyph_char_len;
                                        record_font_append(&current_font);
                                    } else {
                                        current_line.push_str(&ch);
                                        page_char_counter += ch.chars().count();
                                        record_font_append(&current_font);
                                    }
                                }
                                first_char = false;
                                last_end = x + w0 * transformed_font_size;
                                last_y = y;
                                let tx = ts.horizontal_scaling
                                    * ((w0 - 0. / 1000.) * ts.font_size + spacing);
                                ts.tm = ts
                                    .tm
                                    .pre_transform(&Transform2D::create_translation(tx, 0.));
                            }
                        }
                    }
                }
                "q" => {
                    gs_stack.push(gs.clone());
                }
                "Q" => {
                    let s = gs_stack.pop();
                    if let Some(s) = s {
                        gs = s;
                    } else {
                        println!("No state to pop");
                    }
                }
                "gs" => {
                    let ext_gstate: &Dictionary = get(doc, resources, b"ExtGState");
                    let Some(name) = operation
                        .operands
                        .first()
                        .and_then(|obj| obj.as_name().ok())
                    else {
                        warn!("Skipping malformed gs operator on page {}", page_num);
                        continue;
                    };
                    let state: &Dictionary = get(doc, ext_gstate, name);
                    apply_state(doc, &mut gs, state);
                }
                "i" => {
                    dlog!(
                        "unhandled graphics state flattness operator {:?}",
                        operation
                    );
                }
                "w" => {
                    gs.line_width = as_num(&operation.operands[0]);
                }
                "J" | "j" | "M" | "d" | "ri" => {
                    dlog!("unknown graphics state operator {:?}", operation);
                }
                "m" => path.ops.push(PathOp::MoveTo(
                    as_num(&operation.operands[0]),
                    as_num(&operation.operands[1]),
                )),
                "l" => path.ops.push(PathOp::LineTo(
                    as_num(&operation.operands[0]),
                    as_num(&operation.operands[1]),
                )),
                "c" => path.ops.push(PathOp::CurveTo(
                    as_num(&operation.operands[0]),
                    as_num(&operation.operands[1]),
                    as_num(&operation.operands[2]),
                    as_num(&operation.operands[3]),
                    as_num(&operation.operands[4]),
                    as_num(&operation.operands[5]),
                )),
                "v" => {
                    let (x, y) = path.current_point();
                    path.ops.push(PathOp::CurveTo(
                        x,
                        y,
                        as_num(&operation.operands[0]),
                        as_num(&operation.operands[1]),
                        as_num(&operation.operands[2]),
                        as_num(&operation.operands[3]),
                    ))
                }
                "y" => path.ops.push(PathOp::CurveTo(
                    as_num(&operation.operands[0]),
                    as_num(&operation.operands[1]),
                    as_num(&operation.operands[2]),
                    as_num(&operation.operands[3]),
                    as_num(&operation.operands[2]),
                    as_num(&operation.operands[3]),
                )),
                "h" => path.ops.push(PathOp::Close),
                "re" => path.ops.push(PathOp::Rect(
                    as_num(&operation.operands[0]),
                    as_num(&operation.operands[1]),
                    as_num(&operation.operands[2]),
                    as_num(&operation.operands[3]),
                )),
                "s" | "f*" | "B" | "B*" | "b" => {
                    dlog!("unhandled path op {:?}", operation);
                }
                "S" => {
                    path.ops.clear();
                }
                "F" | "f" => {
                    path.ops.clear();
                }
                "W" | "w*" => {
                    dlog!("unhandled clipping operation {:?}", operation);
                }
                "n" => {
                    dlog!("discard {:?}", path);
                    path.ops.clear();
                }
                "BMC" | "BDC" => {
                    mc_stack.push(operation);
                }
                "EMC" => {
                    mc_stack.pop();
                }
                "Do" => {
                    let xobject: &Dictionary = get(&doc, resources, b"XObject");
                    let Some(name) = operation
                        .operands
                        .first()
                        .and_then(|obj| obj.as_name().ok())
                    else {
                        warn!("Skipping malformed Do operator on page {}", page_num);
                        continue;
                    };
                    let xf: &Stream = get(&doc, xobject, name);

                    // Only process XObject if OCR handler is available
                    if ocr_route {
                        if let Some(handler) = ocr_handler {
                            let device_trm =
                                gs.ctm.post_transform(&gs.ts.tm).post_transform(&flip_ctm);
                            let (x, y) = (device_trm.m31, device_trm.m32);

                            // Extract dimensions if this is an image
                            let size = if let Ok(subtype) = xf.dict.get(b"Subtype") {
                                if let Ok(subtype_name) = subtype.as_name() {
                                    if subtype_name == b"Image" {
                                        if let (Ok(width), Ok(height)) = (
                                            xf.dict.get(b"Width").and_then(|w| w.as_i64()),
                                            xf.dict.get(b"Height").and_then(|h| h.as_i64()),
                                        ) {
                                            (width as f64, height as f64)
                                        } else {
                                            (100.0, 100.0) // Fallback if dimensions not found
                                        }
                                    } else {
                                        (100.0, 100.0) // Not an image
                                    }
                                } else {
                                    (100.0, 100.0) // Invalid subtype
                                }
                            } else {
                                (100.0, 100.0) // No subtype
                            };

                            // Debug: Log the current CTM to understand what transformations are applied
                            debug!(
                                "Current graphics state CTM: [{:.3} {:.3} {:.3} {:.3} {:.1} {:.1}]",
                                gs.ctm.m11,
                                gs.ctm.m12,
                                gs.ctm.m21,
                                gs.ctm.m22,
                                gs.ctm.m31,
                                gs.ctm.m32
                            );

                            // The PDF coordinate system and image coordinate system may differ
                            // We need to detect the correct orientation based on the CTM
                            // For now, pass the current CTM to let the image processor figure it out
                            let image_transform = gs.ctm.clone();

                            if let Err(e) = process_xobject(
                                &doc,
                                resources,
                                name,
                                Some(handler),
                                ocr_telemetry,
                                text_segments,
                                (x, y),
                                size,
                                page_num,
                                current_font_size,
                                current_transformed_font_size,
                                &mut page_char_counter,
                                &image_transform, // Use CTM without Y-flip for images
                            ) {
                                // Log error but continue processing
                                eprintln!("Failed to process image in PDF: {}", e);
                            }
                        }
                    }

                    // Handle Form XObjects (Layer 3 transformation)
                    if let Ok(subtype) = xf.dict.get(b"Subtype") {
                        if let Ok(subtype_name) = subtype.as_name() {
                            if subtype_name == b"Form" {
                                // In layout-analysis mode, respect LAParams.all_texts
                                let allow_form_text = match laparams {
                                    Some(lp) => {
                                        lp.all_texts || page_text_stats.visible_useful_chars < 32
                                    }
                                    None => true,
                                };
                                if !allow_form_text {
                                    // Skip analyzing text inside Form XObjects unless explicitly enabled
                                    continue;
                                }
                                // Read the Form's /Matrix if present
                                let form_matrix = if let Ok(matrix_obj) = xf.dict.get(b"Matrix") {
                                    if let Ok(matrix_array) = matrix_obj.as_array() {
                                        if matrix_array.len() >= 6 {
                                            // Create transform from PDF matrix array [a b c d e f]
                                            Transform::row_major(
                                                as_num(&matrix_array[0]),
                                                as_num(&matrix_array[1]),
                                                as_num(&matrix_array[2]),
                                                as_num(&matrix_array[3]),
                                                as_num(&matrix_array[4]),
                                                as_num(&matrix_array[5]),
                                            )
                                        } else {
                                            Transform::identity()
                                        }
                                    } else {
                                        Transform::identity()
                                    }
                                } else {
                                    Transform::identity()
                                };

                                let child_name = xobject
                                    .get(name)
                                    .ok()
                                    .and_then(|obj| match obj {
                                        Object::Reference(id) => Some(format!("{id:?}")),
                                        _ => None,
                                    })
                                    .unwrap_or_else(|| String::from_utf8_lossy(name).into_owned());
                                if stream_context.has_xobject_in_stack(&child_name) {
                                    warn!(
                                        "Skipping recursive Form XObject {} on page {}",
                                        child_name, page_num
                                    );
                                    continue;
                                }
                                let child_initial_ctm = gs.ctm.pre_transform(&form_matrix);
                                let child_context = stream_context.form_child(child_name.clone());

                                // Process the Form's content stream
                                let form_resources = maybe_get_obj(&doc, &xf.dict, b"Resources")
                                    .and_then(|n| n.as_dict().ok())
                                    .unwrap_or(resources);
                                let contents = get_contents(xf);
                                if let Err(error) = self.process_stream(
                                    &doc,
                                    ocr_handler,
                                    ocr_telemetry,
                                    ocr_route,
                                    contents,
                                    form_resources,
                                    &media_box,
                                    page_num,
                                    text_segments,
                                    0, // Form XObjects don't have their own rotation
                                    laparams,
                                    Some(child_initial_ctm),
                                    child_context,
                                ) {
                                    warn!(
                                        "Skipping Form XObject {} on page {} after stream error: {:?}",
                                        child_name, page_num, error
                                    );
                                }
                            }
                        }
                    }
                }
                _ => {
                    dlog!("unknown operation {:?}", operation);
                }
            }
        }
        // if !current_word.is_empty() {
        //     current_line.push_str(&current_word);
        //     current_word.clear();
        // }
        if !use_layout && !current_line.is_empty() {
            // let processed_fill_color = if gs.fill_color.len() >= 3 {
            //     (
            //         (gs.fill_color[0] * 255.0).round() as u8,
            //         (gs.fill_color[1] * 255.0).round() as u8,
            //         (gs.fill_color[2] * 255.0).round() as u8,
            //     )
            // } else {
            //     (0, 0, 0) // Fallback to black if not enough color values are provided
            // };

            // if is_visible_text(Some(processed_fill_color), (255, 255, 255)) {
            let content_str = Self::preserve_sentence_boundaries(&current_line);
            if self.fast_text_output.is_some() {
                self.emit_fast_text(&content_str);
            } else {
                let segments = Self::create_text_segments_with_limit(
                    content_str,
                    current_font_size,
                    current_transformed_font_size,
                    current_x,
                    current_y,
                    current_is_bold,
                    current_font.clone(),
                    current_font_weight.clone(),
                    current_is_italic,
                    page_num,
                    "Tj".to_string(),
                    None,
                    None,
                    current_segment_start,
                    page_char_counter,
                    300, // Conservative limit for individual segments
                );
                text_segments.extend(segments);
            }
            // }
            current_line.clear();
        }

        // If using layout, group glyphs → lines and emit segments
        if use_layout {
            if let Some(lp) = params {
                // Process glyphs in content stream order to detect newlines from position changes
                // Key insight: when X resets to the left AND Y changes, that's a new line
                // This is how PDFs naturally encode line breaks through pen movement

                // Sort glyphs by Y position (top to bottom), then X (left to right)
                // This ensures we process in visual reading order
                glyphs.sort_by(|a, b| {
                    let y_cmp = a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal);
                    if y_cmp == std::cmp::Ordering::Equal {
                        a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal)
                    } else {
                        y_cmp
                    }
                });

                // The line builder owns its glyphs. Keep a second snapshot only
                // for the opt-in vertical-text pass; the normal path can move
                // the sorted vector instead of cloning every glyph.
                let glyphs_for_vertical = lp.detect_vertical.then(|| glyphs.clone());
                let glyphs_for_lines = if lp.detect_vertical {
                    glyphs.clone()
                } else {
                    std::mem::take(&mut glyphs)
                };

                // 1) Group glyphs into lines based on vertical overlap (like pdfminer)
                let mut raw_lines: Vec<Vec<Glyph>> = Vec::new();
                let mut current: Vec<Glyph> = Vec::new();

                // Track previous glyph position for simple Y-based newline detection
                let mut prev_y: f64 = 0.0;
                let mut prev_x_end: f64 = 0.0;
                let mut prev_nonspace_x_end: f64 = 0.0;
                let mut avg_char_width: f64 = 8.0;
                let mut avg_char_height: f64 = 12.0;

                for g in glyphs_for_lines {
                    let is_space = g.ch.trim().is_empty();

                    if current.is_empty() {
                        prev_y = g.y;
                        prev_x_end = g.x + g.width;
                        if !is_space {
                            prev_nonspace_x_end = g.x + g.width;
                            avg_char_width = g.width.max(1.0);
                            avg_char_height = g.height.max(1.0);
                        }
                        current.push(g);
                        continue;
                    }

                    // Update average char dimensions from non-space glyphs
                    if !is_space && g.width > 0.0 && g.height > 0.0 {
                        avg_char_width = (avg_char_width + g.width) / 2.0;
                        avg_char_height = (avg_char_height + g.height) / 2.0;
                    }

                    // Get the previous glyph for vertical overlap check
                    let prev_glyph = current.last();

                    // Get the last NON-SPACE glyph for horizontal distance check
                    // Space glyphs fill gaps and shouldn't be used for distance calculation
                    let prev_nonspace_glyph =
                        current.iter().rev().find(|g| !g.ch.trim().is_empty());

                    // Check vertical overlap like pdfminer:
                    // Two glyphs are on the same line if they have significant vertical overlap
                    let has_vertical_overlap = if let Some(prev) = prev_glyph {
                        let prev_top = prev.y;
                        let prev_bottom = prev.y + prev.height;
                        let g_top = g.y;
                        let g_bottom = g.y + g.height;

                        // Calculate overlap
                        let overlap = (prev_bottom.min(g_bottom) - prev_top.max(g_top)).max(0.0);
                        let min_height = prev.height.min(g.height).max(0.1);

                        // Same line if overlap is more than 50% of smaller height
                        // This handles most cases - consecutive glyphs on same line have good overlap
                        overlap > min_height * 0.5
                    } else {
                        true // First glyph
                    };

                    // Check horizontal distance against last NON-SPACE glyph
                    // This prevents space glyphs from artificially reducing the measured gap
                    let hdist_ok = if !is_space {
                        if let Some(prev) = prev_nonspace_glyph {
                            let hdist = if g.x > prev.x + prev.width {
                                g.x - (prev.x + prev.width)
                            } else if prev.x > g.x + g.width {
                                prev.x - (g.x + g.width)
                            } else {
                                0.0 // overlapping
                            };
                            let max_width = prev.width.max(g.width).max(0.1);
                            // char_margin = 3.0 allows normal word spacing while catching column gaps
                            hdist < max_width * 3.0
                        } else {
                            true
                        }
                    } else {
                        true // Space glyphs don't trigger hdist check
                    };

                    // Same line only if: vertical overlap AND horizontal distance OK
                    // New line if EITHER condition fails
                    let is_new_line = !has_vertical_overlap || !hdist_ok;

                    let g_y = g.y;
                    let g_x_end = g.x + g.width;
                    if is_new_line {
                        // Start new line
                        if !current.is_empty() {
                            raw_lines.push(std::mem::take(&mut current));
                        }
                        current.push(g);
                    } else {
                        // Continue current line
                        current.push(g);
                    }

                    // Update previous positions
                    prev_y = g_y;
                    prev_x_end = g_x_end;
                    if !is_space {
                        prev_nonspace_x_end = g_x_end;
                    }
                }
                if !current.is_empty() {
                    raw_lines.push(current);
                }

                // 2) Build LineInfo with spacing via word_margin
                let glyph_total: usize = raw_lines.iter().map(|v| v.len()).sum();
                #[derive(Clone)]
                struct LineInfo {
                    text: String,
                    min_x: f64,
                    max_x: f64,
                    min_y: f64,
                    max_y: f64,
                    height: f64,
                    char_start: usize,
                    char_end: usize,
                    font_name: String,
                    font_size: f64,
                    transformed_font_size: f64,
                    font_weight: FontWeight,
                    is_italic: bool,
                    fill_color: Option<(u8, u8, u8)>,
                    render_mode: i32,
                    source_kind: StreamSourceKind,
                    xobject_name: Option<String>,
                }
                let mut lines: Vec<LineInfo> = Vec::new();
                for mut line in raw_lines.into_iter() {
                    line.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
                    let mut positive_nonspace_gaps = Vec::new();
                    let mut prev_nonspace_right_for_stats: Option<f64> = None;
                    for g in line.iter().filter(|g| !g.ch.trim().is_empty()) {
                        if let Some(prev_right) = prev_nonspace_right_for_stats {
                            let gap = g.x - prev_right;
                            if gap > 0.0 {
                                positive_nonspace_gaps.push(gap);
                            }
                        }
                        prev_nonspace_right_for_stats = Some(g.x + g.width);
                    }
                    let median_nonspace_gap = median_f64(&mut positive_nonspace_gaps);
                    let adaptive_word_gap = if median_nonspace_gap > 0.0 {
                        median_nonspace_gap * 2.5
                    } else {
                        0.0
                    };
                    let adaptive_large_gap = if median_nonspace_gap > 0.0 {
                        median_nonspace_gap * 6.0
                    } else {
                        0.0
                    };
                    let mut s = String::new();
                    let mut min_x = f64::INFINITY;
                    let mut max_x = f64::NEG_INFINITY;
                    let mut min_y = f64::INFINITY;
                    let mut max_y = f64::NEG_INFINITY;
                    let mut prev_right: Option<f64> = None;
                    let mut prev_w: f64 = 0.0;
                    let mut prev_h: f64 = 0.0;
                    // Track last non-space glyph for large gap detection
                    let mut last_nonspace_right: Option<f64> = None;
                    let mut last_nonspace_w: f64 = 0.0;
                    let mut last_nonspace_h: f64 = 0.0;
                    for g in line.iter() {
                        let is_space = g.ch.trim().is_empty();
                        min_x = min_x.min(g.x);
                        max_x = max_x.max(g.x + g.width);
                        min_y = min_y.min(g.y);
                        max_y = max_y.max(g.y + g.height);

                        // For non-space glyphs, check gap from last non-space glyph
                        // to detect column breaks (large horizontal gaps)
                        if !is_space {
                            if let Some(lnr) = last_nonspace_right {
                                let gap = g.x - lnr;
                                let dim_prev = last_nonspace_w.max(last_nonspace_h);
                                let dim_curr = g.width.max(g.height);
                                let width_ref = dim_prev.max(dim_curr).max(1e-6);

                                // Large gap detection: if gap is larger than 1.5x glyph width,
                                // treat as separate column and insert newline instead of space
                                let large_gap_threshold = (width_ref * 1.5).max(adaptive_large_gap);
                                if gap > large_gap_threshold {
                                    // Trim trailing spaces before newline
                                    while s.ends_with(' ') {
                                        s.pop();
                                    }
                                    s.push('\n');
                                } else if let Some(pr) = prev_right {
                                    // Normal word spacing check
                                    let small_gap = g.x - pr;
                                    let dim_prev_small = prev_w.max(prev_h);
                                    let width_ref_small = dim_prev_small.max(dim_curr).max(1e-6);
                                    let word_gap_threshold = ((lp.word_margin as f64)
                                        * width_ref_small)
                                        .max(adaptive_word_gap);
                                    if small_gap > word_gap_threshold {
                                        s.push(' ');
                                    }
                                }
                            } else if let Some(pr) = prev_right {
                                // First non-space glyph but have prev_right from space glyphs
                                let gap = g.x - pr;
                                let dim_prev = prev_w.max(prev_h);
                                let dim_curr = g.width.max(g.height);
                                let width_ref = dim_prev.max(dim_curr).max(1e-6);
                                let word_gap_threshold =
                                    ((lp.word_margin as f64) * width_ref).max(adaptive_word_gap);
                                if gap > word_gap_threshold {
                                    s.push(' ');
                                }
                            }
                        } else {
                            // Space glyph - check for word spacing
                            if let Some(pr) = prev_right {
                                let gap = g.x - pr;
                                let dim_prev = prev_w.max(prev_h);
                                let dim_curr = g.width.max(g.height);
                                let width_ref = dim_prev.max(dim_curr).max(1e-6);
                                let word_gap_threshold =
                                    ((lp.word_margin as f64) * width_ref).max(adaptive_word_gap);
                                if gap > word_gap_threshold {
                                    s.push(' ');
                                }
                            }
                        }

                        s.push_str(&g.ch);
                        prev_right = Some(g.x + g.width);
                        prev_w = g.width;
                        prev_h = g.height;
                        if !is_space {
                            last_nonspace_right = Some(g.x + g.width);
                            last_nonspace_w = g.width;
                            last_nonspace_h = g.height;
                        }
                    }
                    let text =
                        normalize_display_spaced_text(&Self::preserve_sentence_boundaries(&s));
                    let height = (max_y - min_y).max(0.0);
                    let char_start = line.iter().map(|g| g.page_char_start).min().unwrap_or(0);
                    let char_end = line
                        .iter()
                        .map(|g| g.page_char_end)
                        .max()
                        .unwrap_or(char_start);
                    let mut font_counts: HashMap<String, usize> = HashMap::new();
                    for g in &line {
                        *font_counts.entry(g.font_name.clone()).or_default() += 1;
                    }
                    let font_name = font_counts
                        .into_iter()
                        .max_by_key(|(_, count)| *count)
                        .map(|(font, _)| font)
                        .unwrap_or_default();
                    let font_size = line
                        .iter()
                        .map(|g| g.font_size)
                        .fold(0.0, f64::max)
                        .max(height);
                    let transformed_font_size = line
                        .iter()
                        .map(|g| g.transformed_font_size)
                        .fold(0.0, f64::max)
                        .max(height);
                    let bold_count = line.iter().filter(|g| g.font_weight.is_bold()).count();
                    let font_weight = if bold_count * 2 >= line.len().max(1) {
                        FontWeight::Bold
                    } else {
                        FontWeight::Regular
                    };
                    let italic_count = line.iter().filter(|g| g.is_italic).count();
                    let fill_color = line.iter().find_map(|g| g.fill_color);
                    let render_mode = line.first().map(|g| g.render_mode).unwrap_or(0);
                    let source_kind = if line
                        .iter()
                        .any(|g| matches!(g.source_kind, StreamSourceKind::FormXObject))
                    {
                        StreamSourceKind::FormXObject
                    } else {
                        StreamSourceKind::Page
                    };
                    let xobject_name = line.iter().find_map(|g| g.xobject_name.clone());
                    lines.push(LineInfo {
                        text,
                        min_x,
                        max_x,
                        min_y,
                        max_y,
                        height,
                        char_start,
                        char_end,
                        font_name,
                        font_size,
                        transformed_font_size,
                        font_weight,
                        is_italic: italic_count * 2 >= line.len().max(1),
                        fill_color,
                        render_mode,
                        source_kind,
                        xobject_name,
                    });
                }

                // Optional: detect vertical text lines (minimal support)
                if lp.detect_vertical {
                    let glyphs_for_vertical = glyphs_for_vertical
                        .expect("vertical glyph snapshot enabled with detect_vertical");
                    let nonspace_glyph_count = glyphs_for_vertical
                        .iter()
                        .filter(|g| !g.ch.trim().is_empty())
                        .count()
                        .max(1);
                    let candidate_count = glyphs_for_vertical
                        .iter()
                        .filter(|g| {
                            !g.ch.trim().is_empty() && g.width > 0.0 && g.height > 1.5 * g.width
                        })
                        .count();
                    let vertical_candidate_ratio =
                        candidate_count as f64 / nonspace_glyph_count as f64;

                    // Bbox aspect ratio alone misclassifies normal Latin text. Treat "almost
                    // every glyph is tall" as a Latin false-positive signal unless there is
                    // stronger angle/writing-mode evidence, which the lopdf path lacks.
                    let has_vertical_signal =
                        candidate_count >= 3 && vertical_candidate_ratio < 0.40;

                    let mut vg: Vec<Glyph> = if has_vertical_signal {
                        glyphs_for_vertical
                            .into_iter()
                            .filter(|g| {
                                !g.ch.trim().is_empty() && g.width > 0.0 && g.height > 1.5 * g.width
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    // Group into vertical lines by horizontal overlap
                    vg.sort_by(|a, b| {
                        let xcmp = a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal);
                        if xcmp == std::cmp::Ordering::Equal {
                            a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal)
                        } else {
                            xcmp
                        }
                    });
                    let mut v_lines: Vec<Vec<Glyph>> = Vec::new();
                    let mut cur: Vec<Glyph> = Vec::new();
                    let mut v_minx = 0.0;
                    let mut v_maxx = 0.0;
                    for g in vg.into_iter() {
                        if cur.is_empty() {
                            v_minx = g.x;
                            v_maxx = g.x + g.width;
                            cur.push(g);
                            continue;
                        }
                        // Horizontal overlap ratio
                        let left = v_minx.max(g.x);
                        let right = v_maxx.min(g.x + g.width);
                        let overlap = (right - left).max(0.0);
                        let cur_w = (v_maxx - v_minx).max(1e-6);
                        let g_w = g.width.max(1e-6);
                        let overlap_ratio = overlap / cur_w.min(g_w);
                        if overlap_ratio >= lp.line_overlap as f64 {
                            // Same vertical line
                            v_minx = v_minx.min(g.x);
                            v_maxx = v_maxx.max(g.x + g.width);
                            cur.push(g);
                        } else {
                            v_lines.push(cur);
                            cur = vec![g];
                            v_minx = cur[0].x;
                            v_maxx = cur[0].x + cur[0].width;
                        }
                    }
                    if !cur.is_empty() {
                        v_lines.push(cur);
                    }

                    // Build LineInfo for vertical lines, inserting spaces on large vertical gaps
                    for mut vline in v_lines.into_iter() {
                        if vline.len() < 3 {
                            continue;
                        }
                        vline.sort_by(|a, b| {
                            a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        let mut s = String::new();
                        let mut min_x = f64::INFINITY;
                        let mut max_x = f64::NEG_INFINITY;
                        let mut min_y = f64::INFINITY;
                        let mut max_y = f64::NEG_INFINITY;
                        let mut prev_bottom: Option<f64> = None;
                        let mut prev_w: f64 = 0.0;
                        let mut prev_h: f64 = 0.0;
                        for g in vline.iter() {
                            min_x = min_x.min(g.x);
                            max_x = max_x.max(g.x + g.width);
                            min_y = min_y.min(g.y);
                            max_y = max_y.max(g.y + g.height);
                            if let Some(pb) = prev_bottom {
                                let gap = g.y - pb;
                                let dim_prev = prev_w.max(prev_h);
                                let dim_curr = g.width.max(g.height);
                                let width_ref = dim_prev.max(dim_curr).max(1e-6);
                                if gap > (lp.word_margin as f64) * width_ref {
                                    s.push(' ');
                                }
                            }
                            s.push_str(&g.ch);
                            prev_bottom = Some(g.y + g.height);
                            prev_w = g.width;
                            prev_h = g.height;
                        }
                        let text =
                            normalize_display_spaced_text(&Self::preserve_sentence_boundaries(&s));
                        let norm_vertical = normalize_for_duplicate_detection(&text);
                        let height = (max_y - min_y).max(0.0);
                        let duplicate_horizontal = lines.iter().any(|line| {
                            normalize_for_duplicate_detection(&line.text) == norm_vertical
                                && bbox_overlap_ratio(
                                    min_x, min_y, max_x, max_y, line.min_x, line.min_y, line.max_x,
                                    line.max_y,
                                ) > 0.20
                        });
                        if duplicate_horizontal {
                            continue;
                        }
                        lines.push(LineInfo {
                            text,
                            min_x,
                            max_x,
                            min_y,
                            max_y,
                            height,
                            char_start: vline.iter().map(|g| g.page_char_start).min().unwrap_or(0),
                            char_end: vline.iter().map(|g| g.page_char_end).max().unwrap_or(0),
                            font_name: vline
                                .first()
                                .map(|g| g.font_name.clone())
                                .unwrap_or_default(),
                            font_size: vline.iter().map(|g| g.font_size).fold(0.0, f64::max),
                            transformed_font_size: vline
                                .iter()
                                .map(|g| g.transformed_font_size)
                                .fold(0.0, f64::max),
                            font_weight: if vline.iter().any(|g| g.font_weight.is_bold()) {
                                FontWeight::Bold
                            } else {
                                FontWeight::Regular
                            },
                            is_italic: vline.iter().any(|g| g.is_italic),
                            fill_color: vline.iter().find_map(|g| g.fill_color),
                            render_mode: vline.first().map(|g| g.render_mode).unwrap_or(0),
                            source_kind: if vline
                                .iter()
                                .any(|g| matches!(g.source_kind, StreamSourceKind::FormXObject))
                            {
                                StreamSourceKind::FormXObject
                            } else {
                                StreamSourceKind::Page
                            },
                            xobject_name: vline.iter().find_map(|g| g.xobject_name.clone()),
                        });
                    }
                }

                // Default to line-by-line output in layout mode for cleaner, readable text.
                // Set PDF_EXTRACT_LA_BOXES=1 to enable paragraph/box grouping.
                let lines_only = std::env::var("PDF_EXTRACT_LA_BOXES").is_err()
                    || std::env::var("PDF_EXTRACT_LA_LINES_ONLY").is_ok();
                if lines_only {
                    log::info!(
                        "LA: page {} glyphs={} lines={}",
                        page_num,
                        glyph_total,
                        lines.len()
                    );
                    let mut page_char_pos = 0usize;
                    let mut prev_line_bottom: Option<f64> = None;
                    let mut avg_line_gap: f64 = 0.0;
                    let mut line_gap_count: usize = 0;

                    // First pass: calculate average line gap for paragraph detection
                    let lines_vec: Vec<_> = lines.into_iter().collect();
                    for i in 1..lines_vec.len() {
                        let gap = lines_vec[i].min_y - lines_vec[i - 1].max_y;
                        if gap > 0.0 && gap < lines_vec[i].height * 3.0 {
                            avg_line_gap += gap;
                            line_gap_count += 1;
                        }
                    }
                    avg_line_gap = if line_gap_count > 0 {
                        avg_line_gap / line_gap_count as f64
                    } else {
                        10.0
                    };

                    // Collect lines for smarter joining
                    let lines_collected: Vec<_> = lines_vec.into_iter().collect();

                    for (idx, ln) in lines_collected.iter().enumerate() {
                        // Detect paragraph breaks: gap > 1.3x average line gap
                        let is_paragraph_break = if let Some(prev_bottom) = prev_line_bottom {
                            let gap = ln.min_y - prev_bottom;
                            gap > avg_line_gap * 1.3 && gap > ln.height * 0.5
                        } else {
                            false
                        };

                        let trimmed = ln.text.trim();

                        // Check if this line should be joined with the next (no newline)
                        // Join when: line doesn't end with terminal punctuation AND next line exists
                        // AND next line doesn't start with paragraph indicators (numbers, bullets)
                        // AND next line is not a new logical section
                        let ends_with_terminal = trimmed.ends_with('.')
                            || trimmed.ends_with('!')
                            || trimmed.ends_with('?')
                            || trimmed.ends_with(':')
                            || trimmed.ends_with(')') // For things like "APPELLANT (S)"
                            || trimmed.ends_with('"')
                            || trimmed.ends_with('\'');

                        // Check if current line looks like a complete phrase that shouldn't be joined
                        // Common legal document patterns: party names, roles, etc.
                        let current_is_complete = trimmed.to_uppercase() == trimmed // ALL CAPS often complete
                            && (trimmed.contains("APPELLANT") || trimmed.contains("RESPONDENT")
                                || trimmed.contains("VERSUS") || trimmed.contains("PETITIONER")
                                || trimmed.contains("JUDGMENT") || trimmed.contains("ORDER"));

                        let (next_starts_paragraph, next_is_section_marker) =
                            if let Some(next_ln) = lines_collected.get(idx + 1) {
                                let next_trimmed = next_ln.text.trim();
                                // Check if next line starts with paragraph marker
                                let starts_para = next_trimmed.chars().next().map_or(false, |c| {
                                    c.is_ascii_digit() // Numbered list
                                || c == '(' // Parenthetical like "(a)"
                                || c == '-' // Bullet
                                || c == '•' // Bullet
                                }) || next_trimmed.starts_with("A.")
                                    || next_trimmed.starts_with("B.")
                                    || next_trimmed.starts_with("C.")
                                    || next_trimmed.starts_with("D.");

                                // Check if next line is a section marker (ALL CAPS short line)
                                let is_section = next_trimmed.to_uppercase() == next_trimmed
                                    && next_trimmed.len() < 50
                                    && (next_trimmed.contains("APPELLANT")
                                        || next_trimmed.contains("RESPONDENT")
                                        || next_trimmed.contains("VERSUS")
                                        || next_trimmed.contains("PETITIONER"));
                                (starts_para, is_section)
                            } else {
                                // Last line of this XObject - don't automatically treat as paragraph end
                                // Let terminal punctuation or other signals determine if it should be joined
                                (false, false)
                            };

                        // Join lines if: not terminal punctuation AND not followed by paragraph start
                        // AND no paragraph break detected AND not a complete legal phrase
                        let should_join = !ends_with_terminal
                            && !next_starts_paragraph
                            && !is_paragraph_break
                            && !current_is_complete
                            && !next_is_section_marker;

                        // Build line text with appropriate ending
                        let line_text = if is_paragraph_break {
                            format!("\n{}\n", trimmed)
                        } else if should_join {
                            format!("{} ", trimmed) // Space instead of newline for continuation
                        } else {
                            format!("{}\n", trimmed)
                        };
                        let content_len = line_text.chars().count();
                        if let Some(mut segment) = Self::create_text_segment(
                            line_text,
                            ln.font_size.max(ln.height),
                            ln.transformed_font_size.max(ln.height),
                            ln.min_x,
                            ln.min_y,
                            ln.font_weight.is_bold(),
                            ln.font_name.clone(),
                            ln.font_weight.clone(),
                            ln.is_italic,
                            page_num,
                            match ln.source_kind {
                                StreamSourceKind::FormXObject => ln
                                    .xobject_name
                                    .as_ref()
                                    .map(|name| format!("LA-Line:Form:{name}"))
                                    .unwrap_or_else(|| "LA-Line:Form".to_string()),
                                StreamSourceKind::Page => "LA-Line".to_string(),
                            },
                            ln.fill_color,
                            None,
                            ln.char_start,
                            ln.char_end.max(ln.char_start + content_len),
                        ) {
                            segment.width = (ln.max_x - ln.min_x).max(0.0);
                            segment.height = (ln.max_y - ln.min_y).max(0.0);
                            text_segments.push(segment);
                        }
                        page_char_pos += content_len;
                        prev_line_bottom = Some(ln.max_y);
                    }
                    return Ok(());
                }

                // 3) Group lines into text boxes using greedy merges with blocking (pdfminer-like)
                #[derive(Clone)]
                struct BoxInfo {
                    lines: Vec<LineInfo>,
                    min_x: f64,
                    max_x: f64,
                    min_y: f64,
                    max_y: f64,
                    avg_h: f64,
                }

                // helper closures
                let mut make_box = |ln: LineInfo| -> BoxInfo {
                    BoxInfo {
                        min_x: ln.min_x,
                        max_x: ln.max_x,
                        min_y: ln.min_y,
                        max_y: ln.max_y,
                        avg_h: ln.height,
                        lines: vec![ln],
                    }
                };

                fn bbox_area(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> f64 {
                    (max_x - min_x).max(0.0) * (max_y - min_y).max(0.0)
                }

                fn bbox_union(a: (&BoxInfo, usize), b: (&BoxInfo, usize)) -> (f64, f64, f64, f64) {
                    let (a, _) = a;
                    let (b, _) = b;
                    (
                        a.min_x.min(b.min_x),
                        a.min_y.min(b.min_y),
                        a.max_x.max(b.max_x),
                        a.max_y.max(b.max_y),
                    )
                }

                // Alignment and overlap heuristics (from P1)
                const ALIGN_TOL_FRAC: f64 = 0.25;
                const HOVERLAP_MIN_FRAC: f64 = 0.2;
                // Build initial boxes from lines
                let mut boxes: Vec<BoxInfo> = {
                    let mut ls = lines;
                    ls.sort_by(|a, b| {
                        let ycmp = a
                            .min_y
                            .partial_cmp(&b.min_y)
                            .unwrap_or(std::cmp::Ordering::Equal);
                        if ycmp == std::cmp::Ordering::Equal {
                            a.min_x
                                .partial_cmp(&b.min_x)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        } else {
                            ycmp
                        }
                    });
                    ls.into_iter().map(|ln| make_box(ln)).collect()
                };

                if std::env::var("PDF_EXTRACT_LA_HEAP").is_ok() {
                    use ordered_float::OrderedFloat;
                    use std::cmp::Reverse;
                    use std::collections::{BinaryHeap, HashMap, HashSet};
                    #[derive(Clone)]
                    struct Plane {
                        cell: f64,
                        map: HashMap<(i32, i32), Vec<usize>>,
                    }
                    impl Plane {
                        fn new(cell: f64) -> Self {
                            Self {
                                cell,
                                map: HashMap::new(),
                            }
                        }
                        fn cells_for(
                            &self,
                            x0: f64,
                            y0: f64,
                            x1: f64,
                            y1: f64,
                        ) -> (i32, i32, i32, i32) {
                            let cx0 = (x0 / self.cell).floor() as i32;
                            let cy0 = (y0 / self.cell).floor() as i32;
                            let cx1 = (x1 / self.cell).floor() as i32;
                            let cy1 = (y1 / self.cell).floor() as i32;
                            (cx0.min(cx1), cy0.min(cy1), cx0.max(cx1), cy0.max(cy1))
                        }
                        fn insert(&mut self, idx: usize, x0: f64, y0: f64, x1: f64, y1: f64) {
                            let (cx0, cy0, cx1, cy1) = self.cells_for(x0, y0, x1, y1);
                            for cx in cx0..=cx1 {
                                for cy in cy0..=cy1 {
                                    self.map.entry((cx, cy)).or_default().push(idx);
                                }
                            }
                        }
                        fn remove(&mut self, idx: usize, x0: f64, y0: f64, x1: f64, y1: f64) {
                            let (cx0, cy0, cx1, cy1) = self.cells_for(x0, y0, x1, y1);
                            for cx in cx0..=cx1 {
                                for cy in cy0..=cy1 {
                                    if let Some(v) = self.map.get_mut(&(cx, cy)) {
                                        v.retain(|&k| k != idx);
                                    }
                                }
                            }
                        }
                        fn query(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<usize> {
                            let (cx0, cy0, cx1, cy1) = self.cells_for(x0, y0, x1, y1);
                            let mut out = Vec::new();
                            for cx in cx0..=cx1 {
                                for cy in cy0..=cy1 {
                                    if let Some(v) = self.map.get(&(cx, cy)) {
                                        out.extend_from_slice(v);
                                    }
                                }
                            }
                            out
                        }
                    }

                    let n0 = boxes.len();
                    let mut alive = vec![true; n0];
                    let mut ver: Vec<u64> = vec![0; n0];
                    let avg_h =
                        boxes.iter().map(|b| b.avg_h).sum::<f64>() / (boxes.len() as f64).max(1.0);
                    let cell = (avg_h * 2.0).max(8.0);
                    let mut plane = Plane::new(cell);
                    for (i, b) in boxes.iter().enumerate() {
                        plane.insert(i, b.min_x, b.min_y, b.max_x, b.max_y);
                    }

                    let align_tol = |a: &BoxInfo, b: &BoxInfo| -> bool {
                        let tol_x = (lp.line_overlap as f64) * a.avg_h * ALIGN_TOL_FRAC;
                        let left = (b.min_x - a.min_x).abs() <= tol_x;
                        let right = (b.max_x - a.max_x).abs() <= tol_x;
                        let ca = (a.min_x + a.max_x) * 0.5;
                        let cb = (b.min_x + b.max_x) * 0.5;
                        let center = (cb - ca).abs() <= tol_x;
                        left || right || center
                    };
                    let is_neighbor = |a: &BoxInfo, b: &BoxInfo| -> bool {
                        let vgap = (b.min_y - a.max_y).max(a.min_y - b.max_y);
                        let vgap_ok = vgap <= (lp.line_margin as f64) * a.avg_h;
                        let h_left = a.min_x.max(b.min_x);
                        let h_right = a.max_x.min(b.max_x);
                        let h_ov = (h_right - h_left).max(0.0);
                        let w_a = (a.max_x - a.min_x).max(1e-6);
                        let w_b = (b.max_x - b.min_x).max(1e-6);
                        let h_frac = h_ov / w_a.max(w_b);
                        vgap_ok && (h_frac >= HOVERLAP_MIN_FRAC || align_tol(a, b))
                    };

                    let mut heap: BinaryHeap<Reverse<(OrderedFloat<f64>, usize, usize, u64, u64)>> =
                        BinaryHeap::new();
                    let mut seen: HashSet<(usize, usize)> = HashSet::new();
                    fn enqueue_neighbors_for(
                        i: usize,
                        plane: &Plane,
                        boxes_local: &Vec<BoxInfo>,
                        alive: &Vec<bool>,
                        ver: &Vec<u64>,
                        lp: &crate::LAParams,
                        is_neighbor: &dyn Fn(&BoxInfo, &BoxInfo) -> bool,
                        seen: &mut HashSet<(usize, usize)>,
                        heap: &mut BinaryHeap<Reverse<(OrderedFloat<f64>, usize, usize, u64, u64)>>,
                    ) {
                        if !alive[i] {
                            return;
                        }
                        let bi = &boxes_local[i];
                        let margin = (lp.line_margin as f64) * bi.avg_h;
                        let qx0 = bi.min_x - margin;
                        let qx1 = bi.max_x + margin;
                        let qy0 = bi.min_y - margin;
                        let qy1 = bi.max_y + margin;
                        let cand = plane.query(qx0, qy0, qx1, qy1);
                        for &j in cand.iter() {
                            if j == i || !alive[j] {
                                continue;
                            }
                            let a = i.min(j);
                            let b = i.max(j);
                            if !seen.insert((a, b)) {
                                continue;
                            }
                            let bj = &boxes_local[j];
                            if !is_neighbor(bi, bj) {
                                continue;
                            }
                            let (ux0, uy0, ux1, uy1) = bbox_union((bi, i), (bj, j));
                            let a_area = bbox_area(bi.min_x, bi.min_y, bi.max_x, bi.max_y);
                            let b_area = bbox_area(bj.min_x, bj.min_y, bj.max_x, bj.max_y);
                            let u_area = bbox_area(ux0, uy0, ux1, uy1);
                            let dist = (u_area - a_area - b_area).max(0.0);
                            heap.push(Reverse((OrderedFloat(dist), a, b, ver[a], ver[b])));
                        }
                    }
                    for i in 0..boxes.len() {
                        enqueue_neighbors_for(
                            i,
                            &plane,
                            &boxes,
                            &alive,
                            &ver,
                            &lp,
                            &is_neighbor,
                            &mut seen,
                            &mut heap,
                        );
                    }

                    while let Some(Reverse((_d, a, b, va, vb))) = heap.pop() {
                        if !alive[a] || !alive[b] {
                            continue;
                        }
                        if ver[a] != va || ver[b] != vb {
                            continue;
                        }
                        let bi = boxes[a].clone();
                        let bj = boxes[b].clone();
                        if !is_neighbor(&bi, &bj) {
                            continue;
                        }
                        let (ux0, uy0, ux1, uy1) = bbox_union((&bi, a), (&bj, b));
                        let mut blocked = false;
                        for k in plane.query(ux0, uy0, ux1, uy1) {
                            if k == a || k == b || !alive[k] {
                                continue;
                            }
                            let bk = &boxes[k];
                            let cx = (bk.min_x + bk.max_x) * 0.5;
                            let cy = (bk.min_y + bk.max_y) * 0.5;
                            let in_union = cx >= ux0 && cx <= ux1 && cy >= uy0 && cy <= uy1;
                            let in_a = cx >= bi.min_x
                                && cx <= bi.max_x
                                && cy >= bi.min_y
                                && cy <= bi.max_y;
                            let in_b = cx >= bj.min_x
                                && cx <= bj.max_x
                                && cy >= bj.min_y
                                && cy <= bj.max_y;
                            if in_union && !(in_a || in_b) {
                                blocked = true;
                                break;
                            }
                        }
                        if blocked {
                            continue;
                        }

                        // Merge b into a
                        let mut na = boxes[a].clone();
                        let ob = boxes[b].clone();
                        na.lines.extend(ob.lines.into_iter());
                        na.lines.sort_by(|l1, l2| {
                            let ycmp = l1
                                .min_y
                                .partial_cmp(&l2.min_y)
                                .unwrap_or(std::cmp::Ordering::Equal);
                            if ycmp == std::cmp::Ordering::Equal {
                                l1.min_x
                                    .partial_cmp(&l2.min_x)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            } else {
                                ycmp
                            }
                        });
                        na.min_x = na.min_x.min(boxes[b].min_x);
                        na.max_x = na.max_x.max(boxes[b].max_x);
                        na.min_y = na.min_y.min(boxes[b].min_y);
                        na.max_y = na.max_y.max(boxes[b].max_y);
                        let mut sum_h = 0.0;
                        let mut cnt = 0.0;
                        for ln in na.lines.iter() {
                            sum_h += ln.height;
                            cnt += 1.0;
                        }
                        na.avg_h = if cnt > 0.0 { sum_h / cnt } else { na.avg_h };
                        plane.remove(
                            a,
                            boxes[a].min_x,
                            boxes[a].min_y,
                            boxes[a].max_x,
                            boxes[a].max_y,
                        );
                        boxes[a] = na;
                        plane.insert(
                            a,
                            boxes[a].min_x,
                            boxes[a].min_y,
                            boxes[a].max_x,
                            boxes[a].max_y,
                        );
                        plane.remove(
                            b,
                            boxes[b].min_x,
                            boxes[b].min_y,
                            boxes[b].max_x,
                            boxes[b].max_y,
                        );
                        alive[b] = false;
                        ver[a] = ver[a].wrapping_add(1);
                        enqueue_neighbors_for(
                            a,
                            &plane,
                            &boxes,
                            &alive,
                            &ver,
                            &lp,
                            &is_neighbor,
                            &mut seen,
                            &mut heap,
                        );
                    }
                    // Keep only alive boxes
                    let mut out: Vec<BoxInfo> = Vec::new();
                    for (i, b) in boxes.into_iter().enumerate() {
                        if alive[i] {
                            out.push(b);
                        }
                    }
                    boxes = out;
                }

                // Ordering: columns or flow/geometric
                let use_columns = std::env::var("PDF_EXTRACT_LA_COLUMNS").is_ok();
                let flow_none = std::env::var("PDF_EXTRACT_LA_BOXES_FLOW_NONE").is_ok();
                if use_columns && boxes.len() > 2 {
                    #[derive(Default)]
                    struct Col {
                        idxs: Vec<usize>,
                        min_x: f64,
                        max_x: f64,
                    }
                    let widths: Vec<f64> = boxes
                        .iter()
                        .map(|b| (b.max_x - b.min_x).max(1e-6))
                        .collect();
                    let mut w_sorted = widths.clone();
                    w_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    let median_w = if w_sorted.is_empty() {
                        1.0
                    } else {
                        w_sorted[w_sorted.len() / 2]
                    };
                    let mut cols: Vec<Col> = Vec::new();
                    for (i, b) in boxes.iter().enumerate() {
                        let cx = (b.min_x + b.max_x) * 0.5;
                        let mut placed = false;
                        for col in cols.iter_mut() {
                            let left = col.min_x.max(b.min_x);
                            let right = col.max_x.min(b.max_x);
                            let ov = (right - left).max(0.0);
                            let frac = ov / median_w.max(1e-6);
                            let col_cx = (col.min_x + col.max_x) * 0.5;
                            let prox = (cx - col_cx).abs() <= 0.5 * median_w;
                            if frac >= 0.2 || prox {
                                col.idxs.push(i);
                                col.min_x = col.min_x.min(b.min_x);
                                col.max_x = col.max_x.max(b.max_x);
                                placed = true;
                                break;
                            }
                        }
                        if !placed {
                            cols.push(Col {
                                idxs: vec![i],
                                min_x: b.min_x,
                                max_x: b.max_x,
                            });
                        }
                    }
                    cols.sort_by(|a, b| {
                        a.min_x
                            .partial_cmp(&b.min_x)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    let mut ordered: Vec<BoxInfo> = Vec::new();
                    for col in cols.into_iter() {
                        let mut v: Vec<&BoxInfo> =
                            col.idxs.into_iter().map(|k| &boxes[k]).collect();
                        v.sort_by(|a, b| {
                            a.min_y
                                .partial_cmp(&b.min_y)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        for bx in v {
                            ordered.push(bx.clone());
                        }
                    }
                    boxes = ordered;
                } else if !flow_none {
                    let wf = lp.boxes_flow as f64;
                    let wx = ((wf + 1.0) / 2.0).clamp(0.0, 1.0);
                    let wy = 1.0 - wx;
                    boxes.sort_by(|a, b| {
                        let ka = (wy * a.min_y, wx * a.min_x);
                        let kb = (wy * b.min_y, wx * b.min_x);
                        ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
                    });
                } else {
                    boxes.sort_by(|a, b| {
                        let ycmp = a
                            .min_y
                            .partial_cmp(&b.min_y)
                            .unwrap_or(std::cmp::Ordering::Equal);
                        if ycmp == std::cmp::Ordering::Equal {
                            a.min_x
                                .partial_cmp(&b.min_x)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        } else {
                            ycmp
                        }
                    });
                }

                // 5) Emit TextSegments per box
                let mut page_char_pos = 0usize;
                for bx in boxes.into_iter() {
                    let mut content = String::new();
                    for (i, ln) in bx.lines.iter().enumerate() {
                        if i > 0 {
                            content.push('\n');
                        }
                        content.push_str(&ln.text);
                    }
                    let content_len = content.chars().count();
                    if let Some(mut segment) = Self::create_text_segment(
                        content,
                        bx.avg_h,
                        bx.avg_h,
                        bx.min_x,
                        bx.min_y,
                        false,
                        String::new(),
                        FontWeight::Regular,
                        false,
                        page_num,
                        "LA-Box".to_string(),
                        None,
                        None,
                        page_char_pos,
                        page_char_pos + content_len,
                    ) {
                        segment.width = (bx.max_x - bx.min_x).max(0.0);
                        segment.height = (bx.max_y - bx.min_y).max(0.0);
                        text_segments.push(segment);
                    }
                    page_char_pos += content_len;
                }
            }
        }

        Ok(())
    }
}

pub trait OutputDev {
    fn begin_page(
        &mut self,
        page_num: u32,
        media_box: &MediaBox,
        art_box: Option<(f64, f64, f64, f64)>,
    ) -> Result<(), OutputError>;
    fn end_page(&mut self) -> Result<(), OutputError>;
    fn output_character(
        &mut self,
        trm: &Transform,
        width: f64,
        spacing: f64,
        font_size: f64,
        char: &str,
    ) -> Result<(), OutputError>;
    fn begin_word(&mut self) -> Result<(), OutputError>;
    fn end_word(&mut self) -> Result<(), OutputError>;
    fn end_line(&mut self) -> Result<(), OutputError>;
    fn stroke(
        &mut self,
        _ctm: &Transform,
        _colorspace: &ColorSpace,
        _color: &[f64],
        _path: &Path,
    ) -> Result<(), OutputError> {
        Ok(())
    }
    fn fill(
        &mut self,
        _ctm: &Transform,
        _colorspace: &ColorSpace,
        _color: &[f64],
        _path: &Path,
    ) -> Result<(), OutputError> {
        Ok(())
    }
}

pub struct HTMLOutput<'a> {
    file: &'a mut dyn std::io::Write,
    flip_ctm: Transform,
    last_ctm: Transform,
    buf_ctm: Transform,
    buf_font_size: f64,
    buf: String,
}

fn insert_nbsp(input: &str) -> String {
    let mut result = String::new();
    let mut word_end = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ' ' {
            if !word_end || chars.peek().filter(|x| **x != ' ').is_none() {
                result += "&nbsp;";
            } else {
                result += " ";
            }
            word_end = false;
        } else {
            word_end = true;
            result.push(c);
        }
    }
    result
}

impl<'a> HTMLOutput<'a> {
    pub fn new(file: &mut dyn std::io::Write) -> HTMLOutput {
        HTMLOutput {
            file,
            flip_ctm: Transform2D::identity(),
            last_ctm: Transform2D::identity(),
            buf_ctm: Transform2D::identity(),
            buf: String::new(),
            buf_font_size: 0.,
        }
    }
    fn flush_string(&mut self) -> Result<(), OutputError> {
        if self.buf.len() != 0 {
            let position = self.buf_ctm.post_transform(&self.flip_ctm);
            let transformed_font_size_vec = self
                .buf_ctm
                .transform_vector(vec2(self.buf_font_size, self.buf_font_size));
            // get the length of one sized of the square with the same area with a rectangle of size (x, y)
            let transformed_font_size =
                (transformed_font_size_vec.x * transformed_font_size_vec.y).sqrt();
            let (x, y) = (position.m31, position.m32);
            println!("flush {} {:?}", self.buf, (x, y));

            write!(self.file, "<div style='position: absolute; left: {}px; top: {}px; font-size: {}px'>{}</div>\n",
                   x, y, transformed_font_size, insert_nbsp(&self.buf))?;
        }
        Ok(())
    }
}

type ArtBox = (f64, f64, f64, f64);

impl<'a> OutputDev for HTMLOutput<'a> {
    fn begin_page(
        &mut self,
        page_num: u32,
        media_box: &MediaBox,
        _: Option<ArtBox>,
    ) -> Result<(), OutputError> {
        write!(self.file, "<meta charset='utf-8' /> ")?;
        write!(self.file, "<!-- page {} -->", page_num)?;
        write!(self.file, "<div id='page{}' style='position: relative; height: {}px; width: {}px; border: 1px black solid'>", page_num, media_box.ury - media_box.lly, media_box.urx - media_box.llx)?;
        self.flip_ctm = Transform::row_major(1., 0., 0., -1., -media_box.llx, media_box.ury);
        Ok(())
    }
    fn end_page(&mut self) -> Result<(), OutputError> {
        self.flush_string()?;
        self.buf = String::new();
        self.last_ctm = Transform::identity();
        write!(self.file, "</div>")?;
        Ok(())
    }
    fn output_character(
        &mut self,
        trm: &Transform,
        width: f64,
        spacing: f64,
        font_size: f64,
        char: &str,
    ) -> Result<(), OutputError> {
        if trm.approx_eq(&self.last_ctm) {
            let position = trm.post_transform(&self.flip_ctm);
            let (x, y) = (position.m31, position.m32);

            println!("accum {} {:?}", char, (x, y));
            self.buf += char;
        } else {
            println!(
                "flush {} {:?} {:?} {} {} {}",
                char, trm, self.last_ctm, width, font_size, spacing
            );
            self.flush_string()?;
            self.buf = char.to_owned();
            self.buf_font_size = font_size;
            self.buf_ctm = *trm;
        }
        let position = trm.post_transform(&self.flip_ctm);
        let transformed_font_size_vec = trm.transform_vector(vec2(font_size, font_size));
        // get the length of one sized of the square with the same area with a rectangle of size (x, y)
        let transformed_font_size =
            (transformed_font_size_vec.x * transformed_font_size_vec.y).sqrt();
        let (x, y) = (position.m31, position.m32);
        write!(self.file, "<div style='position: absolute; color: red; left: {}px; top: {}px; font-size: {}px'>{}</div>",
               x, y, transformed_font_size, char)?;
        self.last_ctm = trm.pre_transform(&Transform2D::create_translation(
            width * font_size + spacing,
            0.,
        ));

        Ok(())
    }
    fn begin_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
}

pub struct SVGOutput<'a> {
    file: &'a mut dyn std::io::Write,
}
impl<'a> SVGOutput<'a> {
    pub fn new(file: &mut dyn std::io::Write) -> SVGOutput {
        SVGOutput { file }
    }
}

#[cfg(test)]
mod content_ext_compaction_tests;

#[cfg(test)]
mod reconstruction_tests {
    use super::{
        create_content_core_with_identity, create_content_ext_with_spans,
        create_pdf_location_from_output_spans, decompress_content_ext,
        looks_like_symbol_glyph_soup, normalize_display_spaced_text,
        production_telemetry_for_results, BoundingBox, CharRange, ExtractionOptions,
        ExtractionResult, FormatLocation, OcrImageTelemetrySnapshot, PageFragment, PdfLocation,
    };
    use crate::document::{LocatedText, OutputSpan, SpanSource};
    use std::collections::HashMap;

    #[test]
    fn rejoins_display_letter_spaced_words() {
        assert_eq!(normalize_display_spaced_text("J U D G M E N T"), "JUDGMENT");
        assert_eq!(
            normalize_display_spaced_text("i n f o r m a t i o n a l privacy"),
            "informational privacy"
        );
    }

    #[test]
    fn preserves_short_initial_runs() {
        assert_eq!(normalize_display_spaced_text("M P Sharma"), "M P Sharma");
        assert_eq!(
            normalize_display_spaced_text("D Y Chandrachud"),
            "D Y Chandrachud"
        );
    }

    #[test]
    fn detects_symbol_glyph_soup() {
        assert!(looks_like_symbol_glyph_soup(
            r#"!% &!*#/!'#$+''%'#*!$)&%'%&./'.-#!#$%+-'#''"#
        ));
    }

    #[test]
    fn keeps_legal_numeric_text() {
        assert!(!looks_like_symbol_glyph_soup(
            "Article 21, Section 14(2), W.P.(C) No. 372/2017"
        ));
        assert!(!looks_like_symbol_glyph_soup(
            "2024-01 2024-02 2024-03 2024-04"
        ));
    }

    #[test]
    fn content_ext_serializes_output_spans_when_present() {
        let bbox = BoundingBox {
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
        };
        let location = FormatLocation::Pdf(PdfLocation {
            fragments: vec![PageFragment {
                page: 1,
                char_range: CharRange { start: 10, end: 14 },
                bbox: bbox.clone(),
            }],
        });
        let located = LocatedText {
            text: "test".to_string(),
            spans: vec![OutputSpan {
                output_start: 0,
                output_end: 4,
                source: SpanSource::Pdf {
                    page: 1,
                    char_start: 10,
                    char_end: 14,
                    bbox,
                },
            }],
        };

        let ext = create_content_ext_with_spans(
            "chunk",
            &location,
            Some(10),
            Some(14),
            None,
            Some(&located),
            ExtractionOptions {
                emit_output_spans: true,
                ..ExtractionOptions::default()
            },
        )
        .expect("content ext");
        let value = decompress_content_ext(&ext).expect("decompress");

        assert_eq!(value["output_text_len"], 4);
        assert_eq!(value["output_spans"].as_array().unwrap().len(), 1);
        assert_eq!(value["chunk_locations"].as_array().unwrap().len(), 1);
        assert_eq!(
            value["location_model"]["source_span_granularity"],
            "output_span"
        );
    }

    #[test]
    fn pdf_location_fragments_follow_output_span_bboxes() {
        let first_bbox = BoundingBox {
            x: 10.0,
            y: 20.0,
            width: 40.0,
            height: 10.0,
        };
        let second_bbox = BoundingBox {
            x: 10.0,
            y: 50.0,
            width: 40.0,
            height: 10.0,
        };
        let located = LocatedText {
            text: "alpha\nbeta".to_string(),
            spans: vec![
                OutputSpan {
                    output_start: 0,
                    output_end: 5,
                    source: SpanSource::Pdf {
                        page: 1,
                        char_start: 100,
                        char_end: 105,
                        bbox: first_bbox,
                    },
                },
                OutputSpan {
                    output_start: 5,
                    output_end: 6,
                    source: SpanSource::Synthetic {
                        kind: crate::document::SyntheticKind::InsertedWhitespace,
                        parent_refs: Vec::new(),
                    },
                },
                OutputSpan {
                    output_start: 6,
                    output_end: 10,
                    source: SpanSource::Pdf {
                        page: 1,
                        char_start: 106,
                        char_end: 110,
                        bbox: second_bbox,
                    },
                },
            ],
        };

        let FormatLocation::Pdf(location) = create_pdf_location_from_output_spans(&located, &[])
        else {
            panic!("expected pdf location");
        };

        assert_eq!(location.fragments.len(), 2);
        assert_eq!(
            location.fragments[0].char_range,
            CharRange { start: 0, end: 5 }
        );
        assert_eq!(
            location.fragments[1].char_range,
            CharRange { start: 6, end: 10 }
        );
        assert_eq!(location.fragments[0].bbox.y, 20.0);
        assert_eq!(location.fragments[1].bbox.y, 50.0);
    }

    #[test]
    fn production_telemetry_reports_content_ext_and_span_quality() {
        let bbox = BoundingBox {
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
        };
        let located = LocatedText {
            text: "abc ".to_string(),
            spans: vec![
                OutputSpan {
                    output_start: 0,
                    output_end: 3,
                    source: SpanSource::Pdf {
                        page: 1,
                        char_start: 10,
                        char_end: 13,
                        bbox,
                    },
                },
                OutputSpan {
                    output_start: 3,
                    output_end: 4,
                    source: SpanSource::Synthetic {
                        kind: crate::document::SyntheticKind::InsertedWhitespace,
                        parent_refs: Vec::new(),
                    },
                },
            ],
        };
        let location = create_pdf_location_from_output_spans(&located, &[]);
        let content_core =
            create_content_core_with_identity(&located.text, &[], 1, "file", Some(1), Some(10), 0);
        let content_ext = create_content_ext_with_spans(
            &content_core.chunk_id,
            &location,
            Some(10),
            Some(14),
            None,
            Some(&located),
            ExtractionOptions {
                emit_output_spans: true,
                ..ExtractionOptions::default()
            },
        )
        .expect("content ext");
        let result = ExtractionResult {
            content_core,
            content_ext,
            repairs: Vec::new(),
        };

        let telemetry = production_telemetry_for_results(
            &[result],
            Some(500),
            true,
            false,
            false,
            Some(4),
            Some(4),
            &HashMap::new(),
            &OcrImageTelemetrySnapshot::default(),
        );

        assert_eq!(telemetry.chunk_count, 1);
        assert!(telemetry.content_ext_max_compressed_bytes > 0);
        assert!(telemetry.content_ext_max_uncompressed_bytes > 0);
        assert_eq!(telemetry.chars_without_span, 0);
        assert_eq!(telemetry.synthetic_char_ratio, 0.25);
        assert_eq!(telemetry.pdf_backed_nonsynthetic_char_ratio, 1.0);
    }
}

impl<'a> OutputDev for SVGOutput<'a> {
    fn begin_page(
        &mut self,
        _page_num: u32,
        media_box: &MediaBox,
        art_box: Option<(f64, f64, f64, f64)>,
    ) -> Result<(), OutputError> {
        let ver = 1.1;
        write!(self.file, "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n")?;
        if ver == 1.1 {
            write!(
                self.file,
                r#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">"#
            )?;
        } else {
            write!(
                self.file,
                r#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.0//EN" "http://www.w3.org/TR/2001/REC-SVG-20010904/DTD/svg10.dtd">"#
            )?;
        }
        if let Some(art_box) = art_box {
            let width = art_box.2 - art_box.0;
            let height = art_box.3 - art_box.1;
            let y = media_box.ury - art_box.1 - height;
            write!(self.file, "<svg width=\"{}\" height=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" version=\"{}\" viewBox='{} {} {} {}'>", width, height, ver, art_box.0, y, width, height)?;
        } else {
            let width = media_box.urx - media_box.llx;
            let height = media_box.ury - media_box.lly;
            write!(self.file, "<svg width=\"{}\" height=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" version=\"{}\" viewBox='{} {} {} {}'>", width, height, ver, media_box.llx, media_box.lly, width, height)?;
        }
        write!(self.file, "\n")?;
        type Mat = Transform;

        let ctm = Mat::create_scale(1., -1.).post_translate(vec2(0., media_box.ury));
        write!(
            self.file,
            "<g transform='matrix({}, {}, {}, {}, {}, {})'>\n",
            ctm.m11, ctm.m12, ctm.m21, ctm.m22, ctm.m31, ctm.m32,
        )?;
        Ok(())
    }
    fn end_page(&mut self) -> Result<(), OutputError> {
        write!(self.file, "</g>\n")?;
        write!(self.file, "</svg>")?;
        Ok(())
    }
    fn output_character(
        &mut self,
        _trm: &Transform,
        _width: f64,
        _spacing: f64,
        _font_size: f64,
        _char: &str,
    ) -> Result<(), OutputError> {
        Ok(())
    }
    fn begin_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn fill(
        &mut self,
        ctm: &Transform,
        _colorspace: &ColorSpace,
        _color: &[f64],
        path: &Path,
    ) -> Result<(), OutputError> {
        write!(
            self.file,
            "<g transform='matrix({}, {}, {}, {}, {}, {})'>",
            ctm.m11, ctm.m12, ctm.m21, ctm.m22, ctm.m31, ctm.m32,
        )?;

        let mut d = Vec::new();
        for op in &path.ops {
            match op {
                &PathOp::MoveTo(x, y) => d.push(format!("M{} {}", x, y)),
                &PathOp::LineTo(x, y) => d.push(format!("L{} {}", x, y)),
                &PathOp::CurveTo(x1, y1, x2, y2, x, y) => {
                    d.push(format!("C{} {} {} {} {} {}", x1, y1, x2, y2, x, y))
                }
                &PathOp::Close => d.push(format!("Z")),
                &PathOp::Rect(x, y, width, height) => {
                    d.push(format!("M{} {}", x, y));
                    d.push(format!("L{} {}", x + width, y));
                    d.push(format!("L{} {}", x + width, y + height));
                    d.push(format!("L{} {}", x, y + height));
                    d.push(format!("Z"));
                }
            }
        }
        write!(self.file, "<path d='{}' />", d.join(" "))?;
        write!(self.file, "</g>")?;
        write!(self.file, "\n")?;
        Ok(())
    }
}

/*
File doesn't implement std::fmt::Write so we have
to do some gymnastics to accept a File or String
See https://github.com/rust-lang/rust/issues/51305
*/

pub trait ConvertToFmt {
    type Writer: std::fmt::Write;
    fn convert(self) -> Self::Writer;
}

impl<'a> ConvertToFmt for &'a mut String {
    type Writer = &'a mut String;
    fn convert(self) -> Self::Writer {
        self
    }
}

pub struct WriteAdapter<W> {
    f: W,
}

impl<W: std::io::Write> std::fmt::Write for WriteAdapter<W> {
    fn write_str(&mut self, s: &str) -> Result<(), std::fmt::Error> {
        self.f.write_all(s.as_bytes()).map_err(|_| fmt::Error)
    }
}

impl<'a> ConvertToFmt for &'a mut dyn std::io::Write {
    type Writer = WriteAdapter<Self>;
    fn convert(self) -> Self::Writer {
        WriteAdapter { f: self }
    }
}

impl<'a> ConvertToFmt for &'a mut File {
    type Writer = WriteAdapter<Self>;
    fn convert(self) -> Self::Writer {
        WriteAdapter { f: self }
    }
}

pub struct PlainTextOutput<W: ConvertToFmt> {
    writer: W::Writer,
    last_end: f64,
    last_y: f64,
    first_char: bool,
    flip_ctm: Transform,
}

impl<W: ConvertToFmt> PlainTextOutput<W> {
    pub fn new(writer: W) -> PlainTextOutput<W> {
        PlainTextOutput {
            writer: writer.convert(),
            last_end: 100000.,
            first_char: false,
            last_y: 0.,
            flip_ctm: Transform2D::identity(),
        }
    }
}

/* There are some structural hints that PDFs can use to signal word and line endings:
 * however relying on these is not likely to be sufficient. */
impl<W: ConvertToFmt> OutputDev for PlainTextOutput<W> {
    fn begin_page(
        &mut self,
        _page_num: u32,
        media_box: &MediaBox,
        _: Option<ArtBox>,
    ) -> Result<(), OutputError> {
        self.flip_ctm = Transform2D::row_major(1., 0., 0., -1., 0., media_box.ury - media_box.lly);
        Ok(())
    }
    fn end_page(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn output_character(
        &mut self,
        trm: &Transform,
        width: f64,
        _spacing: f64,
        font_size: f64,
        char: &str,
    ) -> Result<(), OutputError> {
        let position = trm.post_transform(&self.flip_ctm);
        let transformed_font_size_vec = trm.transform_vector(vec2(font_size, font_size));
        // get the length of one sized of the square with the same area with a rectangle of size (x, y)
        let transformed_font_size =
            (transformed_font_size_vec.x * transformed_font_size_vec.y).sqrt();
        let (x, y) = (position.m31, position.m32);
        use std::fmt::Write;
        //dlog!("last_end: {} x: {}, width: {}", self.last_end, x, width);
        if self.first_char {
            // Check for paragraph break
            if is_paragraph_break(y, self.last_y, transformed_font_size) {
                write!(self.writer, "\n")?;
            }

            // we've moved to the left and down (new line)
            if is_new_line(x, self.last_end, y, self.last_y, transformed_font_size) {
                write!(self.writer, "\n")?;
            }
            // Check for large horizontal gap (column break)
            else if is_column_break(x, self.last_end, transformed_font_size) {
                write!(self.writer, "\n")?;
            } else if should_insert_space(x, self.last_end, transformed_font_size) {
                dlog!(
                    "width: {}, space: {}, thresh: {}",
                    width,
                    x - self.last_end,
                    if transformed_font_size < 10.0 {
                        transformed_font_size * MIN_SPACE_GAP
                    } else {
                        transformed_font_size * SPACE_THRESHOLD_RATIO
                    }
                );
                write!(self.writer, " ")?;
            }
        }
        //let norm = unicode_normalization::UnicodeNormalization::nfkc(char);
        write!(self.writer, "{}", char)?;
        self.first_char = false;
        self.last_y = y;
        self.last_end = x + width * transformed_font_size;
        Ok(())
    }
    fn begin_word(&mut self) -> Result<(), OutputError> {
        self.first_char = true;
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), OutputError> {
        //write!(self.file, "\n");
        Ok(())
    }
}

pub fn print_metadata(doc: &Document) {
    dlog!("Version: {}", doc.version);
    if let Some(ref info) = get_info(&doc) {
        for (k, v) in *info {
            match v {
                &Object::String(ref s, StringFormat::Literal) => {
                    dlog!("{}: {}", pdf_to_utf8(k), pdf_to_utf8(s));
                }
                _ => {}
            }
        }
    }
    dlog!(
        "Page count: {}",
        get::<i64>(&doc, &get_pages(&doc), b"Count")
    );
    dlog!("Pages: {:?}", get_pages(&doc));
    dlog!(
        "Type: {:?}",
        get_pages(&doc)
            .get(b"Type")
            .and_then(|x| x.as_name())
            .unwrap()
    );
}

/// Extract the text from a pdf at `path` and return a `String` with the results
pub fn extract_text<P: std::convert::AsRef<std::path::Path>>(
    path: P,
    ocr_handler: Option<&OcrHandler>,
) -> Result<String, OutputError> {
    let loaded = load_pdf_from_path(path, &ExtractionOptions::default())?;
    let doc = loaded.document;
    let content_outputs = output_doc(&doc, ocr_handler, None, None)?;

    // Concatenate all content from the ContentOutput structs
    let mut result = String::new();
    for output in content_outputs {
        if !output.headings.is_empty() {
            result.push_str(&output.headings.join(" > "));
            result.push_str("\n\n");
        }
        result.push_str(&output.paragraph);
        result.push_str("\n\n");
    }

    Ok(result.trim().to_string())
}

/// Extract text without OCR, layout analysis, heading/table detection,
/// location spans, or exact tokenization. This is the explicit low-latency
/// override for callers that only need sentence-aware text chunks.
///
/// The returned chunks intentionally carry no page or font metadata. Use
/// [`parse_pdf`] when citations, bounding boxes, or structured document
/// semantics are required. `max_tokens` uses a cheap word-count estimate and
/// is a chunking hint rather than an exact tokenizer cap.
pub fn extract_text_fast<P: std::convert::AsRef<std::path::Path>>(
    path: P,
    max_tokens: Option<usize>,
    options: ExtractionOptions,
) -> Result<Vec<String>, OutputError> {
    let loaded = load_pdf_from_path(path, &options)?;
    fast_text_chunks(&loaded.document, max_tokens, &options)
}

/// In-memory counterpart to [`extract_text_fast`].
pub fn extract_text_fast_from_mem(
    buffer: &[u8],
    max_tokens: Option<usize>,
    options: ExtractionOptions,
) -> Result<Vec<String>, OutputError> {
    let loaded = load_pdf_from_mem(buffer, &options)?;
    fast_text_chunks(&loaded.document, max_tokens, &options)
}

fn fast_text_chunks(
    doc: &Document,
    max_tokens: Option<usize>,
    options: &ExtractionOptions,
) -> Result<Vec<String>, OutputError> {
    let empty_resources = &Dictionary::new();
    let page_texts: Result<Vec<(u32, String)>, OutputError> = doc
        .get_pages()
        .par_iter()
        .map(|(page_num, object_id)| {
            let page_dict = doc
                .get_object(*object_id)
                .and_then(Object::as_dict)
                .map_err(OutputError::from)?;
            let resources = get_inherited(doc, page_dict, b"Resources").unwrap_or(empty_resources);
            let media_box = get_inherited(doc, page_dict, b"MediaBox")
                .map(|values: Vec<f64>| MediaBox {
                    llx: values.first().copied().unwrap_or(0.0),
                    lly: values.get(1).copied().unwrap_or(0.0),
                    urx: values.get(2).copied().unwrap_or(612.0),
                    ury: values.get(3).copied().unwrap_or(792.0),
                })
                .unwrap_or(MediaBox {
                    llx: 0.0,
                    lly: 0.0,
                    urx: 612.0,
                    ury: 792.0,
                });
            let mut processor = Processor::new_fast_text();
            let mut segments = Vec::new();
            if let Ok(content) = doc.get_page_content(*object_id) {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    processor.process_stream(
                        doc,
                        None,
                        None,
                        false,
                        content,
                        resources,
                        &media_box,
                        *page_num,
                        &mut segments,
                        get_page_rotation(page_dict, doc),
                        None,
                        None,
                        StreamContext::page_with_limit(options.max_recursion_depth.unwrap_or(8)),
                    )
                }));
                if let Ok(Err(error)) = result {
                    if matches!(error, OutputError::ResourceLimit { .. }) {
                        return Err(error);
                    }
                }
            }
            let text = processor.take_fast_text_output().unwrap_or_else(|| {
                let mut text = String::new();
                for segment in segments {
                    let segment = segment.content.trim();
                    if segment.is_empty() {
                        continue;
                    }
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(segment);
                }
                text
            });
            Ok((*page_num, text))
        })
        .collect();

    let mut page_texts = page_texts?;
    page_texts.sort_by_key(|(page, _)| *page);
    let mut total_bytes = 0usize;
    let mut chunks = Vec::new();
    for (_, page_text) in page_texts {
        let text = crate::document::processing::clean_text_for_indexing(&page_text);
        total_bytes = total_bytes.saturating_add(text.len());
        if let Some(limit) = options.max_output_bytes {
            if total_bytes > limit {
                return Err(OutputError::resource_limit(
                    ResourceLimitKind::OutputBytes,
                    limit,
                    Some(total_bytes),
                ));
            }
        }
        chunks.extend(split_fast_sentences(&text, max_tokens));
    }
    Ok(chunks)
}

fn split_fast_sentences(text: &str, max_tokens: Option<usize>) -> Vec<String> {
    // ponytail: one corpus-calibrated ratio; raise to 2.0 if hard-cap safety
    // matters more than chunk density, or replace with a caller knob later.
    const FAST_TOKEN_ESTIMATE_RATIO: f64 = 1.5;
    let sentences = text
        .split_inclusive(|character: char| matches!(character, '.' | '!' | '?'))
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty());
    let Some(limit) = max_tokens.filter(|limit| *limit > 0) else {
        return sentences.map(str::to_owned).collect();
    };

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut estimated_tokens = 0usize;
    for sentence in sentences {
        let sentence_tokens = text_splitting::estimate_tokens_from_words(
            text_splitting::count_words(sentence),
            FAST_TOKEN_ESTIMATE_RATIO,
        );
        if !current.is_empty() && estimated_tokens.saturating_add(sentence_tokens) > limit {
            chunks.push(std::mem::take(&mut current));
            estimated_tokens = 0;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(sentence);
        estimated_tokens = estimated_tokens.saturating_add(sentence_tokens);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod fast_text_tests {
    use super::{
        extract_text_fast_from_mem, split_fast_sentences, ExtractionOptions, OutputError, Processor,
    };

    #[test]
    fn fast_text_splits_on_sentence_boundaries() {
        assert_eq!(
            split_fast_sentences("First sentence. Second sentence!", None),
            vec!["First sentence.", "Second sentence!"]
        );
    }

    #[test]
    fn fast_text_keeps_pdf_validation() {
        assert!(matches!(
            extract_text_fast_from_mem(b"not a PDF", Some(350), ExtractionOptions::default()),
            Err(OutputError::NotAPdf { .. })
        ));
    }

    #[test]
    fn fast_text_sink_matches_segment_joining() {
        let mut processor = Processor::new_fast_text();
        processor.emit_fast_text(" first\n");
        processor.emit_fast_text("second ");
        assert_eq!(
            processor.take_fast_text_output().as_deref(),
            Some("first second")
        );
    }
}

/// Number of pages in the PDF at `path`, without extracting any text.
///
/// This exists so a caller that got zero characters back can tell the two cases
/// apart: a document with no pages has nothing to extract, while a document with
/// pages that yielded no text means either a scan with no text layer or a parse
/// failure on our side. Both of those are worth reporting; "empty" on its own is
/// not actionable.
///
/// Intended to be called only on the zero-character path, where re-reading the
/// file is cheap relative to having lost the document. Do not call it for every
/// document — it parses the cross-reference table and page tree again.
pub fn page_count<P: std::convert::AsRef<std::path::Path>>(path: P) -> Result<usize, OutputError> {
    let loaded = load_pdf_from_path(path, &ExtractionOptions::default())?;
    let doc = loaded.document;
    Ok(doc.get_pages().len())
}

/// Number of pages in an in-memory PDF, without extracting any text.
///
/// See [`page_count`].
pub fn page_count_from_mem(buffer: &[u8]) -> Result<usize, OutputError> {
    let loaded = load_pdf_from_mem(buffer, &ExtractionOptions::default())?;
    let doc = loaded.document;
    Ok(doc.get_pages().len())
}

// pub fn extract_text_encrypted<P: std::convert::AsRef<std::path::Path>, PW: AsRef<[u8]>>(
//     path: P,
//     password: PW,
// ) -> Result<String, OutputError> {
//     let mut s = String::new();
//     {
//         // let mut output = PlainTextOutput::new(&mut s);
//         let mut doc = Document::load(path)?;
//         output_doc_encrypted(&mut doc, password)?;
//     }
//     Ok(s)
// }

pub fn extract_text_from_mem(
    buffer: &[u8],
    ocr_handler: Option<&OcrHandler>,
) -> Result<String, OutputError> {
    let loaded = load_pdf_from_mem(buffer, &ExtractionOptions::default())?;
    let doc = loaded.document;
    let content_outputs = output_doc(&doc, ocr_handler, None, None)?;

    // Concatenate all content from the ContentOutput structs
    let mut result = String::new();
    for output in content_outputs {
        if !output.headings.is_empty() {
            result.push_str(&output.headings.join(" > "));
            result.push_str("\n\n");
        }
        result.push_str(&output.paragraph);
        result.push_str("\n\n");
    }

    Ok(result.trim().to_string())
}

// pub fn extract_text_from_mem_encrypted<PW: AsRef<[u8]>>(
//     buffer: &[u8],
//     password: PW,
// ) -> Result<String, OutputError> {
//     let mut s = String::new();
//     {
//         // let mut output = PlainTextOutput::new(&mut s);
//         let mut doc = Document::load_mem(buffer)?;
//         output_doc_encrypted(&mut doc, password)?;
//     }
//     Ok(s)
// }

fn get_inherited<'a, T: FromObj<'a>>(
    doc: &'a Document,
    dict: &'a Dictionary,
    key: &[u8],
) -> Option<T> {
    let o: Option<T> = get(doc, dict, key);
    if let Some(o) = o {
        Some(o)
    } else {
        let parent = dict
            .get(b"Parent")
            .and_then(|parent| parent.as_reference())
            .and_then(|id| doc.get_dictionary(id))
            .ok()?;
        get_inherited(doc, parent, key)
    }
}

// pub fn output_doc_encrypted<PW: AsRef<[u8]>>(
//     doc: &mut Document,
//     // output: &mut dyn OutputDev,
//     password: PW,
// ) -> Result<Vec<ContentOutput>, OutputError> {
//     doc.decrypt(password)?;
//     output_doc(doc, ocr, detection_model, recognition_model)
// }

// #[derive(Debug)]
// pub struct ContentOutput {
//     pub headings: Vec<String>,
//     pub paragraph: String,
//     pub page: u32,
// }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Compact per-chunk PDF location summary.
///
/// Extraction emits `Vec<ChunkLocation>` with one entry per page touched by
/// PDF-sourced spans. Each entry carries one to three union boxes for that
/// chunk on the page. Synthetic-only chunks, and chunks with no valid
/// PDF-sourced bounding boxes, emit an empty vector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkLocation {
    pub page: u32,
    pub bboxes: Vec<BoundingBox>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepairKind {
    HeaderGarbageStripped,
    TruncatedEofRepaired,
}

/// A password whose secret value is never exposed by formatting.
///
/// The string remains available to the parser through a private accessor, but
/// the wrapper prevents accidental `Debug`/`Display` logging at API seams.
#[derive(Clone, PartialEq, Eq)]
pub struct PdfPassword(String);

impl PdfPassword {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for PdfPassword {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for PdfPassword {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl std::fmt::Debug for PdfPassword {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.write_str("[redacted]")
    }
}

impl std::fmt::Display for PdfPassword {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.write_str("[redacted]")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionOptions {
    pub emit_output_spans: bool,
    pub max_pages: Option<usize>,
    pub max_objects: Option<usize>,
    pub max_recursion_depth: Option<usize>,
    pub max_decompressed_stream_bytes: Option<usize>,
    pub max_output_bytes: Option<usize>,
    /// Runtime-only secret. It is intentionally omitted from serde
    /// serialization and deserialization; configure it in memory.
    #[serde(skip)]
    pub password: Option<PdfPassword>,
    pub enable_repairs: bool,
}

impl std::fmt::Debug for ExtractionOptions {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("ExtractionOptions")
            .field("emit_output_spans", &self.emit_output_spans)
            .field("max_pages", &self.max_pages)
            .field("max_objects", &self.max_objects)
            .field("max_recursion_depth", &self.max_recursion_depth)
            .field(
                "max_decompressed_stream_bytes",
                &self.max_decompressed_stream_bytes,
            )
            .field("max_output_bytes", &self.max_output_bytes)
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .field("enable_repairs", &self.enable_repairs)
            .finish()
    }
}

impl Default for ExtractionOptions {
    fn default() -> Self {
        Self {
            emit_output_spans: false,
            max_pages: Some(10_000),
            max_objects: Some(1_000_000),
            max_recursion_depth: Some(8),
            max_decompressed_stream_bytes: Some(128 * 1024 * 1024),
            max_output_bytes: Some(128 * 1024 * 1024),
            password: None,
            enable_repairs: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PagePosition {
    pub page: u32,
    pub char_start: usize,
    pub char_end: usize,
    pub bbox: BoundingBox,
}

// Result structure for PDF extraction
#[derive(Debug)]
pub struct ExtractionResult {
    pub content_core: ContentCore,
    pub content_ext: ContentExt,
    pub repairs: Vec<RepairKind>,
}

pub(crate) struct LoadedPdf {
    pub document: Document,
    pub repairs: Vec<RepairKind>,
}

fn decompression_limit_error(limit: usize, observed: Option<usize>) -> OutputError {
    OutputError::resource_limit(ResourceLimitKind::DecompressedStreamBytes, limit, observed)
}

fn bounded_decompressed_content(
    stream: &Stream,
    remaining: usize,
    image_limit: usize,
) -> Result<Vec<u8>, OutputError> {
    // Image XObjects are decoded one at a time, so charging every raw bitmap
    // cumulatively rejects ordinary scanned volumes. Enforce the configured
    // limit against each image's decoded working size, then charge its stored
    // bytes to the cumulative document budget.
    if stream
        .dict
        .get(b"Subtype")
        .ok()
        .and_then(|value| value.as_name().ok())
        == Some(b"Image".as_slice())
    {
        let width = stream
            .dict
            .get(b"Width")
            .ok()
            .and_then(|value| value.as_i64().ok())
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(usize::MAX);
        let height = stream
            .dict
            .get(b"Height")
            .ok()
            .and_then(|value| value.as_i64().ok())
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(usize::MAX);
        let bits_per_component = stream
            .dict
            .get(b"BitsPerComponent")
            .ok()
            .and_then(|value| value.as_i64().ok())
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(8);
        let color_space_name = stream.dict.get(b"ColorSpace").ok().and_then(|value| {
            value.as_name().ok().or_else(|| {
                value
                    .as_array()
                    .ok()
                    .and_then(|values| values.first())
                    .and_then(|value| value.as_name().ok())
            })
        });
        let components = match color_space_name {
            Some(b"DeviceGray") | Some(b"Indexed") => 1,
            Some(b"DeviceRGB") => 3,
            Some(b"DeviceCMYK") => 4,
            _ => 4,
        };
        let predictor = stream
            .dict
            .get(b"DecodeParms")
            .ok()
            .and_then(|value| value.as_dict().ok())
            .and_then(|params| params.get(b"Predictor").ok())
            .and_then(|value| value.as_i64().ok())
            .is_some_and(|value| (10..=15).contains(&value));
        let working_set_bytes = image_memory_bounds(
            width,
            height,
            components,
            bits_per_component,
            predictor,
            stream.content.len(),
        )
        .map(|bounds| bounds.working_set_bytes)
        .unwrap_or(usize::MAX);
        if working_set_bytes > image_limit {
            return Err(decompression_limit_error(
                image_limit,
                Some(working_set_bytes),
            ));
        }
        if stream.content.len() > remaining {
            return Err(decompression_limit_error(
                remaining,
                Some(stream.content.len()),
            ));
        }
        return Ok(stream.content.clone());
    }

    let filters = match stream.filters() {
        Ok(filters) if !filters.is_empty() => filters,
        _ => {
            if stream.content.len() > remaining {
                return Err(decompression_limit_error(
                    remaining,
                    Some(stream.content.len()),
                ));
            }
            return Ok(stream.content.clone());
        }
    };

    let mut input = stream.content.clone();
    for filter in filters {
        let output = match filter {
            b"FlateDecode" => bounded_flate_decode(&input, remaining)?,
            b"LZWDecode" => bounded_lzw_decode(stream, &input, remaining)?,
            b"ASCII85Decode" => bounded_ascii85_decode(&input, remaining)?,
            _ => {
                // Image codecs such as DCT/JPX are intentionally opaque to
                // text extraction and are not inflated by lopdf's content
                // decoder. Bound their stored bytes without rejecting every
                // ordinary JPEG-backed PDF. All filters that this crate (or
                // lopdf) can inflate are handled above with bounded decoders.
                if input.len() > remaining {
                    return Err(decompression_limit_error(remaining, Some(input.len())));
                }
                input.clone()
            }
        };
        input = output;
    }
    Ok(input)
}

fn bounded_flate_decode(input: &[u8], limit: usize) -> Result<Vec<u8>, OutputError> {
    fn read_bounded<R: std::io::Read>(mut reader: R, limit: usize) -> Result<Vec<u8>, bool> {
        let mut output = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let read = reader.read(&mut buffer).map_err(|_| false)?;
            if read == 0 {
                return Ok(output);
            }
            if output.len().saturating_add(read) > limit {
                return Err(true);
            }
            output.extend_from_slice(&buffer[..read]);
        }
    }

    let zlib = flate2::read::ZlibDecoder::new(std::io::Cursor::new(input));
    match read_bounded(zlib, limit) {
        Ok(output) => Ok(output),
        Err(true) => Err(decompression_limit_error(
            limit,
            Some(limit.saturating_add(1)),
        )),
        Err(false) => {
            // Match lopdf's raw-deflate compatibility fallback without ever
            // allowing the fallback decoder to allocate beyond the budget.
            if input.len() > 2 {
                let raw = flate2::read::DeflateDecoder::new(std::io::Cursor::new(&input[2..]));
                match read_bounded(raw, limit) {
                    Ok(output) => Ok(output),
                    Err(true) => Err(decompression_limit_error(
                        limit,
                        Some(limit.saturating_add(1)),
                    )),
                    Err(false) => {
                        if input.len() > limit {
                            Err(decompression_limit_error(limit, Some(input.len())))
                        } else {
                            Ok(input.to_vec())
                        }
                    }
                }
            } else if input.len() > limit {
                Err(decompression_limit_error(limit, Some(input.len())))
            } else {
                Ok(input.to_vec())
            }
        }
    }
}

fn bounded_ascii85_decode(input: &[u8], limit: usize) -> Result<Vec<u8>, OutputError> {
    let mut output = Vec::new();
    let mut group = [0u8; 5];
    let mut group_len = 0usize;
    let flush = |group: &[u8; 5], count: usize, output: &mut Vec<u8>| {
        let mut value = 0u32;
        for digit in group.iter().take(5) {
            value = value
                .saturating_mul(85)
                .saturating_add((*digit - b'!') as u32);
        }
        let bytes = value.to_be_bytes();
        let count = if count == 5 {
            4
        } else {
            count.saturating_sub(1)
        };
        output.extend_from_slice(&bytes[..count]);
    };

    let mut index = 0usize;
    while index < input.len() {
        let byte = input[index];
        index += 1;
        if byte.is_ascii_whitespace() {
            continue;
        }
        if byte == b'~' && input.get(index) == Some(&b'>') {
            break;
        }
        if byte == b'z' {
            if group_len != 0 {
                break;
            }
            if output.len().saturating_add(4) > limit {
                return Err(decompression_limit_error(
                    limit,
                    Some(limit.saturating_add(1)),
                ));
            }
            output.extend_from_slice(&[0; 4]);
            continue;
        }
        if !(b'!'..=b'u').contains(&byte) {
            break;
        }
        group[group_len] = byte;
        group_len += 1;
        if group_len == 5 {
            if output.len().saturating_add(4) > limit {
                return Err(decompression_limit_error(
                    limit,
                    Some(limit.saturating_add(1)),
                ));
            }
            flush(&group, group_len, &mut output);
            group_len = 0;
        }
    }
    if group_len > 0 {
        for digit in group.iter_mut().skip(group_len) {
            *digit = b'u';
        }
        let decoded_len = group_len.saturating_sub(1);
        if output.len().saturating_add(decoded_len) > limit {
            return Err(decompression_limit_error(
                limit,
                Some(limit.saturating_add(1)),
            ));
        }
        flush(&group, group_len, &mut output);
    }
    Ok(output)
}

struct BudgetWriter {
    output: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl std::io::Write for BudgetWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.output.len().saturating_add(bytes.len()) > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "decompression budget exceeded",
            ));
        }
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bounded_lzw_decode(stream: &Stream, input: &[u8], limit: usize) -> Result<Vec<u8>, OutputError> {
    let early_change = stream
        .dict
        .get(b"DecodeParms")
        .ok()
        .and_then(|object| object.as_dict().ok())
        .and_then(|params| params.get(b"EarlyChange").ok())
        .and_then(|object| object.as_i64().ok())
        .map(|value| value != 0)
        .unwrap_or(true);
    let mut decoder = if early_change {
        weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
    } else {
        weezl::decode::Decoder::new(weezl::BitOrder::Msb, 8)
    };
    let mut writer = BudgetWriter {
        output: Vec::new(),
        limit,
        exceeded: false,
    };
    let _ = decoder.into_stream(&mut writer).decode_all(input);
    if writer.exceeded {
        Err(decompression_limit_error(
            limit,
            Some(limit.saturating_add(1)),
        ))
    } else {
        Ok(writer.output)
    }
}

pub(crate) fn validate_document_budgets(
    document: &Document,
    options: &ExtractionOptions,
) -> Result<(), OutputError> {
    if let Some(limit) = options.max_pages {
        let pages = document.get_pages().len();
        if pages > limit {
            return Err(OutputError::resource_limit(
                ResourceLimitKind::Pages,
                limit,
                Some(pages),
            ));
        }
    }
    if let Some(limit) = options.max_objects {
        let indexed_objects = document
            .reference_table
            .entries
            .values()
            .filter(|entry| {
                matches!(
                    entry,
                    lopdf::xref::XrefEntry::Normal { .. }
                        | lopdf::xref::XrefEntry::Compressed { .. }
                )
            })
            .count();
        let objects = indexed_objects.max(document.objects.len());
        if objects > limit {
            return Err(OutputError::resource_limit(
                ResourceLimitKind::Objects,
                limit,
                Some(objects),
            ));
        }
    }
    if let Some(limit) = options.max_decompressed_stream_bytes {
        accounted_stream_bytes(document, limit)?;
    }
    Ok(())
}

fn accounted_stream_bytes(document: &Document, limit: usize) -> Result<usize, OutputError> {
    let mut total = 0usize;
    for object in document.objects.values() {
        let Object::Stream(stream) = object else {
            continue;
        };
        let remaining = limit.saturating_sub(total);
        let decompressed = match bounded_decompressed_content(stream, remaining, limit) {
            Ok(decompressed) => decompressed,
            Err(OutputError::ResourceLimit {
                kind: ResourceLimitKind::DecompressedStreamBytes,
                observed,
                ..
            }) => {
                return Err(OutputError::resource_limit(
                    ResourceLimitKind::DecompressedStreamBytes,
                    limit,
                    observed.map(|value| total.saturating_add(value)),
                ));
            }
            Err(error) => return Err(error),
        };
        total = total.saturating_add(decompressed.len());
        if total > limit {
            return Err(OutputError::resource_limit(
                ResourceLimitKind::DecompressedStreamBytes,
                limit,
                Some(total),
            ));
        }
    }
    Ok(total)
}

/// Resource usage charged to the decompressed-stream budget during preflight.
///
/// Page content, forms, fonts, and other non-image Flate, LZW, and ASCII85 streams
/// are decoded. Image XObjects are charged at stored size because extraction decodes
/// them one at a time, but each image's decoded bitmap/RGB working size must also fit
/// the hard limit. This is therefore the cumulative stream total; image working-set
/// limits are enforced independently rather than summed across every scanned page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PdfDecompressionUsage {
    pub input_bytes: usize,
    pub accounted_stream_bytes: usize,
    pub expansion_ratio: f64,
    pub pages: usize,
    pub objects: usize,
}

/// Measure the exact stream total enforced by extraction, with a caller-supplied
/// hard stop so this diagnostic cannot itself expand a decompression bomb without
/// bound.
pub fn measure_pdf_decompression<P: AsRef<std::path::Path>>(
    path: P,
    hard_limit: usize,
) -> Result<PdfDecompressionUsage, OutputError> {
    let bytes = std::fs::read(path).map_err(OutputError::from)?;
    let mut options = ExtractionOptions::default();
    options.max_decompressed_stream_bytes = None;
    let loaded = load_pdf_from_mem(&bytes, &options)?;
    let accounted = accounted_stream_bytes(&loaded.document, hard_limit)?;
    let input_bytes = bytes.len();
    Ok(PdfDecompressionUsage {
        input_bytes,
        accounted_stream_bytes: accounted,
        expansion_ratio: if input_bytes == 0 {
            0.0
        } else {
            accounted as f64 / input_bytes as f64
        },
        pages: loaded.document.get_pages().len(),
        objects: loaded.document.objects.len(),
    })
}

fn pdf_name_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(byte, b'/' | b'%' | b'[' | b']' | b'<' | b'>' | b'{' | b'}')
}

fn append_missing_eof(bytes: &mut Vec<u8>) {
    if !bytes.last().is_some_and(|byte| byte.is_ascii_whitespace()) {
        bytes.push(b'\n');
    }
    bytes.extend_from_slice(b"%%EOF\n");
}

/// Estimate page objects without invoking the PDF parser.
///
/// This is deliberately a bounded, single-pass byte scan used only to make a
/// total parse failure actionable. It counts `/Type /Page` and excludes the
/// plural `/Pages` node by requiring a name delimiter after `Page`.
pub fn estimate_page_count(bytes: &[u8]) -> usize {
    let mut count = 0usize;
    let mut index = 0usize;
    while index + 5 <= bytes.len() {
        if &bytes[index..index + 5] != b"/Type" {
            index += 1;
            continue;
        }
        let mut cursor = index + 5;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor + 5 <= bytes.len()
            && &bytes[cursor..cursor + 5] == b"/Page"
            && (cursor + 5 == bytes.len() || pdf_name_delimiter(bytes[cursor + 5]))
        {
            count += 1;
            index = cursor + 5;
        } else {
            index += 5;
        }
    }
    count
}

fn decrypt_document(document: &mut Document, password: Option<&str>) -> Result<(), OutputError> {
    if !document.is_encrypted() {
        return Ok(());
    }

    let encrypted = document
        .get_encrypted()
        .map_err(|_| OutputError::Encrypted {
            reason: EncryptionFailure::InvalidDictionary,
            context: ErrorContext {
                phase: Some("decrypt".to_string()),
                ..ErrorContext::default()
            },
        })?;
    match encrypted.get(b"Filter").and_then(Object::as_name) {
        Ok(b"Standard") => {}
        Ok(_) => {
            return Err(OutputError::Encrypted {
                reason: EncryptionFailure::UnsupportedHandler,
                context: ErrorContext {
                    phase: Some("decrypt".to_string()),
                    ..ErrorContext::default()
                },
            });
        }
        Err(_) => {
            return Err(OutputError::Encrypted {
                reason: EncryptionFailure::InvalidDictionary,
                context: ErrorContext {
                    phase: Some("decrypt".to_string()),
                    ..ErrorContext::default()
                },
            });
        }
    }

    match document.decrypt(password.unwrap_or("")) {
        Ok(()) => Ok(()),
        Err(lopdf::Error::Decryption(DecryptionError::IncorrectPassword)) if password.is_none() => {
            Err(OutputError::Encrypted {
                reason: EncryptionFailure::PasswordRequired,
                context: ErrorContext {
                    phase: Some("decrypt".to_string()),
                    ..ErrorContext::default()
                },
            })
        }
        Err(error) => {
            let invalid_dictionary = matches!(
                &error,
                lopdf::Error::DictKey(_)
                    | lopdf::Error::DictType { .. }
                    | lopdf::Error::ObjectType { .. }
                    | lopdf::Error::Decryption(
                        DecryptionError::MissingEncryptDictionary
                            | DecryptionError::MissingVersion
                            | DecryptionError::MissingRevision
                            | DecryptionError::MissingOwnerPassword
                            | DecryptionError::MissingUserPassword
                            | DecryptionError::MissingPermissions
                            | DecryptionError::MissingFileID
                            | DecryptionError::InvalidHashLength
                            | DecryptionError::InvalidKeyLength
                            | DecryptionError::InvalidCipherTextLength
                            | DecryptionError::InvalidPermissionLength
                            | DecryptionError::InvalidVersion
                            | DecryptionError::InvalidRevision
                            | DecryptionError::InvalidType
                            | DecryptionError::NotDecryptable
                    )
            );
            if invalid_dictionary {
                Err(OutputError::Encrypted {
                    reason: EncryptionFailure::InvalidDictionary,
                    context: ErrorContext {
                        phase: Some("decrypt".to_string()),
                        ..ErrorContext::default()
                    },
                })
            } else {
                Err(OutputError::from(error))
            }
        }
    }
}

fn load_pdf_bytes_once(bytes: &[u8], password: Option<&str>) -> Result<Document, OutputError> {
    // Supplying a password to lopdf's loader is important: encrypted PDFs
    // keep ordinary objects out of `Document::objects` until authentication
    // succeeds. A post-load `Document::decrypt` cannot recover those raw
    // objects because it only walks the already-materialized map.
    let mut document = match password {
        Some(password) => {
            Document::load_mem_with_options(bytes, lopdf::LoadOptions::with_password(password))
                .map_err(OutputError::from)?
        }
        None => Document::load_mem(bytes).map_err(OutputError::from)?,
    };
    // Encrypted documents intentionally do not materialize ordinary objects
    // until authentication succeeds. Decrypt first; otherwise the catalog
    // probe below misclassifies a missing password as a missing object.
    decrypt_document(&mut document, password)?;
    // lopdf 0.42 intentionally keeps ordinary objects lazy. Force the
    // catalog path here so a deep nested object cannot hide behind a
    // successful xref load and reach extraction later.
    if let Ok(root) = document.trailer.get(b"Root").and_then(Object::as_reference) {
        document.get_object(root).map_err(OutputError::from)?;
    }
    Ok(document)
}

pub(crate) fn load_pdf_from_mem(
    bytes: &[u8],
    options: &ExtractionOptions,
) -> Result<LoadedPdf, OutputError> {
    let estimated_pages = estimate_page_count(bytes);
    let header_offset = bytes.windows(5).position(|window| window == b"%PDF-");
    let should_repair_eof =
        header_offset.is_some() && !bytes.windows(5).any(|window| window == b"%%EOF");

    match load_pdf_bytes_once(bytes, options.password.as_ref().map(PdfPassword::as_str)) {
        Ok(document) => {
            validate_document_budgets(&document, options)?;
            Ok(LoadedPdf {
                document,
                repairs: Vec::new(),
            })
        }
        Err(first_error) if !options.enable_repairs => {
            Err(first_error.with_estimated_pages(estimated_pages))
        }
        Err(first_error) => {
            let first_error = first_error.with_estimated_pages(estimated_pages);
            let mut repaired = bytes.to_vec();
            let mut repairs = Vec::new();
            if let Some(header) = header_offset.filter(|offset| *offset > 0) {
                // Original bytes always get the first parse attempt. A
                // repair is only a fallback, because a valid file may carry
                // harmless prefix bytes while retaining self-consistent xref
                // offsets that the parser can already handle.
                repaired.drain(..header);
                repairs.push(RepairKind::HeaderGarbageStripped);
            }
            if should_repair_eof {
                append_missing_eof(&mut repaired);
                repairs.push(RepairKind::TruncatedEofRepaired);
            }
            if repairs.is_empty() {
                Err(first_error)
            } else {
                match load_pdf_bytes_once(
                    &repaired,
                    options.password.as_ref().map(PdfPassword::as_str),
                ) {
                    Ok(document) => {
                        validate_document_budgets(&document, options)
                            .map_err(|error| error.with_repair_attempted(true))?;
                        Ok(LoadedPdf { document, repairs })
                    }
                    Err(_) => {
                        // Preserve the semantic error from the original
                        // bytes. In particular, a repair attempt must not
                        // turn PasswordRequired into InvalidStructure.
                        Err(first_error.with_repair_attempted(true))
                    }
                }
            }
        }
    }
}

pub(crate) fn load_pdf_from_path(
    path: impl AsRef<std::path::Path>,
    options: &ExtractionOptions,
) -> Result<LoadedPdf, OutputError> {
    let bytes = std::fs::read(path).map_err(OutputError::from)?;
    load_pdf_from_mem(&bytes, options)
}

// Font weight enum (needed in main processing)
#[derive(Debug, Clone, PartialEq)]
pub enum FontWeight {
    Thin,       // 100
    ExtraLight, // 200
    Light,      // 300
    Regular,    // 400
    Medium,     // 500
    SemiBold,   // 600
    Bold,       // 700
    ExtraBold,  // 800
    Black,      // 900
}

impl FontWeight {
    pub fn from_font_name(font_name: &str) -> Self {
        let name_lower = font_name.to_lowercase();

        // Check for weight keywords in order of specificity
        if name_lower.contains("thin") || name_lower.contains("hairline") {
            FontWeight::Thin
        } else if name_lower.contains("extralight") || name_lower.contains("ultralight") {
            FontWeight::ExtraLight
        } else if name_lower.contains("light") {
            FontWeight::Light
        } else if name_lower.contains("black") || name_lower.contains("heavy") {
            FontWeight::Black
        } else if name_lower.contains("extrabold") || name_lower.contains("ultrabold") {
            FontWeight::ExtraBold
        } else if name_lower.contains("bold") || name_lower.contains("demi") {
            FontWeight::Bold
        } else if name_lower.contains("semibold") || name_lower.contains("demibold") {
            FontWeight::SemiBold
        } else if name_lower.contains("medium") {
            FontWeight::Medium
        } else if name_lower.contains("regular")
            || name_lower.contains("normal")
            || name_lower.contains("book")
        {
            FontWeight::Regular
        } else {
            // Default to regular if no weight indicator found
            FontWeight::Regular
        }
    }

    pub fn to_numeric(&self) -> u16 {
        match self {
            FontWeight::Thin => 100,
            FontWeight::ExtraLight => 200,
            FontWeight::Light => 300,
            FontWeight::Regular => 400,
            FontWeight::Medium => 500,
            FontWeight::SemiBold => 600,
            FontWeight::Bold => 700,
            FontWeight::ExtraBold => 800,
            FontWeight::Black => 900,
        }
    }

    pub fn is_bold(&self) -> bool {
        self.to_numeric() >= 600
    }
}

// Font analysis structures
#[derive(Debug, Clone)]
pub struct FontInfo {
    pub name: String,
    pub family: String,
    pub weight: FontWeight,
    pub is_italic: bool,
}

impl FontInfo {
    pub fn from_font_name(font_name: &str) -> Self {
        let weight = FontWeight::from_font_name(font_name);
        let is_italic = font_name.to_lowercase().contains("italic")
            || font_name.to_lowercase().contains("oblique");

        // Try to extract font family by removing weight and style indicators
        let family = Self::extract_font_family(font_name);

        FontInfo {
            name: font_name.to_string(),
            family,
            weight,
            is_italic,
        }
    }

    fn extract_font_family(font_name: &str) -> String {
        // Remove common weight and style suffixes
        let suffixes = [
            "-Bold", "-Regular", "-Light", "-Medium", "-Heavy", "-Black", "-Italic", "-Oblique",
            "-Roman", "-Book", "-Demi", "-Semi", "Bold", "Regular", "Light", "Medium", "Heavy",
            "Black", "Italic", "Oblique", "Roman", "Book", "Demi", "Semi",
        ];

        let mut family = font_name.to_string();
        for suffix in &suffixes {
            if family.ends_with(suffix) {
                family = family[..family.len() - suffix.len()].to_string();
                family = family.trim_end_matches('-').to_string();
            }
        }

        family
    }
}

#[derive(Clone)]
pub struct LayoutAnalyzer {
    last_y: f64,
    y_gap_threshold: f64,
    page_height: f64,
}

fn normalize_for_duplicate_detection(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn bbox_overlap_ratio(
    ax0: f64,
    ay0: f64,
    ax1: f64,
    ay1: f64,
    bx0: f64,
    by0: f64,
    bx1: f64,
    by1: f64,
) -> f64 {
    let inter_w = (ax1.min(bx1) - ax0.max(bx0)).max(0.0);
    let inter_h = (ay1.min(by1) - ay0.max(by0)).max(0.0);
    let inter = inter_w * inter_h;
    if inter <= 0.0 {
        return 0.0;
    }
    let area_a = ((ax1 - ax0).max(0.0)) * ((ay1 - ay0).max(0.0));
    let area_b = ((bx1 - bx0).max(0.0)) * ((by1 - by0).max(0.0));
    let denom = area_a.min(area_b).max(1e-6);
    inter / denom
}

impl LayoutAnalyzer {
    pub fn new(page_height: f64) -> Self {
        Self {
            last_y: f64::MAX,
            y_gap_threshold: 0.0,
            page_height,
        }
    }

    pub fn update_threshold(&mut self, font_size: f64) {
        // Dynamic gap threshold based on font size and page dimensions
        self.y_gap_threshold = (font_size * 1.5).max(self.page_height * 0.03);
    }

    pub fn is_section_break(&mut self, current_y: f64) -> bool {
        let abs_gap = (current_y - self.last_y).abs();
        let is_large_gap = abs_gap > self.y_gap_threshold * 2.5;
        self.last_y = current_y;
        is_large_gap
    }
}

// Helper functions for creating new schema structures
pub fn create_content_core(
    content: &str,
    headings: &[String],
    source_id: i64,
    source_type: &str,
) -> ContentCore {
    create_content_core_with_identity(content, headings, source_id, source_type, None, None, 0)
}

pub fn create_content_core_with_identity(
    content: &str,
    headings: &[String],
    source_id: i64,
    source_type: &str,
    page_start: Option<u32>,
    char_start: Option<usize>,
    ordinal: usize,
) -> ContentCore {
    create_content_core_with_token_mode(
        content,
        headings,
        source_id,
        source_type,
        page_start,
        char_start,
        ordinal,
        TokenCountMode::Exact,
    )
}

pub(crate) fn create_content_core_with_token_mode(
    content: &str,
    headings: &[String],
    source_id: i64,
    source_type: &str,
    page_start: Option<u32>,
    char_start: Option<usize>,
    ordinal: usize,
    token_count_mode: TokenCountMode,
) -> ContentCore {
    use blake3::Hasher;

    let mut content_hasher = Hasher::new();
    content_hasher.update(content.as_bytes());
    let content_hash = content_hasher.finalize().to_hex().to_string();

    let mut id_hasher = Hasher::new();
    id_hasher.update(source_type.as_bytes());
    id_hasher.update(b":");
    id_hasher.update(source_id.to_string().as_bytes());
    id_hasher.update(b":");
    id_hasher.update(page_start.unwrap_or(0).to_string().as_bytes());
    id_hasher.update(b":");
    id_hasher.update(char_start.unwrap_or(0).to_string().as_bytes());
    id_hasher.update(b":");
    id_hasher.update(ordinal.to_string().as_bytes());
    id_hasher.update(b":");
    id_hasher.update(content_hash.as_bytes());
    let chunk_id = id_hasher.finalize().to_hex().to_string();

    // Serialize headings to JSON
    let headings_json = if !headings.is_empty() {
        Some(serde_json::to_string(headings).unwrap_or_default())
    } else {
        None
    };

    let token_count = match token_count_mode {
        TokenCountMode::Approximate => estimate_tokens_from_words(count_words(content), 1.3) as i32,
        TokenCountMode::Exact => CONTENT_CORE_TOKENIZER
            .as_ref()
            .map(|tokenizer| tokenizer.encode_ordinary(content).len() as i32)
            .unwrap_or_else(|| estimate_tokens_from_words(count_words(content), 1.3) as i32),
    };

    ContentCore {
        chunk_id,
        content_hash,
        source_id,
        source_type: source_type.to_string(),
        content: content.to_string(),
        token_count,
        headings_json,
        status: "extracted".to_string(),
        schema_version: 2,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
    }
}

pub fn create_content_ext(
    chunk_id: &str,
    format_location: &FormatLocation,
    page_char_start: Option<usize>,
    page_char_end: Option<usize>,
    bbox: Option<&BoundingBox>,
) -> Result<ContentExt, OutputError> {
    create_content_ext_with_spans(
        chunk_id,
        format_location,
        page_char_start,
        page_char_end,
        bbox,
        None,
        ExtractionOptions::default(),
    )
}

pub fn create_content_ext_with_spans(
    chunk_id: &str,
    format_location: &FormatLocation,
    page_char_start: Option<usize>,
    page_char_end: Option<usize>,
    bbox: Option<&BoundingBox>,
    located_text: Option<&crate::document::LocatedText>,
    options: ExtractionOptions,
) -> Result<ContentExt, OutputError> {
    #[derive(serde::Serialize)]
    struct ContentExtPayload<'a> {
        format_location: &'a FormatLocation,
        page_char_start: Option<usize>,
        page_char_end: Option<usize>,
        bbox: Option<&'a BoundingBox>,
        chunk_locations: &'a [ChunkLocation],
        #[serde(skip_serializing_if = "Option::is_none")]
        output_spans: Option<&'a [crate::document::OutputSpan]>,
        output_text_len: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        decode_quality: Option<&'a DecodeQualityMetrics>,
        location_model: ContentExtLocationModel<'a>,
    }

    #[derive(serde::Serialize)]
    struct ContentExtLocationModel<'a> {
        source_span_granularity: &'a str,
        synthetic_spans: &'a str,
        notes: &'a str,
    }

    let compacted_spans = located_text.map(|located| {
        // Detailed spans are not serialized in the default mode.
        // Keep chunk_locations below based on the original spans in both modes.
        if !options.emit_output_spans || located.spans.is_empty() {
            Vec::new()
        } else {
            crate::document::compact_output_spans(&located.spans)
        }
    });
    let chunk_locations = located_text
        .map(|located| crate::document::chunk_locations_from_output_spans(&located.spans))
        .unwrap_or_default();
    let output_spans = if options.emit_output_spans {
        compacted_spans.as_deref()
    } else {
        None
    };
    let source_span_granularity = if options.emit_output_spans && located_text.is_some() {
        "output_span"
    } else if located_text.is_some() {
        "chunk_location"
    } else {
        "segment"
    };
    let synthetic_spans = if options.emit_output_spans && located_text.is_some() {
        "typed"
    } else if located_text.is_some() {
        "omitted"
    } else {
        "coarse"
    };
    let notes = if options.emit_output_spans {
        "chunk_locations summarize PDF-backed fragments; output_spans preserve PDF-backed fragments plus typed synthetic spans"
    } else {
        "chunk_locations summarize PDF-backed fragments; synthetic spans do not contribute bboxes"
    };
    let decode_quality = located_text.map(|located| assess_decode_quality(&located.text));
    let ext_data = ContentExtPayload {
        format_location,
        page_char_start,
        page_char_end,
        bbox,
        chunk_locations: &chunk_locations,
        output_spans,
        output_text_len: located_text.map(|located| located.text.chars().count()),
        decode_quality: decode_quality.as_ref(),
        location_model: ContentExtLocationModel {
            source_span_granularity,
            synthetic_spans,
            notes,
        },
    };

    // Serialize to JSON and compress with zstd
    let json_bytes = serde_json::to_vec(&ext_data).map_err(|error| OutputError::Format {
        message: error.to_string(),
        context: ErrorContext::default(),
    })?;
    let compressed = zstd::bulk::compress(&json_bytes, 3)?;

    Ok(ContentExt {
        chunk_id: chunk_id.to_string(),
        ext_json: compressed,
    })
}

/// Helper function to create PDF location from page positions
pub fn create_pdf_location_from_positions(
    page_start: u32,
    page_end: Option<u32>,
    page_positions: &[PagePosition],
    content_len: usize,
) -> FormatLocation {
    // Convert page positions to fragments
    let fragments: Vec<PageFragment> = page_positions
        .iter()
        .filter_map(|pos| {
            if !pos.bbox.x.is_finite()
                || !pos.bbox.y.is_finite()
                || !pos.bbox.width.is_finite()
                || !pos.bbox.height.is_finite()
            {
                return None;
            }
            Some(PageFragment {
                page: pos.page,
                char_range: CharRange {
                    start: pos.char_start,
                    end: pos.char_end,
                },
                bbox: BoundingBox {
                    x: pos.bbox.x,
                    y: pos.bbox.y,
                    width: pos.bbox.width.max(0.01),
                    height: pos.bbox.height.max(0.01),
                },
            })
        })
        .collect();

    FormatLocation::Pdf(PdfLocation { fragments })
}

pub fn create_pdf_location_from_output_spans(
    located: &crate::document::LocatedText,
    fallback_positions: &[PagePosition],
) -> FormatLocation {
    let mut spans = located.spans.clone();
    spans.sort_by_key(|span| (span_pdf_page(span).unwrap_or(u32::MAX), span.output_start));

    let mut fragments: Vec<PageFragment> = Vec::new();
    for span in spans {
        let crate::document::SpanSource::Pdf { page, bbox, .. } = span.source else {
            continue;
        };
        if !valid_bbox(&bbox) || span.output_start >= span.output_end {
            continue;
        }

        if let Some(last) = fragments.last_mut() {
            if last.page == page
                && last.char_range.end == span.output_start
                && spatially_compatible_for_fragment_merge(&last.bbox, &bbox)
            {
                last.char_range.end = span.output_end;
                last.bbox = bbox_union(&last.bbox, &bbox);
                continue;
            }
        }

        fragments.push(PageFragment {
            page,
            char_range: CharRange {
                start: span.output_start,
                end: span.output_end,
            },
            bbox: BoundingBox {
                x: bbox.x,
                y: bbox.y,
                width: bbox.width.max(0.01),
                height: bbox.height.max(0.01),
            },
        });
    }

    if fragments.is_empty() {
        return create_pdf_location_from_positions(
            0,
            None,
            fallback_positions,
            located.text.chars().count(),
        );
    }

    FormatLocation::Pdf(PdfLocation { fragments })
}

fn span_pdf_page(span: &crate::document::OutputSpan) -> Option<u32> {
    match &span.source {
        crate::document::SpanSource::Pdf { page, .. } => Some(*page),
        crate::document::SpanSource::Synthetic { .. } => None,
    }
}

fn valid_bbox(bbox: &BoundingBox) -> bool {
    bbox.x.is_finite()
        && bbox.y.is_finite()
        && bbox.width.is_finite()
        && bbox.height.is_finite()
        && bbox.width >= 0.0
        && bbox.height >= 0.0
}

fn spatially_compatible_for_fragment_merge(a: &BoundingBox, b: &BoundingBox) -> bool {
    if !valid_bbox(a) || !valid_bbox(b) {
        return false;
    }
    let tolerance = a.height.max(b.height).max(1.0) * 0.05;
    (a.x - b.x).abs() <= tolerance
        && (a.y - b.y).abs() <= tolerance
        && (a.width - b.width).abs() <= tolerance
        && (a.height - b.height).abs() <= tolerance
}

fn bbox_union(a: &BoundingBox, b: &BoundingBox) -> BoundingBox {
    let min_x = a.x.min(b.x);
    let min_y = a.y.min(b.y);
    let max_x = (a.x + a.width).max(b.x + b.width);
    let max_y = (a.y + a.height).max(b.y + b.height);
    BoundingBox {
        x: min_x,
        y: min_y,
        width: (max_x - min_x).max(0.01),
        height: (max_y - min_y).max(0.01),
    }
}

pub const CONTENT_EXT_DECOMPRESS_LIMIT_BYTES: usize = 16 * 1024 * 1024;

pub fn decompress_content_ext_bytes(content_ext: &ContentExt) -> Result<Vec<u8>, OutputError> {
    zstd::bulk::decompress(&content_ext.ext_json, CONTENT_EXT_DECOMPRESS_LIMIT_BYTES).map_err(
        |error| OutputError::InvalidStructure {
            message: format!("invalid ContentExt compression: {error}"),
            context: ErrorContext::default(),
        },
    )
}

/// Helper function to decompress and deserialize ContentExt data.
pub fn decompress_content_ext(content_ext: &ContentExt) -> Result<serde_json::Value, OutputError> {
    let decompressed = decompress_content_ext_bytes(content_ext)?;
    let json_value: serde_json::Value =
        serde_json::from_slice(&decompressed).map_err(|error| OutputError::InvalidStructure {
            message: format!("invalid ContentExt JSON: {error}"),
            context: ErrorContext::default(),
        })?;
    Ok(json_value)
}

pub fn extract_chunk_locations(
    content_ext: &ContentExt,
) -> Result<Vec<ChunkLocation>, OutputError> {
    let json_data = decompress_content_ext(content_ext)?;
    let Some(chunk_locations) = json_data.get("chunk_locations") else {
        return Err(OutputError::InvalidStructure {
            message: "no chunk_locations data found in ContentExt".to_string(),
            context: ErrorContext::default(),
        });
    };
    serde_json::from_value(chunk_locations.clone()).map_err(|error| OutputError::InvalidStructure {
        message: format!("invalid chunk_locations data in ContentExt: {error}"),
        context: ErrorContext::default(),
    })
}

pub fn pdf_page_dimensions(doc: &Document) -> HashMap<u32, (f64, f64)> {
    let mut dimensions = HashMap::new();
    for (page_num, object_id) in doc.get_pages() {
        let Ok(page_obj) = doc.get_object(object_id) else {
            continue;
        };
        let Ok(page_dict) = page_obj.as_dict() else {
            continue;
        };
        let Some(media_box): Option<Vec<f64>> = get_inherited(doc, page_dict, b"MediaBox") else {
            continue;
        };
        if media_box.len() >= 4 {
            dimensions.insert(
                page_num,
                (
                    (media_box[2] - media_box[0]).abs(),
                    (media_box[3] - media_box[1]).abs(),
                ),
            );
        }
    }
    dimensions
}

pub fn production_telemetry_for_results(
    results: &[ExtractionResult],
    max_tokens: Option<usize>,
    layout_enabled: bool,
    all_texts_enabled: bool,
    layout_fallback_used: bool,
    layout_normalized_chars: Option<usize>,
    no_layout_normalized_chars: Option<usize>,
    page_dimensions: &HashMap<u32, (f64, f64)>,
    ocr: &OcrImageTelemetrySnapshot,
) -> ProductionTelemetry {
    let quality = assess_parse_quality(results);
    let mut out = ProductionTelemetry {
        layout_enabled,
        all_texts_enabled,
        layout_fallback_used,
        layout_normalized_chars,
        no_layout_normalized_chars,
        layout_to_no_layout_normalized_char_ratio: layout_normalized_chars
            .zip(no_layout_normalized_chars)
            .and_then(|(layout, no_layout)| {
                if no_layout == 0 {
                    None
                } else {
                    Some(layout as f64 / no_layout as f64)
                }
            }),
        chunk_count: results.len(),
        extracted_chars: quality.extracted_chars,
        normalized_chars: quality.normalized_chars,
        repeated_line_ratio: quality.repeated_line_ratio,
        unique_token_ratio: quality.unique_token_ratio,
        parse_quality_status: serde_json::to_value(&quality.status)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string()),
        parse_quality_score: quality.score,
        decode_confidence: quality.decode_confidence,
        mojibake_char_ratio: quality.mojibake_char_ratio,
        symbol_char_ratio: quality.symbol_char_ratio,
        non_ascii_symbol_char_ratio: quality.non_ascii_symbol_char_ratio,
        suspicious_chunk_ratio: quality.suspicious_chunk_ratio,
        ocr_images_seen: ocr.images_seen,
        ocr_images_converted: ocr.images_converted,
        ocr_images_skipped: ocr.images_skipped,
        ocr_images_with_text: ocr.images_with_text,
        ocr_text_chars_emitted: ocr.text_chars_emitted,
        ..ProductionTelemetry::default()
    };

    let max_tokens = max_tokens.unwrap_or(usize::MAX);
    let mut output_chars_total = 0usize;
    let mut output_chars_pdf_backed = 0usize;
    let mut output_chars_synthetic = 0usize;

    for result in results {
        let tokens = result.content_core.token_count.max(0) as usize;
        out.max_chunk_tokens = out.max_chunk_tokens.max(tokens);
        if tokens > max_tokens {
            out.over_cap_chunks += 1;
        }

        let chunk_locations = extract_chunk_locations(&result.content_ext).unwrap_or_default();
        if chunk_locations.is_empty() {
            out.chunks_without_location += 1;
        }
        for location in &chunk_locations {
            for bbox in &location.bboxes {
                if !valid_bbox(bbox) {
                    out.invalid_bbox_count += 1;
                }
                if page_dimensions
                    .get(&location.page)
                    .map(|dimensions| bbox_outside_page(bbox, *dimensions))
                    .unwrap_or(false)
                {
                    out.out_of_page_bbox_count += 1;
                }
            }
        }

        out.content_ext_max_compressed_bytes = out
            .content_ext_max_compressed_bytes
            .max(result.content_ext.ext_json.len());
        out.content_ext_total_compressed_bytes += result.content_ext.ext_json.len();

        let output_chars = result.content_core.content.chars().count();
        output_chars_total += output_chars;
        if let Ok(bytes) = decompress_content_ext_bytes(&result.content_ext) {
            out.content_ext_max_uncompressed_bytes =
                out.content_ext_max_uncompressed_bytes.max(bytes.len());
            out.content_ext_total_uncompressed_bytes += bytes.len();
            if let Ok(ext) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(spans) = ext.get("output_spans").and_then(|value| value.as_array()) {
                    let mut coverage = vec![0usize; output_chars];
                    let mut pdf_coverage = vec![false; output_chars];
                    let mut synthetic_coverage = vec![false; output_chars];
                    for span in spans {
                        let start = span
                            .get("output_start")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(0) as usize;
                        let end = span
                            .get("output_end")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(start as u64) as usize;
                        let start = start.min(output_chars);
                        let end = end.min(output_chars);
                        if start >= end {
                            continue;
                        }
                        let source = span.get("source");
                        let is_pdf = source.and_then(|source| source.get("Pdf")).is_some();
                        let is_synthetic =
                            source.and_then(|source| source.get("Synthetic")).is_some();
                        for idx in start..end {
                            coverage[idx] += 1;
                            if is_pdf {
                                pdf_coverage[idx] = true;
                            }
                            if is_synthetic {
                                synthetic_coverage[idx] = true;
                            }
                        }
                        if is_pdf {
                            if let Some(pdf_source) = source.and_then(|source| source.get("Pdf")) {
                                if let Some(span_bbox) = pdf_source.get("bbox") {
                                    let invalid = match json_bbox(span_bbox) {
                                        Some(bbox) => !valid_bbox(&bbox),
                                        None => true,
                                    };
                                    if invalid {
                                        out.invalid_bbox_count += 1;
                                    }
                                } else {
                                    out.invalid_bbox_count += 1;
                                }
                            }
                        }
                    }
                    out.chars_without_span += coverage.iter().filter(|count| **count == 0).count();
                    out.chars_with_overlapping_spans +=
                        coverage.iter().filter(|count| **count > 1).count();
                    output_chars_pdf_backed +=
                        pdf_coverage.iter().filter(|covered| **covered).count();
                    output_chars_synthetic += synthetic_coverage
                        .iter()
                        .filter(|covered| **covered)
                        .count();
                }
            }
        }
    }

    let nonsynthetic_chars = output_chars_total.saturating_sub(output_chars_synthetic);
    out.pdf_backed_nonsynthetic_char_ratio = if nonsynthetic_chars == 0 {
        0.0
    } else {
        output_chars_pdf_backed as f64 / nonsynthetic_chars as f64
    };
    out.synthetic_char_ratio = if output_chars_total == 0 {
        0.0
    } else {
        output_chars_synthetic as f64 / output_chars_total as f64
    };
    out
}

fn json_bbox(value: &serde_json::Value) -> Option<BoundingBox> {
    Some(BoundingBox {
        x: value.get("x")?.as_f64()?,
        y: value.get("y")?.as_f64()?,
        width: value.get("width")?.as_f64()?,
        height: value.get("height")?.as_f64()?,
    })
}

fn bbox_outside_page(bbox: &BoundingBox, dimensions: (f64, f64)) -> bool {
    let (width, height) = dimensions;
    let tolerance = 1.0;
    bbox.x < -tolerance
        || bbox.y < -tolerance
        || bbox.x + bbox.width > width + tolerance
        || bbox.y + bbox.height > height + tolerance
}

/// Helper function to extract PdfLocation from ContentExt
pub fn extract_pdf_location(content_ext: &ContentExt) -> Result<PdfLocation, OutputError> {
    let json_data = decompress_content_ext(content_ext)?;

    if let Some(format_location) = json_data.get("format_location") {
        // Check if it's the new format structure
        if let Some(format_type) = format_location.get("format") {
            if format_type.as_str() == Some("Pdf") {
                // Extract fragments directly from format_location
                if let Some(fragments) = format_location.get("fragments") {
                    let pdf_location = PdfLocation {
                        fragments: serde_json::from_value(fragments.clone()).map_err(|error| {
                            OutputError::InvalidStructure {
                                message: format!("invalid PDF location fragments: {error}"),
                                context: ErrorContext::default(),
                            }
                        })?,
                    };
                    return Ok(pdf_location);
                }
            }
        } else if let Some(pdf_data) = format_location.get("Pdf") {
            // Legacy format - tagged enum style
            let pdf_location: PdfLocation =
                serde_json::from_value(pdf_data.clone()).map_err(|error| {
                    OutputError::InvalidStructure {
                        message: format!("invalid PDF location data: {error}"),
                        context: ErrorContext::default(),
                    }
                })?;
            return Ok(pdf_location);
        }
    }

    Err(OutputError::InvalidStructure {
        message: "no PDF location data found in ContentExt".to_string(),
        context: ErrorContext::default(),
    })
}
