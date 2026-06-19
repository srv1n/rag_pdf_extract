# PDF Extract Integration Guide

This guide covers `pdf-extract` `0.8.0`, the lopdf-only API, schema version `2`,
and the current 9-argument `parse_pdf(...)` entry point.

## Install

```toml
[dependencies]
pdf-extract = "0.8.0"
```

OCR is off by default. Enable OCRS/RTen OCR with:

```toml
[dependencies]
pdf-extract = { version = "0.8.0", features = ["ocr"] }
```

`ocr-ocrs` is the explicit feature name. `ocr-tesseract` remains as a legacy
alias for compatibility only; this crate does not bundle or call Tesseract.

## Basic Parse

```rust
use pdf_extract::{parse_pdf, ExtractionResult};

fn extract_pdf(path: &str) -> Result<Vec<ExtractionResult>, Box<dyn std::error::Error>> {
    parse_pdf(
        path,
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        None,
        Some(true),
    )
}
```

## Product Layout Parse

```rust
use pdf_extract::{parse_pdf, LAParams};

let mut laparams = LAParams::product_layout();
laparams.all_texts = false;

let chunks = parse_pdf(
    "document.pdf",
    1,
    "file",
    None,
    None,
    None,
    Some(500),
    Some(laparams),
    Some(true),
)?;
```

`all_texts` is workload-specific. Enable it for corpora that prove they need
Form XObject text; keep it off for broad ingestion to reduce duplicate hidden or
template text.

## Output Contract

```rust
pub struct ContentCore {
    pub chunk_id: String,      // stable source/range/ordinal identity
    pub content_hash: String,  // blake3(content)
    pub source_id: i64,
    pub source_type: String,
    pub content: String,
    pub token_count: i32,
    pub headings_json: Option<String>,
    pub status: String,
    pub schema_version: i32,   // 2
    pub created_at: i64,
}

pub struct ContentExt {
    pub chunk_id: String,
    pub ext_json: Vec<u8>,     // zstd-compressed metadata
}
```

`chunk_id` is not `blake3(content)`. Use `content_hash` for content-dedup or
change detection. Repeated identical text chunks can share `content_hash` while
retaining distinct `chunk_id` values.

## Location Metadata

```rust
use pdf_extract::{decompress_content_ext, extract_pdf_location};

let location = extract_pdf_location(&chunk.content_ext)?;
let metadata = decompress_content_ext(&chunk.content_ext)?;
```

`PageFragment.char_range` is an output-chunk character range into
`ContentCore.content`. Source-PDF character ranges live in
`metadata["output_spans"][i]["source"]["Pdf"]["char_start"]` and `char_end`.

`decompress_content_ext()` accepts payloads up to 16 MiB. Track
`ContentExt.ext_json.len()` and production telemetry size fields if your
downstream storage has tighter limits.

## Production Telemetry

`parse_pdf(...)` logs one JSON `pdf_extraction_telemetry` event per document:

```text
layout_enabled
all_texts_enabled
layout_fallback_used
layout/no_layout normalized char ratio
chunk_count
max_chunk_tokens
over_cap_chunks
chunks_without_location
chars_without_span
chars_with_overlapping_spans
pdf_backed_nonsynthetic_char_ratio
synthetic_char_ratio
invalid_bbox_count
out_of_page_bbox_count
ContentExt compressed/uncompressed byte sizes
OCR images seen/converted/skipped/text-emitted
```

Call `production_telemetry_for_results(...)` if you need the same metrics as a
typed Rust value.

## OCR

```rust
use pdf_extract::{parse_pdf, OcrConfig};

let ocr_config = OcrConfig {
    detection_model: Some("models/text-detection.rten".to_string()),
    recognition_model: Some("models/text-recognition.rten".to_string()),
};

let chunks = parse_pdf(
    "scanned.pdf",
    1,
    "file",
    Some(ocr_config),
    None,
    None,
    Some(500),
    None,
    Some(true),
)?;
```

Download the OCRS/RTen models from the official OCRS model URLs used by your
deployment. Keep OCR disabled in default builds unless scanned-image extraction
is part of the workload.

## Migration

`0.8.0` is a breaking release. See [MIGRATION.md](MIGRATION.md) for removed API
replacements and downstream examples.
