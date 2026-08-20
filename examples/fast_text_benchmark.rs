use pdf_extract::{extract_text_fast, parse_pdf, ExtractionOptions, LAParams};
use serde::Serialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Serialize)]
struct VariantReport {
    variant: String,
    runs: usize,
    documents: usize,
    successful_documents: usize,
    total_ms: f64,
    median_document_ms: f64,
    total_chars: usize,
    total_chunks: usize,
    error_count: usize,
}

fn usage() -> &'static str {
    "Usage: fast_text_benchmark --corpus-dir DIR [--max-tokens N] [--runs N] [--output PATH]"
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut corpus_dir = None;
    let mut max_tokens = 350usize;
    let mut runs = 2usize;
    let mut output = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--corpus-dir" => corpus_dir = args.next().map(PathBuf::from),
            "--max-tokens" => max_tokens = args.next().ok_or(usage())?.parse()?,
            "--runs" => runs = args.next().ok_or(usage())?.parse()?,
            "--output" => output = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                println!("{}", usage());
                return Ok(());
            }
            other => return Err(format!("unknown argument {other}\n{}", usage()).into()),
        }
    }
    let corpus_dir = corpus_dir.ok_or(usage())?;
    let mut paths = fs::read_dir(&corpus_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "pdf"))
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        return Err(format!("no PDFs found in {}", corpus_dir.display()).into());
    }

    let variants = [
        "layout_all_texts_false",
        "layout_all_texts_true",
        "no_layout_structured",
        "fast_text",
    ];
    let mut reports = Vec::new();
    for variant in variants {
        let mut document_times = Vec::new();
        let mut successful_documents = 0;
        let mut total_chars = 0;
        let mut total_chunks = 0;
        let mut error_count = 0;
        let mut total_ms = 0.0;

        for run in 0..runs.max(1) {
            for (index, path) in paths.iter().enumerate() {
                let started = Instant::now();
                let result = match variant {
                    "layout_all_texts_false" => {
                        let mut params = LAParams::default();
                        params.all_texts = false;
                        parse_pdf(
                            path.to_str().unwrap_or_default(),
                            index as i64 + 1,
                            "file",
                            None,
                            None,
                            None,
                            Some(max_tokens),
                            Some(params),
                            Some(true),
                            ExtractionOptions::default(),
                        )
                        .map(|docs| {
                            let chars: usize = docs
                                .iter()
                                .map(|doc| doc.content_core.content.chars().count())
                                .sum();
                            (docs.len(), chars)
                        })
                    }
                    "layout_all_texts_true" => {
                        let mut params = LAParams::default();
                        params.all_texts = true;
                        parse_pdf(
                            path.to_str().unwrap_or_default(),
                            index as i64 + 1,
                            "file",
                            None,
                            None,
                            None,
                            Some(max_tokens),
                            Some(params),
                            Some(true),
                            ExtractionOptions::default(),
                        )
                        .map(|docs| {
                            let chars: usize = docs
                                .iter()
                                .map(|doc| doc.content_core.content.chars().count())
                                .sum();
                            (docs.len(), chars)
                        })
                    }
                    "no_layout_structured" => parse_pdf(
                        path.to_str().unwrap_or_default(),
                        index as i64 + 1,
                        "file",
                        None,
                        None,
                        None,
                        Some(max_tokens),
                        None,
                        Some(true),
                        ExtractionOptions::default(),
                    )
                    .map(|docs| {
                        let chars: usize = docs
                            .iter()
                            .map(|doc| doc.content_core.content.chars().count())
                            .sum();
                        (docs.len(), chars)
                    }),
                    "fast_text" => {
                        extract_text_fast(path, Some(max_tokens), ExtractionOptions::default()).map(
                            |chunks| {
                                let chars: usize =
                                    chunks.iter().map(|chunk| chunk.chars().count()).sum();
                                (chunks.len(), chars)
                            },
                        )
                    }
                    _ => unreachable!(),
                };
                let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
                if run > 0 {
                    total_ms += elapsed_ms;
                    document_times.push(elapsed_ms);
                    match result {
                        Ok((chunks, chars)) => {
                            successful_documents += 1;
                            total_chunks += chunks;
                            total_chars += chars;
                        }
                        Err(_) => error_count += 1,
                    }
                }
            }
        }
        document_times.sort_by(f64::total_cmp);
        let median_document_ms = document_times
            .get(document_times.len() / 2)
            .copied()
            .unwrap_or(0.0);
        reports.push(VariantReport {
            variant: variant.to_owned(),
            runs: runs.saturating_sub(1).max(1),
            documents: paths.len(),
            successful_documents,
            total_ms,
            median_document_ms,
            total_chars,
            total_chunks,
            error_count,
        });
    }

    let report = serde_json::json!({
        "schema_version": 1,
        "corpus_dir": corpus_dir,
        "max_tokens": max_tokens,
        "warmup_runs": 1,
        "measured_runs": runs.saturating_sub(1).max(1),
        "reports": reports,
    });
    let rendered = serde_json::to_string_pretty(&report)?;
    println!("{}", rendered);
    if let Some(path) = output {
        if let Some(parent) = Path::new(&path).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, rendered)?;
    }
    Ok(())
}
