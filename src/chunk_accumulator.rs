use crate::document::processing::ContentOutput;
use crate::document::{LocatedText, OutputSpan, SourceRef, SpanSource, SyntheticKind};
use crate::{BoundingBox, PagePosition, TextSegment};
use lazy_static::lazy_static;
use regex::Regex;
use std::sync::Arc;
use tiktoken_rs::CoreBPE;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BoundaryKind {
    Sentence,
    Paragraph,
}

#[derive(Clone, Debug)]
struct BoundaryBookmark {
    seg_idx: usize,
    byte_in_seg: usize,
    text_byte_idx: usize,
    kind: BoundaryKind,
    tokens_at_boundary: Option<usize>,
}

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
            1.5 // Slightly conservative default to reduce early overshoot
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
    if SENTENCE_END.is_match(text) {
        true
    } else {
        let trimmed = text.trim_end_matches([' ', '\t', '\r', '\n']);
        trimmed.ends_with("\n\n")
    }
}

/// Check if text contains a sentence end
pub fn contains_sentence_end(text: &str) -> bool {
    lazy_static! {
        static ref SENTENCE_MIDDLE: Regex =
            Regex::new(r#"[.!?…。！？]+['")\]]?\s+[A-Z0-9]"#).unwrap();
    }
    SENTENCE_MIDDLE.is_match(text) || ends_with_sentence_boundary(text)
}

fn split_by_graphemes(text: &str, max_tokens: usize, tokenizer: &CoreBPE) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for grapheme in text.graphemes(true) {
        let candidate = if current.is_empty() {
            grapheme.to_string()
        } else {
            format!("{current}{grapheme}")
        };

        if !current.is_empty() && tokenizer.encode_ordinary(&candidate).len() > max_tokens {
            chunks.push(current);
            current = grapheme.to_string();
        } else {
            current = candidate;
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

fn split_at_token_boundary(text: &str, max_tokens: usize, tokenizer: &CoreBPE) -> (String, String) {
    if text.is_empty() {
        return (String::new(), String::new());
    }

    let mut low = 0usize;
    let mut high = text.len();
    let mut best = 0usize;

    while low <= high {
        let mut mid = (low + high) / 2;
        while mid > 0 && !text.is_char_boundary(mid) {
            mid -= 1;
        }

        let candidate = text[..mid].trim_end();
        let tokens = tokenizer.encode_ordinary(candidate).len();
        if tokens <= max_tokens {
            best = mid;
            low = mid.saturating_add(1);
        } else if mid == 0 {
            break;
        } else {
            high = mid.saturating_sub(1);
        }
    }

    if best == 0 {
        let mut pieces = split_by_graphemes(text, max_tokens, tokenizer);
        if pieces.is_empty() {
            return (String::new(), String::new());
        }
        let head = pieces.remove(0);
        let tail = text[head.len()..].trim_start().to_string();
        return (head, tail);
    }

    let mut cut = best;
    for pattern in ["\n\n", "\n", ". ", "? ", "! ", "; ", ": ", ", ", " "] {
        if let Some(pos) = text[..best].rfind(pattern) {
            let candidate_cut = pos + pattern.len();
            let candidate = text[..candidate_cut].trim_end();
            if !candidate.is_empty() && tokenizer.encode_ordinary(candidate).len() <= max_tokens {
                cut = candidate_cut;
                break;
            }
        }
    }

    let head = text[..cut].trim().to_string();
    let tail = text[cut..].trim_start().to_string();
    (head, tail)
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
    if words.is_empty() {
        return split_by_graphemes(text, max_tokens, tokenizer);
    }
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

    if chunks.is_empty()
        || chunks
            .iter()
            .any(|chunk| tokenizer.encode_ordinary(chunk).len() > max_tokens)
    {
        return split_by_graphemes(text, max_tokens, tokenizer);
    }

    chunks
}

pub fn split_text_hard_capped(text: &str, max_tokens: usize, tokenizer: &CoreBPE) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut remaining = text.trim().to_string();

    while !remaining.is_empty() {
        if tokenizer.encode_ordinary(&remaining).len() <= max_tokens {
            chunks.push(remaining);
            break;
        }

        let (head, tail) = split_at_token_boundary(&remaining, max_tokens, tokenizer);
        if head.is_empty() {
            break;
        }

        chunks.push(head);
        remaining = tail;
    }

    if chunks.is_empty() {
        return split_by_graphemes(text.trim(), max_tokens, tokenizer);
    }

    chunks
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

fn segment_source_ref(segment: &TextSegment) -> SourceRef {
    SourceRef {
        page: segment.page_num,
        char_start: segment.char_start,
        char_end: segment.char_end,
    }
}

fn push_synthetic_span(
    spans: &mut Vec<OutputSpan>,
    output_start: usize,
    output_end: usize,
    kind: SyntheticKind,
    parent_refs: Vec<SourceRef>,
) {
    if output_start >= output_end {
        return;
    }
    spans.push(OutputSpan {
        output_start,
        output_end,
        source: SpanSource::Synthetic { kind, parent_refs },
    });
}

fn push_pdf_span(
    spans: &mut Vec<OutputSpan>,
    segment: &TextSegment,
    output_start: usize,
    output_end: usize,
    source_offset: usize,
) {
    if output_start >= output_end {
        return;
    }
    let len = output_end - output_start;
    let source_start = segment.char_start.saturating_add(source_offset);
    let source_end = source_start
        .saturating_add(len)
        .min(segment.char_end.max(source_start));
    spans.push(OutputSpan {
        output_start,
        output_end,
        source: SpanSource::Pdf {
            page: segment.page_num,
            char_start: source_start,
            char_end: source_end,
            bbox: BoundingBox {
                x: segment.x,
                y: segment.y,
                width: segment.width,
                height: segment.height,
            },
        },
    });
}

fn heading_marker_len(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut hashes = 0usize;
    while hashes < bytes.len() && hashes < 6 && bytes[hashes] == b'#' {
        hashes += 1;
    }
    if hashes > 0 && bytes.get(hashes) == Some(&b' ') {
        hashes + 1
    } else {
        0
    }
}

fn push_segment_spans(
    spans: &mut Vec<OutputSpan>,
    segment: &TextSegment,
    matched_text: &str,
    output_start: usize,
) {
    let parent_ref = segment_source_ref(segment);
    let text_len = char_len(matched_text);
    if text_len == 0 {
        return;
    }

    let marker_chars = heading_marker_len(matched_text);
    if marker_chars > 0 {
        push_synthetic_span(
            spans,
            output_start,
            output_start + marker_chars,
            SyntheticKind::HeadingMarker,
            vec![parent_ref.clone()],
        );
        push_pdf_span(
            spans,
            segment,
            output_start + marker_chars,
            output_start + text_len,
            0,
        );
        return;
    }

    if segment.font_name == "Table" {
        let mut source_offset = 0usize;
        let mut run_start: Option<usize> = None;
        let mut run_source_offset = 0usize;

        for (idx, ch) in matched_text.chars().enumerate() {
            let table_syntax =
                matches!(ch, '|' | '\n') || (ch == '-' && matched_text.contains("---|"));
            if table_syntax {
                if let Some(start) = run_start.take() {
                    push_pdf_span(
                        spans,
                        segment,
                        output_start + start,
                        output_start + idx,
                        run_source_offset,
                    );
                }
                push_synthetic_span(
                    spans,
                    output_start + idx,
                    output_start + idx + 1,
                    SyntheticKind::TableMarkdown,
                    vec![parent_ref.clone()],
                );
            } else {
                if run_start.is_none() {
                    run_start = Some(idx);
                    run_source_offset = source_offset;
                }
                if !ch.is_whitespace() {
                    source_offset += 1;
                }
            }
        }

        if let Some(start) = run_start {
            push_pdf_span(
                spans,
                segment,
                output_start + start,
                output_start + text_len,
                run_source_offset,
            );
        }
        return;
    }

    push_pdf_span(spans, segment, output_start, output_start + text_len, 0);
}

fn slice_text_segment_by_chars(
    segment: &TextSegment,
    start_chars: usize,
    len_chars: usize,
    new_content: String,
) -> TextSegment {
    let total_chars = segment.content.chars().count().max(1);
    let start_chars = start_chars.min(total_chars);
    let end_chars = start_chars.saturating_add(len_chars).min(total_chars);
    let start_ratio = start_chars as f64 / total_chars as f64;
    let end_ratio = end_chars.max(start_chars + 1) as f64 / total_chars as f64;

    let mut partial = segment.clone();
    partial.content = new_content;
    partial.word_count = count_words(&partial.content);
    partial.char_start = segment.char_start.saturating_add(start_chars);
    partial.char_end = segment.char_start.saturating_add(end_chars);
    partial.x = segment.x + segment.width * start_ratio;
    partial.width = (segment.width * (end_ratio - start_ratio).max(0.0)).max(0.0);
    partial.located_text = segment.located_text.as_ref().map(|located| {
        located.slice_chars(
            start_chars,
            end_chars - start_chars,
            partial.content.clone(),
        )
    });
    partial
}

#[derive(Clone, Debug, Default)]
struct LocatedBuilder {
    text: String,
    spans: Vec<OutputSpan>,
}

impl LocatedBuilder {
    fn clear(&mut self) {
        self.text.clear();
        self.spans.clear();
    }

    fn char_len(&self) -> usize {
        char_len(&self.text)
    }

    fn push_synthetic(&mut self, text: &str, kind: SyntheticKind, parent_refs: Vec<SourceRef>) {
        if text.is_empty() {
            return;
        }
        let start = self.char_len();
        self.text.push_str(text);
        let end = self.char_len();
        push_synthetic_span(&mut self.spans, start, end, kind, parent_refs);
    }

    fn push_source_segment(&mut self, segment: &TextSegment) {
        if let Some(located) = &segment.located_text {
            let start = self.char_len();
            self.text.push_str(&located.text);
            self.spans
                .extend(located.spans.iter().cloned().map(|mut span| {
                    span.output_start += start;
                    span.output_end += start;
                    span
                }));
            return;
        }

        let start = self.char_len();
        self.text.push_str(&segment.content);
        push_segment_spans(&mut self.spans, segment, &segment.content, start);
    }

    fn pop_last_char(&mut self) -> Option<char> {
        let ch = self.text.pop()?;
        let new_len = self.char_len();
        for span in self.spans.iter_mut().rev() {
            if span.output_end > new_len {
                span.output_end = new_len;
                if let SpanSource::Pdf {
                    char_end,
                    char_start,
                    ..
                } = &mut span.source
                {
                    *char_end = (*char_end).saturating_sub(1).max(*char_start);
                }
                break;
            }
        }
        self.spans
            .retain(|span| span.output_start < span.output_end);
        Some(ch)
    }

    fn located_text(&self) -> LocatedText {
        LocatedText {
            text: self.text.clone(),
            spans: self.spans.clone(),
        }
    }

    fn trimmed_located_text(&self) -> LocatedText {
        let leading = self
            .text
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .count();
        let trimmed = self.text.trim().to_string();
        let trimmed_len = trimmed.chars().count();
        self.located_text()
            .slice_chars(leading, trimmed_len, trimmed)
    }

    fn slice_chars(&self, start: usize, len: usize) -> Self {
        let text = self.text.chars().skip(start).take(len).collect::<String>();
        let located = self.located_text().slice_chars(start, len, text);
        Self {
            text: located.text,
            spans: located.spans,
        }
    }
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

    // Boundary bookmarks for backtracking on overflow
    boundaries: Vec<BoundaryBookmark>,

    located_builder: LocatedBuilder,
}

impl ChunkAccumulator {
    pub fn new(max_tokens: usize, tokenizer: Arc<CoreBPE>) -> Self {
        let word_threshold = ((max_tokens as f64 * 0.6) / 1.5) as usize;
        Self {
            segments: Vec::new(),
            text: String::new(),
            word_count: 0,
            token_count: None,
            token_buffer: Vec::new(),
            max_tokens,
            word_threshold,
            max_overshoot: max_tokens, // Enforce strict cap: no overshoot beyond max_tokens
            tokenizer,
            token_stats: TokenStats::default(),
            current_headings: Vec::new(),
            boundaries: Vec::new(),
            located_builder: LocatedBuilder::default(),
        }
    }

    /// Reset for next chunk
    pub fn reset(&mut self) {
        self.segments.clear();
        self.text.clear();
        self.word_count = 0;
        self.token_count = None;
        self.token_buffer.clear();
        self.boundaries.clear();
        self.located_builder.clear();

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
        // Always compute tokens for the incoming segment to prevent silent overshoot
        let segment_tokens = self.tokenizer.encode_ordinary(&segment.content).len();
        let space_token = if !self.text.is_empty()
            && !self.text.ends_with(' ')
            && !self.text.ends_with('\n')
            && !segment.content.starts_with(' ')
        {
            1
        } else {
            0
        };

        // Quick strict check using current known tokens or an estimate
        let ratio = self.token_stats.current_ratio();
        let mut base_tokens = if let Some(t) = self.token_count {
            t
        } else {
            ((self.word_count as f64) * ratio).ceil() as usize
        };
        // If we're about to cross half the capacity, switch to exact counting for the current chunk
        if self.token_count.is_none()
            && base_tokens + space_token + segment_tokens > (self.max_tokens / 2)
        {
            let tokens = self.tokenizer.encode_ordinary(&self.text);
            self.token_count = Some(tokens.len());
            // Update stats so future estimates are better
            self.token_stats.update(self.word_count, tokens.len());
            base_tokens = tokens.len();
        }
        if base_tokens + space_token + segment_tokens > self.max_tokens {
            return false;
        }

        let segment_words = segment.word_count;
        let new_word_count = self.word_count + segment_words;
        // Estimation guard: avoid silent overshoot when relying on word threshold
        let est_total_tokens = ((new_word_count as f64) * ratio).ceil() as usize;
        // If estimate already exceeds limit, force precise token path to trigger flush.
        // Also, if we're still early but the incoming segment is large in characters, be cautious.
        let mut require_precise = est_total_tokens > self.max_tokens;
        if !require_precise {
            // If comfortably under 80% of limit and below word threshold, allow fast path
            if new_word_count < self.word_threshold
                && est_total_tokens <= self.max_tokens.saturating_mul(6) / 10
            {
                // Large segments can still cause big jumps; precheck tokens for very long lines
                let seg_chars = segment.content.chars().count();
                if seg_chars <= 400 {
                    return true;
                }
            }
            // If we are near capacity, force precise path
            let est_current = ((self.word_count as f64) * ratio).ceil() as usize;
            if est_current + space_token + segment_tokens > self.max_tokens.saturating_mul(8) / 10 {
                require_precise = true;
            }
        }

        if !require_precise
            && new_word_count < self.word_threshold
            && est_total_tokens <= self.max_tokens.saturating_mul(6) / 10
        {
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
        let mut hyphen_join = false;
        if !self.text.is_empty() {
            // Hyphenation fix across line/segment boundaries: "word-" + "next" -> "wordnext"
            if self.text.ends_with('-') {
                if let Some(first) = segment.content.chars().next() {
                    if first.is_alphabetic() {
                        self.text.pop(); // drop trailing '-'
                        self.located_builder.pop_last_char();
                        // no space before joining
                        hyphen_join = true;
                    } else {
                        // non-alpha next token; fall through to space logic
                        let needs_space = !self.text.ends_with(' ')
                            && !self.text.ends_with('\n')
                            && !segment.content.starts_with(' ');
                        if needs_space {
                            self.text.push(' ');
                            self.located_builder.push_synthetic(
                                " ",
                                SyntheticKind::InsertedWhitespace,
                                self.segments
                                    .last()
                                    .map(segment_source_ref)
                                    .into_iter()
                                    .chain(std::iter::once(segment_source_ref(&segment)))
                                    .collect(),
                            );
                            space_added = true;
                        }
                    }
                }
            } else {
                // Check if we need a space
                let needs_space = !self.text.ends_with(' ')
                    && !self.text.ends_with('\n')
                    && !segment.content.starts_with(' ');
                if needs_space {
                    self.text.push(' ');
                    self.located_builder.push_synthetic(
                        " ",
                        SyntheticKind::InsertedWhitespace,
                        self.segments
                            .last()
                            .map(segment_source_ref)
                            .into_iter()
                            .chain(std::iter::once(segment_source_ref(&segment)))
                            .collect(),
                    );
                    space_added = true;
                }
            }
        }
        let start_idx = self.text.len();
        self.text.push_str(&segment.content);
        self.located_builder.push_source_segment(&segment);

        // Scan appended content for wide boundaries and bookmark them (tokens filled lazily)
        let seg_idx = self.segments.len();
        let appended = &self.text[start_idx..];
        // Paragraph boundaries: double newline
        let mut search_from = 0usize;
        while let Some(pos) = appended[search_from..].find("\n\n") {
            let local_end = search_from + pos + 2; // position right after boundary
            self.boundaries.push(BoundaryBookmark {
                seg_idx,
                byte_in_seg: local_end,
                text_byte_idx: start_idx + local_end,
                kind: BoundaryKind::Paragraph,
                tokens_at_boundary: None,
            });
            if self.boundaries.len() > 16 {
                let drop_n = self.boundaries.len() - 16;
                self.boundaries.drain(0..drop_n);
            }
            search_from = local_end;
        }
        // Sentence/clause boundaries with guards
        let bytes = appended.as_bytes();
        let n = bytes.len();
        let mut i = 0usize;
        let mut boundaries_to_add: Vec<usize> = Vec::new();
        while i < n {
            let b = bytes[i];
            if b == b'.' || b == b'!' || b == b'?' {
                let mut j = i + 1;
                while j < n {
                    let c = bytes[j];
                    if c == b'\'' || c == b'\"' || c == b')' || c == b']' {
                        j += 1;
                    } else {
                        break;
                    }
                }
                // Require whitespace or end after boundary
                if j == n || matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                    boundaries_to_add.push(j);
                }
                i = j;
                continue;
            }
            if b == b';' {
                let j = i + 1;
                if j == n || matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                    boundaries_to_add.push(j);
                }
                i = j;
                continue;
            }
            if b == b':' {
                let prev_digit = i > 0 && bytes[i - 1].is_ascii_digit();
                let next_digit = i + 1 < n && bytes[i + 1].is_ascii_digit();
                let j = i + 1;
                if (j == n || matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r'))
                    && !(prev_digit && next_digit)
                {
                    boundaries_to_add.push(j);
                }
                i = j;
                continue;
            }
            if b == 0xE2 && i + 2 < n {
                let b1 = bytes[i + 1];
                let b2 = bytes[i + 2];
                if b1 == 0x80 && (b2 == 0x94 || b2 == 0x93) {
                    let j = i + 3;
                    if j == n || matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                        boundaries_to_add.push(j);
                    }
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
        for j_after in boundaries_to_add {
            self.boundaries.push(BoundaryBookmark {
                seg_idx,
                byte_in_seg: j_after,
                text_byte_idx: start_idx + j_after,
                kind: BoundaryKind::Sentence,
                tokens_at_boundary: None,
            });
            if self.boundaries.len() > 16 {
                let drop_n = self.boundaries.len() - 16;
                self.boundaries.drain(0..drop_n);
            }
        }

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
            if tokens > self.max_tokens.saturating_mul(9) / 10 {
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
        let mut page_segments: Vec<&TextSegment> = Vec::new();
        let multi = std::env::var("PDF_EXTRACT_MULTIPLE_FRAGMENTS").is_ok();
        let grouped = std::env::var("PDF_EXTRACT_DISABLE_FRAGMENT_GROUPS").is_err()
            || std::env::var("PDF_EXTRACT_FRAGMENT_GROUPS").is_ok();

        for segment in &self.segments {
            if segment.page_num != current_page {
                // Process segments for previous page
                if !page_segments.is_empty() {
                    if multi {
                        for s in &page_segments {
                            let bbox = calculate_bbox_for_segments(&[*s]);
                            page_positions.push(PagePosition {
                                page: current_page,
                                char_start: s.char_start,
                                char_end: s.char_end,
                                bbox,
                            });
                        }
                    } else if grouped {
                        let mut groups = group_page_segments(&page_segments);
                        for (char_start, char_end, bbox) in groups.drain(..) {
                            page_positions.push(PagePosition {
                                page: current_page,
                                char_start,
                                char_end,
                                bbox,
                            });
                        }
                    } else {
                        let bbox = calculate_bbox_for_segments(&page_segments);
                        page_positions.push(PagePosition {
                            page: current_page,
                            char_start: page_segments.first().unwrap().char_start,
                            char_end: page_segments.last().unwrap().char_end,
                            bbox,
                        });
                    }
                }
                page_segments.clear();
                current_page = segment.page_num;
            }
            page_segments.push(segment);
        }

        // Don't forget the last page
        if !page_segments.is_empty() {
            if multi {
                for s in &page_segments {
                    let bbox = calculate_bbox_for_segments(&[*s]);
                    page_positions.push(PagePosition {
                        page: current_page,
                        char_start: s.char_start,
                        char_end: s.char_end,
                        bbox,
                    });
                }
            } else if grouped {
                let mut groups = group_page_segments(&page_segments);
                for (char_start, char_end, bbox) in groups.drain(..) {
                    page_positions.push(PagePosition {
                        page: current_page,
                        char_start,
                        char_end,
                        bbox,
                    });
                }
            } else {
                let bbox = calculate_bbox_for_segments(&page_segments);
                page_positions.push(PagePosition {
                    page: current_page,
                    char_start: page_segments.first().unwrap().char_start,
                    char_end: page_segments.last().unwrap().char_end,
                    bbox,
                });
            }
        }

        // Overall bounding box (single page only)
        let bbox = if end_page.is_none() && !self.segments.is_empty() {
            let segment_refs: Vec<&TextSegment> = self.segments.iter().collect();
            Some(calculate_bbox_for_segments(&segment_refs))
        } else {
            None
        };

        let located_text = self.located_builder.trimmed_located_text();
        let paragraph = located_text.text.clone();

        ContentOutput {
            headings: self.current_headings.clone(),
            paragraph,
            page: start_page,
            end_page,
            page_char_start,
            page_char_end,
            bbox,
            page_positions,
            located_text: Some(located_text),
        }
    }

    /// Create output with warning flag for truncated sentences
    pub fn create_output_with_warning(&self) -> ContentOutput {
        let output = self.create_output();
        // Could add a warning field if needed
        output
    }

    /// Attempt to flush at the last bookmarked boundary under the token cap.
    /// On success, mutates the accumulator to keep only the suffix (content after the boundary)
    /// and returns a ContentOutput for the prefix. Returns None if no suitable boundary exists.
    pub fn flush_at_last_boundary(&mut self) -> Option<ContentOutput> {
        if self.segments.is_empty() || self.text.is_empty() {
            return None;
        }
        // Choose the last boundary whose tokens are under cap; prefer the latest
        let mut candidate_idx: Option<usize> = None;
        for (i, b) in self.boundaries.iter().enumerate().rev() {
            let t = match b.tokens_at_boundary {
                Some(t) => t,
                None => {
                    // Lazily compute and store
                    let prefix = &self.text[..b.text_byte_idx];
                    self.tokenizer.encode_ordinary(prefix).len()
                }
            };
            if t <= self.max_tokens {
                candidate_idx = Some(i);
                break;
            }
        }
        let Some(bidx) = candidate_idx else {
            return None;
        };
        if self.boundaries[bidx].tokens_at_boundary.is_none() {
            let t = self
                .tokenizer
                .encode_ordinary(&self.text[..self.boundaries[bidx].text_byte_idx])
                .len();
            self.boundaries[bidx].tokens_at_boundary = Some(t);
        }
        let b = self.boundaries[bidx].clone();

        // Build prefix segments
        let mut prefix_segments: Vec<TextSegment> = Vec::new();
        for (si, seg) in self.segments.iter().enumerate() {
            if si < b.seg_idx {
                prefix_segments.push(seg.clone());
            } else if si == b.seg_idx {
                let head_content = seg.content[..b.byte_in_seg].to_string();
                let head_chars = head_content.chars().count();
                let head = slice_text_segment_by_chars(seg, 0, head_chars, head_content);
                prefix_segments.push(head);
                break;
            } else {
                break;
            }
        }

        let start_page = prefix_segments.first().map(|s| s.page_num).unwrap_or(0);
        let last_page = prefix_segments.last().map(|s| s.page_num).unwrap_or(0);
        let end_page = if last_page != start_page {
            Some(last_page)
        } else {
            None
        };

        // Page positions
        let mut page_positions: Vec<PagePosition> = Vec::new();
        if !prefix_segments.is_empty() {
            let mut current_page = prefix_segments.first().unwrap().page_num;
            let mut page_group: Vec<&TextSegment> = Vec::new();
            let multi = std::env::var("PDF_EXTRACT_MULTIPLE_FRAGMENTS").is_ok();
            let grouped = std::env::var("PDF_EXTRACT_DISABLE_FRAGMENT_GROUPS").is_err()
                || std::env::var("PDF_EXTRACT_FRAGMENT_GROUPS").is_ok();
            for s in &prefix_segments {
                if s.page_num != current_page {
                    if multi {
                        for ps in &page_group {
                            let bbox = calculate_bbox_for_segments(&[ps]);
                            page_positions.push(PagePosition {
                                page: current_page,
                                char_start: ps.char_start,
                                char_end: ps.char_end,
                                bbox,
                            });
                        }
                    } else if grouped {
                        let mut groups = group_page_segments(&page_group);
                        for (char_start, char_end, bbox) in groups.drain(..) {
                            page_positions.push(PagePosition {
                                page: current_page,
                                char_start,
                                char_end,
                                bbox,
                            });
                        }
                    } else {
                        let bbox = calculate_bbox_for_segments(&page_group);
                        page_positions.push(PagePosition {
                            page: current_page,
                            char_start: page_group.first().unwrap().char_start,
                            char_end: page_group.last().unwrap().char_end,
                            bbox,
                        });
                    }
                    page_group.clear();
                    current_page = s.page_num;
                }
                page_group.push(s);
            }
            if !page_group.is_empty() {
                if multi {
                    for ps in &page_group {
                        let bbox = calculate_bbox_for_segments(&[ps]);
                        page_positions.push(PagePosition {
                            page: current_page,
                            char_start: ps.char_start,
                            char_end: ps.char_end,
                            bbox,
                        });
                    }
                } else if grouped {
                    let mut groups = group_page_segments(&page_group);
                    for (char_start, char_end, bbox) in groups.drain(..) {
                        page_positions.push(PagePosition {
                            page: current_page,
                            char_start,
                            char_end,
                            bbox,
                        });
                    }
                } else {
                    let bbox = calculate_bbox_for_segments(&page_group);
                    page_positions.push(PagePosition {
                        page: current_page,
                        char_start: page_group.first().unwrap().char_start,
                        char_end: page_group.last().unwrap().char_end,
                        bbox,
                    });
                }
            }
        }

        let prefix_raw_chars = self.text[..b.text_byte_idx].chars().count();
        let prefix_builder = self.located_builder.slice_chars(0, prefix_raw_chars);
        let located_text = prefix_builder.trimmed_located_text();
        let prefix_text = located_text.text.clone();
        let bbox_overall = if end_page.is_none() && !prefix_segments.is_empty() {
            let refs: Vec<&TextSegment> = prefix_segments.iter().collect();
            Some(calculate_bbox_for_segments(&refs))
        } else {
            None
        };

        let output = ContentOutput {
            headings: self.current_headings.clone(),
            paragraph: prefix_text,
            page: start_page,
            end_page,
            page_char_start: prefix_segments.first().map(|s| s.char_start),
            page_char_end: prefix_segments.last().map(|s| s.char_end),
            bbox: bbox_overall,
            page_positions,
            located_text: Some(located_text),
        };

        // Mutate accumulator to keep suffix
        let mut tail_segments: Vec<TextSegment> = Vec::new();
        for (si, seg) in self.segments.iter().enumerate() {
            if si < b.seg_idx {
                continue;
            }
            if si == b.seg_idx {
                if b.byte_in_seg < seg.content.len() {
                    let tail_content = seg.content[b.byte_in_seg..].to_string();
                    let tail_chars = tail_content.chars().count();
                    let head_chars = seg.content[..b.byte_in_seg].chars().count();
                    let tail =
                        slice_text_segment_by_chars(seg, head_chars, tail_chars, tail_content);
                    tail_segments.push(tail);
                }
            } else {
                tail_segments.push(seg.clone());
            }
        }
        self.segments = tail_segments;
        let suffix_raw_chars = self.text[b.text_byte_idx..].chars().count();
        let mut suffix_builder = self
            .located_builder
            .slice_chars(prefix_raw_chars, suffix_raw_chars);
        let trim_leading = suffix_builder
            .text
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .count();
        if trim_leading > 0 {
            suffix_builder = suffix_builder.slice_chars(
                trim_leading,
                suffix_builder.char_len().saturating_sub(trim_leading),
            );
        }
        self.text = suffix_builder.text.clone();
        self.located_builder = suffix_builder;
        self.word_count = count_words(&self.text);
        self.token_count = None;

        // Rebase boundaries
        if !self.boundaries.is_empty() {
            let mut new_b: Vec<BoundaryBookmark> = Vec::new();
            for mut bm in self.boundaries.clone() {
                if bm.text_byte_idx <= b.text_byte_idx {
                    continue;
                }
                bm.text_byte_idx -= b.text_byte_idx;
                if bm.seg_idx < b.seg_idx {
                    continue;
                }
                if bm.seg_idx == b.seg_idx {
                    if bm.byte_in_seg <= b.byte_in_seg {
                        continue;
                    }
                    bm.byte_in_seg -= b.byte_in_seg;
                    bm.seg_idx = 0;
                } else {
                    bm.seg_idx = bm.seg_idx - b.seg_idx;
                }
                bm.tokens_at_boundary = None;
                new_b.push(bm);
            }
            self.boundaries = new_b;
        }

        Some(output)
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

// Group segments on a single page into contiguous line groups and return positions
// Returns Vec of (char_start, char_end, bbox)
fn group_page_segments(segments: &[&TextSegment]) -> Vec<(usize, usize, BoundingBox)> {
    if segments.is_empty() {
        return Vec::new();
    }
    // Sort by y (top to bottom), then x (left to right)
    let mut segs: Vec<&TextSegment> = segments.iter().cloned().collect();
    segs.sort_by(|a, b| {
        let ycmp = a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal);
        if ycmp == std::cmp::Ordering::Equal {
            a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            ycmp
        }
    });

    // Tolerances
    let mut groups: Vec<Vec<&TextSegment>> = Vec::new();
    let mut current: Vec<&TextSegment> = Vec::new();
    let mut last: Option<&TextSegment> = None;

    for s in segs.into_iter() {
        if let Some(prev) = last {
            let avg_h = ((prev.height + s.height) / 2.0).max(0.1);
            let tol_x = 0.25 * avg_h;
            let vgap = (s.y - (prev.y + prev.height)).max(0.0); // 0 if overlapping/adjacent
            let gap_ok = vgap <= 1.3 * avg_h;

            let prev_left = prev.x;
            let prev_right = prev.x + prev.width;
            let prev_center = prev.x + prev.width * 0.5;
            let s_left = s.x;
            let s_right = s.x + s.width;
            let s_center = s.x + s.width * 0.5;

            let left_aligned = (s_left - prev_left).abs() <= tol_x;
            let right_aligned = (s_right - prev_right).abs() <= tol_x;
            let center_aligned = (s_center - prev_center).abs() <= tol_x;

            let overlap_left = s_left.max(prev_left);
            let overlap_right = s_right.min(prev_right);
            let overlap = (overlap_right - overlap_left).max(0.0);
            let minw = prev.width.min(s.width).max(1e-6);
            let h_overlap_ok = overlap / minw >= 0.2;

            if gap_ok && (left_aligned || right_aligned || center_aligned || h_overlap_ok) {
                current.push(s);
            } else {
                if !current.is_empty() {
                    groups.push(current);
                }
                current = vec![s];
            }
        } else {
            current.push(s);
        }
        last = Some(s);
    }
    if !current.is_empty() {
        groups.push(current);
    }

    let mut out: Vec<(usize, usize, BoundingBox)> = Vec::new();
    for g in groups.into_iter() {
        let bbox = calculate_bbox_for_segments(&g.iter().cloned().collect::<Vec<&TextSegment>>());
        let char_start = g.first().unwrap().char_start;
        let char_end = g.last().unwrap().char_end;
        out.push((char_start, char_end, bbox));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FontWeight;

    fn segment(content: &str, char_start: usize) -> TextSegment {
        TextSegment {
            content: content.to_string(),
            font_size: 12.0,
            transformed_font_size: 12.0,
            x: 10.0,
            y: 20.0,
            is_bold: false,
            font_name: "Test".to_string(),
            font_weight: FontWeight::Regular,
            is_italic: false,
            page_num: 1,
            cutat: String::new(),
            fill_color: None,
            stroke_color: None,
            char_start,
            char_end: char_start + content.chars().count(),
            width: content.chars().count() as f64 * 6.0,
            height: 12.0,
            word_count: count_words(content),
            located_text: None,
        }
    }

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

    #[test]
    fn split_long_sentence_hard_caps_symbol_soup() {
        let tokenizer = tiktoken_rs::cl100k_base().unwrap();
        let text = "# ?;#% # # \" ' ".repeat(1500);
        let chunks = split_long_sentence(&text, 256, &tokenizer);

        assert!(!chunks.is_empty());
        assert!(chunks
            .iter()
            .all(|chunk| tokenizer.encode_ordinary(chunk).len() <= 256));
    }

    #[test]
    fn split_text_hard_capped_caps_every_chunk() {
        let tokenizer = tiktoken_rs::cl100k_base().unwrap();
        let text = "This is a sentence. ".repeat(500);
        let chunks = split_text_hard_capped(&text, 64, &tokenizer);

        assert!(!chunks.is_empty());
        assert!(chunks
            .iter()
            .all(|chunk| tokenizer.encode_ordinary(chunk).len() <= 64));
    }

    #[test]
    fn chunk_output_records_pdf_and_synthetic_space_spans() {
        let tokenizer = Arc::new(tiktoken_rs::cl100k_base().unwrap());
        let mut acc = ChunkAccumulator::new(128, tokenizer);
        acc.add_segment(segment("Hello", 10));
        acc.add_segment(segment("world", 20));

        let output = acc.create_output();
        let located = output.located_text.expect("located text");

        assert_eq!(located.text, "Hello world");
        assert!(located.spans.iter().any(|span| matches!(
            span.source,
            SpanSource::Synthetic {
                kind: SyntheticKind::InsertedWhitespace,
                ..
            }
        )));
        assert!(located.spans.iter().any(|span| matches!(
            span.source,
            SpanSource::Pdf {
                page: 1,
                char_start: 10,
                ..
            }
        )));
        assert!(located.spans.iter().any(|span| matches!(
            span.source,
            SpanSource::Pdf {
                page: 1,
                char_start: 20,
                ..
            }
        )));
    }

    #[test]
    fn heading_marker_is_typed_synthetic_span() {
        let tokenizer = Arc::new(tiktoken_rs::cl100k_base().unwrap());
        let mut acc = ChunkAccumulator::new(128, tokenizer);
        acc.add_segment(segment("# Title", 30));
        let located = acc
            .create_output()
            .located_text
            .expect("chunk should carry located text");
        assert!(located.spans.iter().any(|span| {
            span.output_start == 0
                && span.output_end == 2
                && matches!(
                    span.source,
                    SpanSource::Synthetic {
                        kind: SyntheticKind::HeadingMarker,
                        ..
                    }
                )
        }));
    }
}
