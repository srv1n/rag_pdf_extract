// Minimal repro probe for the RZN Phase-1 "empty_text" finding.
// Prints the exact text length returned by the public entry points the RZN
// backend uses, so we can compare reportlab-generated PDFs against known-good
// PDFs. Run: cargo run --example rzn_extract_check -- <pdf> [<pdf> ...]
use pdf_extract::{extract_text, extract_text_from_mem};
use std::{env, fs};

fn main() {
    for p in env::args().skip(1) {
        let bytes = fs::read(&p).unwrap_or_default();
        let mem = extract_text_from_mem(&bytes, None);
        let path_r = extract_text(&p, None);
        let fmt = |r: &Result<String, pdf_extract::OutputError>| match r {
            Ok(t) => format!(
                "ok chars={} sample={:?}",
                t.chars().count(),
                t.chars().take(120).collect::<String>()
            ),
            Err(e) => format!("ERR {:?}", e),
        };
        println!(
            "{} ({} bytes)\n  from_mem:  {}\n  from_path: {}",
            p,
            bytes.len(),
            fmt(&mem),
            fmt(&path_r)
        );
    }
}
