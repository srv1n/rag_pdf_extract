//! Extract PDF to Markdown format
//!
//! Clean output suitable for evaluation and comparison.
//!
//! Usage:
//!   cargo run --release --example extract_markdown -- input.pdf [output.md]
//!
//! If output.md is not specified, writes to stdout.

use pdf_extract::{parse_pdf, ExtractionResult, LAParams};
use std::env;
use std::fs::File;
use std::io::Write;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 || args[1] == "--help" || args[1] == "-h" {
        eprintln!(
            "Usage: {} <input.pdf> [output.md] [--max-tokens N] [--la-all-texts]",
            args[0]
        );
        eprintln!();
        eprintln!("Extracts PDF content to markdown format.");
        eprintln!("If output.md is not specified, writes to stdout.");
        process::exit(1);
    }

    let input_pdf = &args[1];
    let output_file = args.get(2).filter(|s| !s.starts_with("--"));

    // Parse max-tokens flag
    let max_tokens = args
        .iter()
        .position(|a| a == "--max-tokens")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok())
        .or(Some(500));

    // Use layout analysis with pdfminer-style line grouping. Keep Form XObject
    // extraction opt-in; legal/form fixtures should request it explicitly.
    let mut laparams = LAParams::default();
    laparams.all_texts = args.iter().any(|arg| arg == "--la-all-texts");

    // Extract PDF
    let docs = match parse_pdf(
        input_pdf,
        1,              // source_id
        "file",         // source_type
        None,           // ocr_config
        None,           // pages
        None,           // password
        max_tokens,     // max_tokens
        Some(laparams), // Enable layout analysis
        None,           // clean_text (default: true)
    ) {
        Ok(docs) => docs,
        Err(e) => {
            eprintln!("Error extracting PDF: {}", e);
            process::exit(1);
        }
    };

    // Convert to markdown
    let markdown = chunks_to_markdown(&docs);

    // Write output
    match output_file {
        Some(path) => {
            let mut file = File::create(path).expect("Failed to create output file");
            file.write_all(markdown.as_bytes())
                .expect("Failed to write output");
            eprintln!("Written to: {}", path);
        }
        None => {
            print!("{}", markdown);
        }
    }
}

/// Convert extraction results to markdown format
fn chunks_to_markdown(docs: &[ExtractionResult]) -> String {
    let mut md = String::new();

    for result in docs {
        let content_core = &result.content_core;

        // Output content directly (headings are already formatted in the content)
        // Replace runs of 3+ spaces with newlines (column breaks)
        let content = content_core.content.trim();
        let content = regex::Regex::new(r" {3,}")
            .unwrap()
            .replace_all(content, "\n")
            .to_string();
        if !content.is_empty() {
            md.push_str(&content);
            md.push_str("\n\n");
        }
    }

    md
}
