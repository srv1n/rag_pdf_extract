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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodeQualityMetrics {
    pub confidence: f64,
    pub mojibake_char_ratio: f64,
    pub symbol_char_ratio: f64,
    pub non_ascii_symbol_char_ratio: f64,
    pub ascii_alpha_ratio: f64,
    pub suspicious_chunk_ratio: f64,
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
    decode_quality_for_text(normalized_text, suspicious_chunk_ratio)
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
    let quality = assess_decode_quality(text);
    let visible_chars = text.chars().filter(|ch| !ch.is_whitespace()).count();
    visible_chars >= 24
        && (quality.mojibake_char_ratio > 0.03
            || (quality.non_ascii_symbol_char_ratio > 0.08 && quality.symbol_char_ratio > 0.20)
            || (quality.symbol_char_ratio > 0.45 && quality.ascii_alpha_ratio < 0.35))
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

#[cfg(test)]
mod tests {
    use super::{assess_decode_quality, decide_quality, ParseQualityStatus};

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
}
