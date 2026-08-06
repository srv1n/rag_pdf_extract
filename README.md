# PDF Extract - Advanced PDF Content Extraction Library

A Rust library for extracting structured content from PDF files with precise positioning data and intelligent text processing for RAG applications.

## Features

- **Text Extraction with Layout Analysis** - Extracts text with precise positioning, font information, and layout awareness
- **Form XObject Support** - Handles text embedded in PDF Form XObjects (common in legal documents)
- **Geometric Heading Detection** - Uses visual/geometric features instead of just font properties
- **Smart Line Joining** - Joins continuation lines while preserving document structure
- **Token-Aware Chunking** - Splits content respecting sentence/paragraph boundaries
- **Location Tracking** - Maintains page numbers, bounding boxes, and character ranges for highlighting
- **Header/Footer Filtering** - Automatically identifies and filters repetitive content
- **OCR Integration** - Built-in support for scanned documents

## Installation

```toml
[dependencies]
pdf-extract = "0.9.0"
```

## Quick Start

### Basic Extraction

```rust
use pdf_extract::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let results = parse_pdf(
        "document.pdf",
        1,              // source_id
        "file",         // source_type
        None,           // OCR config
        None,           // OCR cache
        None,           // resume from
        Some(500),      // max tokens per chunk
        None,           // LAParams (no layout analysis)
        Some(true),     // clean text for indexing
        Default::default(), // parse budgets, repairs, and runtime options
    )?;

    for result in results {
        println!("Content: {}", result.content_core.content);
        println!("Tokens: {}", result.content_core.token_count);
    }

    Ok(())
}
```

### With Layout Analysis

Layout analysis enables better text extraction for complex documents:

```rust
use pdf_extract::*;

// Enable product layout analysis.
let mut laparams = LAParams::product_layout();
laparams.all_texts = false; // Enable per workload when Form XObject text is required.

let results = parse_pdf(
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

**When to use layout analysis:**
- Legal documents (text often in Form XObjects)
- Multi-column layouts
- Complex document structures
- When you need precise line grouping

`all_texts` is workload-specific. It helps when visible text is stored in Form
XObjects, but can duplicate hidden/template text in other PDFs. Start with
`LAParams::product_layout()` and enable `all_texts` only for corpus types that
prove they need it.

## Architecture

### Text Extraction Pipeline

```
PDF File
  ↓
Layout Analysis (lib.rs process_stream)
  - Glyph collection from content streams
  - Form XObject processing (if all_texts=true)
  - Line grouping by Y-coordinate proximity
  ↓
Segment Processing (document/processing.rs)
  - Line joining (within Form XObjects)
  - Segment merging (across Form XObjects)
  - Title block entity merging
  ↓
Heading Detection (document/analysis.rs)
  - Geometric features (height, width ratios)
  - ALL CAPS detection
  - Standalone line detection
  ↓
Chunking (chunk_accumulator.rs)
  - Token-limited chunks
  - Sentence/paragraph boundary awareness
  - Location metadata tracking
```

### Key Components

**Layout Analysis (lib.rs)**
- Extracts glyphs from PDF content streams
- Groups glyphs into visual lines
- Processes Form XObjects when `LAParams.all_texts = true`
- Joins lines within XObjects based on terminal punctuation

**Segment Processing (document/processing.rs)**
- `merge_continuation_segments()` - Merges segments on same visual line (Y-proximity)
- `merge_title_block_entities()` - Joins consecutive short ALL CAPS lines (party names, etc.)
- Filters headers/footers based on repetition patterns

**Heading Detection (document/analysis.rs)**
- `classify_line()` - Uses geometric features:
  - Height ratio vs body text
  - Width ratio (short lines)
  - ALL CAPS detection
  - Standalone detection (next line at margin)
- Title block heuristic: 3+ consecutive heading-like lines = metadata block

**Chunking (chunk_accumulator.rs)**
- Token-limited accumulation with GPT-4 tokenizer
- Intelligent boundary detection (sentences, paragraphs)
- Tracks heading hierarchy per chunk
- Maintains location metadata (pages, bounding boxes, char ranges)

## Data Schema

### ExtractionResult

```rust
pub struct ExtractionResult {
    pub content_core: ContentCore,
    pub content_ext: ContentExt,
}

pub struct ContentCore {
    pub chunk_id: String,           // stable source/range/ordinal identity
    pub content_hash: String,       // blake3(content)
    pub source_id: i64,
    pub source_type: String,        // "file" | "web" | "api"
    pub content: String,            // extracted text
    pub token_count: i32,
    pub headings_json: Option<String>,  // heading hierarchy
    pub status: String,
    pub schema_version: i32,
    pub created_at: i64,
}

pub struct ContentExt {
    pub chunk_id: String,
    pub ext_json: Vec<u8>,          // zstd compressed location data
}
```

### Location Tracking

```rust
pub enum FormatLocation {
    Pdf(PdfLocation),
    // Other formats...
}

pub struct PdfLocation {
    pub fragments: Vec<PageFragment>,
}

pub struct PageFragment {
    pub page: u32,
    pub char_range: CharRange,      // offsets in this output chunk
    pub bbox: BoundingBox,          // x, y, width, height
}
```

`PageFragment.char_range` is an output-chunk character range into
`ContentCore.content`. Source-PDF character ranges live in
`ContentExt.output_spans[].source.Pdf.char_start` and `char_end`.

`ContentCore.schema_version` is `2`. `chunk_id` is not `blake3(content)`; use
`content_hash` when you need the content hash.

`decompress_content_ext()` accepts payloads up to 16 MiB. Production telemetry
reports compressed and uncompressed `ContentExt` sizes so callers can alert
before chunks approach that ceiling.

## Production Telemetry

Each `parse_pdf(...)` call logs one JSON `pdf_extraction_telemetry` event with
layout/fallback state, normalized character ratios, chunk/token counts, location
coverage, span overlap, synthetic span ratio, invalid/out-of-page boxes,
`ContentExt` sizes, and OCR image counters. Use
`production_telemetry_for_results(...)` to compute the same summary directly.

## Migration

`0.9.0` is a breaking release. See [MIGRATION.md](MIGRATION.md) for removed API
replacements and downstream update examples.

## Configuration

### LAParams (Layout Analysis Parameters)

```rust
pub struct LAParams {
    pub char_margin: f32,        // Max horizontal gap for word grouping (default: 2.0)
    pub word_margin: f32,        // Space injection threshold (default: 0.10)
    pub line_overlap: f32,       // Min vertical overlap for same line (default: 0.5)
    pub line_margin: f32,        // Max vertical gap for text box grouping (default: 0.5)
    pub boxes_flow: f32,         // Reading order bias (default: 0.5)
    pub detect_vertical: bool,   // Detect vertical text (default: false)
    pub all_texts: bool,         // Include Form XObject text (default: false)
    pub layout_fallback_policy: LayoutFallbackPolicy, // Product default catches text loss and suspicious volume
}
```

`LAParams::default()` and `LAParams::product_layout()` use suspicious-volume fallback. Use `LAParams::diagnostic_layout()` when you need raw layout behavior with fallback disabled.

**Important:** Set `all_texts = true` only for document classes that need Form
XObject text. Keep it off for general ingestion unless corpus validation shows
text loss without it.

## Examples

### Extract with Markdown Formatting

```bash
cargo run --release --example extract_markdown input.pdf > output.md
```

This example:
- Uses layout analysis, with `all_texts = true` only when `--la-all-texts` is provided
- Converts headings to markdown format (##)
- Joins continuation lines intelligently
- Preserves paragraph structure

### Basic Text Extraction

```bash
cargo run --release --example extract input.pdf 500
```

Arguments:
- `input.pdf` - PDF file path
- `500` - max tokens per chunk

## Document Type Considerations

### Immutable Documents (Court Cases, Published Papers)
- Use library's built-in chunking
- Larger chunks acceptable
- Simpler storage path (no CDC tracking needed)

### Editable Documents (Word docs, collaborative documents)
- Upstream application handles CDC (Change Data Capture)
- Fine-grained chunk tracking for citation stability
- Library provides segments + locations, app re-chunks as needed

**Architecture Decision:** Document type classification and CDC logic belong in the application layer, not the PDF extraction library. This library focuses on quality extraction + location metadata.

## Testing

### Evaluation System

```bash
# Run evaluation on corpus
cd eval && python eval.py

# Generate reference extractions (Gemini, MarkItDown)
python generate_refs.py

# Compare outputs side-by-side
python compare.py "path/to/file.pdf"
```

Evaluation corpus includes:
- Legal documents (Indian court cases)
- Multi-column layouts
- Documents with embedded fonts
- Scanned documents (OCR test cases)

## Known Limitations

### Heading Detection
- Some edge cases with signature lines (`...J.`) detected as headings
- Aggressive merging may lose some intended line breaks in title blocks
- Fine-tuning available via geometric thresholds in `document/analysis.rs`

### Layout Analysis
- Y-tolerance for line grouping: `body_line_height * 0.25`
- May need adjustment for documents with unusual line spacing

### Form XObjects
- Set `LAParams.all_texts = true` for corpora that prove Form XObject text is required
- Keep it off by default for broad ingestion to avoid hidden/template duplication

## Performance Considerations

- Layout analysis adds overhead but improves quality for complex documents
- Token counting uses estimation until 50% of chunk capacity, then switches to exact
- Header/footer detection requires full document pass
- OCR (when enabled) is the primary performance bottleneck

## Contributing

The codebase is organized as:

```
src/
├── lib.rs                      # Core PDF parsing, layout analysis
├── chunk_accumulator.rs        # Token-aware chunking
├── layout_params.rs            # LAParams configuration
└── document/
    ├── processing.rs           # Segment processing, merging
    ├── analysis.rs             # Heading detection
    ├── stats.rs                # Document statistics, visual lines
    └── header_footer.rs        # Header/footer filtering
```

## License

This project is licensed under the MIT License.
