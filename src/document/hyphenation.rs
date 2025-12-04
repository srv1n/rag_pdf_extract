//! Hyphenation recovery for PDF documents
//!
//! Joins words that were split across lines with hyphens.
//! Distinguishes soft hyphens (word continuation) from hard hyphens (compound words).

use crate::TextSegment;

/// Common word prefixes that often appear before hyphens in compound words
const COMPOUND_PREFIXES: &[&str] = &[
    "self", "well", "ill", "cross", "all", "ex", "half", "high", "low",
    "mid", "non", "anti", "co", "pre", "post", "re", "sub", "semi",
    "multi", "inter", "intra", "counter", "super", "ultra", "under",
    "over", "out", "pseudo", "quasi", "vice",
];

/// Common compound word patterns (words that should keep their hyphen)
const COMPOUND_PATTERNS: &[&str] = &[
    "state-of", "out-of", "up-to", "day-to", "face-to", "one-on",
    "word-of", "matter-of", "point-of", "end-to", "back-to",
    "well-known", "well-being", "self-", "non-", "anti-", "co-",
    "e-mail", "e-commerce", "re-elect", "re-enter",
];

/// Result of hyphen analysis
#[derive(Debug, Clone, PartialEq)]
pub enum HyphenType {
    /// Soft hyphen - word was split for line break, should be joined
    Soft,
    /// Hard hyphen - compound word, hyphen should be preserved
    Hard,
    /// Not a hyphen case
    None,
}

/// Analyze if a hyphen at end of text is soft (continuation) or hard (compound)
pub fn analyze_hyphen(text_before: &str, text_after: &str) -> HyphenType {
    let trimmed_before = text_before.trim_end();
    let trimmed_after = text_after.trim_start();

    // Must end with hyphen
    if !trimmed_before.ends_with('-') {
        return HyphenType::None;
    }

    // Get the word fragment before hyphen
    let word_before = trimmed_before
        .trim_end_matches('-')
        .split_whitespace()
        .last()
        .unwrap_or("");

    // Get the word fragment after (first word of next line)
    let word_after = trimmed_after
        .split_whitespace()
        .next()
        .unwrap_or("");

    if word_before.is_empty() || word_after.is_empty() {
        return HyphenType::None;
    }

    // Check for compound word patterns
    if is_likely_compound(word_before, word_after) {
        return HyphenType::Hard;
    }

    // Check if joining creates a plausible word
    // Heuristic: if next line starts with lowercase, likely continuation
    if let Some(first_char) = word_after.chars().next() {
        if first_char.is_lowercase() {
            return HyphenType::Soft;
        }
    }

    // Default: if word_before is short and looks like a prefix, it's soft
    if word_before.len() <= 4 && !is_standalone_word(word_before) {
        return HyphenType::Soft;
    }

    // If word_before ends with consonant cluster and word_after starts with vowel,
    // likely a soft hyphen (word split at syllable boundary)
    if ends_with_consonants(word_before) && starts_with_vowel(word_after) {
        return HyphenType::Soft;
    }

    // Default to soft hyphen for typical line-break cases
    HyphenType::Soft
}

/// Check if the combination is likely a compound word
fn is_likely_compound(before: &str, after: &str) -> bool {
    let before_lower = before.to_lowercase();
    let after_lower = after.to_lowercase();

    // Check if before is a known compound prefix
    for prefix in COMPOUND_PREFIXES {
        if before_lower == *prefix {
            return true;
        }
    }

    // Check for known compound patterns
    let combined = format!("{}-{}", before_lower, after_lower);
    for pattern in COMPOUND_PATTERNS {
        if combined.starts_with(pattern) || combined.contains(pattern) {
            return true;
        }
    }

    // If after starts with uppercase, likely a compound (e.g., "Indo-European")
    if after.chars().next().map_or(false, |c| c.is_uppercase()) {
        // But not if it's just the start of a new sentence
        if before.ends_with(|c: char| c.is_alphabetic()) {
            return true;
        }
    }

    false
}

/// Check if a short string is likely a standalone word vs. word fragment
fn is_standalone_word(word: &str) -> bool {
    let lower = word.to_lowercase();
    // Common short words that shouldn't be joined
    matches!(
        lower.as_str(),
        "the" | "and" | "for" | "but" | "not" | "you" | "all" | "can" | "had" |
        "her" | "was" | "one" | "our" | "out" | "are" | "has" | "his" | "how" |
        "its" | "may" | "new" | "now" | "old" | "see" | "two" | "way" | "who" |
        "did" | "get" | "let" | "put" | "say" | "she" | "too" | "use" | "man" |
        "day" | "per" | "sub" | "non" | "pre" | "pro"
    )
}

/// Check if string ends with consonant cluster
fn ends_with_consonants(s: &str) -> bool {
    let consonants = "bcdfghjklmnpqrstvwxyz";
    let chars: Vec<char> = s.to_lowercase().chars().collect();

    if chars.len() >= 2 {
        let last = chars[chars.len() - 1];
        let second_last = chars[chars.len() - 2];
        consonants.contains(last) && consonants.contains(second_last)
    } else if chars.len() == 1 {
        consonants.contains(chars[0])
    } else {
        false
    }
}

/// Check if string starts with vowel
fn starts_with_vowel(s: &str) -> bool {
    let vowels = "aeiouAEIOU";
    s.chars().next().map_or(false, |c| vowels.contains(c))
}

/// Join hyphenated word across two text segments
pub fn join_hyphenated(before: &str, after: &str) -> Option<String> {
    let trimmed_before = before.trim_end();

    if !trimmed_before.ends_with('-') {
        return None;
    }

    match analyze_hyphen(before, after) {
        HyphenType::Soft => {
            // Remove hyphen and join
            let without_hyphen = trimmed_before.trim_end_matches('-');
            let trimmed_after = after.trim_start();

            // Get everything before the last word
            let prefix = if let Some(last_space) = without_hyphen.rfind(' ') {
                &without_hyphen[..=last_space]
            } else {
                ""
            };

            // Get the word fragment
            let word_frag = without_hyphen.split_whitespace().last().unwrap_or("");

            // Get the continuation and rest
            let after_parts: Vec<&str> = trimmed_after.splitn(2, char::is_whitespace).collect();
            let continuation = after_parts.get(0).unwrap_or(&"");
            let rest = after_parts.get(1).map(|s| format!(" {}", s)).unwrap_or_default();

            Some(format!("{}{}{}{}", prefix, word_frag, continuation, rest))
        }
        HyphenType::Hard => {
            // Keep hyphen
            None
        }
        HyphenType::None => None,
    }
}

/// Process a vector of segments, joining hyphenated words across segment boundaries
pub fn recover_hyphenation(segments: Vec<TextSegment>) -> Vec<TextSegment> {
    if segments.len() < 2 {
        return segments;
    }

    let mut result: Vec<TextSegment> = Vec::with_capacity(segments.len());
    let mut i = 0;

    while i < segments.len() {
        let current = &segments[i];

        // Check if this segment ends with a hyphen and next segment continues
        if i + 1 < segments.len() {
            let next = &segments[i + 1];

            // Only join if same page or consecutive pages
            let same_or_consecutive_page =
                current.page_num == next.page_num ||
                current.page_num + 1 == next.page_num;

            if same_or_consecutive_page {
                if let Some(joined) = join_hyphenated(&current.content, &next.content) {
                    // Create merged segment
                    let mut merged = current.clone();
                    merged.content = joined;
                    merged.char_end = next.char_end;
                    merged.width = current.width + next.width;
                    merged.word_count = merged.content.split_whitespace().count();

                    result.push(merged);
                    i += 2; // Skip both segments
                    continue;
                }
            }
        }

        // No hyphen joining, keep segment as-is
        result.push(current.clone());
        i += 1;
    }

    result
}

/// Clean hyphenation within a single text string
/// (for text that's already been combined but has internal soft hyphens)
pub fn clean_internal_hyphens(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let lines: Vec<&str> = text.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_end();

        if i + 1 < lines.len() && trimmed.ends_with('-') {
            let next_line = lines[i + 1].trim_start();

            match analyze_hyphen(trimmed, next_line) {
                HyphenType::Soft => {
                    // Remove hyphen, don't add newline
                    result.push_str(trimmed.trim_end_matches('-'));
                }
                _ => {
                    // Keep hyphen and newline
                    result.push_str(line);
                    result.push('\n');
                }
            }
        } else {
            result.push_str(line);
            if i + 1 < lines.len() {
                result.push('\n');
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soft_hyphen_detection() {
        // Clear word continuation
        assert_eq!(
            analyze_hyphen("competi-", "tion is important"),
            HyphenType::Soft
        );

        assert_eq!(
            analyze_hyphen("The govern-", "ment decided"),
            HyphenType::Soft
        );
    }

    #[test]
    fn test_hard_hyphen_detection() {
        // Compound words
        assert_eq!(
            analyze_hyphen("self-", "evident truth"),
            HyphenType::Hard
        );

        assert_eq!(
            analyze_hyphen("well-", "known fact"),
            HyphenType::Hard
        );

        assert_eq!(
            analyze_hyphen("non-", "profit organization"),
            HyphenType::Hard
        );
    }

    #[test]
    fn test_join_hyphenated() {
        assert_eq!(
            join_hyphenated("competi-", "tion is key"),
            Some("competition is key".to_string())
        );

        assert_eq!(
            join_hyphenated("The govern-", "ment said"),
            Some("The government said".to_string())
        );

        // Should not join compound words
        assert_eq!(
            join_hyphenated("self-", "evident"),
            None
        );
    }

    #[test]
    fn test_clean_internal_hyphens() {
        let text = "The competi-\ntion was fierce";
        assert_eq!(
            clean_internal_hyphens(text),
            "The competition was fierce"
        );

        // Should preserve hard hyphens
        let text2 = "A well-\nknown fact";
        assert_eq!(
            clean_internal_hyphens(text2),
            "A well-\nknown fact"
        );
    }

    #[test]
    fn test_no_hyphen() {
        assert_eq!(
            analyze_hyphen("normal text", "more text"),
            HyphenType::None
        );
    }
}
