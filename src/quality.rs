use crate::{extract_chunk_locations, ExtractionResult};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseQualityStatus {
    Usable,
    Empty,
    TooShort,
    MostlyBoilerplate,
    LikelyGarbled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepeatedLine {
    pub text: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodeQualityMetrics {
    pub confidence: f64,
    pub mojibake_char_ratio: f64,
    pub symbol_char_ratio: f64,
    pub non_ascii_symbol_char_ratio: f64,
    pub ascii_alpha_ratio: f64,
    pub suspicious_chunk_ratio: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct TextQualityPage<'a> {
    pub page: u32,
    pub text: &'a str,
}

impl<'a> TextQualityPage<'a> {
    pub fn new(page: u32, text: &'a str) -> Self {
        Self { page, text }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextQualityCharRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextQualityEvidence {
    pub page: Option<u32>,
    pub char_range: TextQualityCharRange,
    pub excerpt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextQualitySignal {
    ReplacementCharRun,
    PrivateUseOrC1Density,
    BrokenCMapArtifact,
    SymbolDominant,
    SubstitutionCipherLike,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextQualityFinding {
    pub signal: TextQualitySignal,
    pub score: f64,
    pub count: usize,
    pub ratio: f64,
    pub evidence: Vec<TextQualityEvidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextQualityAssessment {
    pub decode: DecodeQualityMetrics,
    pub findings: Vec<TextQualityFinding>,
}

impl TextQualityAssessment {
    pub fn suspected_garbled_text(&self) -> bool {
        !self.findings.is_empty()
    }

    pub fn signals(&self) -> Vec<TextQualitySignal> {
        self.findings.iter().map(|finding| finding.signal).collect()
    }

    pub fn finding(&self, signal: TextQualitySignal) -> Option<&TextQualityFinding> {
        self.findings
            .iter()
            .find(|finding| finding.signal == signal)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseQualityMetrics {
    pub status: ParseQualityStatus,
    pub score: f64,
    pub extracted_chars: usize,
    pub normalized_chars: usize,
    pub chunk_count: usize,
    pub chunks_with_location: usize,
    pub location_coverage_ratio: f64,
    pub repeated_line_ratio: f64,
    pub unique_token_ratio: f64,
    pub decode_confidence: f64,
    pub mojibake_char_ratio: f64,
    pub symbol_char_ratio: f64,
    pub non_ascii_symbol_char_ratio: f64,
    pub suspicious_chunk_ratio: f64,
    pub top_repeated_lines: Vec<RepeatedLine>,
}

pub fn assess_parse_quality(results: &[ExtractionResult]) -> ParseQualityMetrics {
    let extracted_chars = results
        .iter()
        .map(|doc| doc.content_core.content.chars().count())
        .sum::<usize>();
    let normalized_text = normalized_text(results);
    let normalized_chars = normalized_text.chars().count();
    let chunk_count = results.len();
    let chunks_with_location = results
        .iter()
        .filter(|doc| {
            extract_chunk_locations(&doc.content_ext)
                .map(|locations| !locations.is_empty())
                .unwrap_or(false)
        })
        .count();
    let location_coverage_ratio = if chunk_count == 0 {
        0.0
    } else {
        chunks_with_location as f64 / chunk_count as f64
    };
    let repeated_line = repeated_line_metrics(results);
    let unique_token_ratio = unique_token_ratio(&normalized_text);
    let decode_quality = decode_quality_metrics(results, &normalized_text);
    let (status, score) = decide_quality(
        normalized_chars,
        chunk_count,
        repeated_line.ratio,
        unique_token_ratio,
        decode_quality.confidence,
        decode_quality.suspicious_chunk_ratio,
    );

    ParseQualityMetrics {
        status,
        score,
        extracted_chars,
        normalized_chars,
        chunk_count,
        chunks_with_location,
        location_coverage_ratio,
        repeated_line_ratio: repeated_line.ratio,
        unique_token_ratio,
        decode_confidence: decode_quality.confidence,
        mojibake_char_ratio: decode_quality.mojibake_char_ratio,
        symbol_char_ratio: decode_quality.symbol_char_ratio,
        non_ascii_symbol_char_ratio: decode_quality.non_ascii_symbol_char_ratio,
        suspicious_chunk_ratio: decode_quality.suspicious_chunk_ratio,
        top_repeated_lines: repeated_line.top,
    }
}

fn decide_quality(
    normalized_chars: usize,
    chunk_count: usize,
    repeated_line_ratio: f64,
    unique_token_ratio: f64,
    decode_confidence: f64,
    suspicious_chunk_ratio: f64,
) -> (ParseQualityStatus, f64) {
    if normalized_chars == 0 || chunk_count == 0 {
        return (ParseQualityStatus::Empty, 0.0);
    }
    if normalized_chars < 500 {
        return (ParseQualityStatus::TooShort, 0.15);
    }
    if decode_confidence < 0.50 || suspicious_chunk_ratio >= 0.35 {
        return (
            ParseQualityStatus::LikelyGarbled,
            0.20 * decode_confidence.max(0.05),
        );
    }

    let substantial_body =
        normalized_chars >= 20_000 && chunk_count >= 5 && repeated_line_ratio < 0.20;
    if substantial_body {
        let score = 0.80_f64.max(1.0 - repeated_line_ratio).min(0.95);
        return (ParseQualityStatus::Usable, score);
    }

    let boilerplate_heavy = repeated_line_ratio >= 0.50
        || (repeated_line_ratio >= 0.25 && unique_token_ratio < 0.08)
        || (normalized_chars < 2_000 && unique_token_ratio < 0.05);
    if boilerplate_heavy {
        return (ParseQualityStatus::MostlyBoilerplate, 0.25);
    }

    let score =
        (0.65 + (unique_token_ratio.min(0.25) * 0.8) - repeated_line_ratio * 0.4).clamp(0.35, 0.90);
    (ParseQualityStatus::Usable, score)
}

pub fn assess_decode_quality(text: &str) -> DecodeQualityMetrics {
    decode_quality_for_text(text, 0.0)
}

pub fn assess_text_quality(text: &str) -> TextQualityAssessment {
    assess_text_quality_pages(&[TextQualityPage::new(0, text)])
}

pub fn assess_text_quality_pages(pages: &[TextQualityPage<'_>]) -> TextQualityAssessment {
    let text = pages
        .iter()
        .map(|page| page.text)
        .collect::<Vec<_>>()
        .join("\n");
    let decode = assess_decode_quality(&text);
    let visible_chars = text.chars().filter(|ch| !ch.is_whitespace()).count();
    let script = script_profile(&text);

    let replacement_runs = collect_replacement_runs(pages);
    let private_use_or_c1 = collect_private_use_or_c1(pages);
    let broken_cmap = collect_broken_cmap_artifacts(pages);
    let substitution_cipher = substitution_cipher_like(&text, &script);
    let mut findings = Vec::new();

    let replacement_count = text.chars().filter(|ch| *ch == '\u{fffd}').count();
    let replacement_ratio = ratio(replacement_count, visible_chars);
    if !replacement_runs.is_empty() || (replacement_count >= 3 && replacement_ratio >= 0.01) {
        findings.push(TextQualityFinding {
            signal: TextQualitySignal::ReplacementCharRun,
            score: (replacement_ratio * 8.0).clamp(0.25, 1.0),
            count: replacement_count,
            ratio: replacement_ratio,
            evidence: replacement_runs,
        });
    }

    let private_c1_count = text
        .chars()
        .filter(|ch| is_private_use(*ch) || is_c1_control(*ch))
        .count();
    let private_c1_ratio = ratio(private_c1_count, visible_chars);
    if private_c1_count >= 2 && private_c1_ratio >= 0.02 {
        findings.push(TextQualityFinding {
            signal: TextQualitySignal::PrivateUseOrC1Density,
            score: (private_c1_ratio * 7.0).clamp(0.25, 1.0),
            count: private_c1_count,
            ratio: private_c1_ratio,
            evidence: private_use_or_c1,
        });
    }

    if !broken_cmap.is_empty() {
        findings.push(TextQualityFinding {
            signal: TextQualitySignal::BrokenCMapArtifact,
            score: (broken_cmap.len() as f64 / 3.0).clamp(0.35, 1.0),
            count: broken_cmap.len(),
            ratio: ratio(broken_cmap.len(), visible_chars),
            evidence: broken_cmap,
        });
    }

    if visible_chars >= 24
        && !script.non_latin_dominant
        && decode.symbol_char_ratio >= 0.45
        && decode.ascii_alpha_ratio < 0.35
    {
        findings.push(TextQualityFinding {
            signal: TextQualitySignal::SymbolDominant,
            score: ((decode.symbol_char_ratio - 0.45) * 2.0).clamp(0.25, 1.0),
            count: text
                .chars()
                .filter(|ch| !ch.is_alphanumeric() && !ch.is_whitespace())
                .count(),
            ratio: decode.symbol_char_ratio,
            evidence: representative_spans(pages),
        });
    }

    if substitution_cipher {
        findings.push(TextQualityFinding {
            signal: TextQualitySignal::SubstitutionCipherLike,
            score: 0.65,
            count: ascii_words(&text).len(),
            ratio: 0.0,
            evidence: representative_spans(pages),
        });
    }

    TextQualityAssessment { decode, findings }
}

fn decode_quality_metrics(
    results: &[ExtractionResult],
    normalized_text: &str,
) -> DecodeQualityMetrics {
    let suspicious_chunks = results
        .iter()
        .filter(|doc| looks_like_decode_garbled(&doc.content_core.content))
        .count();
    let suspicious_chunk_ratio = if results.is_empty() {
        0.0
    } else {
        suspicious_chunks as f64 / results.len() as f64
    };
    let mut quality = decode_quality_for_text(normalized_text, suspicious_chunk_ratio);
    if decode_quality_is_suspicious(&quality) {
        // Document-level mojibake and non-ASCII symbol density are signals in
        // their own right. Do not lose them merely because no individual
        // chunk crossed the older per-chunk finding threshold.
        quality.suspicious_chunk_ratio = quality.suspicious_chunk_ratio.max(1.0);
    }
    quality
}

fn decode_quality_for_text(text: &str, suspicious_chunk_ratio: f64) -> DecodeQualityMetrics {
    let visible_chars = text.chars().filter(|ch| !ch.is_whitespace()).count();
    if visible_chars == 0 {
        return DecodeQualityMetrics {
            confidence: 0.0,
            mojibake_char_ratio: 0.0,
            symbol_char_ratio: 0.0,
            non_ascii_symbol_char_ratio: 0.0,
            ascii_alpha_ratio: 0.0,
            suspicious_chunk_ratio,
        };
    }

    let ascii_alpha = text.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
    let symbols = text
        .chars()
        .filter(|ch| !ch.is_alphanumeric() && !ch.is_whitespace())
        .count();
    let non_ascii_symbols = text
        .chars()
        .filter(|ch| !ch.is_ascii() && !ch.is_alphanumeric() && !ch.is_whitespace())
        .count();
    let mojibake_chars = text.chars().filter(|ch| is_mojibake_marker(*ch)).count();

    let ascii_alpha_ratio = ascii_alpha as f64 / visible_chars as f64;
    let symbol_char_ratio = symbols as f64 / visible_chars as f64;
    let non_ascii_symbol_char_ratio = non_ascii_symbols as f64 / visible_chars as f64;
    let mojibake_char_ratio = mojibake_chars as f64 / visible_chars as f64;

    let mut penalty = 0.0_f64;
    penalty = penalty.max(mojibake_char_ratio * 6.0);
    penalty = penalty.max(non_ascii_symbol_char_ratio * 4.0);
    if symbol_char_ratio > 0.40 && ascii_alpha_ratio < 0.45 {
        penalty = penalty.max((symbol_char_ratio - 0.40) * 2.0 + (0.45 - ascii_alpha_ratio));
    }
    penalty = penalty.max(suspicious_chunk_ratio * 1.25);

    DecodeQualityMetrics {
        confidence: (1.0 - penalty).clamp(0.0, 1.0),
        mojibake_char_ratio,
        symbol_char_ratio,
        non_ascii_symbol_char_ratio,
        ascii_alpha_ratio,
        suspicious_chunk_ratio,
    }
}

fn looks_like_decode_garbled(text: &str) -> bool {
    let quality = assess_text_quality(text);
    quality.suspected_garbled_text() || decode_quality_is_suspicious(&quality.decode)
}

fn decode_quality_is_suspicious(quality: &DecodeQualityMetrics) -> bool {
    quality.mojibake_char_ratio > 0.03
        || quality.non_ascii_symbol_char_ratio > 0.03
        || quality.confidence < 0.50
}

fn is_mojibake_marker(ch: char) -> bool {
    matches!(
        ch,
        '\u{02d8}'
            | '\u{02c7}'
            | '\u{02c6}'
            | '\u{02d9}'
            | '\u{02dd}'
            | '\u{02db}'
            | '\u{02da}'
            | '\u{02dc}'
    )
}

#[derive(Debug, Clone, Copy, Default)]
struct TextScriptProfile {
    latin_alpha: usize,
    non_latin_alpha: usize,
    non_latin_dominant: bool,
}

fn script_profile(text: &str) -> TextScriptProfile {
    let mut profile = TextScriptProfile::default();
    for ch in text.chars().filter(|ch| ch.is_alphabetic()) {
        if is_latin_letter(ch) {
            profile.latin_alpha += 1;
        } else {
            profile.non_latin_alpha += 1;
        }
    }
    let total = profile.latin_alpha + profile.non_latin_alpha;
    profile.non_latin_dominant =
        total > 0 && profile.non_latin_alpha >= 16 && profile.non_latin_alpha * 2 >= total;
    profile
}

fn collect_replacement_runs(pages: &[TextQualityPage<'_>]) -> Vec<TextQualityEvidence> {
    let mut evidence = Vec::new();
    for page in pages {
        let chars = page.text.chars().collect::<Vec<_>>();
        let mut index = 0usize;
        while index < chars.len() {
            if chars[index] != '\u{fffd}' {
                index += 1;
                continue;
            }
            let start = index;
            while index < chars.len() && chars[index] == '\u{fffd}' {
                index += 1;
            }
            if index - start >= 2 {
                push_evidence(&mut evidence, page, start, index);
            }
            if evidence.len() >= 5 {
                return evidence;
            }
        }
    }
    evidence
}

fn collect_private_use_or_c1(pages: &[TextQualityPage<'_>]) -> Vec<TextQualityEvidence> {
    let mut evidence = Vec::new();
    for page in pages {
        for (index, ch) in page.text.chars().enumerate() {
            if is_private_use(ch) || is_c1_control(ch) {
                push_evidence(&mut evidence, page, index, index + 1);
                if evidence.len() >= 5 {
                    return evidence;
                }
            }
        }
    }
    evidence
}

fn collect_broken_cmap_artifacts(pages: &[TextQualityPage<'_>]) -> Vec<TextQualityEvidence> {
    let mut evidence = Vec::new();
    for page in pages {
        collect_ascii_marker(page, "(cid:", &mut evidence);
        collect_ascii_marker(page, "cid+", &mut evidence);
        collect_ascii_marker(page, ".notdef", &mut evidence);
        collect_uni_hex_markers(page, &mut evidence);
        if evidence.len() >= 5 {
            evidence.truncate(5);
            return evidence;
        }
    }
    evidence.truncate(5);
    evidence
}

fn collect_ascii_marker(
    page: &TextQualityPage<'_>,
    marker: &str,
    evidence: &mut Vec<TextQualityEvidence>,
) {
    let haystack = page.text.to_ascii_lowercase();
    let marker = marker.to_ascii_lowercase();
    let mut byte_offset = 0usize;
    while byte_offset < haystack.len() {
        let Some(relative_start) = haystack[byte_offset..].find(&marker) else {
            break;
        };
        let start = byte_offset + relative_start;
        let mut end = start + marker.len();
        while end < page.text.len() {
            let Some(ch) = page.text[end..].chars().next() else {
                break;
            };
            if marker == "(cid:" {
                end += ch.len_utf8();
                if ch == ')' {
                    break;
                }
            } else if marker == "cid+" && ch.is_ascii_digit() {
                end += ch.len_utf8();
            } else {
                break;
            }
        }
        push_byte_evidence(evidence, page, start, end);
        if evidence.len() >= 5 {
            return;
        }
        byte_offset = end.max(start + 1);
    }
}

fn collect_uni_hex_markers(page: &TextQualityPage<'_>, evidence: &mut Vec<TextQualityEvidence>) {
    let bytes = page.text.as_bytes();
    let mut candidates = Vec::new();
    let mut index = 0usize;
    while index + 7 <= bytes.len() {
        if bytes[index..index + 3].eq_ignore_ascii_case(b"uni")
            && bytes[index + 3..index + 7]
                .iter()
                .all(|byte| byte.is_ascii_hexdigit())
            && (index == 0 || !bytes[index - 1].is_ascii_alphanumeric())
            && (index + 7 >= bytes.len() || !bytes[index + 7].is_ascii_alphanumeric())
        {
            candidates.push((index, index + 7));
            index += 7;
        } else {
            index += 1;
        }
    }
    if candidates.len() < 2 {
        return;
    }
    for (start, end) in candidates {
        push_byte_evidence(evidence, page, start, end);
        if evidence.len() >= 5 {
            return;
        }
    }
}

fn representative_spans(pages: &[TextQualityPage<'_>]) -> Vec<TextQualityEvidence> {
    let mut evidence = Vec::new();
    for page in pages {
        let len = page.text.chars().count();
        if len == 0 {
            continue;
        }
        push_evidence(&mut evidence, page, 0, len.min(96));
        if evidence.len() >= 5 {
            break;
        }
    }
    evidence
}

fn push_byte_evidence(
    evidence: &mut Vec<TextQualityEvidence>,
    page: &TextQualityPage<'_>,
    byte_start: usize,
    byte_end: usize,
) {
    let start = page.text[..byte_start].chars().count();
    let end = page.text[..byte_end].chars().count();
    push_evidence(evidence, page, start, end);
}

fn push_evidence(
    evidence: &mut Vec<TextQualityEvidence>,
    page: &TextQualityPage<'_>,
    start: usize,
    end: usize,
) {
    if start >= end || evidence.len() >= 5 {
        return;
    }
    evidence.push(TextQualityEvidence {
        page: Some(page.page),
        char_range: TextQualityCharRange { start, end },
        excerpt: excerpt_for_range(page.text, start, end),
    });
}

fn excerpt_for_range(text: &str, start: usize, end: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let excerpt_start = start.saturating_sub(16);
    let excerpt_end = (end + 16).min(chars.len());
    let mut excerpt = String::new();
    if excerpt_start > 0 {
        excerpt.push('…');
    }
    for ch in &chars[excerpt_start..excerpt_end] {
        if is_c1_control(*ch) {
            excerpt.push_str("\\u{");
            excerpt.push_str(&format!("{:04x}", *ch as u32));
            excerpt.push('}');
        } else {
            excerpt.push(*ch);
        }
    }
    if excerpt_end < chars.len() {
        excerpt.push('…');
    }
    excerpt
}

fn substitution_cipher_like(text: &str, script: &TextScriptProfile) -> bool {
    let total_alpha = script.latin_alpha + script.non_latin_alpha;
    if script.latin_alpha < 64 || total_alpha == 0 || script.latin_alpha * 100 < total_alpha * 80 {
        return false;
    }
    let words = ascii_words(text);
    if words.len() < 10 {
        return false;
    }
    let common = words
        .iter()
        .filter(|word| COMMON_ENGLISH_WORDS.contains(&word.as_str()))
        .count();
    let common_ratio = ratio(common, words.len());
    let letters = words.iter().map(|word| word.len()).sum::<usize>();
    let vowels = words
        .iter()
        .flat_map(|word| word.chars())
        .filter(|ch| matches!(ch, 'a' | 'e' | 'i' | 'o' | 'u'))
        .count();
    common_ratio <= 0.06 && ratio(vowels, letters) <= 0.28
}

fn ascii_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphabetic() {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            if current.len() >= 2 {
                words.push(current.clone());
            }
            current.clear();
        }
    }
    if current.len() >= 2 {
        words.push(current);
    }
    words
}

fn is_latin_letter(ch: char) -> bool {
    let code = ch as u32;
    ch.is_ascii_alphabetic() || matches!(code, 0x00c0..=0x024f | 0x1e00..=0x1eff)
}

fn is_private_use(ch: char) -> bool {
    matches!(ch as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
}

fn is_c1_control(ch: char) -> bool {
    matches!(ch as u32, 0x0080..=0x009f)
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn normalized_text(results: &[ExtractionResult]) -> String {
    results
        .iter()
        .flat_map(|doc| doc.content_core.content.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
}

struct RepeatedLineMetrics {
    ratio: f64,
    top: Vec<RepeatedLine>,
}

fn repeated_line_metrics(results: &[ExtractionResult]) -> RepeatedLineMetrics {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut total = 0usize;

    for doc in results {
        for line in doc.content_core.content.lines() {
            let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
            if normalized.len() < 20 {
                continue;
            }
            total += 1;
            *counts.entry(normalized).or_insert(0) += 1;
        }
    }

    let repeated = counts
        .values()
        .filter(|count| **count > 1)
        .map(|count| count - 1)
        .sum::<usize>();
    let mut top = counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(line, count)| RepeatedLine {
            text: truncate_chars(&line, 120),
            count,
        })
        .collect::<Vec<_>>();
    top.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.text.cmp(&b.text)));
    top.truncate(10);

    RepeatedLineMetrics {
        ratio: if total == 0 {
            0.0
        } else {
            repeated as f64 / total as f64
        },
        top,
    }
}

fn unique_token_ratio(text: &str) -> f64 {
    let tokens = text
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| token.chars().count() >= 2)
        .map(|token| token.to_lowercase())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return 0.0;
    }
    let unique = tokens.iter().collect::<HashSet<_>>().len();
    unique as f64 / tokens.len() as f64
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

const COMMON_ENGLISH_WORDS: &[&str] = &[
    "a", "about", "after", "all", "also", "an", "and", "any", "are", "as", "at", "be", "because",
    "been", "but", "by", "can", "case", "could", "court", "did", "do", "does", "for", "from",
    "had", "has", "have", "he", "her", "him", "his", "if", "in", "into", "is", "it", "its",
    "judgment", "law", "may", "more", "not", "of", "on", "one", "or", "order", "other", "out",
    "section", "shall", "should", "state", "than", "that", "the", "their", "them", "then", "there",
    "these", "they", "this", "to", "under", "was", "were", "which", "who", "will", "with", "would",
];

#[cfg(test)]
mod tests {
    use super::{
        assess_decode_quality, assess_text_quality, decide_quality, decode_quality_is_suspicious,
        ParseQualityStatus, TextQualitySignal,
    };

    #[test]
    fn long_legal_body_is_not_boilerplate_only_for_low_unique_tokens() {
        let (status, score) = decide_quality(377_331, 450, 0.051, 0.073, 0.99, 0.0);

        assert_eq!(status, ParseQualityStatus::Usable);
        assert!(score > 0.75);
    }

    #[test]
    fn repeated_short_body_still_fails_as_boilerplate() {
        let (status, score) = decide_quality(8_000, 10, 0.55, 0.04, 0.99, 0.0);

        assert_eq!(status, ParseQualityStatus::MostlyBoilerplate);
        assert!(score <= 0.25);
    }

    #[test]
    fn symbol_mojibake_has_low_decode_confidence() {
        let quality = assess_decode_quality("ˆ˙ˇ˝˙˛˜˚ !˜\"ˆ#ˇ *,˝˝+. 56 # #,1,5,. ˙˚1 ˜ !ˆ ˙˛");

        assert!(quality.confidence < 0.50);
        assert!(quality.mojibake_char_ratio > 0.05);
    }

    #[test]
    fn normal_legal_text_has_high_decode_confidence() {
        let quality = assess_decode_quality(
            "This appeal is directed against the judgment dated 4.10.2012 passed by Allahabad High Court.",
        );

        assert!(quality.confidence > 0.90);
        assert!(quality.mojibake_char_ratio < 0.01);
    }

    #[test]
    fn canonical_text_quality_flags_broken_cmap_not_clean_cjk() {
        let garbled = assess_text_quality("decoded text leaked (cid:12) and CID+99 markers");
        assert!(garbled
            .finding(TextQualitySignal::BrokenCMapArtifact)
            .is_some());

        let clean = assess_text_quality("本院认为原审判决事实清楚适用法律正确双方证据能够相互印证");
        assert!(
            !clean.suspected_garbled_text(),
            "clean CJK should not be garbled: {:?}",
            clean.findings
        );
    }

    #[test]
    fn document_quality_keeps_mojibake_status_trigger() {
        let text = "ˆ˙ˇ˝˙˛˜˚ ".repeat(20_000);
        let decode = assess_decode_quality(&text);
        assert!(decode.mojibake_char_ratio > 0.03);
        assert!(decode_quality_is_suspicious(&decode));

        let (status, _) = decide_quality(20_000, 10, 0.0, 0.20, decode.confidence, 1.0);
        assert_eq!(status, ParseQualityStatus::LikelyGarbled);
    }

    #[test]
    fn clean_hebrew_text_is_not_a_decode_failure() {
        let quality =
            assess_text_quality("בית המשפט העליון דן בערעור וקבע כי הראיות תומכות במסקנה זו");
        assert!(
            !quality.suspected_garbled_text(),
            "unexpected findings: {:?}",
            quality.findings
        );
        assert!(!decode_quality_is_suspicious(&quality.decode));
    }
}
