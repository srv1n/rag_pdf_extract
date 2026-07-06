use crate::{extract_pdf_location, parse_pdf, LAParams};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FounderRegressionManifest {
    pub name: String,
    pub corpus_root_env: String,
    pub default_corpus_root: String,
    pub cases: Vec<FounderRegressionCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FounderRegressionCase {
    pub id: String,
    pub label: String,
    pub relative_path: String,
    pub severity: RegressionSeverity,
    pub min_chars: usize,
    pub min_chunks: usize,
    pub min_pages: usize,
    #[serde(default)]
    pub expected_terms: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RegressionSeverity {
    Hard,
    Warn,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RegressionStatus {
    Passed,
    Warned,
    Failed,
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FounderRegressionCaseReport {
    pub id: String,
    pub label: String,
    pub severity: RegressionSeverity,
    pub status: RegressionStatus,
    pub pdf_path: String,
    pub extracted_chars: usize,
    pub extracted_chunks: usize,
    pub extracted_pages: usize,
    pub max_chunk_tokens: usize,
    pub over_cap_chunks: usize,
    pub garbage_chunks: usize,
    pub duplicate_line_ratio: f64,
    pub matched_terms: Vec<String>,
    pub failure_reasons: Vec<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FounderRegressionReport {
    pub manifest_name: String,
    pub corpus_root: String,
    pub generated_at_epoch_ms: u128,
    pub hard_failures: usize,
    pub warnings: usize,
    pub passed: usize,
    pub cases: Vec<FounderRegressionCaseReport>,
}

impl FounderRegressionReport {
    pub fn overall_ok(&self) -> bool {
        self.hard_failures == 0
    }
}

pub fn load_manifest(path: &Path) -> Result<FounderRegressionManifest, Box<dyn std::error::Error>> {
    let raw = fs::read_to_string(path)?;
    let manifest = serde_json::from_str::<FounderRegressionManifest>(&raw)?;
    Ok(manifest)
}

pub fn resolve_corpus_root(manifest: &FounderRegressionManifest) -> PathBuf {
    std::env::var(&manifest.corpus_root_env)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&manifest.default_corpus_root))
}

pub fn run_founder_regression(
    manifest_path: &Path,
) -> Result<FounderRegressionReport, Box<dyn std::error::Error>> {
    let manifest = load_manifest(manifest_path)?;
    let corpus_root = resolve_corpus_root(&manifest);
    let mut reports = Vec::with_capacity(manifest.cases.len());
    let mut hard_failures = 0usize;
    let mut warnings = 0usize;
    let mut passed = 0usize;

    for case in &manifest.cases {
        let report = evaluate_case(&corpus_root, case)?;
        match report.status {
            RegressionStatus::Passed => passed += 1,
            RegressionStatus::Warned => warnings += 1,
            RegressionStatus::Failed | RegressionStatus::Missing => {
                if report.severity == RegressionSeverity::Hard {
                    hard_failures += 1;
                } else {
                    warnings += 1;
                }
            }
        }
        reports.push(report);
    }

    Ok(FounderRegressionReport {
        manifest_name: manifest.name,
        corpus_root: corpus_root.display().to_string(),
        generated_at_epoch_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        hard_failures,
        warnings,
        passed,
        cases: reports,
    })
}

pub fn render_markdown_summary(report: &FounderRegressionReport) -> String {
    let mut out = String::new();
    out.push_str("# Founder PDF Regression Report\n\n");
    out.push_str(&format!(
        "- Manifest: `{}`\n- Corpus root: `{}`\n- Passed: `{}`\n- Warnings: `{}`\n- Hard failures: `{}`\n\n",
        report.manifest_name, report.corpus_root, report.passed, report.warnings, report.hard_failures
    ));
    out.push_str("| Case | Severity | Status | Chars | Chunks | Pages | Max Tokens | Over Cap | Garbage | Dup Line Ratio | Terms |\n");
    out.push_str("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for case in &report.cases {
        out.push_str(&format!(
            "| `{}` | `{:?}` | `{:?}` | {} | {} | {} | {} | {} | {} | {:.3} | {} |\n",
            case.id,
            case.severity,
            case.status,
            case.extracted_chars,
            case.extracted_chunks,
            case.extracted_pages,
            case.max_chunk_tokens,
            case.over_cap_chunks,
            case.garbage_chunks,
            case.duplicate_line_ratio,
            if case.matched_terms.is_empty() {
                "-".to_string()
            } else {
                case.matched_terms.join(", ")
            }
        ));
        if !case.failure_reasons.is_empty() {
            out.push_str(&format!(
                "\n`{}` issues: {}\n\n",
                case.id,
                case.failure_reasons.join("; ")
            ));
        }
    }
    out
}

fn evaluate_case(
    corpus_root: &Path,
    case: &FounderRegressionCase,
) -> Result<FounderRegressionCaseReport, Box<dyn std::error::Error>> {
    let pdf_path = corpus_root.join(&case.relative_path);
    if !pdf_path.exists() {
        return Ok(FounderRegressionCaseReport {
            id: case.id.clone(),
            label: case.label.clone(),
            severity: case.severity,
            status: RegressionStatus::Missing,
            pdf_path: pdf_path.display().to_string(),
            extracted_chars: 0,
            extracted_chunks: 0,
            extracted_pages: 0,
            max_chunk_tokens: 0,
            over_cap_chunks: 0,
            garbage_chunks: 0,
            duplicate_line_ratio: 0.0,
            matched_terms: Vec::new(),
            failure_reasons: vec!["pdf file not found".to_string()],
            notes: case.notes.clone(),
        });
    }

    let mut laparams = LAParams::default();
    laparams.all_texts = true;
    laparams.detect_vertical = false;

    let docs = parse_pdf(
        pdf_path.to_str().unwrap_or_default(),
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        Some(laparams),
        Some(true),
        Default::default(),
    )?;

    let extracted_chunks = docs
        .iter()
        .filter(|doc| !doc.content_core.content.trim().is_empty())
        .count();
    let extracted_chars = docs
        .iter()
        .map(|doc| doc.content_core.content.chars().count())
        .sum::<usize>();

    let mut pages = BTreeSet::new();
    let mut combined_text = String::new();
    for doc in &docs {
        combined_text.push_str(&doc.content_core.content);
        combined_text.push('\n');
        if let Ok(loc) = extract_pdf_location(&doc.content_ext) {
            for fragment in loc.fragments {
                pages.insert(fragment.page);
            }
        }
    }

    let lower_text = combined_text.to_lowercase();
    let matched_terms = case
        .expected_terms
        .iter()
        .filter(|term| lower_text.contains(&term.to_lowercase()))
        .cloned()
        .collect::<Vec<_>>();

    let extracted_pages = pages.len();
    let tokenizer = tiktoken_rs::get_bpe_from_model("gpt-4o")?;
    let chunk_token_counts = docs
        .iter()
        .map(|doc| tokenizer.encode_ordinary(&doc.content_core.content).len())
        .collect::<Vec<_>>();
    let max_chunk_tokens = chunk_token_counts.iter().copied().max().unwrap_or(0);
    let over_cap_chunks = chunk_token_counts
        .iter()
        .filter(|count| **count > 500)
        .count();
    let garbage_chunks = docs
        .iter()
        .filter(|doc| looks_like_garbage(&doc.content_core.content))
        .count();
    let duplicate_line_ratio = compute_duplicate_line_ratio(&docs);
    let mut failure_reasons = Vec::new();

    if extracted_chars < case.min_chars {
        failure_reasons.push(format!(
            "chars {} below minimum {}",
            extracted_chars, case.min_chars
        ));
    }
    if extracted_chunks < case.min_chunks {
        failure_reasons.push(format!(
            "chunks {} below minimum {}",
            extracted_chunks, case.min_chunks
        ));
    }
    if extracted_pages < case.min_pages {
        failure_reasons.push(format!(
            "pages {} below minimum {}",
            extracted_pages, case.min_pages
        ));
    }
    if !case.expected_terms.is_empty() && matched_terms.is_empty() {
        failure_reasons.push("no expected terms matched".to_string());
    }
    if over_cap_chunks > 0 {
        failure_reasons.push(format!("{} chunks exceeded hard cap 500", over_cap_chunks));
    }

    let status = if failure_reasons.is_empty() {
        RegressionStatus::Passed
    } else if case.severity == RegressionSeverity::Warn {
        RegressionStatus::Warned
    } else {
        RegressionStatus::Failed
    };

    Ok(FounderRegressionCaseReport {
        id: case.id.clone(),
        label: case.label.clone(),
        severity: case.severity,
        status,
        pdf_path: pdf_path.display().to_string(),
        extracted_chars,
        extracted_chunks,
        extracted_pages,
        max_chunk_tokens,
        over_cap_chunks,
        garbage_chunks,
        duplicate_line_ratio,
        matched_terms,
        failure_reasons,
        notes: case.notes.clone(),
    })
}

fn compute_duplicate_line_ratio(docs: &[crate::ExtractionResult]) -> f64 {
    use std::collections::HashMap;

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut total = 0usize;
    for doc in docs {
        for line in doc.content_core.content.lines() {
            let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
            if normalized.len() < 20 {
                continue;
            }
            total += 1;
            *counts.entry(normalized).or_insert(0) += 1;
        }
    }

    if total == 0 {
        return 0.0;
    }

    let duplicated = counts
        .values()
        .filter(|count| **count > 1)
        .map(|count| count - 1)
        .sum::<usize>();
    duplicated as f64 / total as f64
}

fn looks_like_garbage(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 40 {
        return false;
    }

    let alpha = trimmed.chars().filter(|c| c.is_alphabetic()).count();
    let digits = trimmed.chars().filter(|c| c.is_ascii_digit()).count();
    let symbols = trimmed
        .chars()
        .filter(|c| !c.is_alphanumeric() && !c.is_whitespace())
        .count();
    let total = trimmed.chars().count().max(1);

    let alpha_ratio = alpha as f64 / total as f64;
    let symbol_ratio = symbols as f64 / total as f64;
    let digit_ratio = digits as f64 / total as f64;

    alpha_ratio < 0.25 && symbol_ratio > 0.35 && digit_ratio < 0.35
}
