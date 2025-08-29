# PDF Text Extraction & Chunking Architecture

## Overview

This document describes the complete text extraction and chunking logic used in the pdf-extract library. The system is designed to extract text from PDFs while respecting token limits, handling OCR, and maintaining document structure.

## Table of Contents

1. [Overall Architecture](#overall-architecture)
2. [Text Extraction Flow](#text-extraction-flow)
3. [OCR Text Extraction](#ocr-text-extraction)
4. [Chunk Accumulator](#chunk-accumulator)
5. [Token Counting Strategy](#token-counting-strategy)
6. [Data Flow Example](#data-flow-example)
7. [Limit Enforcement](#limit-enforcement)
8. [Key Design Decisions](#key-design-decisions)

## Overall Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         PDF DOCUMENT                              │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐        │
│  │    Page 1    │   │    Page 2    │   │    Page N    │        │
│  └──────┬───────┘   └──────┬───────┘   └──────┬───────┘        │
│         │                   │                   │                 │
│         ▼                   ▼                   ▼                 │
│  ┌─────────────────────────────────────────────────────┐        │
│  │           Parallel Page Processing                    │        │
│  │  - Extract text content                              │        │
│  │  - Extract images for OCR                            │        │
│  │  - Extract form fields                               │        │
│  └─────────────┬───────────────────────────────────────┘        │
│                 │                                                 │
│                 ▼                                                 │
│  ┌─────────────────────────────────────────────────────┐        │
│  │           Text Segment Creation                       │        │
│  │  Each piece of text becomes a TextSegment            │        │
│  └─────────────┬───────────────────────────────────────┘        │
│                 │                                                 │
│                 ▼                                                 │
│  ┌─────────────────────────────────────────────────────┐        │
│  │           Document Processing (output_doc)            │        │
│  │  ChunkAccumulator combines segments into chunks       │        │
│  └─────────────┬───────────────────────────────────────┘        │
│                 │                                                 │
│                 ▼                                                 │
│  ┌─────────────────────────────────────────────────────┐        │
│  │           ContentOutput Structures                    │        │
│  │  Final chunks respecting token limits                 │        │
│  └─────────────────────────────────────────────────────┘        │
└─────────────────────────────────────────────────────────────────┘
```

### Components

- **PDF Document**: Source document with multiple pages
- **Parallel Processing**: Pages are processed in parallel using Rayon
- **Text Segments**: Individual pieces of text with metadata (font, position, etc.)
- **Chunk Accumulator**: Combines segments into larger chunks while respecting token limits
- **Content Output**: Final structured output with headings, paragraphs, and metadata

## Text Extraction Flow

### Normal Text Processing

```
┌─────────────────────────────────────────────────────────────────┐
│                    NORMAL TEXT EXTRACTION                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  PDF Text Operation (Tj, TJ, etc.)                               │
│         │                                                         │
│         ▼                                                         │
│  ┌──────────────────────────────────────────────┐               │
│  │  Processor::process_stream()                   │               │
│  │  - Accumulates text in current_line            │               │
│  │  - Tracks font, position, color                │               │
│  └──────────────┬───────────────────────────────┘               │
│                  │                                                │
│                  ▼                                                │
│  ┌──────────────────────────────────────────────┐               │
│  │  create_text_segments_with_limit()             │               │
│  │  INPUT: content string, max_segment_tokens     │               │
│  └──────────────┬───────────────────────────────┘               │
│                  │                                                │
│                  ▼                                                │
│  ┌──────────────────────────────────────────────┐               │
│  │  STEP 1: Quick Estimation                      │               │
│  │  word_count = count_words_simple(content)      │               │
│  │  estimated = word_count * 1.3                  │               │
│  │                                                │               │
│  │  if estimated <= max_segment_tokens:           │               │
│  │     return single segment ──────────────────────────────┐    │
│  └──────────────┬───────────────────────────────┘         │    │
│                  │ (if too large)                           │    │
│                  ▼                                          │    │
│  ┌──────────────────────────────────────────────┐         │    │
│  │  STEP 2: Use text-splitter                     │         │    │
│  │  splitter = get_text_splitter_shared(max_tokens)│        │    │
│  │  chunks = splitter.chunks(content)             │         │    │
│  │                                                │         │    │
│  │  text-splitter uses tiktoken internally!       │         │    │
│  │  - Precise token counting                      │         │    │
│  │  - Smart boundary detection                    │         │    │
│  └──────────────┬───────────────────────────────┘         │    │
│                  │                                          │    │
│                  ▼                                          ▼    │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Create TextSegment for each chunk                        │   │
│  │  - Preserves font info, position, page num                │   │
│  │  - Each segment guaranteed <= max_segment_tokens          │   │
│  └────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### Key Parameters

- **max_segment_tokens**: 300 (conservative limit for normal text)
- **Word-to-token ratio**: 1.3 (used for estimation only)
- **Fallback**: If text-splitter fails, uses simple word-based splitting

### Code Location

- Main function: `create_text_segments_with_limit()` in `src/lib.rs`
- Called from: `Processor::process_stream()` when handling PDF text operations

## OCR Text Extraction

### OCR Processing Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                      OCR TEXT EXTRACTION                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  Image in PDF (XObject)                                          │
│         │                                                         │
│         ▼                                                         │
│  ┌──────────────────────────────────────────────┐               │
│  │  OcrHandler::process_image()                   │               │
│  │  - Runs OCR engine (ocrs)                      │               │
│  │  - Returns full OCR text string                │               │
│  └──────────────┬───────────────────────────────┘               │
│                  │                                                │
│                  ▼                                                │
│  ┌──────────────────────────────────────────────┐               │
│  │  split_ocr_text_to_segments()                  │               │
│  │  max_ocr_tokens = 200 (MORE CONSERVATIVE!)     │               │
│  └──────────────┬───────────────────────────────┘               │
│                  │                                                │
│                  ▼                                                │
│  ┌──────────────────────────────────────────────┐               │
│  │  ALWAYS uses text-splitter (no estimation!)    │               │
│  │  splitter = get_text_splitter_shared(200)      │               │
│  │  chunks = splitter.chunks(ocr_text)            │               │
│  │                                                │               │
│  │  Why 200 instead of 300?                       │               │
│  │  - OCR text often has weird spacing            │               │
│  │  - More unpredictable token density            │               │
│  │  - Safety margin for garbled text              │               │
│  └──────────────┬───────────────────────────────┘               │
│                  │                                                │
│                  ▼                                                │
│  ┌──────────────────────────────────────────────┐               │
│  │  Create TextSegment for each OCR chunk         │               │
│  │  - font_name = "OCR"                          │               │
│  │  - cutat = "OCR[0]", "OCR[1]", etc.           │               │
│  └────────────────────────────────────────────────┘             │
└─────────────────────────────────────────────────────────────────┘
```

### Key Differences from Normal Text

1. **Lower token limit**: 200 tokens vs 300 for normal text
2. **No estimation phase**: Directly uses text-splitter
3. **Special tagging**: Segments marked with "OCR" font name
4. **More conservative**: Accounts for unpredictable OCR output

### Code Location

- Main function: `split_ocr_text_to_segments()` in `src/lib.rs`
- Called from: `process_xobject()` when processing embedded images

## Chunk Accumulator

### Accumulation Process

```
┌─────────────────────────────────────────────────────────────────┐
│                      CHUNK ACCUMULATOR                            │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  Stream of TextSegments from all sources                         │
│  (normal text, OCR, forms)                                       │
│         │                                                         │
│         ▼                                                         │
│  ┌──────────────────────────────────────────────┐               │
│  │  ChunkAccumulator (in document/processing.rs)  │               │
│  │  max_tokens parameter (e.g., 350)              │               │
│  └──────────────┬───────────────────────────────┘               │
│                  │                                                │
│                  ▼                                                │
│  ┌──────────────────────────────────────────────┐               │
│  │  For each TextSegment:                         │               │
│  │                                                │               │
│  │  1. Fast Path Check:                           │               │
│  │     word_threshold = max_tokens * 0.8 / 1.3   │               │
│  │     if total_words < word_threshold:           │               │
│  │        ADD SEGMENT (no tokenization) ─────────────────┐      │
│  │                                                │       │      │
│  │  2. Slow Path (approaching limit):             │       │      │
│  │     actual_tokens = tokenizer.encode(text)     │       │      │
│  │     if will_fit:                               │       │      │
│  │        ADD SEGMENT ────────────────────────────────────┤      │
│  │     else:                                      │       │      │
│  │        FLUSH CHUNK & START NEW ────────────────────────┤      │
│  └──────────────┬───────────────────────────────┘       │      │
│                  │                                         │      │
│                  ▼                                         ▼      │
│  ┌────────────────────────────────────────────────────────┐     │
│  │  Special Cases:                                         │     │
│  │                                                         │     │
│  │  - Heading detected: FLUSH immediately                 │     │
│  │  - Sentence boundary + 90% full: FLUSH                 │     │
│  │  - Page boundary: Continue (no forced flush)           │     │
│  │  - Oversized segment: Split with split_long_sentence() │     │
│  └─────────────────────────────────────────────────────┘     │
└─────────────────────────────────────────────────────────────────┘
```

### Two-Phase Tokenization

1. **Phase 1 - Word Counting (Fast)**
   - Used when well below limit
   - Approximation: word_count * 1.3
   - No tokenizer calls

2. **Phase 2 - Actual Tokenization (Precise)**
   - Used when approaching limit (>80% capacity)
   - Exact token count via tiktoken
   - Necessary for final decision making

### Code Location

- Main struct: `ChunkAccumulator` in `src/chunk_accumulator.rs`
- Used in: `output_doc()` in `src/document/processing.rs`

## Token Counting Strategy

### Hierarchy of Token Counting Methods

```
┌─────────────────────────────────────────────────────────────────┐
│                    TOKEN COUNTING HIERARCHY                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  Level 1: ESTIMATION (Fastest)                                   │
│  ┌──────────────────────────────────────────────┐               │
│  │  word_count * 1.3 = estimated_tokens           │               │
│  │  Used when well below limits                   │               │
│  │  No tokenizer calls                            │               │
│  └────────────────────────────────────────────────┘             │
│                                                                   │
│  Level 2: TIKTOKEN COUNTING (Precise)                            │
│  ┌──────────────────────────────────────────────┐               │
│  │  tokenizer.encode_ordinary(text)               │               │
│  │  Used when:                                    │               │
│  │  - Near limits (>80% capacity)                 │               │
│  │  - Need exact count for decisions              │               │
│  └────────────────────────────────────────────────┘             │
│                                                                   │
│  Level 3: TEXT-SPLITTER (Smart Splitting)                        │
│  ┌──────────────────────────────────────────────┐               │
│  │  TextSplitter::new(ChunkConfig::new(max_tokens))│              │
│  │  - Uses tiktoken internally                    │               │
│  │  - Respects sentence/word boundaries           │               │
│  │  - Guaranteed to produce valid chunks          │               │
│  └────────────────────────────────────────────────┘             │
└─────────────────────────────────────────────────────────────────┘
```

### Performance Optimization

- **Estimation**: O(n) word counting, no tokenization
- **Tiktoken**: O(n) tokenization, exact but slower
- **Text-splitter**: O(n) with smart boundaries, most expensive

The system uses the cheapest method that provides sufficient accuracy for the current decision.

## Data Flow Example

### Processing a 500-word text segment

```
Example: Processing a text segment

INPUT: "This is a long text with many words..." (500 words)
max_tokens = 350

STEP 1: Create TextSegment
├─> create_text_segments_with_limit(content, max_tokens=300)
│   ├─> word_count = 500
│   ├─> estimated = 500 * 1.3 = 650 tokens
│   ├─> 650 > 300, so SPLIT NEEDED
│   └─> text_splitter.chunks() -> [chunk1, chunk2, chunk3]
│
├─> TextSegment 1: ~200 tokens
├─> TextSegment 2: ~200 tokens  
└─> TextSegment 3: ~250 tokens

STEP 2: ChunkAccumulator processes segments
├─> Segment 1 arrives (200 tokens)
│   ├─> word_count = 154
│   ├─> word_threshold = 350 * 0.8 / 1.3 = 215 words
│   ├─> 154 < 215, so FAST PATH
│   └─> ADD to current chunk
│
├─> Segment 2 arrives (200 tokens)
│   ├─> total_words now = 154 + 154 = 308
│   ├─> 308 > 215, so SLOW PATH
│   ├─> tokenizer.encode() = exactly 400 tokens total
│   ├─> 400 > 350, so WON'T FIT
│   ├─> FLUSH current chunk (200 tokens)
│   └─> START new chunk with Segment 2
│
└─> Segment 3 arrives (250 tokens)
    ├─> Can add to chunk with Segment 2?
    ├─> 200 + 250 = 450 > 350, so NO
    ├─> FLUSH chunk with Segment 2
    └─> START new chunk with Segment 3

FINAL OUTPUT: 3 ContentOutput chunks
- Chunk 1: 200 tokens ✓
- Chunk 2: 200 tokens ✓  
- Chunk 3: 250 tokens ✓
All under 350 limit!
```

## Limit Enforcement

### Where Token Limits Are Applied

```
┌─────────────────────────────────────────────────────────────────┐
│                    TOKEN LIMIT ENFORCEMENT                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  1. SEGMENT CREATION (lib.rs)                                    │
│     ├─> Normal text: max 300 tokens per segment                  │
│     └─> OCR text: max 200 tokens per segment                     │
│                                                                   │
│  2. CHUNK ACCUMULATION (chunk_accumulator.rs)                    │
│     ├─> User-specified limit (e.g., 350)                         │
│     ├─> Combines segments up to limit                            │
│     └─> Splits oversized segments if needed                      │
│                                                                   │
│  3. FORM FIELDS (form.rs)                                        │
│     └─> MAX_FORM_CHUNK_SIZE = 1000 chars                         │
│                                                                   │
│  4. FALLBACK SPLITTING (split_long_sentence)                     │
│     ├─> Tries clause boundaries (, ; —)                          │
│     └─> Falls back to word boundaries                            │
│                                                                   │
│  HIERARCHY OF LIMITS:                                            │
│  OCR segments (200) < Normal segments (300) < Final chunks (350) │
└─────────────────────────────────────────────────────────────────┘
```

### Limit Configuration

| Component | Default Limit | Purpose | Configurable |
|-----------|--------------|---------|--------------|
| OCR Segments | 200 tokens | Conservative for unpredictable OCR text | No |
| Normal Segments | 300 tokens | Standard text extraction | No |
| Final Chunks | 350 tokens | User-facing output | Yes (via parameter) |
| Form Fields | 1000 chars | Prevent huge form chunks | No |

## Key Design Decisions

### 1. Two-Phase Tokenization

**Rationale**: Optimize for the common case (small text) while maintaining precision when needed.

- Fast path uses word counting (O(n) string operations)
- Slow path uses tiktoken (more expensive but precise)
- Threshold at 80% capacity balances speed vs accuracy

### 2. Conservative OCR Limits

**Rationale**: OCR text is unpredictable with unusual spacing and character patterns.

- 200 token limit vs 300 for normal text
- Always uses text-splitter (no estimation)
- Provides safety margin for garbled text

### 3. Segment-Level Splitting

**Rationale**: Prevent oversized inputs to the accumulator.

- Split happens BEFORE accumulation
- Each segment guaranteed to fit within limits
- Simplifies downstream processing

### 4. Smart Text Splitting

**Rationale**: Maintain readability and semantic coherence.

- Respects sentence boundaries when possible
- Falls back to word boundaries if needed
- Uses text-splitter library for intelligent splitting

### 5. Graceful Degradation

**Rationale**: Never fail completely, always extract what's possible.

- Multiple fallback paths
- Error handling at page level
- Continue processing even with failures

## Performance Characteristics

### Time Complexity

| Operation | Complexity | Notes |
|-----------|------------|-------|
| Word counting | O(n) | Simple string traversal |
| Token counting | O(n) | Tiktoken encoding |
| Text splitting | O(n) | With boundary detection overhead |
| Accumulation | O(1) amortized | Fast path for most segments |

### Memory Usage

- Segments are processed streaming-style
- Only current chunk kept in memory
- Parallel processing bounded by CPU cores

## Error Handling

### Common Issues and Solutions

1. **Oversized segments**: Split using text-splitter
2. **OCR failures**: Skip image, continue processing
3. **Invalid PDF structure**: Extract what's possible
4. **Token counting errors**: Fall back to estimation

### Logging

The system provides detailed logging at multiple levels:

- **ERROR**: Critical issues (panics prevented)
- **WARN**: Oversized chunks, fallback paths
- **INFO**: OCR processing, segment counts
- **DEBUG**: Token counts, splitting decisions

## Future Improvements

1. **Dynamic ratio learning**: Adjust word-to-token ratio based on document
2. **Language-specific tokenization**: Different ratios for different languages
3. **Streaming architecture**: Process documents without loading entirely
4. **Smarter OCR splitting**: Use OCR confidence scores
5. **Configurable segment limits**: Allow user control over segment sizes

## Conclusion

The text extraction and chunking system is designed to be:

- **Fast**: Optimized for common cases
- **Precise**: Accurate when it matters
- **Robust**: Handles edge cases gracefully
- **Maintainable**: Clear separation of concerns

The multi-level approach to token counting and limit enforcement ensures that documents are processed efficiently while respecting token constraints at every level.