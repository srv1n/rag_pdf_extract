use lopdf::Document;
use pdf_extract::{parse_pdf, ExtractionOptions, LAParams};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_TOKENS: usize = 350;
const DEFAULT_STABILITY_TOLERANCE_PCT: f64 = 20.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DocumentMetrics {
    filename: String,
    bytes: u64,
    pages: usize,
    chars: usize,
    wall_ms: f64,
    ms_per_page: f64,
    mb_per_s: f64,
    chars_per_s: f64,
    status: String,
    normalized_output_blake3: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AggregateMetrics {
    document_count: usize,
    successful_documents: usize,
    total_bytes: u64,
    total_pages: usize,
    total_chars: usize,
    parse_total_wall_ms: f64,
    median_ms_per_page: f64,
    weighted_ms_per_page: f64,
    mb_per_s: f64,
    chars_per_s: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkRun {
    run_index: usize,
    rayon_threads: usize,
    parse_batch_wall_ms: f64,
    documents: Vec<DocumentMetrics>,
    aggregate: AggregateMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StabilityDocument {
    filename: String,
    first_wall_ms: f64,
    second_wall_ms: f64,
    delta_pct: f64,
    output_match: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StabilityCheck {
    tolerance_pct: f64,
    aggregate_delta_pct: f64,
    max_abs_document_delta_pct: f64,
    output_hashes_match: bool,
    passed: bool,
    documents: Vec<StabilityDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OutputComparison {
    compared_report: String,
    passed: bool,
    mismatches: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkReport {
    schema_version: u32,
    report_date: String,
    generated_at_unix_s: u64,
    host: String,
    architecture: String,
    cpu: String,
    logical_cores: usize,
    label: String,
    scheduler: String,
    corpus_dir: String,
    fetch_ms: f64,
    max_tokens: usize,
    runs: Vec<BenchmarkRun>,
    stability: Option<StabilityCheck>,
    output_comparison: Option<OutputComparison>,
}

#[derive(Debug)]
struct Config {
    corpus_dir: PathBuf,
    output: PathBuf,
    markdown_output: Option<PathBuf>,
    label: String,
    report_date: String,
    fetch_ms: f64,
    max_tokens: usize,
    stability_runs: usize,
    stability_tolerance_pct: f64,
    compare_report: Option<PathBuf>,
}

fn usage() -> &'static str {
    "Usage: corpus_benchmark --corpus-dir DIR --output JSON --label LABEL --date YYYY-MM-DD \
     [--markdown-output PATH] [--fetch-ms MS] [--max-tokens N] \
     [--stability-runs N] [--stability-tolerance-pct N] [--compare-report JSON]"
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value\n{}", usage()))
}

fn parse_args() -> Result<Config, String> {
    let mut corpus_dir = None;
    let mut output = None;
    let mut markdown_output = None;
    let mut label = None;
    let mut report_date = None;
    let mut fetch_ms = 0.0;
    let mut max_tokens = DEFAULT_MAX_TOKENS;
    let mut stability_runs = 1;
    let mut stability_tolerance_pct = DEFAULT_STABILITY_TOLERANCE_PCT;
    let mut compare_report = None;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--corpus-dir" => corpus_dir = Some(PathBuf::from(next_arg(&mut args, &arg)?)),
            "--output" => output = Some(PathBuf::from(next_arg(&mut args, &arg)?)),
            "--markdown-output" => {
                markdown_output = Some(PathBuf::from(next_arg(&mut args, &arg)?))
            }
            "--label" => label = Some(next_arg(&mut args, &arg)?),
            "--date" => report_date = Some(next_arg(&mut args, &arg)?),
            "--fetch-ms" => fetch_ms = next_arg(&mut args, &arg)?.parse().map_err(|_| {
                format!("--fetch-ms must be a number\n{}", usage())
            })?,
            "--max-tokens" => max_tokens = next_arg(&mut args, &arg)?.parse().map_err(|_| {
                format!("--max-tokens must be an integer\n{}", usage())
            })?,
            "--stability-runs" => {
                stability_runs = next_arg(&mut args, &arg)?.parse().map_err(|_| {
                    format!("--stability-runs must be an integer\n{}", usage())
                })?
            }
            "--stability-tolerance-pct" => {
                stability_tolerance_pct = next_arg(&mut args, &arg)?.parse().map_err(|_| {
                    format!("--stability-tolerance-pct must be a number\n{}", usage())
                })?
            }
            "--compare-report" => {
                compare_report = Some(PathBuf::from(next_arg(&mut args, &arg)?))
            }
            "--help" | "-h" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}\n{}", usage())),
        }
    }

    Ok(Config {
        corpus_dir: corpus_dir.ok_or_else(|| format!("--corpus-dir is required\n{}", usage()))?,
        output: output.ok_or_else(|| format!("--output is required\n{}", usage()))?,
        markdown_output,
        label: label.ok_or_else(|| format!("--label is required\n{}", usage()))?,
        report_date: report_date.ok_or_else(|| format!("--date is required\n{}", usage()))?,
        fetch_ms,
        max_tokens,
        stability_runs: stability_runs.max(1),
        stability_tolerance_pct,
        compare_report,
    })
}

fn command_text(program: &str, args: &[&str], fallback: &str) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

fn host_metadata() -> (String, String, String, usize) {
    let host = command_text("hostname", &[], "unknown-host");
    let architecture = command_text("uname", &["-m"], "unknown-arch");
    let cpu = command_text("sysctl", &["-n", "machdep.cpu.brand_string"], "unknown-cpu");
    let logical_cores = command_text("sysctl", &["-n", "hw.ncpu"], "1")
        .parse()
        .unwrap_or(1);
    (host, architecture, cpu, logical_cores)
}

fn pdf_paths(corpus_dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut paths = fs::read_dir(corpus_dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.eq_ignore_ascii_case("pdf"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        left.file_name()
            .unwrap_or_default()
            .cmp(right.file_name().unwrap_or_default())
    });
    if paths.len() != 21 {
        return Err(format!(
            "expected exactly 21 PDFs in {}, found {}",
            corpus_dir.display(),
            paths.len()
        )
        .into());
    }
    Ok(paths)
}

fn normalized_output_hash(contents: &[String]) -> String {
    let normalized = contents
        .iter()
        .flat_map(|content| content.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ");
    blake3::hash(normalized.as_bytes()).to_hex().to_string()
}

fn rate(numerator: f64, wall_ms: f64) -> f64 {
    if wall_ms <= 0.0 {
        0.0
    } else {
        numerator / (wall_ms / 1_000.0)
    }
}

fn aggregate(documents: &[DocumentMetrics]) -> AggregateMetrics {
    let successful = documents
        .iter()
        .filter(|document| document.status == "ok")
        .collect::<Vec<_>>();
    let total_bytes = successful.iter().map(|document| document.bytes).sum();
    let total_pages = successful.iter().map(|document| document.pages).sum();
    let total_chars = successful.iter().map(|document| document.chars).sum();
    let parse_total_wall_ms = successful.iter().map(|document| document.wall_ms).sum();
    let mut per_page = successful
        .iter()
        .map(|document| document.ms_per_page)
        .collect::<Vec<_>>();
    per_page.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let median_ms_per_page = if per_page.is_empty() {
        0.0
    } else if per_page.len() % 2 == 0 {
        (per_page[per_page.len() / 2 - 1] + per_page[per_page.len() / 2]) / 2.0
    } else {
        per_page[per_page.len() / 2]
    };
    AggregateMetrics {
        document_count: documents.len(),
        successful_documents: successful.len(),
        total_bytes,
        total_pages,
        total_chars,
        parse_total_wall_ms,
        median_ms_per_page,
        weighted_ms_per_page: if total_pages == 0 {
            0.0
        } else {
            parse_total_wall_ms / total_pages as f64
        },
        mb_per_s: rate(total_bytes as f64 / 1_000_000.0, parse_total_wall_ms),
        chars_per_s: rate(total_chars as f64, parse_total_wall_ms),
    }
}

fn run_once(
    paths: &[PathBuf],
    max_tokens: usize,
    run_index: usize,
    rayon_threads: usize,
) -> BenchmarkRun {
    let batch_started = Instant::now();
    let documents = paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<non-utf8-filename>")
                .to_owned();
            let bytes = fs::metadata(path).map(|metadata| metadata.len()).unwrap_or(0);
            let pages = Document::load(path)
                .map(|document| document.get_pages().len())
                .unwrap_or(0);
            let mut laparams = LAParams::default();
            laparams.all_texts = true;
            let started = Instant::now();
            let result = parse_pdf(
                path.to_str().unwrap_or_default(),
                index as i64 + 1,
                "file",
                None,
                None,
                None,
                Some(max_tokens),
                Some(laparams),
                Some(true),
                ExtractionOptions {
                    emit_output_spans: true,
                    ..ExtractionOptions::default()
                },
            );
            let wall_ms = started.elapsed().as_secs_f64() * 1_000.0;
            match result {
                Ok(docs) => {
                    let contents = docs
                        .iter()
                        .map(|doc| doc.content_core.content.clone())
                        .collect::<Vec<_>>();
                    let chars = contents
                        .iter()
                        .map(|content| content.chars().count())
                        .sum::<usize>();
                    let ms_per_page = if pages == 0 {
                        0.0
                    } else {
                        wall_ms / pages as f64
                    };
                    DocumentMetrics {
                        filename,
                        bytes,
                        pages,
                        chars,
                        wall_ms,
                        ms_per_page,
                        mb_per_s: rate(bytes as f64 / 1_000_000.0, wall_ms),
                        chars_per_s: rate(chars as f64, wall_ms),
                        status: "ok".to_owned(),
                        normalized_output_blake3: normalized_output_hash(&contents),
                    }
                }
                Err(error) => DocumentMetrics {
                    filename,
                    bytes,
                    pages,
                    chars: 0,
                    wall_ms,
                    ms_per_page: if pages == 0 {
                        0.0
                    } else {
                        wall_ms / pages as f64
                    },
                    mb_per_s: rate(bytes as f64 / 1_000_000.0, wall_ms),
                    chars_per_s: 0.0,
                    status: format!("error: {error}"),
                    normalized_output_blake3: String::new(),
                },
            }
        })
        .collect::<Vec<_>>();
    BenchmarkRun {
        run_index,
        rayon_threads,
        parse_batch_wall_ms: batch_started.elapsed().as_secs_f64() * 1_000.0,
        aggregate: aggregate(&documents),
        documents,
    }
}

fn percentage_delta(first: f64, second: f64) -> f64 {
    if first.abs() < f64::EPSILON {
        0.0
    } else {
        (second - first) / first * 100.0
    }
}

fn stability_check(
    first: &BenchmarkRun,
    second: &BenchmarkRun,
    tolerance_pct: f64,
) -> StabilityCheck {
    let documents = first
        .documents
        .iter()
        .zip(second.documents.iter())
        .map(|(first, second)| StabilityDocument {
            filename: first.filename.clone(),
            first_wall_ms: first.wall_ms,
            second_wall_ms: second.wall_ms,
            delta_pct: percentage_delta(first.wall_ms, second.wall_ms),
            output_match: first.normalized_output_blake3 == second.normalized_output_blake3,
        })
        .collect::<Vec<_>>();
    let max_abs_document_delta_pct = documents
        .iter()
        .map(|document| document.delta_pct.abs())
        .fold(0.0, f64::max);
    let aggregate_delta_pct = percentage_delta(
        first.aggregate.parse_total_wall_ms,
        second.aggregate.parse_total_wall_ms,
    );
    let output_hashes_match = documents.iter().all(|document| document.output_match);
    StabilityCheck {
        tolerance_pct,
        aggregate_delta_pct,
        max_abs_document_delta_pct,
        output_hashes_match,
        passed: output_hashes_match
            && aggregate_delta_pct.abs() <= tolerance_pct
            && max_abs_document_delta_pct <= tolerance_pct,
        documents,
    }
}

fn compare_outputs(
    current: &BenchmarkRun,
    path: &Path,
) -> Result<OutputComparison, Box<dyn std::error::Error>> {
    let previous: BenchmarkReport = serde_json::from_str(&fs::read_to_string(path)?)?;
    let previous_run = previous
        .runs
        .first()
        .ok_or("comparison report has no benchmark run")?;
    let mut mismatches = Vec::new();
    for (current_doc, previous_doc) in current.documents.iter().zip(previous_run.documents.iter()) {
        if current_doc.filename != previous_doc.filename
            || current_doc.normalized_output_blake3 != previous_doc.normalized_output_blake3
        {
            mismatches.push(current_doc.filename.clone());
        }
    }
    Ok(OutputComparison {
        compared_report: path.display().to_string(),
        passed: mismatches.is_empty() && current.documents.len() == previous_run.documents.len(),
        mismatches,
    })
}

fn fmt(value: f64) -> String {
    format!("{value:.2}")
}

fn markdown(report: &BenchmarkReport) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "# Supreme Court corpus benchmark — {}", report.report_date);
    let _ = writeln!(output);
    let _ = writeln!(output, "- Host: `{}` ({})", report.host, report.architecture);
    let _ = writeln!(output, "- CPU: `{}`; logical cores: `{}`", report.cpu, report.logical_cores);
    let _ = writeln!(output, "- Profile: `{}`; scheduler: `{}`; Rayon workers: `{}`", report.label, report.scheduler, report.runs[0].rayon_threads);
    let _ = writeln!(output, "- Corpus: `{}`; documents: `{}`; max tokens: `{}`", report.corpus_dir, report.runs[0].aggregate.document_count, report.max_tokens);
    let _ = writeln!(output, "- Fetch/cache time: **{} ms** (reported separately; excluded from every parse wall metric).", fmt(report.fetch_ms));
    let _ = writeln!(output);

    for run in &report.runs {
        let _ = writeln!(output, "## Parse run {}", run.run_index);
        let _ = writeln!(output);
        let _ = writeln!(output, "| filename | bytes | pages | chars | wall ms | ms/page | MB/s | chars/s | normalized output |");
        let _ = writeln!(output, "|---|---:|---:|---:|---:|---:|---:|---:|---|");
        for document in &run.documents {
            let hash = if document.normalized_output_blake3.is_empty() {
                "-"
            } else {
                &document.normalized_output_blake3[..document.normalized_output_blake3.len().min(16)]
            };
            let _ = writeln!(
                output,
                "| `{}` | {} | {} | {} | {} | {} | {} | {} | `{}` |",
                document.filename,
                document.bytes,
                document.pages,
                document.chars,
                fmt(document.wall_ms),
                fmt(document.ms_per_page),
                fmt(document.mb_per_s),
                fmt(document.chars_per_s),
                hash,
            );
        }
        let aggregate = &run.aggregate;
        let _ = writeln!(output);
        let _ = writeln!(output, "Aggregate: `{}/{} ok`, {} bytes, {} pages, {} chars, parse wall **{} ms**, weighted **{} ms/page**, median **{} ms/page**, **{} MB/s**, **{} chars/s**.", aggregate.successful_documents, aggregate.document_count, aggregate.total_bytes, aggregate.total_pages, aggregate.total_chars, fmt(aggregate.parse_total_wall_ms), fmt(aggregate.weighted_ms_per_page), fmt(aggregate.median_ms_per_page), fmt(aggregate.mb_per_s), fmt(aggregate.chars_per_s));
        let _ = writeln!(output);
    }

    if let Some(stability) = &report.stability {
        let _ = writeln!(output, "## Stability check");
        let _ = writeln!(output);
        let _ = writeln!(output, "Two unchanged-tree parse runs; threshold is ±{}% for both aggregate and every document. Output hashes must also match.", fmt(stability.tolerance_pct));
        let _ = writeln!(output);
        let _ = writeln!(output, "Result: **{}**; aggregate delta **{}%**; maximum absolute document delta **{}%**; normalized outputs: **{}**.", if stability.passed { "PASS" } else { "FAIL" }, fmt(stability.aggregate_delta_pct), fmt(stability.max_abs_document_delta_pct), if stability.output_hashes_match { "identical" } else { "DIFFERENT" });
        let _ = writeln!(output);
        let _ = writeln!(output, "| filename | run 1 wall ms | run 2 wall ms | delta | output |");
        let _ = writeln!(output, "|---|---:|---:|---:|---|");
        for document in &stability.documents {
            let _ = writeln!(output, "| `{}` | {} | {} | {}% | {} |", document.filename, fmt(document.first_wall_ms), fmt(document.second_wall_ms), fmt(document.delta_pct), if document.output_match { "same" } else { "DIFFERENT" });
        }
        let _ = writeln!(output);
    }

    if let Some(comparison) = &report.output_comparison {
        let _ = writeln!(output, "## Output comparison");
        let _ = writeln!(output);
        let _ = writeln!(output, "Compared first-run normalized whitespace-token hashes with `{}`: **{}**.", comparison.compared_report, if comparison.passed { "PASS — identical" } else { "FAIL — mismatch" });
        if !comparison.mismatches.is_empty() {
            let _ = writeln!(output, "Mismatches: {}", comparison.mismatches.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", "));
        }
    }
    output
}

fn write_parent(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let paths = pdf_paths(&config.corpus_dir)?;
    let (host, architecture, cpu, logical_cores) = host_metadata();
    let rayon_threads = rayon::current_num_threads();
    let scheduler = env::var("BENCH_SCHEDULER").unwrap_or_else(|_| "normal".to_owned());
    let runs = (1..=config.stability_runs)
        .map(|run_index| run_once(&paths, config.max_tokens, run_index, rayon_threads))
        .collect::<Vec<_>>();
    let stability = if runs.len() >= 2 {
        Some(stability_check(
            &runs[0],
            &runs[1],
            config.stability_tolerance_pct,
        ))
    } else {
        None
    };
    let output_comparison = match &config.compare_report {
        Some(path) => Some(compare_outputs(&runs[0], path)?),
        None => None,
    };
    let generated_at_unix_s = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let report = BenchmarkReport {
        schema_version: 1,
        report_date: config.report_date,
        generated_at_unix_s,
        host,
        architecture,
        cpu,
        logical_cores,
        label: config.label,
        scheduler,
        corpus_dir: config.corpus_dir.display().to_string(),
        fetch_ms: config.fetch_ms,
        max_tokens: config.max_tokens,
        runs,
        stability,
        output_comparison,
    };
    write_parent(&config.output)?;
    fs::write(&config.output, serde_json::to_vec_pretty(&report)?)?;
    if let Some(path) = &config.markdown_output {
        write_parent(path)?;
        fs::write(path, markdown(&report))?;
    }

    let has_errors = report
        .runs
        .iter()
        .flat_map(|run| run.documents.iter())
        .any(|document| document.status != "ok");
    let stability_failed = report
        .stability
        .as_ref()
        .map(|stability| !stability.passed)
        .unwrap_or(false);
    let output_failed = report
        .output_comparison
        .as_ref()
        .map(|comparison| !comparison.passed)
        .unwrap_or(false);
    if has_errors || stability_failed || output_failed {
        return Err("benchmark contract failed; inspect the emitted report".into());
    }
    Ok(())
}
