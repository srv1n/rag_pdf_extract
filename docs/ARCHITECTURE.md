# PDF Extraction Architecture

This document details the internal architecture and implementation of the PDF extraction pipeline.

## Overview

The extraction pipeline transforms PDF files into structured chunks with location metadata through multiple processing stages:

```
PDF → Layout Analysis → Segment Processing → Heading Detection → Chunking → Output
```

Each stage is designed for:
- **Accuracy**: Precise text extraction with minimal artifacts
- **Structure preservation**: Maintain document hierarchy and formatting
- **Location tracking**: Enable highlighting and citations
- **Performance**: Minimize redundant processing

## Pipeline Stages

### 1. Layout Analysis (lib.rs)

**Purpose**: Extract glyphs from PDF content streams and group into visual lines.

**Key Functions**:
- `process_stream()` - Main content stream processor
- Handles both regular content and Form XObjects (when `LAParams.all_texts = true`)

**Process**:
1. Parse PDF operators (TJ, Tj, Tm, Td, etc.)
2. Extract glyphs with position, font, and rendering info
3. Transform coordinates using CTM (Current Transformation Matrix)
4. Group glyphs into visual lines by Y-coordinate proximity
5. Join lines within Form XObjects based on:
   - Terminal punctuation (., !, ?, :, ), ", ')
   - Next line paragraph markers (numbers, bullets, A./B./C.)
   - Paragraph breaks (vertical gaps > 1.3x average)

**Configuration** (LAParams):
```rust
pub struct LAParams {
    pub char_margin: f32,        // 2.0  - horizontal gap for word grouping
    pub word_margin: f32,        // 0.10 - space injection threshold
    pub line_overlap: f32,       // 0.5  - vertical overlap for same line
    pub line_margin: f32,        // 0.5  - vertical gap for box grouping
    pub boxes_flow: f32,         // 0.5  - reading order bias
    pub detect_vertical: bool,   // false - detect vertical text
    pub all_texts: bool,         // false - include Form XObject text
}
```

**Critical Setting**: `all_texts = true` required for documents with text in Form XObjects (common in legal PDFs).

**Output**: TextSegments with:
- Content (with spaces or newlines as line endings)
- Page number
- Position (x, y)
- Bounding box dimensions
- Font information

### 2. Segment Processing (document/processing.rs)

**Purpose**: Merge segments that are continuations of the same logical text.

**Functions**:

**`merge_continuation_segments()`**
- Merges segments on the same visual line (Y-proximity check)
- Criteria:
  - Previous segment ends with space (not newline)
  - Same page
  - Y-difference < font_size * 0.5
  - Next segment doesn't start paragraph (numbers, bullets)

**`merge_title_block_entities()`**
- Joins consecutive short ALL CAPS lines (party names, organization names)
- Criteria:
  - Both lines ALL CAPS
  - Both < 40 chars
  - At least one ≤ 20 chars (prevents over-merging)
  - Same page
  - Previous doesn't end with terminal punctuation
  - Not section markers (VERSUS, headings with ##)
  - Limited to 2 merges (3 total lines max)

**Example**:
```
Before:
  COMPETITION COMMISSION
  OF INDIA

After:
  COMPETITION COMMISSION OF INDIA
```

**Header/Footer Filtering**:
- Detects repetitive content across pages
- Uses normalized text matching
- Filters out page numbers, headers, footers

### 3. Heading Detection (document/analysis.rs)

**Purpose**: Classify lines as headings or body text using geometric features.

**Function**: `classify_line()`

**Features Used**:
1. **Height ratio**: `line.max_height / doc_stats.body_line_height`
2. **Width ratio**: `line.total_width / doc_stats.body_line_width`
3. **ALL CAPS detection**: All alphabetic characters uppercase
4. **Bold detection**: Font weight ≥ 600
5. **Standalone detection**: Next line starts at left margin or large Y-gap

**Classification Rules** (priority order):
```rust
// H1: Taller + short + standalone
if height_ratio > 1.20 && is_short && is_standalone { return H1 }

// H2: Moderately taller + short + standalone
if height_ratio > 1.08 && is_short && is_standalone { return H2 }

// H2: Bold + short + standalone
if is_bold && is_short && is_standalone && words ≤ 10 { return H2 }

// H2: ALL CAPS + short
if is_all_caps && is_short && words ≤ 10 { return H2 }

// H1: Significantly taller even if not perfectly short
if height_ratio > 1.25 && is_standalone && words ≤ 10 { return H1 }

// Body: Everything else
```

**Thresholds**:
- `is_short`: width_ratio < 0.70
- `is_taller`: height_ratio > 1.08
- Standalone Y-gap: > body_line_height * 1.5

**Title Block Heuristic**:
- 3+ consecutive heading-like lines = metadata block
- No markdown markers added
- Prevents fragmenting document headers

**Document Statistics** (stats.rs):
- `body_line_height`: Mode (most common) bbox height
- `body_line_width`: Median paragraph width
- `left_margin`: 10th percentile of X positions
- `line_height_tolerance`: body_line_height * 0.25

### 4. Chunking (chunk_accumulator.rs)

**Purpose**: Split content into token-limited chunks with intelligent boundaries.

**Class**: `ChunkAccumulator`

**Strategy**:
1. **Estimation phase**: Use word count * learned ratio until ~50% capacity
2. **Precise phase**: Switch to exact tokenization near capacity
3. **Boundary detection**:
   - Sentence boundaries: `.!?` followed by space + capital letter
   - Paragraph boundaries: double newlines
   - Clause boundaries: `;,—` for long sentences
4. **Backtracking**: If overflow, backtrack to last valid boundary

**Token Counting**:
- Uses tiktoken_rs with gpt-4o tokenizer
- Learns word-to-token ratio during processing
- Minimizes tokenization calls (expensive operation)

**Metadata Tracking**:
- Heading hierarchy (updated per segment)
- Page numbers and character ranges
- Bounding box unions across segments
- Compression: zstd on ext_json

**Output**: ExtractionResult
```rust
{
  content_core: {
    chunk_id: "stable source/range/ordinal identity",
    content_hash: "blake3(content)",
    content: "...",
    token_count: 487,
    schema_version: 2,
    headings_json: '["Section 1", "Subsection A"]'
  },
  content_ext: {
    ext_json: zstd_compressed({
      format: "Pdf",
      fragments: [{
        page: 1,
        char_range: {start: 0, end: 1234},
        bbox: {x: 72, y: 200, width: 400, height: 600}
      }]
    })
  }
}
```

`PageFragment.char_range` is an output-chunk range into `content_core.content`.
Source-PDF ranges are stored in `content_ext.output_spans[].source.Pdf`.

## Design Decisions

### Why Layout Analysis?

**Problem**: Text in PDFs is unordered glyphs. Reading order must be inferred.

**Solution**: Group glyphs by visual position:
- Y-coordinate proximity → same line
- X-coordinate order → left-to-right reading
- Vertical gaps → paragraph breaks

**Alternative**: Character-by-character extraction misses structure.

### Why Geometric Heading Detection?

**Problem**: Font size alone is unreliable:
- Fonts may not have size metadata
- Size can vary due to scaling/transforms
- Some headings are bold but not larger

**Solution**: Use multiple geometric signals:
- Rendered height (bounding box)
- Line width (short = likely heading)
- Position (standalone lines)
- ALL CAPS (common in legal docs)

### Why Form XObject Support?

**Problem**: Many PDFs (especially legal documents) embed text in Form XObjects for layout control.

**Solution**: Recursively process Form XObjects when `LAParams.all_texts = true`.

**Default = false** to avoid:
- Processing decorative elements
- Duplicating watermarks
- Extracting irrelevant annotations

### Why Title Block Merging?

**Problem**: Party names, organization names span multiple lines:
```
COMPETITION COMMISSION
OF INDIA
```

**Solution**: Merge consecutive short ALL CAPS lines where at least one is very short (≤20 chars).

**Why "at least one short"**: Prevents merging long section headers:
```
IN THE SUPREME COURT OF INDIA  ← 31 chars, no merge
CIVIL APPELLATE JURISDICTION   ← 28 chars, no merge
```

### Why Two-Phase Tokenization?

**Problem**: Tokenization is expensive (0.1ms per call adds up).

**Solution**:
1. Estimate using word count * learned ratio
2. Switch to exact tokenization when approaching limit
3. Learn better ratio over time

**Result**: 5-10x speedup while maintaining accuracy.

## Performance Characteristics

### Time Complexity

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| PDF parsing | O(n) | n = glyphs |
| Layout grouping | O(n log n) | Sorting by position |
| Segment merging | O(m) | m = segments |
| Heading detection | O(m) | Per-segment classification |
| Chunking | O(k) | k = chunks << m |

### Memory Usage

- Peak memory: ~5-10MB per 100-page document
- Compressed ext_json: ~70% reduction
- Streaming not implemented (requires full document pass for header/footer detection)

### Bottlenecks

1. **Tokenization**: Dominates for large documents
   - Mitigation: Two-phase estimation
2. **Layout analysis**: Complex transform calculations
   - Mitigation: Only when needed (set LAParams)
3. **Header/footer detection**: Requires full document
   - Mitigation: Can be disabled via env var

## Error Handling

### Page-Level Isolation

Errors on individual pages don't stop extraction:
```rust
let page_results: DashMap<u32, Vec<TextSegment>> = DashMap::new();
pages.par_iter().for_each(|(page_num, page)| {
    match process_page(...) {
        Ok(segments) => page_results.insert(*page_num, segments),
        Err(e) => eprintln!("Page {} failed: {}", page_num, e),
    }
});
```

### Token Limit Violations

If a single segment exceeds max_tokens:
1. Try splitting at clause boundaries (`;,—`)
2. Fallback to word-level splitting
3. Log warning if still exceeds (rare)

### Malformed PDFs

- Missing fonts → use default metrics
- Invalid transforms → skip glyph
- Corrupt streams → skip page, continue

## Testing

### Unit Tests
- Geometric calculations (transforms, bbox unions)
- Heading classification logic
- Boundary detection

### Integration Tests
- Full pipeline on synthetic PDFs
- Edge cases (empty pages, scanned docs)

### Evaluation Suite
- Corpus of real documents (legal, scientific, financial)
- Metrics: WER, CER, heading F1, empty section rate
- Regression tracking across commits

## Future Improvements

### Potential Enhancements
1. **Table detection**: Identify and preserve table structure
2. **Multi-column support**: Better handling of academic papers
3. **Language-aware tokenization**: Support non-English languages
4. **Streaming extraction**: Process pages incrementally
5. **Fine-grained heading levels**: H3-H6 classification

### Known Limitations
1. **Signature lines**: `...J.` detected as heading (rare)
2. **Equations**: Math notation may fragment
3. **Rotated text**: Limited support (vertical only)
4. **Right-to-left**: Arabic/Hebrew not fully tested

## References

- PDFMiner architecture: https://github.com/pdfminer/pdfminer.six
- PDF coordinate systems: PDF Reference 1.7, Section 4.2
- Token-aware chunking: OpenAI tokenizer documentation
- Geometric heading detection: First-principles analysis of PDF layout
