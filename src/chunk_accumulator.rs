use crate::document::processing::ContentOutput;
use crate::{BoundingBox, PagePosition, TextSegment};
use lazy_static::lazy_static;
use log::error;
use regex::Regex;
use std::sync::Arc;
use tiktoken_rs::CoreBPE;
use unicode_segmentation::UnicodeSegmentation;

/// Track token statistics to improve word-to-token ratio estimation
#[derive(Default, Clone)]
pub struct TokenStats {
    total_words: usize,
    total_tokens: usize,
}

impl TokenStats {
    pub fn update(&mut self, words: usize, tokens: usize) {
        self.total_words += words;
        self.total_tokens += tokens;
    }

    pub fn current_ratio(&self) -> f64 {
        if self.total_words == 0 {
            1.3 // Default ratio for English
        } else {
            self.total_tokens as f64 / self.total_words as f64
        }
    }
}

/// Count Unicode words properly
pub fn count_words(text: &str) -> usize {
    text.unicode_words().count()
}

/// Check if text ends with a sentence boundary
pub fn ends_with_sentence_boundary(text: &str) -> bool {
    lazy_static! {
        static ref SENTENCE_END: Regex = Regex::new(r#"[.!?…。！？]+['")\]]?\s*$"#).unwrap();
    }
    SENTENCE_END.is_match(text)
}

/// Check if text contains a sentence end
pub fn contains_sentence_end(text: &str) -> bool {
    lazy_static! {
        static ref SENTENCE_MIDDLE: Regex =
            Regex::new(r#"[.!?…。！？]+['")\]]?\s+[A-Z0-9]"#).unwrap();
    }
    SENTENCE_MIDDLE.is_match(text) || ends_with_sentence_boundary(text)
}

/// Split a long sentence at clause boundaries or words
pub fn split_long_sentence(text: &str, max_tokens: usize, tokenizer: &CoreBPE) -> Vec<String> {
    // Try clause separators first
    lazy_static! {
        static ref CLAUSE_SPLIT: Regex = Regex::new(r"([;,—])\s+").unwrap();
    }

    // First check if we can split at clauses
    if let Some(captures) = CLAUSE_SPLIT.captures(text) {
        let mut chunks = Vec::new();
        let mut current_chunk = String::new();
        let mut last_end = 0;

        for mat in CLAUSE_SPLIT.find_iter(text) {
            let part = &text[last_end..mat.start()];
            let separator = mat.as_str();

            // Check if adding this part would exceed limit
            let test_text = if current_chunk.is_empty() {
                part.to_string()
            } else {
                format!("{}{}{}", current_chunk, separator, part)
            };

            let tokens = tokenizer.encode_ordinary(&test_text).len();
            if tokens > max_tokens && !current_chunk.is_empty() {
                // Push current chunk and start new one
                chunks.push(current_chunk.trim().to_string());
                current_chunk = part.to_string();
            } else {
                // Add to current chunk
                if !current_chunk.is_empty() {
                    current_chunk.push_str(separator);
                }
                current_chunk.push_str(part);
            }

            last_end = mat.end();
        }

        // Add remaining text
        if last_end < text.len() {
            let remaining = &text[last_end..];
            if !current_chunk.is_empty() {
                current_chunk.push_str(" ");
            }
            current_chunk.push_str(remaining);
        }

        if !current_chunk.is_empty() {
            chunks.push(current_chunk);
        }

        if chunks.len() > 1 {
            return chunks;
        }
    }

    // Fallback: split at word boundaries
    let words: Vec<&str> = text.unicode_words().collect();
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_tokens = 0;

    for word in words {
        let word_tokens = tokenizer.encode_ordinary(word).len();
        if current_tokens + word_tokens > max_tokens && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current = word.to_string();
            current_tokens = word_tokens;
        } else {
            if !current.is_empty() {
                current.push(' ');
                current_tokens += 1; // Approximate space as 1 token
            }
            current.push_str(word);
            current_tokens += word_tokens;
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Accumulates text segments into chunks with efficient tokenization
pub struct ChunkAccumulator {
    // Current chunk being built
    segments: Vec<TextSegment>,
    text: String,
    word_count: usize,
    token_count: Option<usize>,
    token_buffer: Vec<usize>, // Store token IDs temporarily

    // Configuration
    max_tokens: usize,
    word_threshold: usize,
    max_overshoot: usize,
    tokenizer: Arc<CoreBPE>,
    token_stats: TokenStats,

    // Current heading hierarchy
    current_headings: Vec<String>,
}

impl ChunkAccumulator {
    pub fn new(max_tokens: usize, tokenizer: Arc<CoreBPE>) -> Self {
        let word_threshold = ((max_tokens as f64 * 0.8) / 1.3) as usize;
        Self {
            segments: Vec::new(),
            text: String::new(),
            word_count: 0,
            token_count: None,
            token_buffer: Vec::new(),
            max_tokens,
            word_threshold,
            max_overshoot: max_tokens + 50, // Allow small overshoot for sentence completion
            tokenizer,
            token_stats: TokenStats::default(),
            current_headings: Vec::new(),
        }
    }

    /// Reset for next chunk
    pub fn reset(&mut self) {
        self.segments.clear();
        self.text.clear();
        self.word_count = 0;
        self.token_count = None;
        self.token_buffer.clear();

        // Update word threshold based on learned ratio
        let ratio = self.token_stats.current_ratio();
        self.word_threshold = ((self.max_tokens as f64 * 0.8) / ratio) as usize;
    }

    /// Set current heading hierarchy
    pub fn set_headings(&mut self, headings: Vec<String>) {
        self.current_headings = headings;
    }

    /// Check if we can add a segment without exceeding token limit
    pub fn can_add_segment(&mut self, segment: &TextSegment) -> bool {
        let segment_words = segment.word_count;
        let new_word_count = self.word_count + segment_words;

        // Fast path: well below threshold
        if new_word_count < self.word_threshold {
            return true;
        }

        // Need precise token count
        if self.token_count.is_none() {
            // First tokenization of accumulated text
            let tokens = self.tokenizer.encode_ordinary(&self.text);
            self.token_count = Some(tokens.len());

            // Update statistics
            self.token_stats.update(self.word_count, tokens.len());
        }

        // Tokenize the new segment
        let segment_tokens = self.tokenizer.encode_ordinary(&segment.content).len();

        // Account for potential space token when joining
        let space_token = if !self.text.is_empty()
            && !self.text.ends_with(' ')
            && !self.text.ends_with('\n')
            && !segment.content.starts_with(' ')
        {
            1
        } else {
            0
        };

        let total_tokens = self.token_count.unwrap() + segment_tokens + space_token;

        total_tokens <= self.max_tokens
    }

    /// Add a segment to the current chunk
    pub fn add_segment(&mut self, segment: TextSegment) {
        self.word_count += segment.word_count;

        // Smart text joining
        let mut space_added = false;
        if !self.text.is_empty() {
            // Check if we need a space
            let needs_space = !self.text.ends_with(' ')
                && !self.text.ends_with('\n')
                && !segment.content.starts_with(' ');
            if needs_space {
                self.text.push(' ');
                space_added = true;
            }
        }
        self.text.push_str(&segment.content);

        // Update token count if we're tracking it
        if let Some(count) = self.token_count {
            let segment_tokens = self.tokenizer.encode_ordinary(&segment.content).len();
            // Account for the space token if one was added
            let space_token = if space_added { 1 } else { 0 };
            self.token_count = Some(count + segment_tokens + space_token);
        } else if self.word_count >= self.word_threshold {
            // Start tracking tokens if we're approaching the threshold
            let tokens = self.tokenizer.encode_ordinary(&self.text);
            self.token_count = Some(tokens.len());
        }

        self.segments.push(segment);
    }

    /// Check if we should flush at a sentence boundary
    pub fn should_flush(&self) -> bool {
        if let Some(tokens) = self.token_count {
            // Flush when we're at 90% capacity and at a sentence boundary
            if tokens > (self.max_tokens * 9 / 10) {
                return ends_with_sentence_boundary(&self.text);
            }
        }
        false
    }

    /// Check if adding a segment would cross page boundary
    pub fn would_cross_page(&self, segment: &TextSegment) -> bool {
        if let Some(last_segment) = self.segments.last() {
            last_segment.page_num != segment.page_num
        } else {
            false
        }
    }

    /// Force add a segment to complete a sentence (with overshoot limit)
    pub fn can_force_add_for_sentence(&self, segment: &TextSegment) -> bool {
        if let Some(current) = self.token_count {
            let segment_tokens = self.tokenizer.encode_ordinary(&segment.content).len();
            // Account for potential space token when joining
            let space_token = if !self.text.is_empty()
                && !self.text.ends_with(' ')
                && !self.text.ends_with('\n')
                && !segment.content.starts_with(' ')
            {
                1
            } else {
                0
            };
            current + segment_tokens + space_token <= self.max_overshoot
        } else {
            // If we haven't started counting tokens, allow it
            true
        }
    }

    /// Check if accumulator is empty
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Get current token count if available
    pub fn get_token_count(&self) -> Option<usize> {
        self.token_count
    }

    /// Get current word count
    pub fn get_word_count(&self) -> usize {
        self.word_count
    }

    /// Create output from current chunk
    pub fn create_output(&self) -> ContentOutput {
        // Debug: Check final token count
        let actual_tokens = self.tokenizer.encode_ordinary(&self.text).len();
        if actual_tokens > self.max_tokens {
            error!(
                "WARNING: Creating output with {} actual tokens, exceeds max_tokens {}",
                actual_tokens, self.max_tokens
            );
            error!("Text preview: {}", &self.text[..100.min(self.text.len())]);
            if let Some(tracked) = self.token_count {
                error!(
                    "Tracked tokens: {}, Actual tokens: {}",
                    tracked, actual_tokens
                );
            }
        }
        // Calculate page range
        let start_page = self.segments.first().map(|s| s.page_num).unwrap_or(0);
        let end_page = self.segments.last().map(|s| s.page_num).unwrap_or(0);
        let end_page = if end_page != start_page {
            Some(end_page)
        } else {
            None
        };

        // Character positions
        let page_char_start = self.segments.first().map(|s| s.char_start);
        let page_char_end = self.segments.last().map(|s| s.char_end);

        // Build page positions
        let mut page_positions = Vec::new();
        let mut current_page = start_page;
        let mut page_segments = Vec::new();

        for segment in &self.segments {
            if segment.page_num != current_page {
                // Process segments for previous page
                if !page_segments.is_empty() {
                    let bbox = calculate_bbox_for_segments(&page_segments);
                    page_positions.push(PagePosition {
                        page: current_page,
                        char_start: page_segments.first().unwrap().char_start,
                        char_end: page_segments.last().unwrap().char_end,
                        bbox,
                    });
                }
                page_segments.clear();
                current_page = segment.page_num;
            }
            page_segments.push(segment);
        }

        // Don't forget the last page
        if !page_segments.is_empty() {
            let bbox = calculate_bbox_for_segments(&page_segments);
            page_positions.push(PagePosition {
                page: current_page,
                char_start: page_segments.first().unwrap().char_start,
                char_end: page_segments.last().unwrap().char_end,
                bbox,
            });
        }

        // Overall bounding box (single page only)
        let bbox = if end_page.is_none() && !self.segments.is_empty() {
            let segment_refs: Vec<&TextSegment> = self.segments.iter().collect();
            Some(calculate_bbox_for_segments(&segment_refs))
        } else {
            None
        };

        ContentOutput {
            headings: self.current_headings.clone(),
            paragraph: self.text.trim().to_string(),
            page: start_page,
            end_page,
            page_char_start,
            page_char_end,
            bbox,
            page_positions,
        }
    }

    /// Create output with warning flag for truncated sentences
    pub fn create_output_with_warning(&self) -> ContentOutput {
        let output = self.create_output();
        // Could add a warning field if needed
        output
    }
}

/// Calculate bounding box for a group of segments
fn calculate_bbox_for_segments(segments: &[&TextSegment]) -> BoundingBox {
    let min_x = segments.iter().map(|s| s.x).fold(f64::INFINITY, f64::min);
    let max_x = segments
        .iter()
        .map(|s| s.x + s.width)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = segments.iter().map(|s| s.y).fold(f64::INFINITY, f64::min);
    let max_y = segments
        .iter()
        .map(|s| s.y + s.height)
        .fold(f64::NEG_INFINITY, f64::max);

    BoundingBox {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_counting() {
        assert_eq!(count_words("Hello world"), 2);
        assert_eq!(count_words("e-mail co-operation"), 4); // Proper handling
        assert_eq!(count_words("Hello, world!"), 2);
        assert_eq!(count_words("   multiple   spaces   "), 2);
    }

    #[test]
    fn test_sentence_boundaries() {
        assert!(ends_with_sentence_boundary("End of sentence."));
        assert!(ends_with_sentence_boundary("Question?"));
        assert!(ends_with_sentence_boundary("Exclamation!"));
        assert!(ends_with_sentence_boundary("Quote.\""));
        assert!(ends_with_sentence_boundary("Parenthetical.)"));
        assert!(!ends_with_sentence_boundary("Not the end"));
        assert!(!ends_with_sentence_boundary("Comma,"));
    }
}
