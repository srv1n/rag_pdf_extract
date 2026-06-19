use log::debug;
use std::collections::HashSet;

/// OCR text quality filter to remove noise while preserving important information
///
/// This filter uses multiple heuristics to identify and remove OCR noise:
/// - Single character filtering (keeps valid section markers, removes random chars)
/// - Gibberish detection (consonant clusters, repetition patterns)
/// - Word validation (checks for vowels, reasonable patterns)
/// - Noise ratio calculation (ratio of non-alphanumeric characters)
///
/// The filter is enabled by default but can be disabled if needed.
/// It's designed to be computationally efficient without requiring external dictionaries.
pub struct OcrTextFilter {
    /// Minimum word length to consider valid (unless it's a known valid short word)
    min_word_length: usize,
    /// Maximum ratio of non-alphanumeric characters allowed
    max_noise_ratio: f32,
    /// Common valid single characters (section markers, etc.)
    valid_single_chars: HashSet<char>,
    /// Common valid short words
    valid_short_words: HashSet<String>,
}

impl Default for OcrTextFilter {
    fn default() -> Self {
        // Initialize with common valid single characters and short words
        let mut valid_single_chars = HashSet::new();
        // Section markers, bullet points, common abbreviations
        for c in [
            'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q',
            'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h',
            'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y',
            'z', '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', '•', '·', '◦', '▪', '▫', '‣',
            '⁃', '※', '→', '»',
        ] {
            valid_single_chars.insert(c);
        }

        let mut valid_short_words = HashSet::new();
        // Common 1-2 letter words that are valid
        for word in [
            "I", "a", "A", "in", "In", "IN", "on", "On", "ON", "at", "At", "AT", "to", "To", "TO",
            "by", "By", "BY", "of", "Of", "OF", "or", "Or", "OR", "is", "Is", "IS", "it", "It",
            "IT", "we", "We", "WE", "he", "He", "HE", "be", "Be", "BE", "as", "As", "AS", "do",
            "Do", "DO", "if", "If", "IF", "so", "So", "SO", "no", "No", "NO", "my", "My", "MY",
            "an", "An", "AN", "up", "Up", "UP", "go", "Go", "GO", "me", "Me", "ME", "am", "Am",
            "AM", "vs", "VS", "Vs", "re", "Re", "RE", "ex", "Ex", "EX",
        ] {
            valid_short_words.insert(word.to_string());
        }

        Self {
            min_word_length: 2,
            max_noise_ratio: 0.5,
            valid_single_chars,
            valid_short_words,
        }
    }
}

impl OcrTextFilter {
    /// Filter OCR text to remove noise while preserving important content
    pub fn filter(&self, text: &str) -> String {
        let lines: Vec<&str> = text.lines().collect();
        let mut filtered_lines = Vec::new();

        for line in lines {
            let trimmed = line.trim();

            // Skip empty lines
            if trimmed.is_empty() {
                continue;
            }

            // Check if line should be filtered
            if self.should_keep_line(trimmed) {
                filtered_lines.push(trimmed);
            } else {
                debug!("Filtered out noisy line: {:?}", trimmed);
            }
        }

        filtered_lines.join("\n")
    }

    /// Determine if a line should be kept based on quality heuristics
    fn should_keep_line(&self, line: &str) -> bool {
        // Special case: single character lines
        if line.chars().count() == 1 {
            return self.is_valid_single_char(line);
        }

        // Split into words
        let words: Vec<&str> = line.split_whitespace().collect();

        // If no words, skip
        if words.is_empty() {
            return false;
        }

        // Count valid vs invalid words
        let mut valid_words = 0;
        let mut invalid_words = 0;

        for word in &words {
            if self.is_valid_word(word) {
                valid_words += 1;
            } else {
                invalid_words += 1;
            }
        }

        // If we have no valid words, filter out
        if valid_words == 0 {
            return false;
        }

        // Check noise ratio for the entire line
        let noise_ratio = self.calculate_noise_ratio(line);
        if noise_ratio > self.max_noise_ratio {
            return false;
        }

        // Keep if majority of words are valid
        let valid_ratio = valid_words as f32 / (valid_words + invalid_words) as f32;
        valid_ratio >= 0.5
    }

    /// Check if a single character is valid (section marker, bullet, etc.)
    fn is_valid_single_char(&self, text: &str) -> bool {
        if text.chars().count() != 1 {
            return false;
        }

        let ch = text.chars().next().unwrap();

        // Check if it's in our valid single chars set
        if self.valid_single_chars.contains(&ch) {
            return true;
        }

        // Check if it's a letter or digit (potential section marker)
        ch.is_alphanumeric()
    }

    /// Check if a word is likely valid
    fn is_valid_word(&self, word: &str) -> bool {
        // Remove common punctuation from edges
        let cleaned = word.trim_matches(|c: char| !c.is_alphanumeric());

        // Empty after cleaning = invalid
        if cleaned.is_empty() {
            return false;
        }

        // Check valid short words
        if cleaned.len() <= 2 {
            return self.valid_short_words.contains(cleaned);
        }

        // For longer words, check character composition
        let alpha_count = cleaned.chars().filter(|c| c.is_alphabetic()).count();
        let digit_count = cleaned.chars().filter(|c| c.is_ascii_digit()).count();
        let total_alnum = alpha_count + digit_count;

        // Must be mostly alphanumeric
        if total_alnum < cleaned.len() / 2 {
            return false;
        }

        // Check for reasonable patterns
        if cleaned.len() >= self.min_word_length {
            // Has at least one vowel (for non-acronyms)
            let has_vowel = cleaned.chars().any(|c| "aeiouAEIOU".contains(c));

            // All caps might be acronym
            let all_caps = cleaned
                .chars()
                .all(|c| !c.is_alphabetic() || c.is_uppercase());

            // Number patterns (dates, IDs, etc.)
            let has_digits = digit_count > 0;

            // Valid if: has vowels, or is all caps (acronym), or contains digits (ID/date)
            return has_vowel || all_caps || has_digits;
        }

        true
    }

    /// Calculate the ratio of non-alphanumeric characters (noise)
    fn calculate_noise_ratio(&self, text: &str) -> f32 {
        if text.is_empty() {
            return 1.0;
        }

        let total_chars = text.len();
        let alnum_chars = text
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .count();

        1.0 - (alnum_chars as f32 / total_chars as f32)
    }

    /// Advanced gibberish detection using character frequency analysis
    pub fn is_likely_gibberish(&self, text: &str) -> bool {
        // Too short to analyze
        if text.len() < 3 {
            return false;
        }

        // Check for excessive consonant clusters (unlikely in real text)
        let consonant_clusters = self.count_consonant_clusters(text);
        if consonant_clusters > text.len() / 4 {
            return true;
        }

        // Check for repeated patterns (common in OCR errors)
        if self.has_excessive_repetition(text) {
            return true;
        }

        // Check character diversity (gibberish often has limited character set)
        let unique_chars: HashSet<char> = text.chars().collect();
        let diversity_ratio = unique_chars.len() as f32 / text.len() as f32;

        // Very low diversity suggests repetitive gibberish
        if text.len() > 10 && diversity_ratio < 0.3 {
            return true;
        }

        false
    }

    /// Count consonant clusters (3+ consonants in a row)
    fn count_consonant_clusters(&self, text: &str) -> usize {
        let consonants = "bcdfghjklmnpqrstvwxyzBCDFGHJKLMNPQRSTVWXYZ";
        let mut cluster_count = 0;
        let mut consecutive = 0;

        for ch in text.chars() {
            if consonants.contains(ch) {
                consecutive += 1;
                if consecutive >= 3 {
                    cluster_count += 1;
                }
            } else {
                consecutive = 0;
            }
        }

        cluster_count
    }

    /// Check for excessive character repetition
    fn has_excessive_repetition(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let mut max_repeat = 0;
        let mut current_repeat = 1;

        for i in 1..chars.len() {
            if chars[i] == chars[i - 1] {
                current_repeat += 1;
                max_repeat = max_repeat.max(current_repeat);
            } else {
                current_repeat = 1;
            }
        }

        // More than 3 of the same character in a row is suspicious
        max_repeat > 3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_single_chars() {
        let filter = OcrTextFilter::default();

        // Valid single chars
        assert_eq!(filter.filter("A"), "A");
        assert_eq!(filter.filter("1"), "1");
        assert_eq!(filter.filter("•"), "•");

        // Invalid single chars
        assert_eq!(filter.filter("&"), "");
        assert_eq!(filter.filter("#"), "");
    }

    #[test]
    fn test_filter_gibberish() {
        let filter = OcrTextFilter::default();

        // Gibberish lines
        assert_eq!(filter.filter("qwrtpsdfg"), "");
        assert_eq!(filter.filter("xxxxx"), "");
        assert_eq!(filter.filter("@#$%^&"), "");

        // Valid text
        assert_eq!(filter.filter("Hello world"), "Hello world");
        assert_eq!(filter.filter("Page 123"), "Page 123");
    }

    #[test]
    fn test_mixed_content() {
        let filter = OcrTextFilter::default();

        let input = "Chapter 1\n&\nIntroduction\nqwrtps\nThis is valid text.\n###\nA\n";
        let expected = "Chapter 1\nIntroduction\nThis is valid text.\nA";

        assert_eq!(filter.filter(input), expected);
    }
}
