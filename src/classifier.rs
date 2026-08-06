use crate::quality::{assess_text_quality_pages, TextQualityPage, TextQualitySignal};
use crate::OutputError;
use lopdf::{content::Content, Dictionary, Document, Object, ObjectId, Stream};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const TEXT_OPS_PER_PAGE: usize = 3;
const TEXT_CHARS_PER_PAGE: usize = 8;
const VECTOR_PATH_OPS: usize = 1_000;
const VECTOR_PATH_TO_TEXT_RATIO: usize = 200;
const FULL_PAGE_IMAGE_PIXELS: usize = 500_000;
const MAX_FORM_XOBJECT_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum DocumentType {
    TextBased,
    Scanned,
    ImageBased,
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrReason {
    Scanned,
    NoText,
    VectorText,
    SuspectedGarbledText,
}

impl OcrReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scanned => "scanned",
            Self::NoText => "no_text",
            Self::VectorText => "vector_text",
            Self::SuspectedGarbledText => "suspected_garbled_text",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ClassifierOptions {
    pub max_sample_pages: usize,
}

impl Default for ClassifierOptions {
    fn default() -> Self {
        Self {
            max_sample_pages: 8,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageAssessment {
    /// PDF page number, using the PDF one-based convention.
    pub page: u32,
    pub sampled: bool,
    pub text_operator_count: usize,
    pub text_char_count: usize,
    pub path_operator_count: usize,
    pub image_operator_count: usize,
    pub image_area: usize,
    pub needs_ocr: bool,
    pub ocr_reason: Option<OcrReason>,
    pub garble_signals: Vec<TextQualitySignal>,
}

impl PageAssessment {
    pub fn ocr_reason_code(&self) -> Option<&'static str> {
        self.ocr_reason.map(OcrReason::as_str)
    }

    /// The single routing decision for this sampled page. Callers should not
    /// reconstruct OCR heuristics from operator counts independently.
    pub fn should_route_ocr(&self) -> bool {
        self.needs_ocr && self.ocr_reason.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentClassification {
    pub document_type: DocumentType,
    pub confidence: f64,
    pub total_pages: usize,
    pub sampled_pages: usize,
    pub pages: Vec<PageAssessment>,
}

pub fn classify_pdf<P: AsRef<std::path::Path>>(
    path: P,
    options: ClassifierOptions,
) -> Result<DocumentClassification, OutputError> {
    let bytes = std::fs::read(path).map_err(OutputError::from)?;
    classify_pdf_from_mem(&bytes, options)
}

pub fn classify_pdf_from_mem(
    bytes: &[u8],
    options: ClassifierOptions,
) -> Result<DocumentClassification, OutputError> {
    let loaded = crate::load_pdf_from_mem(bytes, &crate::ExtractionOptions::default())?;
    classify_document(&loaded.document, options)
}

pub(crate) fn classify_document(
    document: &Document,
    options: ClassifierOptions,
) -> Result<DocumentClassification, OutputError> {
    let pages = document.get_pages();
    let page_numbers = sampled_page_numbers(pages.len(), options.max_sample_pages.max(1));
    let decoded_text = decoded_text_by_page(document, &page_numbers);
    let mut assessments = Vec::with_capacity(page_numbers.len());
    for page_number in page_numbers {
        let Some(page_id) = pages.get(&(page_number as u32)) else {
            continue;
        };
        let page = page_number as u32;
        assessments.push(scan_page(
            document,
            page,
            *page_id,
            decoded_text.get(&page).map(String::as_str).unwrap_or(""),
        ));
    }

    let sampled_pages = assessments.len();
    let text_pages = assessments
        .iter()
        .filter(|page| page_has_decoded_text(page) && !page.needs_ocr)
        .count();
    let image_pages = assessments
        .iter()
        .filter(|page| page.image_operator_count > 0 && !page_has_decoded_text(page))
        .count();
    let vector_pages = assessments
        .iter()
        .filter(|page| page.ocr_reason == Some(OcrReason::VectorText))
        .count();
    let non_text_pages = sampled_pages.saturating_sub(text_pages);
    let document_type = if sampled_pages == 0 {
        DocumentType::ImageBased
    } else if text_pages * 100 >= sampled_pages * 60 && image_pages == 0 {
        DocumentType::TextBased
    } else if image_pages * 100 >= sampled_pages * 60 {
        if assessments
            .iter()
            .any(|page| page.image_area >= FULL_PAGE_IMAGE_PIXELS)
        {
            DocumentType::Scanned
        } else {
            DocumentType::ImageBased
        }
    } else if text_pages > 0 && (non_text_pages > 0 || vector_pages > 0) {
        DocumentType::Mixed
    } else if vector_pages > 0 {
        DocumentType::ImageBased
    } else {
        DocumentType::Mixed
    };
    let dominant = [text_pages, image_pages, vector_pages, non_text_pages]
        .iter()
        .copied()
        .max()
        .unwrap_or(0);
    let sample_confidence = dominant as f64 / sampled_pages.max(1) as f64;
    let coverage = sampled_pages as f64 / pages.len().max(1) as f64;

    Ok(DocumentClassification {
        document_type,
        confidence: (sample_confidence * 0.7 + coverage * 0.3).clamp(0.0, 1.0),
        total_pages: pages.len(),
        sampled_pages,
        pages: assessments,
    })
}

/// Build the one canonical page assessment used by extraction's OCR gate.
/// This is deliberately a full scan, while the public classifier defaults to
/// a bounded sample for fast document-level attribution.
pub(crate) fn full_page_assessments(
    document: &Document,
) -> Result<HashMap<u32, PageAssessment>, OutputError> {
    let classification = classify_document(
        document,
        ClassifierOptions {
            max_sample_pages: document.get_pages().len().max(1),
        },
    )?;
    Ok(classification
        .pages
        .into_iter()
        .map(|page| (page.page, page))
        .collect())
}

fn sampled_page_numbers(total_pages: usize, max_sample_pages: usize) -> Vec<usize> {
    if total_pages <= max_sample_pages {
        return (1..=total_pages).collect();
    }
    let mut pages = Vec::with_capacity(max_sample_pages);
    for index in 0..max_sample_pages {
        let page = 1 + index * (total_pages - 1) / (max_sample_pages - 1).max(1);
        if pages.last().copied() != Some(page) {
            pages.push(page);
        }
    }
    pages
}

fn scan_page(
    document: &Document,
    page: u32,
    page_id: ObjectId,
    decoded_text: &str,
) -> PageAssessment {
    let stats = scan_page_operators(document, page_id);
    let text_for_quality = if decoded_text.trim().is_empty()
        && fallback_utf8_literal_text_is_safe(&stats.literal_utf8_text)
    {
        stats.literal_utf8_text.as_str()
    } else {
        decoded_text
    };
    let text_char_count = text_for_quality.chars().count();
    let quality = assess_text_quality_pages(&[TextQualityPage::new(page, text_for_quality)]);
    let garble_signals = quality
        .findings
        .iter()
        .map(|finding| finding.signal)
        .collect();
    let vector_text = stats.path_operator_count >= VECTOR_PATH_OPS
        && (stats.text_operator_count == 0
            || stats.path_operator_count
                > stats
                    .text_operator_count
                    .saturating_mul(VECTOR_PATH_TO_TEXT_RATIO));
    let has_text = text_char_count >= TEXT_CHARS_PER_PAGE
        || (stats.text_operator_count >= TEXT_OPS_PER_PAGE && text_char_count > 0);
    let ocr_reason = if quality.suspected_garbled_text() {
        Some(OcrReason::SuspectedGarbledText)
    } else if vector_text {
        Some(OcrReason::VectorText)
    } else if stats.image_operator_count > 0 && !has_text {
        Some(OcrReason::Scanned)
    } else if !has_text {
        Some(OcrReason::NoText)
    } else {
        None
    };

    PageAssessment {
        page,
        sampled: true,
        text_operator_count: stats.text_operator_count,
        text_char_count,
        path_operator_count: stats.path_operator_count,
        image_operator_count: stats.image_operator_count,
        image_area: stats.image_area,
        needs_ocr: ocr_reason.is_some(),
        ocr_reason,
        garble_signals,
    }
}

fn decoded_text_by_page(document: &Document, sampled_pages: &[usize]) -> HashMap<u32, String> {
    let sampled_pages = sampled_pages
        .iter()
        .map(|page| *page as u32)
        .collect::<HashSet<_>>();
    let Ok(outputs) = crate::document::processing::output_doc_for_pages(
        document,
        &sampled_pages.iter().copied().collect::<Vec<_>>(),
    ) else {
        return HashMap::new();
    };
    let mut by_page = HashMap::new();
    for output in outputs {
        let end_page = output.end_page.unwrap_or(output.page).max(output.page);
        for page in output.page..=end_page {
            if !sampled_pages.contains(&page) {
                continue;
            }
            let page_text = by_page.entry(page).or_insert_with(String::new);
            if !page_text.is_empty() {
                page_text.push('\n');
            }
            page_text.push_str(&output.paragraph);
        }
    }
    by_page
}

fn page_has_decoded_text(page: &PageAssessment) -> bool {
    page.text_char_count >= TEXT_CHARS_PER_PAGE
        || (page.text_operator_count >= TEXT_OPS_PER_PAGE && page.text_char_count > 0)
}

#[derive(Debug, Default)]
struct OperatorStats {
    text_operator_count: usize,
    path_operator_count: usize,
    image_operator_count: usize,
    image_area: usize,
    literal_utf8_text: String,
}

impl OperatorStats {
    fn add(&mut self, other: OperatorStats) {
        self.text_operator_count = self
            .text_operator_count
            .saturating_add(other.text_operator_count);
        self.path_operator_count = self
            .path_operator_count
            .saturating_add(other.path_operator_count);
        self.image_operator_count = self
            .image_operator_count
            .saturating_add(other.image_operator_count);
        self.image_area = self.image_area.saturating_add(other.image_area);
        if !other.literal_utf8_text.is_empty() {
            if !self.literal_utf8_text.is_empty() {
                self.literal_utf8_text.push('\n');
            }
            self.literal_utf8_text.push_str(&other.literal_utf8_text);
        }
    }
}

fn scan_page_operators(document: &Document, page_id: ObjectId) -> OperatorStats {
    let Some(resources) = inherited_dictionary(document, page_id, b"Resources") else {
        return scan_page_content_without_resources(document, page_id);
    };
    let Ok(content) = document.get_page_content(page_id) else {
        return OperatorStats::default();
    };
    let Ok(content) = Content::decode(&content) else {
        return OperatorStats::default();
    };
    let mut visited = HashSet::new();
    scan_content(document, &content, resources, 0, &mut visited)
}

fn scan_page_content_without_resources(document: &Document, page_id: ObjectId) -> OperatorStats {
    let Ok(content) = document.get_page_content(page_id) else {
        return OperatorStats::default();
    };
    let Ok(content) = Content::decode(&content) else {
        return OperatorStats::default();
    };
    let mut stats = OperatorStats::default();
    for operation in content.operations {
        add_non_resource_operator_stats(&mut stats, &operation.operator, &operation.operands);
    }
    stats
}

fn scan_content(
    document: &Document,
    content: &Content,
    resources: &Dictionary,
    depth: usize,
    visited: &mut HashSet<ObjectId>,
) -> OperatorStats {
    let mut stats = OperatorStats::default();
    for operation in &content.operations {
        add_non_resource_operator_stats(&mut stats, &operation.operator, &operation.operands);
        if operation.operator != "Do" {
            continue;
        }

        let Some(name) = operation
            .operands
            .first()
            .and_then(|operand| operand.as_name().ok())
        else {
            continue;
        };
        let Some((object_id, stream, kind)) = xobject_stream(document, resources, name) else {
            continue;
        };
        match kind {
            XObjectKind::Image => {
                stats.image_operator_count = stats.image_operator_count.saturating_add(1);
                stats.image_area = stats
                    .image_area
                    .saturating_add(image_stream_pixel_area(stream).unwrap_or(0));
            }
            XObjectKind::Form => {
                if depth >= MAX_FORM_XOBJECT_DEPTH {
                    continue;
                }
                if let Some(id) = object_id {
                    if !visited.insert(id) {
                        continue;
                    }
                }
                let form_resources = stream
                    .dict
                    .get(b"Resources")
                    .ok()
                    .and_then(|object| object_dictionary(document, object))
                    .unwrap_or(resources);
                let content = decoded_stream_content(stream);
                if let Ok(content) = Content::decode(&content) {
                    stats.add(scan_content(
                        document,
                        &content,
                        form_resources,
                        depth + 1,
                        visited,
                    ));
                }
                if let Some(id) = object_id {
                    visited.remove(&id);
                }
            }
            XObjectKind::Other => {}
        }
    }
    stats
}

fn add_non_resource_operator_stats(stats: &mut OperatorStats, operator: &str, operands: &[Object]) {
    if is_text_operator(operator) {
        stats.text_operator_count = stats.text_operator_count.saturating_add(1);
        append_utf8_text_operands(operands, &mut stats.literal_utf8_text);
    }
    if is_path_operator(operator) {
        stats.path_operator_count = stats.path_operator_count.saturating_add(1);
    }
    if is_inline_image_operator(operator) {
        stats.image_operator_count = stats.image_operator_count.saturating_add(1);
        stats.image_area = stats
            .image_area
            .saturating_add(inline_image_pixel_area(operands).unwrap_or(0));
    }
}

fn fallback_utf8_literal_text_is_safe(text: &str) -> bool {
    let visible = text.chars().filter(|ch| !ch.is_whitespace()).count();
    if visible == 0 {
        return false;
    }
    let controls = text
        .chars()
        .filter(|ch| ch.is_control() && !ch.is_whitespace())
        .count();
    controls * 10 <= visible
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XObjectKind {
    Image,
    Form,
    Other,
}

fn xobject_stream<'a>(
    document: &'a Document,
    resources: &'a Dictionary,
    name: &[u8],
) -> Option<(Option<ObjectId>, &'a Stream, XObjectKind)> {
    let xobjects = resources
        .get(b"XObject")
        .ok()
        .and_then(|object| object_dictionary(document, object))?;
    let object = xobjects.get(name).ok()?;
    let (object_id, object) = match object {
        Object::Reference(id) => (Some(*id), document.get_object(*id).ok()?),
        _ => (None, object),
    };
    let stream = object.as_stream().ok()?;
    let kind = match stream
        .dict
        .get(b"Subtype")
        .ok()
        .and_then(|subtype| subtype.as_name().ok())
    {
        Some(b"Image") => XObjectKind::Image,
        Some(b"Form") => XObjectKind::Form,
        _ => XObjectKind::Other,
    };
    Some((object_id, stream, kind))
}

fn inherited_dictionary<'a>(
    document: &'a Document,
    page_id: ObjectId,
    key: &[u8],
) -> Option<&'a Dictionary> {
    let mut current = Some(page_id);
    while let Some(id) = current {
        let page = document.get_dictionary(id).ok()?;
        if let Ok(object) = page.get(key) {
            if let Some(dictionary) = object_dictionary(document, object) {
                return Some(dictionary);
            }
        }
        current = page
            .get(b"Parent")
            .ok()
            .and_then(|object| object.as_reference().ok());
    }
    None
}

fn object_dictionary<'a>(
    document: &'a Document,
    object: &'a Object,
) -> Option<&'a lopdf::Dictionary> {
    match object {
        Object::Reference(id) => document.get_dictionary(*id).ok(),
        Object::Dictionary(dictionary) => Some(dictionary),
        _ => None,
    }
}

fn is_text_operator(operator: &str) -> bool {
    matches!(operator, "Tj" | "TJ" | "'" | "\"")
}

fn is_path_operator(operator: &str) -> bool {
    matches!(
        operator,
        "m" | "l"
            | "c"
            | "v"
            | "y"
            | "h"
            | "re"
            | "S"
            | "s"
            | "f"
            | "F"
            | "f*"
            | "B"
            | "b"
            | "b*"
            | "n"
    )
}

fn is_inline_image_operator(operator: &str) -> bool {
    matches!(operator, "BI" | "ID")
}

fn append_utf8_text_operands(operands: &[Object], output: &mut String) {
    for operand in operands {
        match operand {
            Object::String(bytes, _) => {
                if let Ok(text) = std::str::from_utf8(bytes) {
                    output.push_str(text);
                }
            }
            Object::Array(items) => append_utf8_text_operands(items, output),
            _ => {}
        }
    }
}

fn decoded_stream_content(stream: &Stream) -> Vec<u8> {
    if stream.filters().is_ok() {
        stream
            .decompressed_content()
            .unwrap_or_else(|_| stream.content.clone())
    } else {
        stream.content.clone()
    }
}

fn image_stream_pixel_area(stream: &Stream) -> Option<usize> {
    let width = object_usize(stream.dict.get(b"Width").ok()?)?;
    let height = object_usize(stream.dict.get(b"Height").ok()?)?;
    width.checked_mul(height)
}

fn inline_image_pixel_area(operands: &[Object]) -> Option<usize> {
    let width = image_dimension_from_operands(operands, &[b"W".as_slice(), b"Width".as_slice()])?;
    let height = image_dimension_from_operands(operands, &[b"H".as_slice(), b"Height".as_slice()])?;
    width.checked_mul(height)
}

fn image_dimension_from_operands(operands: &[Object], names: &[&[u8]]) -> Option<usize> {
    for window in operands.windows(2) {
        let Some(name) = window[0].as_name().ok() else {
            continue;
        };
        if names.iter().any(|candidate| *candidate == name) {
            return object_usize(&window[1]);
        }
    }
    operands.iter().find_map(|operand| {
        let dict = operand.as_dict().ok()?;
        names
            .iter()
            .find_map(|name| dict.get(name).ok().and_then(object_usize))
    })
}

fn object_usize(object: &Object) -> Option<usize> {
    object.as_i64().ok().and_then(|value| {
        if value >= 0 {
            Some(value as usize)
        } else {
            None
        }
    })
}
