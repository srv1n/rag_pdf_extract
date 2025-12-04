use adobe_cmap_parser::{ByteMapping, CIDRange, CodeRange};
use encoding_rs::UTF_16BE;
use euclid::*;
use log::{debug, error, info, trace, warn};
use lopdf::content::Content;
use lopdf::encryption::DecryptionError;
use lopdf::*;

use std::fmt::{Debug, Formatter};

use euclid::vec2;
use rayon::prelude::*;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::marker::PhantomData;
use std::rc::Rc;
use std::result::Result;
use std::slice::Iter;
use std::str;
use unicode_normalization::UnicodeNormalization;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use lazy_static::lazy_static;

// Import text splitting utilities
use crate::text_splitting::{
    count_words as count_words_simple, estimate_tokens_from_words, get_text_splitter_shared,
};

mod core_fonts;
mod encodings;

mod chunk_accumulator;
pub mod document;
mod form;
mod glyphnames;
mod heading_hierarchy;
mod layout_params;
mod ocrs;
mod pdf_image;
pub mod text_splitting;
mod zapfglyphnames;

// Re-export OCR types
pub use ocrs::{OcrConfig, OcrHandler};
// Use the transform function internally
use ocrs::apply_transform_to_image;

// Re-export document processing types and functions
pub use document::{
    calculate_document_stats, calculate_heading_thresholds, calculate_mode, is_heading, output_doc,
    output_doc_new_schema, parse_pdf, DocumentStats, FontStats, HeaderFooterDetector,
    HeaderFooterPattern, HeaderFooterType, PageOccurrence, PageText, PostProcessor, TextLevel,
};

// Re-export LAParams configuration
pub use layout_params::LAParams;

use crate::chunk_accumulator::count_words as unicode_count_words;
use crate::text_splitting::count_words;
use regex::Regex;
use serde::{Deserialize, Serialize};

// New schema structures for content extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentCore {
    pub chunk_id: String,    // blake3(content)
    pub source_id: i64,      // files.id
    pub source_type: String, // 'file' | 'web' | 'api'
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
    pub char_range: CharRange,
    pub bbox: BoundingBox, // Renamed from bbox_union for clarity
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug)]
pub enum OutputError {
    FormatError(std::fmt::Error),
    IoError(std::io::Error),
    PdfError(lopdf::Error),
    Custom(String),
    Other(String), // Add this variant if it doesn't exist
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            OutputError::FormatError(e) => write!(f, "Formating error: {}", e),
            OutputError::IoError(e) => write!(f, "IO error: {}", e),
            OutputError::PdfError(e) => write!(f, "PDF error: {}", e),
            OutputError::Custom(e) => write!(f, "Custom error: {}", e),
            OutputError::Other(e) => write!(f, "Other error: {}", e),
        }
    }
}

impl std::error::Error for OutputError {}

impl From<std::fmt::Error> for OutputError {
    fn from(e: std::fmt::Error) -> Self {
        OutputError::FormatError(e)
    }
}

impl From<std::io::Error> for OutputError {
    fn from(e: std::io::Error) -> Self {
        OutputError::IoError(e)
    }
}

impl From<lopdf::Error> for OutputError {
    fn from(e: lopdf::Error) -> Self {
        OutputError::PdfError(e)
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

fn record_font_decode(font_key: &str, decoded_nonempty: bool) {
    if std::env::var("PDF_EXTRACT_LOG_FONTS").is_err() { return; }
    if let Ok(mut map) = FONT_DECODE_COUNTS.lock() {
        let entry = map.entry(font_key.to_string()).or_insert((0, 0));
        entry.0 += 1;
        if !decoded_nonempty { entry.1 += 1; }
    }
}

fn log_font_once(prefix: &str, font_key: &str, details: &str) {
    if std::env::var("PDF_EXTRACT_LOG_FONTS").is_err() { return; }
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
    if std::env::var("PDF_EXTRACT_LOG_FONTS").is_err() { return; }
    if let Ok(map) = FONT_DECODE_COUNTS.lock() {
        for (k, (total, empty)) in map.iter() {
            log::debug!("fontstats: {} total={} empty={} empty_ratio={:.2}", k, total, empty, (*empty as f64)/(*total as f64 + 1e-9));
        }
    }
    if let Ok(map) = FONT_APPEND_COUNTS.lock() {
        for (k, appended) in map.iter() {
            log::debug!("fontstats: append {} appended={}", k, appended);
        }
    }
}

fn record_font_append(font_id: &str) {
    if std::env::var("PDF_EXTRACT_LOG_FONTS").is_err() { return; }
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

// we follow the same conventions as pdfium for when to support indirect objects:
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

        let mut unicode_map = get_unicode_map(doc, font);

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
            for font_metrics in core_fonts::metrics().iter() {
                if font_metrics.0 == base_name {
                    if let Some(ref encoding) = encoding_table {
                        dlog!("has encoding");
                        for w in font_metrics.2 {
                            let c = glyphnames::name_to_unicode(w.2).unwrap();
                            for i in 0..encoding.len() {
                                if encoding[i] == c {
                                    width_map.insert(i as CharCode, w.1 as f64);
                                }
                            }
                        }
                    } else {
                        // Instead of using the encoding from the core font we'll just look up all
                        // of the character names. We should probably verify that this produces the
                        // same result.

                        let mut table = vec![0; 256];
                        for w in font_metrics.2 {
                            dlog!("{} {}", w.0, w.2);
                            // -1 is "not encoded"
                            if w.0 != -1 {
                                table[w.0 as usize] = if base_name == "ZapfDingbats" {
                                    zapfglyphnames::zapfdigbats_names_to_unicode(w.2)
                                        .unwrap_or_else(|| panic!("bad name {:?}", w))
                                } else {
                                    glyphnames::name_to_unicode(w.2).unwrap()
                                }
                            }
                        }

                        let encoding = &table[..];
                        for w in font_metrics.2 {
                            width_map.insert(w.0 as CharCode, w.1 as f64);
                            // -1 is "not encoded"
                        }
                        encoding_table = Some(encoding.to_vec());
                    }
                    /* "Ordinarily, a font dictionary that refers to one of the standard fonts
                    should omit the FirstChar, LastChar, Widths, and FontDescriptor entries.
                    However, it is permissible to override a standard font by including these
                    entries and embedding the font program in the PDF file."

                    Note: some PDFs include a descriptor but still don't include these entries */
                    // assert!(maybe_get_obj(doc, font, b"FirstChar").is_none());
                    // assert!(maybe_get_obj(doc, font, b"LastChar").is_none());
                    // assert!(maybe_get_obj(doc, font, b"Widths").is_none());
                }
            }
        } else {
            panic!("no widths");
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
        let base_name = get_name_string(self.doc, self.font, b"BaseFont");
        let subtype = get_name_string(self.doc, self.font, b"Subtype");
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
                    println!(
                        "missing char {:?} in unicode map {:?} for {:?}",
                        char, unicode_map, self.font
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
            return s;
        }
        let encoding = self
            .encoding
            .as_ref()
            .map(|x| &x[..])
            .unwrap_or(&PDFDocEncoding);
        //dlog!("char_code {:?} {:?}", char, self.encoding);
        let s = to_utf8(encoding, &slice);
        record_font_decode(&font_key, !s.is_empty());
        s
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
            panic!("missing width for {} {:?}", id, self.font);
        }
    }

    fn next_char(&self, iter: &mut Iter<u8>) -> Option<(CharCode, u8)> {
        iter.next().map(|x| (*x as CharCode, 1))
    }
    fn decode_char(&self, char: CharCode) -> String {
        let base_name = get_name_string(self.doc, self.font, b"BaseFont");
        let subtype = get_name_string(self.doc, self.font, b"Subtype");
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
                    panic!("missing char {:?} in map {:?}", char, unicode_map)
                }
                Some(s) => s.clone(),
            };
            record_font_decode(&font_key, !s.is_empty());
            return s;
        }
        let encoding = self
            .encoding
            .as_ref()
            .map(|x| &x[..])
            .unwrap_or(&PDFDocEncoding);
        //dlog!("char_code {:?} {:?}", char, self.encoding);
        let s = to_utf8(encoding, &slice);
        record_font_decode(&font_key, !s.is_empty());
        s
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

            let cmap = adobe_cmap_parser::get_unicode_map(&contents).unwrap();
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
            if name != "Identity-H" {
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
                assert!(name == "Identity-H");
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
            }
            &Object::Stream(ref stream) => {
                let contents = get_contents(stream);
                dlog!("Stream: {}", String::from_utf8(contents.clone()).unwrap());
                adobe_cmap_parser::get_byte_mapping(&contents).unwrap()
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
                if let &Object::Array(ref wa) = w[i + 1] {
                    let cid = w[i].as_i64().expect("id should be num");
                    let mut j = 0;
                    dlog!("wa: {:?} -> {:?}", cid, wa);
                    for w in wa {
                        widths.insert((cid + j) as CharCode, as_num(w));
                        j += 1;
                    }
                    i += 2;
                } else {
                    let c_first = w[i].as_i64().expect("first should be num");
                    let c_last = w[i].as_i64().expect("last should be num");
                    let c_width = as_num(&w[i]);
                    for id in c_first..c_last {
                        widths.insert(id as CharCode, c_width);
                    }
                    i += 3;
                }
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
                id, key
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
        let base_name = get_name_string(self.doc, self.font, b"BaseFont");
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
        out
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
        // and arbitrarily choose y_min to match pdfium
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
                // We ignore 'Order' like pdfium, poppler and pdf.js

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
}

/// Get the page rotation from the page dictionary (inheritable)
fn get_page_rotation(page_dict: &Dictionary, doc: &Document) -> i32 {
    // /Rotate is inheritable – walk up the page tree
    get_inherited(doc, page_dict, b"Rotate")
        .and_then(|o: &Object| o.as_i64().ok())
        .unwrap_or(0) as i32
}

/// Build the initial CTM with page rotation and viewer Y-flip
fn build_initial_ctm(media_box: &[f64], page_rotate: i32) -> Transform {
    let h_pts = media_box[3]; // page height in PostScript points

    let mut init = Transform::identity();
    // Layer 2: viewer Y-flip (PDF has bottom-left origin, viewers use top-left)
    // NOTE: This Y-flip is for coordinate system only, not for image content!
    init = init.pre_scale(1.0, -1.0).pre_translate(vec2(0.0, h_pts));

    // Layer 1: page /Rotate
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
    use miniz_oxide::inflate::decompress_to_vec_zlib;

    let mut data = img.content.to_vec();

    // 1. Handle compression filters
    if let Some(filters) = &img.filters {
        for filter in filters {
            match filter.as_str() {
                "FlateDecode" => {
                    debug!("Decompressing FlateDecode filter");
                    data = decompress_to_vec_zlib(&data)
                        .map_err(|e| format!("FlateDecode decompression failed: {:?}", e))?;
                }
                "LZWDecode" => {
                    // For now, treat LZW same as Flate (many PDFs mislabel)
                    debug!("Decompressing LZWDecode filter (treating as FlateDecode)");
                    match decompress_to_vec_zlib(&data) {
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
            let word_count = unicode_count_words(text);
            segments.push(TextSegment {
                content: text.to_string(),
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
            });
        }
    }

    segments
}

// In your content processing function:
fn process_xobject(
    doc: &Document,
    resources: &Dictionary,
    ocr_handler: Option<&OcrHandler>,
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

    for (_, xvalue) in xobject.iter() {
        let id = xvalue.as_reference()?;
        let xvalue = doc.get_object(id)?;
        let xvalue = xvalue.as_stream()?;
        let dict = &xvalue.dict;

        // Only process images
        let subtype = dict.get(b"Subtype")?.as_name()?;
        debug!("XObject subtype: {:?}", String::from_utf8_lossy(subtype));
        if subtype != b"Image" {
            continue;
        }
        debug!("Found Image XObject!");

        // Extract image information
        let width = dict.get(b"Width")?.as_i64()?;
        let height = dict.get(b"Height")?.as_i64()?;
        info!("Found image: {}x{} pixels", width, height);

        // Skip extremely large images that might cause issues
        if width > 10000 || height > 10000 {
            warn!("Skipping extremely large image: {}x{}", width, height);
            continue;
        }
        let color_space = match dict.get(b"ColorSpace") {
            Ok(cs) => match cs {
                Object::Array(array) => {
                    Some(String::from_utf8_lossy(array[0].as_name()?).to_string())
                }
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
                    debug!(
                        "Successful image conversion {}x{}",
                        img.dimensions().0,
                        img.dimensions().1
                    );
                    if let Ok(ocr_text) = handler.process_image(&img) {
                        if !ocr_text.is_empty() {
                            info!(
                                "OCR extracted {} chars from image (Page {}, Position {:.1},{:.1})",
                                ocr_text.chars().count(),
                                page_num,
                                position.0,
                                position.1
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
                    } else {
                        match handler.process_image(&img) {
                            Ok(text) if text.is_empty() => {
                                debug!("OCR: No text detected in image at page {} position ({:.1},{:.1})", 
                                    page_num, position.0, position.1);
                            }
                            Err(e) => {
                                error!(
                                    "OCR error at page {} position ({:.1},{:.1}): {}",
                                    page_num, position.0, position.1, e
                                );
                            }
                            _ => {
                                // This shouldn't happen since we handled the Ok case above
                            }
                        }
                    }
                }
                Err(e) => {
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
                    _ => {
                        panic!("color_space {:?} {:?} {:?}", name, cs_name, cs)
                    }
                }
            } else if let Ok(cs) = cs.as_name() {
                match pdf_to_utf8(cs).as_ref() {
                    "DeviceRGB" => ColorSpace::DeviceRGB,
                    "DeviceGray" => ColorSpace::DeviceGray,
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
}
struct Processor<'a> {
    _none: PhantomData<&'a ()>,
}

impl<'a> Processor<'a> {
    fn new() -> Processor<'a> {
        Processor { _none: PhantomData }
    }

    // Helper: previously added a trailing space; now only trims to avoid double spaces
    fn preserve_sentence_boundaries(content: &str) -> String {
        content.trim().to_string()
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
            return vec![Self::create_text_segment(
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
            )];
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

                    segments.push(Self::create_text_segment(
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
                    ));
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
                        segments.push(Self::create_text_segment(
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
                        ));
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
                    segments.push(Self::create_text_segment(
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
                    ));
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
    ) -> TextSegment {
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
        TextSegment {
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
        }
    }

    fn is_visible_text(
        &self,
        transform: &Transform,
        font_size: f64,
        color: &(f64, f64, f64),
        media_box: &MediaBox,
        rendering_mode: i32,
    ) -> bool {
        // Default: allow text unless explicitly invisible (Tr=3).
        // Many PDFs use transforms that make simple bounds checks unreliable.
        if rendering_mode == 3 {
            return false;
        }

        // Strict visibility checks are opt-in via PDF_EXTRACT_ENFORCE_VISIBILITY
        if std::env::var("PDF_EXTRACT_ENFORCE_VISIBILITY").is_ok() {
            const MIN_COLOR_DIFF: f64 = 0.1;
            // Bounds check (using the provided transform origin)
            let point = transform.transform_point(Point2D::new(0.0, 0.0));
            if point.x < media_box.llx
                || point.x > media_box.urx
                || point.y < media_box.lly
                || point.y > media_box.ury
            {
                return false;
            }
            // Optional: drop near-white text on assumed white background
            if std::env::var("PDF_EXTRACT_DROP_NEAR_WHITE").is_ok() {
                if color.0 > 1.0 - MIN_COLOR_DIFF
                    && color.1 > 1.0 - MIN_COLOR_DIFF
                    && color.2 > 1.0 - MIN_COLOR_DIFF
                {
                    return false;
                }
            }
        }

        true
    }

    fn process_stream(
        &mut self,
        doc: &'a Document,
        ocr_handler: Option<&OcrHandler>,
        content: Vec<u8>,
        resources: &'a Dictionary,
        media_box: &MediaBox,
        page_num: u32,
        text_segments: &mut Vec<TextSegment>,
        page_rotate: i32,
        laparams: Option<&crate::LAParams>,
    ) -> Result<(), OutputError> {
        let use_layout = laparams.is_some();
        let params = laparams.cloned();
        #[derive(Clone, Debug)]
        struct Glyph {
            ch: String,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
            text_obj_id: u32, // Track which BT/ET text object this glyph belongs to
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

        // Build initial CTM with page rotation and viewer Y-flip
        let media_box_array = [media_box.llx, media_box.lly, media_box.urx, media_box.ury];
        let initial_ctm = build_initial_ctm(&media_box_array, page_rotate);

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
        let flip_ctm = Transform2D::<f64, Space, Space>::row_major(
            1.,
            0.,
            0.,
            -1.,
            0.,
            media_box.ury - media_box.lly,
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
                    assert!(operation.operands.len() == 6);
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
                    let name = operation.operands[0].as_name().unwrap();
                    gs.stroke_colorspace = make_colorspace(doc, name, resources);
                }
                "cs" => {
                    let name = operation.operands[0].as_name().unwrap();
                    gs.fill_colorspace = make_colorspace(doc, name, resources);
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
                    let fonts: &Dictionary = get(&doc, resources, b"Font");
                    let name = operation.operands[0].as_name().unwrap();
                    let font = make_font(doc, get::<&Dictionary>(doc, fonts, name));

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
                            let char_count = content_str.chars().count();
                            // Calculate approximate width - for multi-line segments, use average line width
                            let avg_chars_per_line = 80.0;
                            let lines = (char_count as f64 / avg_chars_per_line).max(1.0);
                            let approx_width = if lines > 1.0 {
                                avg_chars_per_line * current_font_size * 0.6
                            } else {
                                char_count as f64 * current_font_size * 0.6
                            };
                            let word_count = unicode_count_words(&content_str);
                            // Use the new function that respects token limits
                            // Conservative limit: 300 tokens per segment
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
                "TJ" => match operation.operands[0] {
                    Object::Array(ref array) => {
                        // current_line.push_str("*");
                        for e in array {
                            match e {
                                &Object::String(ref s, _) => {
                                    let font: &Rc<dyn PdfFont> = gs.ts.font.as_ref().unwrap();
                                    first_char = true;

                                    for (c, length) in font.char_codes(s) {
                                        let w0 = font.get_width(c) / 1000.;
                                        let mut spacing = gs.ts.character_spacing;
                                        let is_space = c == 32 && length == 1;
                                        if is_space {
                                            spacing += gs.ts.word_spacing;
                                        }

                                        let char = font.decode_char(c);

                                        let position = gs.ts.tm.post_transform(&flip_ctm);
                                        let (x, y) = (position.m31, position.m32);
                                        let transformed_font_size_vec = gs.ts.tm.transform_vector(
                                            vec2(gs.ts.font_size, gs.ts.font_size),
                                        );
                                        let transformed_font_size = ((transformed_font_size_vec.x
                                            * transformed_font_size_vec.y)
                                            .sqrt()
                                            * 100.0)
                                            .round()
                                            / 100.0;
                                        // Use full device-space transform: CTM × Tm × viewer Y-flip
                                        let trm_vis = gs
                                            .ctm
                                            .post_transform(&gs.ts.tm)
                                            .post_transform(&flip_ctm);
                                        let is_add = if use_layout {
                                            true
                                        } else {
                                            self.is_visible_text(
                                                &trm_vis,
                                                transformed_font_size,
                                                &current_color,
                                                media_box,
                                                gs.ts.rendering_mode,
                                            )
                                        };

                                        if transformed_font_size != current_transformed_font_size
                                            && !transformed_font_size.is_nan()
                                            && !current_transformed_font_size.is_nan()
                                        {
                                            if !current_line.trim().is_empty() {
                                                let content_str = Self::preserve_sentence_boundaries(
                                                    &current_line,
                                                );
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
                                                    5000, // Avoid pre-splitting here; let the chunker split
                                                );
                                                text_segments.extend(segments);
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
                                            else if is_column_break(x, last_end, transformed_font_size) {
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
                                                    text_obj_id: current_text_obj_id,
                                                });
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
                "Tj" => match operation.operands[0] {
                    Object::String(ref s, _) => {
                        let ts = &mut gs.ts;
                        let font: &Rc<dyn PdfFont> = gs.ts.font.as_ref().unwrap();
                        first_char = true;

                        for (c, length) in font.char_codes(s) {
                            let w0 = font.get_width(c) / 1000.;
                            let mut spacing = gs.ts.character_spacing;
                            let is_space = c == 32 && length == 1;
                            if is_space {
                                spacing += gs.ts.word_spacing;
                            }

                            let char = font.decode_char(c);

                            let position = gs.ts.tm.post_transform(&flip_ctm);
                            let (x, y) = (position.m31, position.m32);
                            let transformed_font_size_vec = gs
                                .ts
                                .tm
                                .transform_vector(vec2(gs.ts.font_size, gs.ts.font_size));
                            let transformed_font_size = ((transformed_font_size_vec.x
                                * transformed_font_size_vec.y)
                                .sqrt()
                                * 100.0)
                                .round()
                                / 100.0;

                            // Use full device-space transform: CTM × Tm × viewer Y-flip
                            let trm_vis = gs
                                .ctm
                                .post_transform(&gs.ts.tm)
                                .post_transform(&flip_ctm);
                            let is_add = if use_layout {
                                // In layout mode, still ignore invisible/clip-only text
                                matches!(gs.ts.rendering_mode, 0 | 1 | 2)
                            } else {
                                self.is_visible_text(
                                    &trm_vis,
                                    transformed_font_size,
                                    &current_color,
                                    media_box,
                                    gs.ts.rendering_mode,
                                )
                            };
                            if transformed_font_size != current_transformed_font_size
                                && !transformed_font_size.is_nan()
                                && !current_transformed_font_size.is_nan()
                            {
                                if !current_line.trim().is_empty() {
                                    let content_str =
                                        Self::preserve_sentence_boundaries(&current_line);
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
                                    current_segment_start = page_char_counter;
                                    current_line.clear();
                                }

                                current_transformed_font_size = transformed_font_size;
                            }

                            if first_char {
                                // Check for paragraph break (large vertical gap)
                                if is_paragraph_break(y, last_y, transformed_font_size) {
                                    current_line.push('\n')
                                }
                                // Check for new line (moved down and back to left)
                                else if is_new_line(x, last_end, y, last_y, transformed_font_size)
                                {
                                    current_line.push('\n')
                                }
                                // Check for large horizontal gap (column break)
                                else if is_column_break(x, last_end, transformed_font_size) {
                                    current_line.push('\n')
                                }
                                // Check for space between words on same line
                                else if should_insert_space(x, last_end, transformed_font_size) {
                                    current_line.push(' ')
                                }

                                current_x = x;
                                current_y = y;
                            }
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
                                        text_obj_id: current_text_obj_id,
                                    });
                                } else {
                                    current_line.push_str(&char);
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
                        panic!("unexpected Tj operand {:?}", operation)
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
                    assert!(operation.operands.len() == 6);
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
                    assert!(operation.operands.len() == 2);
                    let tx = as_num(&operation.operands[0]);
                    let ty = as_num(&operation.operands[1]);
                    dlog!("translation: {} {}", tx, ty);

                    tlm = tlm.pre_transform(&Transform2D::create_translation(tx, ty));
                    gs.ts.tm = tlm;
                    dlog!("Td matrix {:?}", gs.ts.tm);
                }
                "TD" => {
                    assert!(operation.operands.len() == 2);
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
                        let ts = &mut gs.ts;
                        let font: &Rc<dyn PdfFont> = ts.font.as_ref().unwrap();
                        first_char = true;

                        for (c, length) in font.char_codes(s) {
                            let w0 = font.get_width(c) / 1000.;
                            let mut spacing = ts.character_spacing;
                            let is_space = c == 32 && length == 1;
                            if is_space { spacing += ts.word_spacing; }

                            let ch = font.decode_char(c);

                            let position = ts.tm.post_transform(&flip_ctm);
                            let (x, y) = (position.m31, position.m32);
                            let transformed_font_size_vec = ts.tm.transform_vector(vec2(ts.font_size, ts.font_size));
                            let transformed_font_size = ((transformed_font_size_vec.x * transformed_font_size_vec.y).sqrt() * 100.0).round() / 100.0;

                            let trm_vis = gs.ctm.post_transform(&ts.tm).post_transform(&flip_ctm);
                            let is_add = if use_layout {
                                // LA-path: be stricter when requested; also drop absurd scales
                                let mut ok = matches!(ts.rendering_mode, 0 | 1 | 2);
                                if ok && std::env::var("PDF_EXTRACT_ENFORCE_VISIBILITY").is_ok() {
                                    ok = self.is_visible_text(&trm_vis, transformed_font_size, &current_color, media_box, ts.rendering_mode);
                                }
                                if ok {
                                    let page_h = media_box.ury - media_box.lly;
                                    let max_frac: f64 = std::env::var("PDF_EXTRACT_MAX_FONT_FRAC")
                                        .ok()
                                        .and_then(|v| v.parse().ok())
                                        .unwrap_or(0.35);
                                    if transformed_font_size > page_h * max_frac {
                                        ok = false;
                                    }
                                }
                                ok
                            } else {
                                self.is_visible_text(&trm_vis, transformed_font_size, &current_color, media_box, ts.rendering_mode)
                            };
                            if is_add {
                                if use_layout {
                                    let g_height = transformed_font_size.max(0.0);
                                    let g_width = (w0 * transformed_font_size).max(0.0);
                                    glyphs.push(Glyph { ch, x, y, width: g_width, height: g_height, text_obj_id: current_text_obj_id });
                                } else {
                                    current_line.push_str(&ch);
                                    page_char_counter += ch.chars().count();
                                    record_font_append(&current_font);
                                }
                            }
                            first_char = false;
                            last_end = x + w0 * transformed_font_size;
                            last_y = y;
                            let tx = ts.horizontal_scaling * ((w0 - 0. / 1000.) * ts.font_size + spacing);
                            ts.tm = ts.tm.pre_transform(&Transform2D::create_translation(tx, 0.));
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
                            let ts = &mut gs.ts;
                            let font: &Rc<dyn PdfFont> = ts.font.as_ref().unwrap();
                            first_char = true;

                            for (c, length) in font.char_codes(s) {
                                let w0 = font.get_width(c) / 1000.;
                                let mut spacing = ts.character_spacing;
                                let is_space = c == 32 && length == 1;
                                if is_space { spacing += ts.word_spacing; }

                                let ch = font.decode_char(c);

                                let position = ts.tm.post_transform(&flip_ctm);
                                let (x, y) = (position.m31, position.m32);
                                let transformed_font_size_vec = ts.tm.transform_vector(vec2(ts.font_size, ts.font_size));
                                let transformed_font_size = ((transformed_font_size_vec.x * transformed_font_size_vec.y).sqrt() * 100.0).round() / 100.0;

                                let trm_vis = gs.ctm.post_transform(&ts.tm).post_transform(&flip_ctm);
                                let is_add = if use_layout {
                                    let mut ok = matches!(ts.rendering_mode, 0 | 1 | 2);
                                    if ok && std::env::var("PDF_EXTRACT_ENFORCE_VISIBILITY").is_ok() {
                                        ok = self.is_visible_text(&trm_vis, transformed_font_size, &current_color, media_box, ts.rendering_mode);
                                    }
                                    if ok {
                                        let page_h = media_box.ury - media_box.lly;
                                        let max_frac: f64 = std::env::var("PDF_EXTRACT_MAX_FONT_FRAC")
                                            .ok()
                                            .and_then(|v| v.parse().ok())
                                            .unwrap_or(0.35);
                                        if transformed_font_size > page_h * max_frac {
                                            ok = false;
                                        }
                                    }
                                    ok
                                } else {
                                    self.is_visible_text(&trm_vis, transformed_font_size, &current_color, media_box, ts.rendering_mode)
                                };
                                if is_add {
                                    if use_layout {
                                        let g_height = transformed_font_size.max(0.0);
                                        let g_width = (w0 * transformed_font_size).max(0.0);
                                        glyphs.push(Glyph { ch, x, y, width: g_width, height: g_height, text_obj_id: current_text_obj_id });
                                    } else {
                                        current_line.push_str(&ch);
                                        page_char_counter += ch.chars().count();
                                        record_font_append(&current_font);
                                    }
                                }
                                first_char = false;
                                last_end = x + w0 * transformed_font_size;
                                last_y = y;
                                let tx = ts.horizontal_scaling * ((w0 - 0. / 1000.) * ts.font_size + spacing);
                                ts.tm = ts.tm.pre_transform(&Transform2D::create_translation(tx, 0.));
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
                    let name = operation.operands[0].as_name().unwrap();
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
                    let name = operation.operands[0].as_name().unwrap();
                    let xf: &Stream = get(&doc, xobject, name);

                    // Only process XObject if OCR handler is available
                    if let Some(handler) = ocr_handler {
                        let position = gs.ts.tm.post_transform(&flip_ctm);
                        let (x, y) = (position.m31, position.m32);

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
                            gs.ctm.m11, gs.ctm.m12, gs.ctm.m21, gs.ctm.m22, gs.ctm.m31, gs.ctm.m32
                        );

                        // The PDF coordinate system and image coordinate system may differ
                        // We need to detect the correct orientation based on the CTM
                        // For now, pass the current CTM to let the image processor figure it out
                        let image_transform = gs.ctm.clone();

                        if let Err(e) = process_xobject(
                            &doc,
                            resources,
                            Some(handler),
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

                    // Handle Form XObjects (Layer 3 transformation)
                    if let Ok(subtype) = xf.dict.get(b"Subtype") {
                        if let Ok(subtype_name) = subtype.as_name() {
                            if subtype_name == b"Form" {
                                // In layout-analysis mode, respect LAParams.all_texts
                                let allow_form_text = match laparams {
                                    Some(lp) => lp.all_texts,
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

                                // Push the combined CTM (current CTM × form matrix)
                                let saved_ctm = gs.ctm;
                                gs.ctm = gs.ctm.pre_transform(&form_matrix);

                                // Process the Form's content stream
                                let form_resources = maybe_get_obj(&doc, &xf.dict, b"Resources")
                                    .and_then(|n| n.as_dict().ok())
                                    .unwrap_or(resources);
                                let contents = get_contents(xf);
                                self.process_stream(
                                    &doc,
                                    ocr_handler,
                                    contents,
                                    form_resources,
                                    &media_box,
                                    page_num,
                                    text_segments,
                                    0, // Form XObjects don't have their own rotation
                                    laparams,
                                )?;

                                // Restore the CTM
                                gs.ctm = saved_ctm;
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
            // Use the new function that respects token limits
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

                // 1) Group glyphs into lines based on vertical overlap (like pdfminer)
                let glyphs_for_vertical = glyphs.clone();
                let mut raw_lines: Vec<Vec<Glyph>> = Vec::new();
                let mut current: Vec<Glyph> = Vec::new();

                // Track previous glyph position for simple Y-based newline detection
                let mut prev_y: f64 = 0.0;
                let mut prev_x_end: f64 = 0.0;
                let mut prev_nonspace_x_end: f64 = 0.0;
                let mut avg_char_width: f64 = 8.0;
                let mut avg_char_height: f64 = 12.0;

                for g in glyphs.iter() {
                    let is_space = g.ch.trim().is_empty();

                    if current.is_empty() {
                        prev_y = g.y;
                        prev_x_end = g.x + g.width;
                        if !is_space {
                            prev_nonspace_x_end = g.x + g.width;
                            avg_char_width = g.width.max(1.0);
                            avg_char_height = g.height.max(1.0);
                        }
                        current.push(g.clone());
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
                    let prev_nonspace_glyph = current.iter().rev().find(|g| !g.ch.trim().is_empty());

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

                    if is_new_line {
                        // Start new line
                        if !current.is_empty() {
                            raw_lines.push(std::mem::take(&mut current));
                        }
                        current.push(g.clone());
                    } else {
                        // Continue current line
                        current.push(g.clone());
                    }

                    // Update previous positions
                    prev_y = g.y;
                    prev_x_end = g.x + g.width;
                    if !is_space {
                        prev_nonspace_x_end = g.x + g.width;
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
                }
                let mut lines: Vec<LineInfo> = Vec::new();
                for mut line in raw_lines.into_iter() {
                    line.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
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
                                let large_gap_threshold = width_ref * 1.5;
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
                                    if small_gap > (lp.word_margin as f64) * width_ref_small {
                                        s.push(' ');
                                    }
                                }
                            } else if let Some(pr) = prev_right {
                                // First non-space glyph but have prev_right from space glyphs
                                let gap = g.x - pr;
                                let dim_prev = prev_w.max(prev_h);
                                let dim_curr = g.width.max(g.height);
                                let width_ref = dim_prev.max(dim_curr).max(1e-6);
                                if gap > (lp.word_margin as f64) * width_ref {
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
                                if gap > (lp.word_margin as f64) * width_ref {
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
                    let text = Self::preserve_sentence_boundaries(&s);
                    let height = (max_y - min_y).max(0.0);
                    lines.push(LineInfo { text, min_x, max_x, min_y, max_y, height });
                }

                // Optional: detect vertical text lines (minimal support)
                if lp.detect_vertical {
                    // Use only glyphs that look vertically oriented to reduce duplicates
                    let mut vg: Vec<Glyph> = glyphs_for_vertical
                        .into_iter()
                        .filter(|g| g.height > 1.5 * g.width)
                        .collect();
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
                    if !cur.is_empty() { v_lines.push(cur); }

                    // Build LineInfo for vertical lines, inserting spaces on large vertical gaps
                    for mut vline in v_lines.into_iter() {
                        if vline.len() < 2 { continue; }
                        vline.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));
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
                    let text = Self::preserve_sentence_boundaries(&s);
                        let height = (max_y - min_y).max(0.0);
                        lines.push(LineInfo { text, min_x, max_x, min_y, max_y, height });
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
                        let gap = lines_vec[i].min_y - lines_vec[i-1].max_y;
                        if gap > 0.0 && gap < lines_vec[i].height * 3.0 {
                            avg_line_gap += gap;
                            line_gap_count += 1;
                        }
                    }
                    avg_line_gap = if line_gap_count > 0 { avg_line_gap / line_gap_count as f64 } else { 10.0 };

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

                        let (next_starts_paragraph, next_is_section_marker) = if let Some(next_ln) = lines_collected.get(idx + 1) {
                            let next_trimmed = next_ln.text.trim();
                            // Check if next line starts with paragraph marker
                            let starts_para = next_trimmed.chars().next().map_or(false, |c| {
                                c.is_ascii_digit() // Numbered list
                                || c == '(' // Parenthetical like "(a)"
                                || c == '-' // Bullet
                                || c == '•' // Bullet
                            }) || next_trimmed.starts_with("A.") || next_trimmed.starts_with("B.")
                               || next_trimmed.starts_with("C.") || next_trimmed.starts_with("D.");

                            // Check if next line is a section marker (ALL CAPS short line)
                            let is_section = next_trimmed.to_uppercase() == next_trimmed
                                && next_trimmed.len() < 50
                                && (next_trimmed.contains("APPELLANT") || next_trimmed.contains("RESPONDENT")
                                    || next_trimmed.contains("VERSUS") || next_trimmed.contains("PETITIONER"));
                            (starts_para, is_section)
                        } else {
                            // Last line of this XObject - don't automatically treat as paragraph end
                            // Let terminal punctuation or other signals determine if it should be joined
                            (false, false)
                        };

                        // Join lines if: not terminal punctuation AND not followed by paragraph start
                        // AND no paragraph break detected AND not a complete legal phrase
                        let should_join = !ends_with_terminal && !next_starts_paragraph && !is_paragraph_break
                            && !current_is_complete && !next_is_section_marker;

                        // Build line text with appropriate ending
                        let line_text = if is_paragraph_break {
                            format!("\n{}\n", trimmed)
                        } else if should_join {
                            format!("{} ", trimmed) // Space instead of newline for continuation
                        } else {
                            format!("{}\n", trimmed)
                        };
                        let content_len = line_text.chars().count();
                        let mut segment = Self::create_text_segment(
                            line_text,
                            ln.height,
                            ln.height,
                            ln.min_x,
                            ln.min_y,
                            false,
                            String::new(),
                            FontWeight::Regular,
                            false,
                            page_num,
                            "LA-Line".to_string(),
                            None,
                            None,
                            page_char_pos,
                            page_char_pos + content_len,
                        );
                        segment.width = (ln.max_x - ln.min_x).max(0.0);
                        segment.height = (ln.max_y - ln.min_y).max(0.0);
                        page_char_pos += content_len;
                        prev_line_bottom = Some(ln.max_y);
                        text_segments.push(segment);
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
                        let ycmp = a.min_y.partial_cmp(&b.min_y).unwrap_or(std::cmp::Ordering::Equal);
                        if ycmp == std::cmp::Ordering::Equal {
                            a.min_x.partial_cmp(&b.min_x).unwrap_or(std::cmp::Ordering::Equal)
                        } else { ycmp }
                    });
                    ls.into_iter().map(|ln| make_box(ln)).collect()
                };

                if std::env::var("PDF_EXTRACT_LA_HEAP").is_ok() {
                    use std::collections::{BinaryHeap, HashMap, HashSet};
                    use ordered_float::OrderedFloat;
                    use std::cmp::Reverse;
                    #[derive(Clone)]
                    struct Plane { cell: f64, map: HashMap<(i32,i32), Vec<usize>> }
                    impl Plane {
                        fn new(cell: f64) -> Self { Self { cell, map: HashMap::new() } }
                        fn cells_for(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> (i32,i32,i32,i32) {
                            let cx0 = (x0 / self.cell).floor() as i32;
                            let cy0 = (y0 / self.cell).floor() as i32;
                            let cx1 = (x1 / self.cell).floor() as i32;
                            let cy1 = (y1 / self.cell).floor() as i32;
                            (cx0.min(cx1), cy0.min(cy1), cx0.max(cx1), cy0.max(cy1))
                        }
                        fn insert(&mut self, idx: usize, x0: f64, y0: f64, x1: f64, y1: f64) {
                            let (cx0,cy0,cx1,cy1) = self.cells_for(x0,y0,x1,y1);
                            for cx in cx0..=cx1 { for cy in cy0..=cy1 { self.map.entry((cx,cy)).or_default().push(idx); } }
                        }
                        fn remove(&mut self, idx: usize, x0: f64, y0: f64, x1: f64, y1: f64) {
                            let (cx0,cy0,cx1,cy1) = self.cells_for(x0,y0,x1,y1);
                            for cx in cx0..=cx1 { for cy in cy0..=cy1 { if let Some(v) = self.map.get_mut(&(cx,cy)) { v.retain(|&k| k != idx); } } }
                        }
                        fn query(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<usize> {
                            let (cx0,cy0,cx1,cy1) = self.cells_for(x0,y0,x1,y1);
                            let mut out = Vec::new();
                            for cx in cx0..=cx1 { for cy in cy0..=cy1 { if let Some(v) = self.map.get(&(cx,cy)) { out.extend_from_slice(v); } } }
                            out
                        }
                    }

                    let n0 = boxes.len();
                    let mut alive = vec![true; n0];
                    let mut ver: Vec<u64> = vec![0; n0];
                    let avg_h = boxes.iter().map(|b| b.avg_h).sum::<f64>() / (boxes.len() as f64).max(1.0);
                    let cell = (avg_h * 2.0).max(8.0);
                    let mut plane = Plane::new(cell);
                    for (i,b) in boxes.iter().enumerate() { plane.insert(i, b.min_x, b.min_y, b.max_x, b.max_y); }

                    let align_tol = |a: &BoxInfo, b: &BoxInfo| -> bool {
                        let tol_x = (lp.line_overlap as f64) * a.avg_h * ALIGN_TOL_FRAC;
                        let left = (b.min_x - a.min_x).abs() <= tol_x;
                        let right = (b.max_x - a.max_x).abs() <= tol_x;
                        let ca = (a.min_x + a.max_x) * 0.5; let cb = (b.min_x + b.max_x) * 0.5;
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
                        vgap_ok && (h_frac >= HOVERLAP_MIN_FRAC || align_tol(a,b))
                    };

                    let mut heap: BinaryHeap<Reverse<(OrderedFloat<f64>, usize, usize, u64, u64)>> = BinaryHeap::new();
                    let mut seen: HashSet<(usize,usize)> = HashSet::new();
                    fn enqueue_neighbors_for(
                        i: usize,
                        plane: &Plane,
                        boxes_local: &Vec<BoxInfo>,
                        alive: &Vec<bool>,
                        ver: &Vec<u64>,
                        lp: &crate::LAParams,
                        is_neighbor: &dyn Fn(&BoxInfo,&BoxInfo)->bool,
                        seen: &mut HashSet<(usize,usize)>,
                        heap: &mut BinaryHeap<Reverse<(OrderedFloat<f64>, usize, usize, u64, u64)>>,
                    ) {
                        if !alive[i] { return; }
                        let bi = &boxes_local[i];
                        let margin = (lp.line_margin as f64) * bi.avg_h;
                        let qx0 = bi.min_x - margin; let qx1 = bi.max_x + margin;
                        let qy0 = bi.min_y - margin; let qy1 = bi.max_y + margin;
                        let cand = plane.query(qx0, qy0, qx1, qy1);
                        for &j in cand.iter() {
                            if j == i || !alive[j] { continue; }
                            let a = i.min(j); let b = i.max(j);
                            if !seen.insert((a,b)) { continue; }
                            let bj = &boxes_local[j];
                            if !is_neighbor(bi, bj) { continue; }
                            let (ux0,uy0,ux1,uy1) = bbox_union((bi,i), (bj,j));
                            let a_area = bbox_area(bi.min_x, bi.min_y, bi.max_x, bi.max_y);
                            let b_area = bbox_area(bj.min_x, bj.min_y, bj.max_x, bj.max_y);
                            let u_area = bbox_area(ux0, uy0, ux1, uy1);
                            let dist = (u_area - a_area - b_area).max(0.0);
                            heap.push(Reverse((OrderedFloat(dist), a, b, ver[a], ver[b])));
                        }
                    }
                    for i in 0..boxes.len() { enqueue_neighbors_for(i, &plane, &boxes, &alive, &ver, &lp, &is_neighbor, &mut seen, &mut heap); }

                    while let Some(Reverse((_d, a, b, va, vb))) = heap.pop() {
                        if !alive[a] || !alive[b] { continue; }
                        if ver[a] != va || ver[b] != vb { continue; }
                        let bi = boxes[a].clone(); let bj = boxes[b].clone();
                        if !is_neighbor(&bi, &bj) { continue; }
                        let (ux0,uy0,ux1,uy1) = bbox_union((&bi,a), (&bj,b));
                        let mut blocked = false;
                        for k in plane.query(ux0,uy0,ux1,uy1) {
                            if k == a || k == b || !alive[k] { continue; }
                            let bk = &boxes[k];
                            let cx = (bk.min_x + bk.max_x) * 0.5; let cy = (bk.min_y + bk.max_y) * 0.5;
                            let in_union = cx >= ux0 && cx <= ux1 && cy >= uy0 && cy <= uy1;
                            let in_a = cx >= bi.min_x && cx <= bi.max_x && cy >= bi.min_y && cy <= bi.max_y;
                            let in_b = cx >= bj.min_x && cx <= bj.max_x && cy >= bj.min_y && cy <= bj.max_y;
                            if in_union && !(in_a || in_b) { blocked = true; break; }
                        }
                        if blocked { continue; }

                        // Merge b into a
                        let mut na = boxes[a].clone();
                        let ob = boxes[b].clone();
                        na.lines.extend(ob.lines.into_iter());
                        na.lines.sort_by(|l1,l2| {
                            let ycmp = l1.min_y.partial_cmp(&l2.min_y).unwrap_or(std::cmp::Ordering::Equal);
                            if ycmp == std::cmp::Ordering::Equal { l1.min_x.partial_cmp(&l2.min_x).unwrap_or(std::cmp::Ordering::Equal) } else { ycmp }
                        });
                        na.min_x = na.min_x.min(boxes[b].min_x);
                        na.max_x = na.max_x.max(boxes[b].max_x);
                        na.min_y = na.min_y.min(boxes[b].min_y);
                        na.max_y = na.max_y.max(boxes[b].max_y);
                        let mut sum_h = 0.0; let mut cnt = 0.0; for ln in na.lines.iter() { sum_h += ln.height; cnt += 1.0; }
                        na.avg_h = if cnt > 0.0 { sum_h / cnt } else { na.avg_h };
                        plane.remove(a, boxes[a].min_x, boxes[a].min_y, boxes[a].max_x, boxes[a].max_y);
                        boxes[a] = na;
                        plane.insert(a, boxes[a].min_x, boxes[a].min_y, boxes[a].max_x, boxes[a].max_y);
                        plane.remove(b, boxes[b].min_x, boxes[b].min_y, boxes[b].max_x, boxes[b].max_y);
                        alive[b] = false;
                        ver[a] = ver[a].wrapping_add(1);
                        enqueue_neighbors_for(a, &plane, &boxes, &alive, &ver, &lp, &is_neighbor, &mut seen, &mut heap);
                    }
                    // Keep only alive boxes
                    let mut out: Vec<BoxInfo> = Vec::new();
                    for (i,b) in boxes.into_iter().enumerate() { if alive[i] { out.push(b); } }
                    boxes = out;
                }

                // Ordering: columns or flow/geometric
                let use_columns = std::env::var("PDF_EXTRACT_LA_COLUMNS").is_ok();
                let flow_none = std::env::var("PDF_EXTRACT_LA_BOXES_FLOW_NONE").is_ok();
                if use_columns && boxes.len() > 2 {
                    #[derive(Default)] struct Col { idxs: Vec<usize>, min_x: f64, max_x: f64 }
                    let widths: Vec<f64> = boxes.iter().map(|b| (b.max_x - b.min_x).max(1e-6)).collect();
                    let mut w_sorted = widths.clone();
                    w_sorted.sort_by(|a,b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    let median_w = if w_sorted.is_empty() { 1.0 } else { w_sorted[w_sorted.len()/2] };
                    let mut cols: Vec<Col> = Vec::new();
                    for (i,b) in boxes.iter().enumerate() {
                        let cx = (b.min_x + b.max_x) * 0.5; let mut placed = false;
                        for col in cols.iter_mut() {
                            let left = col.min_x.max(b.min_x); let right = col.max_x.min(b.max_x);
                            let ov = (right - left).max(0.0); let frac = ov / median_w.max(1e-6);
                            let col_cx = (col.min_x + col.max_x) * 0.5; let prox = (cx - col_cx).abs() <= 0.5 * median_w;
                            if frac >= 0.2 || prox { col.idxs.push(i); col.min_x = col.min_x.min(b.min_x); col.max_x = col.max_x.max(b.max_x); placed = true; break; }
                        }
                        if !placed { cols.push(Col{ idxs: vec![i], min_x: b.min_x, max_x: b.max_x }); }
                    }
                    cols.sort_by(|a,b| a.min_x.partial_cmp(&b.min_x).unwrap_or(std::cmp::Ordering::Equal));
                    let mut ordered: Vec<BoxInfo> = Vec::new();
                    for col in cols.into_iter() {
                        let mut v: Vec<&BoxInfo> = col.idxs.into_iter().map(|k| &boxes[k]).collect();
                        v.sort_by(|a,b| a.min_y.partial_cmp(&b.min_y).unwrap_or(std::cmp::Ordering::Equal));
                        for bx in v { ordered.push(bx.clone()); }
                    }
                    boxes = ordered;
                } else if !flow_none {
                    let wf = lp.boxes_flow as f64; let wx = ((wf + 1.0) / 2.0).clamp(0.0, 1.0); let wy = 1.0 - wx;
                    boxes.sort_by(|a, b| { let ka = (wy * a.min_y, wx * a.min_x); let kb = (wy * b.min_y, wx * b.min_x); ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal) });
                } else {
                    boxes.sort_by(|a,b| { let ycmp = a.min_y.partial_cmp(&b.min_y).unwrap_or(std::cmp::Ordering::Equal); if ycmp == std::cmp::Ordering::Equal { a.min_x.partial_cmp(&b.min_x).unwrap_or(std::cmp::Ordering::Equal) } else { ycmp } });
                }

                // 5) Emit TextSegments per box
                let mut page_char_pos = 0usize;
                for bx in boxes.into_iter() {
                    let mut content = String::new();
                    for (i, ln) in bx.lines.iter().enumerate() {
                        if i > 0 { content.push('\n'); }
                        content.push_str(&ln.text);
                    }
                    let content_len = content.chars().count();
                    let mut segment = Self::create_text_segment(
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
                    );
                    segment.width = (bx.max_x - bx.min_x).max(0.0);
                    segment.height = (bx.max_y - bx.min_y).max(0.0);
                    page_char_pos += content_len;
                    text_segments.push(segment);
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
        self.flip_ctm = Transform::row_major(1., 0., 0., -1., 0., media_box.ury - media_box.lly);
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
            }
            else if should_insert_space(x, self.last_end, transformed_font_size) {
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
    let mut doc = Document::load(path)?;
    maybe_decrypt(&mut doc)?;
    let content_outputs = output_doc(&doc, ocr_handler, None, None)
        .map_err(|e| OutputError::Other(e.to_string()))?;

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

fn maybe_decrypt(doc: &mut Document) -> Result<(), OutputError> {
    if !doc.is_encrypted() {
        return Ok(());
    }

    if let Err(e) = doc.decrypt("") {
        if let Error::Decryption(DecryptionError::IncorrectPassword) = e {
            eprintln!("Encrypted documents must be decrypted with a password using {{extract_text|extract_text_from_mem|output_doc}}_encrypted")
        }

        return Err(OutputError::PdfError(e));
    }

    Ok(())
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
    let mut doc = Document::load_mem(buffer)?;
    maybe_decrypt(&mut doc)?;
    let content_outputs = output_doc(&doc, ocr_handler, None, None)
        .map_err(|e| OutputError::Other(e.to_string()))?;

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
    use blake3::Hasher;

    // Generate chunk_id using blake3 hash of content
    let mut hasher = Hasher::new();
    hasher.update(content.as_bytes());
    let chunk_id = hasher.finalize().to_hex().to_string();

    // Serialize headings to JSON
    let headings_json = if !headings.is_empty() {
        Some(serde_json::to_string(headings).unwrap_or_default())
    } else {
        None
    };

    // Estimate token count using existing word counting logic
    let word_count = count_words(content);
    let token_count = estimate_tokens_from_words(word_count, 1.5) as i32;

    ContentCore {
        chunk_id,
        source_id,
        source_type: source_type.to_string(),
        content: content.to_string(),
        token_count,
        headings_json,
        status: "extracted".to_string(),
        schema_version: 1,
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
) -> Result<ContentExt, Box<dyn std::error::Error>> {
    // Only store the format location - it has all the position data we need
    let ext_data = serde_json::json!({
        "format_location": format_location
    });

    // Serialize to JSON and compress with zstd
    let json_bytes = serde_json::to_vec(&ext_data)?;
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
        .map(|pos| PageFragment {
            page: pos.page,
            char_range: CharRange {
                start: pos.char_start,
                end: pos.char_end,
            },
            bbox: pos.bbox.clone(),
        })
        .collect();

    FormatLocation::Pdf(PdfLocation { fragments })
}

/// Helper function to decompress and deserialize ContentExt data
pub fn decompress_content_ext(
    content_ext: &ContentExt,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let decompressed = zstd::bulk::decompress(&content_ext.ext_json, 1024 * 1024)?; // 1MB max
    let json_value: serde_json::Value = serde_json::from_slice(&decompressed)?;
    Ok(json_value)
}

/// Helper function to extract PdfLocation from ContentExt
pub fn extract_pdf_location(
    content_ext: &ContentExt,
) -> Result<PdfLocation, Box<dyn std::error::Error>> {
    let json_data = decompress_content_ext(content_ext)?;

    if let Some(format_location) = json_data.get("format_location") {
        // Check if it's the new format structure
        if let Some(format_type) = format_location.get("format") {
            if format_type.as_str() == Some("Pdf") {
                // Extract fragments directly from format_location
                if let Some(fragments) = format_location.get("fragments") {
                    let pdf_location = PdfLocation {
                        fragments: serde_json::from_value(fragments.clone())?,
                    };
                    return Ok(pdf_location);
                }
            }
        } else if let Some(pdf_data) = format_location.get("Pdf") {
            // Legacy format - tagged enum style
            let pdf_location: PdfLocation = serde_json::from_value(pdf_data.clone())?;
            return Ok(pdf_location);
        }
    }

    Err("No PDF location data found in ContentExt".into())
}
