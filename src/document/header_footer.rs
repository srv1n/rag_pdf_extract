use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

static NORMALIZE_WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static NORMALIZE_DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());
static NORMALIZE_PAGE_OF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"page\s+\d+\s+of\s+\d+").unwrap());

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderFooterPatternKind {
    PageNumber,
    Date,
    Copyright,
    RunningHeader,
    ChapterSectionTitle,
}

pub struct HeaderFooterDetector {
    patterns: Vec<HeaderFooterPattern>,
    // Keyed by normalized text so variants like "Page 1 of 47" align across pages
    occurrence_map: HashMap<String, Vec<PageOccurrence>>,
    page_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageBand {
    Top,
    Bottom,
    Body,
}

#[derive(Debug, Clone)]
pub struct PageOccurrence {
    pub page_num: u32,
    pub position: f64,
    pub font_size: f64,
    pub band: PageBand,
    pub exact_text: String,
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
                regex: Regex::new(r"^(?:page\s+)?#(?:\s+of\s+#)?$").unwrap(),
                pattern_type: HeaderFooterType::PageNumber,
                confidence: 0.9,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"^-\s*#\s*-$").unwrap(),
                pattern_type: HeaderFooterType::PageNumber,
                confidence: 0.9,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"^\[#\]$").unwrap(),
                pattern_type: HeaderFooterType::PageNumber,
                confidence: 0.9,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"^#(?:[/-]#){1,2}$").unwrap(),
                pattern_type: HeaderFooterType::Date,
                confidence: 0.8,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"^(?:chapter|section|part)\s+#").unwrap(),
                pattern_type: HeaderFooterType::ChapterTitle,
                confidence: 0.85,
            },
            HeaderFooterPattern {
                regex: Regex::new(r"(?i)^(?:©|copyright\s+#)").unwrap(),
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
        let mut out = NORMALIZE_PAGE_OF
            .replace_all(&lower, "page # of #")
            .to_string();
        out = NORMALIZE_DIGITS.replace_all(&out, "#").to_string();
        out = NORMALIZE_WS.replace_all(&out, " ").trim().to_string();
        out
    }

    fn canonical_exact_text(text: &str) -> String {
        NORMALIZE_WS
            .replace_all(&text.to_lowercase(), " ")
            .trim()
            .to_string()
    }

    pub fn add_occurrence(
        &mut self,
        text: &str,
        page_num: u32,
        position: f64,
        font_size: f64,
        page_height: f64,
    ) {
        let band = classify_band(position, page_height);
        let occurrence = PageOccurrence {
            page_num,
            position,
            font_size,
            band,
            exact_text: Self::canonical_exact_text(text),
        };

        let key = Self::normalize_text(text);
        self.occurrence_map
            .entry(key)
            .or_insert_with(Vec::new)
            .push(occurrence);
    }

    pub fn analyze(&self) -> HashSet<String> {
        let mut headers_footers = HashSet::new();
        if self.page_count == 0 {
            return headers_footers;
        }
        let min_occurrence_ratio = 0.5;

        for (norm_text, occurrences) in &self.occurrence_map {
            if norm_text.len() > 160 || norm_text.split_whitespace().count() > 24 {
                continue;
            }

            let Some(band) = consistent_non_body_band(occurrences) else {
                continue;
            };

            let distinct_pages = occurrences
                .iter()
                .map(|o| o.page_num)
                .collect::<HashSet<_>>()
                .len();
            let occurrence_ratio = distinct_pages as f64 / self.page_count as f64;
            let pattern_kind = self.strict_pattern_kind(norm_text);
            let pattern_matched = pattern_kind.is_some();
            let chapter_section_title = matches!(
                pattern_kind,
                Some(HeaderFooterPatternKind::ChapterSectionTitle)
            );

            if self.page_count == 1 {
                let bottom = band == PageBand::Bottom;
                let removable = matches!(pattern_kind, Some(HeaderFooterPatternKind::PageNumber))
                    || (bottom
                        && matches!(
                            pattern_kind,
                            Some(
                                HeaderFooterPatternKind::Date | HeaderFooterPatternKind::Copyright
                            )
                        ));
                if removable {
                    headers_footers.insert(norm_text.clone());
                }
                continue;
            }

            if chapter_section_title {
                if self.page_count <= 2 || !exact_text_repeats(occurrences) {
                    continue;
                }
            }

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
                    let same_band = occurrences.iter().all(|o| o.band == band);
                    if same_band
                        && (pattern_matched || (self.page_count >= 3 && occurrence_ratio >= 0.8))
                    {
                        headers_footers.insert(norm_text.clone());
                    }
                }
            }
        }

        headers_footers
    }

    pub fn is_header_footer_text(&self, text: &str) -> bool {
        let norm = Self::normalize_text(text);
        self.analyze().contains(&norm)
    }

    fn strict_pattern_match(&self, norm_text: &str) -> bool {
        self.strict_pattern_kind(norm_text).is_some()
    }

    fn strict_pattern_kind(&self, norm_text: &str) -> Option<HeaderFooterPatternKind> {
        self.patterns.iter().find_map(|pattern| {
            if !pattern.regex.is_match(norm_text) {
                return None;
            }
            Some(match pattern.pattern_type {
                HeaderFooterType::PageNumber => HeaderFooterPatternKind::PageNumber,
                HeaderFooterType::Date => HeaderFooterPatternKind::Date,
                HeaderFooterType::Copyright => HeaderFooterPatternKind::Copyright,
                HeaderFooterType::ChapterTitle => HeaderFooterPatternKind::ChapterSectionTitle,
                HeaderFooterType::Title | HeaderFooterType::Other => {
                    HeaderFooterPatternKind::RunningHeader
                }
            })
        })
    }
}

fn classify_band(position: f64, page_height: f64) -> PageBand {
    let h = page_height.max(1.0);
    if position < h * 0.12 {
        PageBand::Top
    } else if position > h * 0.88 {
        PageBand::Bottom
    } else {
        PageBand::Body
    }
}

fn consistent_non_body_band(occurrences: &[PageOccurrence]) -> Option<PageBand> {
    let mut bands = occurrences
        .iter()
        .map(|o| o.band)
        .filter(|b| *b != PageBand::Body);
    let first = bands.next()?;
    if bands.all(|b| b == first) {
        Some(first)
    } else {
        None
    }
}

fn exact_text_repeats(occurrences: &[PageOccurrence]) -> bool {
    let mut exact = occurrences
        .iter()
        .filter(|occurrence| !occurrence.exact_text.is_empty())
        .map(|occurrence| occurrence.exact_text.as_str());
    let Some(first) = exact.next() else {
        return false;
    };
    exact.all(|text| text == first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_page_body_text_is_not_repetition_header() {
        let mut detector = HeaderFooterDetector::new(1);
        detector.add_occurrence("This is real body content", 1, 300.0, 12.0, 792.0);
        assert!(detector.analyze().is_empty());
    }

    #[test]
    fn one_page_bottom_page_number_can_be_removed() {
        let mut detector = HeaderFooterDetector::new(1);
        detector.add_occurrence("Page 1", 1, 760.0, 10.0, 792.0);
        assert!(detector.analyze().contains("page #"));
    }

    #[test]
    fn one_page_chapter_title_is_kept() {
        let mut detector = HeaderFooterDetector::new(1);
        detector.add_occurrence("Chapter 1", 1, 35.0, 18.0, 792.0);
        assert!(!detector.analyze().contains("chapter #"));
    }

    #[test]
    fn two_page_section_headings_are_kept() {
        let mut detector = HeaderFooterDetector::new(2);
        detector.add_occurrence("Section 1", 1, 35.0, 18.0, 792.0);
        detector.add_occurrence("Section 2", 2, 35.0, 18.0, 792.0);
        assert!(!detector.analyze().contains("section #"));
    }

    #[test]
    fn three_page_different_section_headings_are_kept() {
        let mut detector = HeaderFooterDetector::new(3);
        detector.add_occurrence("Section 1", 1, 35.0, 18.0, 792.0);
        detector.add_occurrence("Section 2", 2, 35.0, 18.0, 792.0);
        detector.add_occurrence("Section 3", 3, 35.0, 18.0, 792.0);
        assert!(!detector.analyze().contains("section #"));
    }

    #[test]
    fn three_page_exact_repeated_section_running_header_is_removed() {
        let mut detector = HeaderFooterDetector::new(3);
        for page in 1..=3 {
            detector.add_occurrence("Section 1", page, 35.0, 12.0, 792.0);
        }
        assert!(detector.analyze().contains("section #"));
    }

    #[test]
    fn two_page_body_title_is_not_removed() {
        let mut detector = HeaderFooterDetector::new(2);
        detector.add_occurrence("Agreement", 1, 300.0, 14.0, 792.0);
        detector.add_occurrence("Agreement", 2, 300.0, 14.0, 792.0);
        assert!(detector.analyze().is_empty());
    }

    #[test]
    fn three_page_repeated_top_title_is_removed() {
        let mut detector = HeaderFooterDetector::new(3);
        for page in 1..=3 {
            detector.add_occurrence("CONFIDENTIAL", page, 35.0, 10.0, 792.0);
        }
        assert!(detector.analyze().contains("confidential"));
    }

    #[test]
    fn repeated_text_with_position_variance_is_not_removed() {
        let mut detector = HeaderFooterDetector::new(3);
        detector.add_occurrence("CONFIDENTIAL", 1, 25.0, 10.0, 792.0);
        detector.add_occurrence("CONFIDENTIAL", 2, 80.0, 10.0, 792.0);
        detector.add_occurrence("CONFIDENTIAL", 3, 25.0, 10.0, 792.0);
        assert!(detector.analyze().is_empty());
    }

    #[test]
    fn normalized_strict_patterns_still_match_dates_chapters_and_copyright() {
        let detector = HeaderFooterDetector::new(1);
        assert!(detector.strict_pattern_match(&HeaderFooterDetector::normalize_text("11/29/2026")));
        assert!(detector.strict_pattern_match(&HeaderFooterDetector::normalize_text("Chapter 12")));
        assert!(
            detector.strict_pattern_match(&HeaderFooterDetector::normalize_text("Copyright 2026"))
        );
    }
}
