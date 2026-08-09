# Migrating to 0.9.0

`0.9.0` is a breaking release that adds typed extraction errors, bounded PDF
parsing, encrypted-PDF password handling, and OCR classification metadata.

The old string-only error variants are replaced by `OutputError`. Configure
passwords with `PdfPassword::new(...)`; the password is runtime-only and is
intentionally omitted from serde configuration. `ExtractionOptions` retains
its serializable, cloneable, equality-comparable configuration surface, but it
is no longer `Copy` because it owns an optional secret.

The release also upgrades `lopdf` to 0.42 and applies original-first bounded
container repairs. Callers that previously matched parser strings should match
the typed variants and inspect `ErrorContext` instead.

`0.8.0` was the prior breaking release that removed the experimental backend-selection
surface and standardizes on the lopdf parser path.

## Removed APIs

| Removed | Replacement |
| --- | --- |
| `ParserBackend` | No replacement. Backend selection is gone; the crate uses the lopdf path. |
| `parse_pdf_with_backend(...)` | Use `parse_pdf(...)` with the current 10-argument signature, ending in `ExtractionOptions`. |
| `backend-lopdf` / `backend-pdfium` features | Remove these feature flags. No PDFium runtime is required. |
| `SpanSource::Pdf { backend, ... }` | Use `SpanSource::Pdf { page, char_start, char_end, bbox }`. |
| Internal processing exports from `document::processing` | Use the public `parse_pdf`, `output_doc_new_schema`, `extract_pdf_location`, and `decompress_content_ext` APIs. |

## Current parse API

```rust
let chunks = pdf_extract::parse_pdf(
    "document.pdf",
    1,
    "file",
    None,
    None,
    None,
    Some(500),
    None,
    Some(true),
    Default::default(),
)?;
```

For product layout extraction:

```rust
let mut laparams = pdf_extract::LAParams::product_layout();
laparams.all_texts = false;

let chunks = pdf_extract::parse_pdf(
    "document.pdf",
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
```

The final `ExtractionOptions` argument carries parse budgets, bounded repair
policy, output-span settings, and the runtime-only `PdfPassword`. Public PDF
and ContentExt helpers return `OutputError`; callers should match its typed
variants and inspect `ErrorContext` rather than parse display strings.

### Decompressed-stream budget

`max_decompressed_stream_bytes` is serde-configurable at runtime and defaults
to 128 MiB. It bounds the cumulative bytes extraction materializes from page
content, forms, fonts, and other non-image streams. Image XObjects are charged
at stored size because OCR decodes them one at a time inside the caller's
killable worker; cumulatively charging every raw page bitmap rejects legitimate
scanned bound volumes without bounding peak memory.

Use `OutputError::reason_code()` when an error crosses a process or reporting
boundary. A refusal by this budget reports
`pdf_resource_limit_decompressed_stream_bytes`, which must not be collapsed
into the same reason as corrupt or low-quality input.

To inspect a corpus with a bounded diagnostic:

```bash
cargo run --example decompression_report -- document.pdf corpus/*.pdf
```

The report's `accounted_stream_bytes` is the exact total enforced by the
option. The diagnostic itself stops at 1 GiB by default; set
`PDF_EXTRACT_MEASUREMENT_HARD_LIMIT_BYTES` to a positive byte count when an
authorized measurement needs a different hard stop.

## Identity and location changes

`ContentCore.schema_version` is `2`.

`ContentCore.chunk_id` is a stable source/range/ordinal identity. It is not
`blake3(content)`. Use `ContentCore.content_hash` when you need the content hash.

`PageFragment.char_range` is an output-chunk character range: offsets are in
`ContentCore.content` for that chunk. Source-PDF character ranges live in
`ContentExt.output_spans[].source.Pdf.char_start` and `char_end`.

## OCR feature names

Use `features = ["ocr"]` or `features = ["ocr-ocrs"]` for OCRS/RTen OCR.
The legacy `ocr-tesseract` feature remains as a compatibility alias only; this
crate does not bundle or call Tesseract.
