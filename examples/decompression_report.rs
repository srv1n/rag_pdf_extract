use pdf_extract::measure_pdf_decompression;
use serde::Serialize;
use std::{env, process};

const DEFAULT_MEASUREMENT_HARD_LIMIT: usize = 1024 * 1024 * 1024;

#[derive(Serialize)]
struct Report<'a> {
    path: &'a str,
    input_bytes: usize,
    accounted_stream_bytes: usize,
    expansion_ratio: f64,
    pages: usize,
    objects: usize,
}

fn main() {
    let paths = env::args().skip(1).collect::<Vec<_>>();
    if paths.is_empty() {
        eprintln!("usage: decompression_report <pdf> [<pdf> ...]");
        process::exit(2);
    }
    let hard_limit = env::var("PDF_EXTRACT_MEASUREMENT_HARD_LIMIT_BYTES")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|limit| *limit > 0)
        .unwrap_or(DEFAULT_MEASUREMENT_HARD_LIMIT);

    let mut failed = false;
    for path in &paths {
        match measure_pdf_decompression(path, hard_limit) {
            Ok(usage) => println!(
                "{}",
                serde_json::to_string(&Report {
                    path,
                    input_bytes: usage.input_bytes,
                    accounted_stream_bytes: usage.accounted_stream_bytes,
                    expansion_ratio: usage.expansion_ratio,
                    pages: usage.pages,
                    objects: usage.objects,
                })
                .expect("serialize report")
            ),
            Err(error) => {
                failed = true;
                eprintln!("{path}: {error}");
            }
        }
    }
    if failed {
        process::exit(1);
    }
}
