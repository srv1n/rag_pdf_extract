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
pdf-extract = "0.7.7"
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
        None            // LAParams (use default)
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

// Enable layout analysis with Form XObject support
let mut laparams = LAParams::default();
laparams.all_texts = true;  // Include text from Form XObjects

let results = parse_pdf(
    "document.pdf",
    1,
    "file",
    None,
    None,
    None,
    Some(500),
    Some(laparams)
)?;
```

**When to use layout analysis:**
- Legal documents (text often in Form XObjects)
- Multi-column layouts
- Complex document structures
- When you need precise line grouping

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
    pub chunk_id: String,           // blake3(content)
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
    pub char_range: CharRange,      // start, end positions
    pub bbox: BoundingBox,          // x, y, width, height
}
```

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
}
```

**Important:** Set `all_texts = true` for documents with text in Form XObjects (common in legal PDFs).

## Examples

### Extract with Markdown Formatting

```bash
cargo run --release --example extract_markdown input.pdf > output.md
```

This example:
- Uses layout analysis with `all_texts = true`
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
- Must set `LAParams.all_texts = true` to extract text from Form XObjects
- This is common in legal documents where text is embedded for layout control

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
