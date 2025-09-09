use regex::Regex;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct HeaderFooterPattern {
    pub regex: Regex,
    pub pattern_type: HeaderFooterType,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HeaderFooterType {
    PageNumber,
    Date,
    Title,
    Copyright,
    ChapterTitle,
    Other,
}

pub struct HeaderFooterDetector {
    patterns: Vec<HeaderFooterPattern>,
    // Keyed by normalized text so variants like "Page 1 of 47" align across pages
    occurrence_map: HashMap<String, Vec<PageOccurrence>>,
    page_count: usize,
}

#[derive(Debug, Clone)]
pub struct PageOccurrence {
    pub page_num: u32,
    pub position: f64,
    pub font_size: f64,
    pub is_top: bool,
}

impl HeaderFooterDetector {
    pub fn new(page_count: usize) -> Self {
        let patterns = vec![
            HeaderFooterPattern {
                regex: Regex::new(r"^(?:Page\s+)?(\d+)(?:\s+of\s+\d+)?$").unwrap(),
                pattern_type: HeaderFooterType::PageNumber,
                confidence: 0.9,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"^-\s*\d+\s*-$").unwrap(),
                pattern_type: HeaderFooterType::PageNumber,
                confidence: 0.9,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"^\[\d+\]$").unwrap(),
                pattern_type: HeaderFooterType::PageNumber,
                confidence: 0.9,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"^\d{1,2}[/-]\d{1,2}[/-]\d{2,4}$").unwrap(),
                pattern_type: HeaderFooterType::Date,
                confidence: 0.8,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"^(?:Chapter|Section|Part)\s+\d+").unwrap(),
                pattern_type: HeaderFooterType::ChapterTitle,
                confidence: 0.85,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"(?i)^©|copyright\s+\d{4}").unwrap(),
                pattern_type: HeaderFooterType::Copyright,
                confidence: 0.9,
            },
        ];

        HeaderFooterDetector {
            patterns,
            occurrence_map: HashMap::new(),
            page_count,
        }
    }

    /// Normalize header/footer-like strings to improve matching across pages.
    /// - Lowercase
    /// - Collapse whitespace
    /// - Replace digit runs with '#'
    /// - Normalize common patterns like "page N of M"
    pub fn normalize_text(text: &str) -> String {
        let lower = text.to_lowercase();
        let ws = Regex::new(r"\s+").unwrap();
        let digits = Regex::new(r"\d+").unwrap();
        let page_pat = Regex::new(r"page\s+\d+\s+of\s+\d+").unwrap();
        let mut out = page_pat.replace_all(&lower, "page # of #").to_string();
        out = digits.replace_all(&out, "#").to_string();
        out = ws.replace_all(&out, " ").trim().to_string();
        out
    }

    pub fn add_occurrence(
        &mut self,
        text: &str,
        page_num: u32,
        position: f64,
        font_size: f64,
        page_height: f64,
    ) {
        // Y=0 is top; consider top if y is within top 15% of page
        let is_top = position < page_height * 0.15;
        let occurrence = PageOccurrence {
            page_num,
            position,
            font_size,
            is_top,
        };

        let key = Self::normalize_text(text);
        self.occurrence_map
            .entry(key)
            .or_insert_with(Vec::new)
            .push(occurrence);
    }

    pub fn analyze(&self) -> HashSet<String> {
        let mut headers_footers = HashSet::new();
        let min_occurrence_ratio = 0.5;

        for (norm_text, occurrences) in &self.occurrence_map {
            let occurrence_count = occurrences.len();
            let occurrence_ratio = occurrence_count as f64 / self.page_count as f64;

            if occurrence_ratio >= min_occurrence_ratio {
                // Check if positions are consistent
                let positions: Vec<f64> = occurrences.iter().map(|o| o.position).collect();
                let avg_position = positions.iter().sum::<f64>() / positions.len() as f64;
                let position_variance = positions
                    .iter()
                    .map(|p| (p - avg_position).powi(2))
                    .sum::<f64>()
                    / positions.len() as f64;

                // Low variance means consistent positioning
                if position_variance < 100.0 {
                    // Check against patterns for higher confidence
                    let mut pattern_matched = false;
                    for pattern in &self.patterns {
                        if pattern.regex.is_match(norm_text) {
                            pattern_matched = true;
                            break;
                        }
                    }

                    if pattern_matched || occurrence_ratio >= 0.8 {
                        headers_footers.insert(norm_text.clone());
                    }
                }
            }
        }

        headers_footers
    }
}
