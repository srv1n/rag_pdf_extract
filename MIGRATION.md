# Migrating to 0.8.0

`0.8.0` is a breaking release that removes the experimental backend-selection
surface and standardizes on the lopdf parser path.

## Removed APIs

| Removed | Replacement |
| --- | --- |
| `ParserBackend` | No replacement. Backend selection is gone; the crate uses the lopdf path. |
| `parse_pdf_with_backend(...)` | Use `parse_pdf(...)` with the current 9-argument signature. |
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
)?;
```

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
