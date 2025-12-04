//! List detection for PDF documents
//!
//! Detects numbered and lettered list patterns and preserves list structure.
//! Handles patterns like: 1. 2. 3., a) b) c), i. ii. iii., (1) (2) (3), etc.

use regex::Regex;
use std::sync::LazyLock;

/// List marker types
#[derive(Debug, Clone, PartialEq)]
pub enum ListMarkerType {
    /// Arabic numerals: 1, 2, 3
    Arabic,
    /// Lowercase letters: a, b, c
    LowerAlpha,
    /// Uppercase letters: A, B, C
    UpperAlpha,
    /// Lowercase roman numerals: i, ii, iii
    LowerRoman,
    /// Uppercase roman numerals: I, II, III
    UpperRoman,
    /// Bullet points: •, -, *, ○
    Bullet,
}

/// A detected list marker
#[derive(Debug, Clone)]
pub struct ListMarker {
    pub marker_type: ListMarkerType,
    pub value: String,        // The actual marker text (e.g., "1.", "(a)", "ii)")
    pub sequence_num: u32,    // Normalized sequence number (1, 2, 3...)
    pub full_match: String,   // Full matched text including trailing space
    pub content_start: usize, // Where the actual content starts after marker
}

/// Result of list detection on a line
#[derive(Debug, Clone)]
pub enum ListDetectionResult {
    /// Line starts with a list marker
    ListItem(ListMarker),
    /// Line is a continuation of previous list item (indented, no marker)
    Continuation,
    /// Line is not part of a list
    NotList,
}

// Regex patterns for list markers
static ARABIC_DOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{1,3})\.\s+").unwrap()
});

static ARABIC_PAREN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{1,3})\)\s+").unwrap()
});

static ARABIC_PAREN_BOTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\((\d{1,3})\)\s+").unwrap()
});

static LOWER_ALPHA_DOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([a-z])\.\s+").unwrap()
});

static LOWER_ALPHA_PAREN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([a-z])\)\s+").unwrap()
});

static LOWER_ALPHA_PAREN_BOTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\(([a-z])\)\s+").unwrap()
});

static UPPER_ALPHA_DOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Z])\.\s+").unwrap()
});

static UPPER_ALPHA_PAREN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Z])\)\s+").unwrap()
});

static UPPER_ALPHA_PAREN_BOTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\(([A-Z])\)\s+").unwrap()
});

static LOWER_ROMAN_DOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([ivxlcdm]+)\.\s+").unwrap()
});

static LOWER_ROMAN_PAREN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([ivxlcdm]+)\)\s+").unwrap()
});

static LOWER_ROMAN_PAREN_BOTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\(([ivxlcdm]+)\)\s+").unwrap()
});

static UPPER_ROMAN_DOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([IVXLCDM]+)\.\s+").unwrap()
});

static UPPER_ROMAN_PAREN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([IVXLCDM]+)\)\s+").unwrap()
});

static BULLET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([•\-\*○◦▪▸►])\s+").unwrap()
});

/// Detect if a line starts with a list marker
pub fn detect_list_marker(text: &str) -> ListDetectionResult {
    let trimmed = text.trim_start();

    if trimmed.is_empty() {
        return ListDetectionResult::NotList;
    }

    // Calculate leading whitespace for continuation detection
    let leading_spaces = text.len() - trimmed.len();

    // Try each pattern in order of specificity

    // Arabic numerals with parentheses: (1), (2), (3)
    if let Some(caps) = ARABIC_PAREN_BOTH.captures(trimmed) {
        let num: u32 = caps[1].parse().unwrap_or(0);
        return ListDetectionResult::ListItem(ListMarker {
            marker_type: ListMarkerType::Arabic,
            value: format!("({})", &caps[1]),
            sequence_num: num,
            full_match: caps[0].to_string(),
            content_start: leading_spaces + caps[0].len(),
        });
    }

    // Arabic numerals: 1. or 1)
    if let Some(caps) = ARABIC_DOT.captures(trimmed) {
        let num: u32 = caps[1].parse().unwrap_or(0);
        // Filter out likely section numbers (very high numbers or followed by another number)
        if num <= 100 {
            return ListDetectionResult::ListItem(ListMarker {
                marker_type: ListMarkerType::Arabic,
                value: format!("{}.", &caps[1]),
                sequence_num: num,
                full_match: caps[0].to_string(),
                content_start: leading_spaces + caps[0].len(),
            });
        }
    }

    if let Some(caps) = ARABIC_PAREN.captures(trimmed) {
        let num: u32 = caps[1].parse().unwrap_or(0);
        if num <= 100 {
            return ListDetectionResult::ListItem(ListMarker {
                marker_type: ListMarkerType::Arabic,
                value: format!("{})", &caps[1]),
                sequence_num: num,
                full_match: caps[0].to_string(),
                content_start: leading_spaces + caps[0].len(),
            });
        }
    }

    // Roman numerals checked BEFORE alpha to correctly classify i, v, x, etc.
    // Lowercase roman: i. ii. iii. or i) ii) iii) or (i) (ii) (iii)
    if let Some(caps) = LOWER_ROMAN_PAREN_BOTH.captures(trimmed) {
        if let Some(num) = roman_to_int(&caps[1].to_lowercase()) {
            return ListDetectionResult::ListItem(ListMarker {
                marker_type: ListMarkerType::LowerRoman,
                value: format!("({})", &caps[1]),
                sequence_num: num,
                full_match: caps[0].to_string(),
                content_start: leading_spaces + caps[0].len(),
            });
        }
    }

    if let Some(caps) = LOWER_ROMAN_DOT.captures(trimmed) {
        if let Some(num) = roman_to_int(&caps[1]) {
            return ListDetectionResult::ListItem(ListMarker {
                marker_type: ListMarkerType::LowerRoman,
                value: format!("{}.", &caps[1]),
                sequence_num: num,
                full_match: caps[0].to_string(),
                content_start: leading_spaces + caps[0].len(),
            });
        }
    }

    if let Some(caps) = LOWER_ROMAN_PAREN.captures(trimmed) {
        if let Some(num) = roman_to_int(&caps[1]) {
            return ListDetectionResult::ListItem(ListMarker {
                marker_type: ListMarkerType::LowerRoman,
                value: format!("{})", &caps[1]),
                sequence_num: num,
                full_match: caps[0].to_string(),
                content_start: leading_spaces + caps[0].len(),
            });
        }
    }

    // Uppercase roman: I. II. III.
    if let Some(caps) = UPPER_ROMAN_DOT.captures(trimmed) {
        if let Some(num) = roman_to_int(&caps[1].to_lowercase()) {
            return ListDetectionResult::ListItem(ListMarker {
                marker_type: ListMarkerType::UpperRoman,
                value: format!("{}.", &caps[1]),
                sequence_num: num,
                full_match: caps[0].to_string(),
                content_start: leading_spaces + caps[0].len(),
            });
        }
    }

    if let Some(caps) = UPPER_ROMAN_PAREN.captures(trimmed) {
        if let Some(num) = roman_to_int(&caps[1].to_lowercase()) {
            return ListDetectionResult::ListItem(ListMarker {
                marker_type: ListMarkerType::UpperRoman,
                value: format!("{})", &caps[1]),
                sequence_num: num,
                full_match: caps[0].to_string(),
                content_start: leading_spaces + caps[0].len(),
            });
        }
    }

    // Lowercase alpha with parentheses: (a), (b), (c)
    if let Some(caps) = LOWER_ALPHA_PAREN_BOTH.captures(trimmed) {
        let ch = caps[1].chars().next().unwrap();
        return ListDetectionResult::ListItem(ListMarker {
            marker_type: ListMarkerType::LowerAlpha,
            value: format!("({})", &caps[1]),
            sequence_num: (ch as u32) - ('a' as u32) + 1,
            full_match: caps[0].to_string(),
            content_start: leading_spaces + caps[0].len(),
        });
    }

    // Lowercase alpha: a. or a)
    if let Some(caps) = LOWER_ALPHA_DOT.captures(trimmed) {
        let ch = caps[1].chars().next().unwrap();
        return ListDetectionResult::ListItem(ListMarker {
            marker_type: ListMarkerType::LowerAlpha,
            value: format!("{}.", &caps[1]),
            sequence_num: (ch as u32) - ('a' as u32) + 1,
            full_match: caps[0].to_string(),
            content_start: leading_spaces + caps[0].len(),
        });
    }

    if let Some(caps) = LOWER_ALPHA_PAREN.captures(trimmed) {
        let ch = caps[1].chars().next().unwrap();
        return ListDetectionResult::ListItem(ListMarker {
            marker_type: ListMarkerType::LowerAlpha,
            value: format!("{})", &caps[1]),
            sequence_num: (ch as u32) - ('a' as u32) + 1,
            full_match: caps[0].to_string(),
            content_start: leading_spaces + caps[0].len(),
        });
    }

    // Uppercase alpha with parentheses: (A), (B), (C)
    if let Some(caps) = UPPER_ALPHA_PAREN_BOTH.captures(trimmed) {
        let ch = caps[1].chars().next().unwrap();
        return ListDetectionResult::ListItem(ListMarker {
            marker_type: ListMarkerType::UpperAlpha,
            value: format!("({})", &caps[1]),
            sequence_num: (ch as u32) - ('A' as u32) + 1,
            full_match: caps[0].to_string(),
            content_start: leading_spaces + caps[0].len(),
        });
    }

    // Uppercase alpha: A. or A)
    if let Some(caps) = UPPER_ALPHA_DOT.captures(trimmed) {
        let ch = caps[1].chars().next().unwrap();
        return ListDetectionResult::ListItem(ListMarker {
            marker_type: ListMarkerType::UpperAlpha,
            value: format!("{}.", &caps[1]),
            sequence_num: (ch as u32) - ('A' as u32) + 1,
            full_match: caps[0].to_string(),
            content_start: leading_spaces + caps[0].len(),
        });
    }

    if let Some(caps) = UPPER_ALPHA_PAREN.captures(trimmed) {
        let ch = caps[1].chars().next().unwrap();
        return ListDetectionResult::ListItem(ListMarker {
            marker_type: ListMarkerType::UpperAlpha,
            value: format!("{})", &caps[1]),
            sequence_num: (ch as u32) - ('A' as u32) + 1,
            full_match: caps[0].to_string(),
            content_start: leading_spaces + caps[0].len(),
        });
    }

    // Bullet points
    if let Some(caps) = BULLET.captures(trimmed) {
        return ListDetectionResult::ListItem(ListMarker {
            marker_type: ListMarkerType::Bullet,
            value: caps[1].to_string(),
            sequence_num: 0, // Bullets don't have sequence
            full_match: caps[0].to_string(),
            content_start: leading_spaces + caps[0].len(),
        });
    }

    ListDetectionResult::NotList
}

/// Convert roman numeral string to integer
fn roman_to_int(s: &str) -> Option<u32> {
    let s = s.to_lowercase();
    let mut result: i32 = 0;
    let mut prev_value: i32 = 0;

    for c in s.chars().rev() {
        let value = match c {
            'i' => 1,
            'v' => 5,
            'x' => 10,
            'l' => 50,
            'c' => 100,
            'd' => 500,
            'm' => 1000,
            _ => return None,
        };

        if value < prev_value {
            result -= value;
        } else {
            result += value;
        }
        prev_value = value;
    }

    if result > 0 && result <= 100 {
        Some(result as u32)
    } else {
        None
    }
}

/// Check if a sequence of markers forms a valid list
/// (consecutive numbers, not skipping too many)
pub fn is_valid_list_sequence(markers: &[ListMarker]) -> bool {
    if markers.len() < 2 {
        return false;
    }

    // All markers should be same type
    let first_type = &markers[0].marker_type;
    if !markers.iter().all(|m| &m.marker_type == first_type) {
        return false;
    }

    // For bullets, any sequence is valid
    if *first_type == ListMarkerType::Bullet {
        return true;
    }

    // For numbered lists, check sequence
    let mut prev_num = 0;
    for marker in markers {
        // Allow starting at 1 or continuing from previous
        if marker.sequence_num != prev_num + 1 && !(prev_num == 0 && marker.sequence_num == 1) {
            // Allow small gaps (missing items)
            if marker.sequence_num > prev_num + 3 {
                return false;
            }
        }
        prev_num = marker.sequence_num;
    }

    true
}

/// Format a list marker for markdown output
pub fn format_list_marker(marker: &ListMarker, indent_level: usize) -> String {
    let indent = "   ".repeat(indent_level);
    match marker.marker_type {
        ListMarkerType::Bullet => format!("{}- ", indent),
        _ => format!("{}{}. ", indent, marker.sequence_num),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arabic_dot() {
        match detect_list_marker("1. First item") {
            ListDetectionResult::ListItem(m) => {
                assert_eq!(m.marker_type, ListMarkerType::Arabic);
                assert_eq!(m.sequence_num, 1);
            }
            _ => panic!("Should detect arabic list"),
        }
    }

    #[test]
    fn test_arabic_paren() {
        match detect_list_marker("2) Second item") {
            ListDetectionResult::ListItem(m) => {
                assert_eq!(m.marker_type, ListMarkerType::Arabic);
                assert_eq!(m.sequence_num, 2);
            }
            _ => panic!("Should detect arabic list"),
        }
    }

    #[test]
    fn test_lower_alpha() {
        match detect_list_marker("a. First sub-item") {
            ListDetectionResult::ListItem(m) => {
                assert_eq!(m.marker_type, ListMarkerType::LowerAlpha);
                assert_eq!(m.sequence_num, 1);
            }
            _ => panic!("Should detect lower alpha list"),
        }

        match detect_list_marker("(b) Second sub-item") {
            ListDetectionResult::ListItem(m) => {
                assert_eq!(m.marker_type, ListMarkerType::LowerAlpha);
                assert_eq!(m.sequence_num, 2);
            }
            _ => panic!("Should detect lower alpha list"),
        }
    }

    #[test]
    fn test_roman_numerals() {
        match detect_list_marker("i. First roman") {
            ListDetectionResult::ListItem(m) => {
                assert_eq!(m.marker_type, ListMarkerType::LowerRoman);
                assert_eq!(m.sequence_num, 1);
            }
            _ => panic!("Should detect roman numeral"),
        }

        match detect_list_marker("iv. Fourth roman") {
            ListDetectionResult::ListItem(m) => {
                assert_eq!(m.marker_type, ListMarkerType::LowerRoman);
                assert_eq!(m.sequence_num, 4);
            }
            _ => panic!("Should detect roman numeral"),
        }
    }

    #[test]
    fn test_bullet() {
        match detect_list_marker("• Bullet point") {
            ListDetectionResult::ListItem(m) => {
                assert_eq!(m.marker_type, ListMarkerType::Bullet);
            }
            _ => panic!("Should detect bullet"),
        }

        match detect_list_marker("- Dash bullet") {
            ListDetectionResult::ListItem(m) => {
                assert_eq!(m.marker_type, ListMarkerType::Bullet);
            }
            _ => panic!("Should detect dash bullet"),
        }
    }

    #[test]
    fn test_not_list() {
        match detect_list_marker("This is regular text") {
            ListDetectionResult::NotList => {}
            _ => panic!("Should not detect list"),
        }

        match detect_list_marker("The year 2024 was eventful") {
            ListDetectionResult::NotList => {}
            _ => panic!("Should not detect year as list"),
        }
    }

    #[test]
    fn test_roman_conversion() {
        assert_eq!(roman_to_int("i"), Some(1));
        assert_eq!(roman_to_int("iv"), Some(4));
        assert_eq!(roman_to_int("ix"), Some(9));
        assert_eq!(roman_to_int("xlii"), Some(42));
    }
}
