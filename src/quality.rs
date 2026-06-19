use crate::{extract_pdf_location, ExtractionResult};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseQualityStatus {
    Usable,
    Empty,
    TooShort,
    MostlyBoilerplate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepeatedLine {
    pub text: String,
    pub count: usize,
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
            extract_pdf_location(&doc.content_ext)
                .map(|location| !location.fragments.is_empty())
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
    let (status, score) = decide_quality(
        normalized_chars,
        chunk_count,
        repeated_line.ratio,
        unique_token_ratio,
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
        top_repeated_lines: repeated_line.top,
    }
}

fn decide_quality(
    normalized_chars: usize,
    chunk_count: usize,
    repeated_line_ratio: f64,
    unique_token_ratio: f64,
) -> (ParseQualityStatus, f64) {
    if normalized_chars == 0 || chunk_count == 0 {
        return (ParseQualityStatus::Empty, 0.0);
    }
    if normalized_chars < 500 {
        return (ParseQualityStatus::TooShort, 0.15);
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
    use super::{decide_quality, ParseQualityStatus};

    #[test]
    fn long_legal_body_is_not_boilerplate_only_for_low_unique_tokens() {
        let (status, score) = decide_quality(377_331, 450, 0.051, 0.073);

        assert_eq!(status, ParseQualityStatus::Usable);
        assert!(score > 0.75);
    }

    #[test]
    fn repeated_short_body_still_fails_as_boilerplate() {
        let (status, score) = decide_quality(8_000, 10, 0.55, 0.04);

        assert_eq!(status, ParseQualityStatus::MostlyBoilerplate);
        assert!(score <= 0.25);
    }
}
