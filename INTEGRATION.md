# PDF Extract Integration Guide

## Overview

The `pdf-extract` crate provides high-quality PDF text extraction with intelligent chunking that respects token limits and sentence boundaries. This guide covers how to integrate the latest version (0.7.7+) which includes significant improvements to text chunking.

## Key Features

- **Token-aware chunking**: Respects OpenAI token limits (gpt-4o tokenizer)
- **Sentence boundary detection**: Avoids splitting text mid-sentence
- **Efficient tokenization**: Two-phase approach minimizes tokenization overhead
- **Metadata preservation**: Maintains page numbers, bounding boxes, and character positions
- **Header hierarchy tracking**: Preserves document structure

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
pdf-extract = "0.7.7"
```

## Basic Usage

### Simple PDF Extraction

```rust
use pdf_extract::{parse_pdf, ExtractionResult};

fn extract_pdf(file_path: &str) -> Result<Vec<ExtractionResult>, Box<dyn std::error::Error>> {
    // Extract with 500 token chunks
    let max_tokens = Some(500);
    let source_id = 1; // Your document ID
    let source_type = "pdf"; // Document type identifier
    
    let results = parse_pdf(
        file_path,
        source_id,
        source_type,
        None,  // No OCR config
        None,  // No OCR cache
        None,  // Resume flag (deprecated)
        max_tokens
    )?;
    
    Ok(results)
}
```

### Advanced Usage with Direct Document Access

```rust
use pdf_extract::{output_doc_new_schema, ExtractionResult};
use lopdf::Document;

fn extract_from_document(doc: &Document) -> Result<Vec<ExtractionResult>, Box<dyn std::error::Error>> {
    let max_tokens = Some(500);
    let source_id = 1;
    let source_type = "pdf";
    
    let results = output_doc_new_schema(
        doc,
        None,  // No OCR handler
        max_tokens,
        source_id,
        source_type
    )?;
    
    Ok(results)
}
```

## Understanding the Output

### ExtractionResult Structure

Each `ExtractionResult` contains:

```rust
pub struct ExtractionResult {
    pub content_core: ContentCore,
    pub content_ext: ContentExt,
}
```

### ContentCore - Main Content

```rust
pub struct ContentCore {
    pub chunk_id: String,        // Unique chunk identifier
    pub source_id: i64,          // Your document ID
    pub source_type: String,     // Document type
    pub content: String,         // The actual text content
    pub token_count: i32,        // Estimated token count
    pub headings_json: Option<String>, // JSON array of headings
}
```

### ContentExt - Metadata

The `ContentExt` contains compressed metadata. To access it:

```rust
use pdf_extract::{decompress_content_ext, extract_pdf_location};

// Decompress all metadata
let metadata = decompress_content_ext(&result.content_ext)?;

// Extract PDF-specific location data
let pdf_location = extract_pdf_location(&result.content_ext)?;
```

### PDF Location Data

```rust
pub struct PdfLocation {
    pub fragments: Vec<Fragment>,
}

pub struct Fragment {
    pub page: u32,
    pub char_range: CharRange,  // Character positions on the page
    pub bbox: BoundingBox,      // Bounding box coordinates
}
```

## Token Limit Behavior

### Strict Token Limits

The chunking system now enforces **strict token limits** with these guarantees:

1. **No chunks will exceed the specified max_tokens**
2. **No content is ever dropped**
3. **Sentence boundaries are respected when possible**

### How It Works

1. **Two-phase tokenization**: 
   - Uses word counting for efficiency until 80% of limit
   - Switches to precise token counting near the limit

2. **Sentence awareness**:
   - At 90% capacity, looks for sentence boundaries to flush
   - Won't split mid-sentence unless absolutely necessary
   - If a single sentence exceeds max_tokens, it's split at clause boundaries

3. **No overshoot tolerance**:
   - Previous versions allowed 20% overshoot
   - Current version has 0% tolerance - max_tokens is a hard limit

### Example Token Limits

```rust
// For different use cases
let max_tokens = Some(500);   // GPT-3.5/4 with room for prompts
let max_tokens = Some(1000);  // Larger context windows
let max_tokens = Some(8000);  // Maximum context utilization
let max_tokens = None;        // No limit - natural paragraph breaks
```

## Handling Multi-Page Chunks

Due to the sentence-aware chunking, some chunks may span multiple pages:

```rust
for result in results {
    if let Ok(pdf_location) = extract_pdf_location(&result.content_ext) {
        let pages: Vec<u32> = pdf_location.fragments.iter()
            .map(|f| f.page)
            .collect();
        
        if pages.len() > 1 {
            println!("Chunk spans pages: {:?}", pages);
        }
    }
}
```

## Working with Headings

Headings are preserved in the heading hierarchy:

```rust
for result in results {
    if let Some(headings_json) = &result.content_core.headings_json {
        let headings: Vec<String> = serde_json::from_str(headings_json)?;
        println!("Section: {}", headings.join(" > "));
    }
}
```

## OCR Support

For scanned PDFs, configure OCR:

```rust
use pdf_extract::{OcrConfig, parse_pdf};

let ocr_config = OcrConfig {
    detection_model_path: "/path/to/detection.rten",
    recognition_model_path: "/path/to/recognition.rten",
};

let results = parse_pdf(
    file_path,
    source_id,
    source_type,
    Some(ocr_config),
    None,  // OCR cache path
    None,
    max_tokens
)?;
```

## Best Practices

### 1. Choose Appropriate Token Limits

- **For embeddings**: 500-1000 tokens (leaves room for model overhead)
- **For QA systems**: 2000-4000 tokens (balanced context)
- **For summarization**: 4000-8000 tokens (maximum context)

### 2. Handle Token Validation

Always verify token counts if critical:

```rust
use tiktoken_rs::get_bpe_from_model;

let tokenizer = get_bpe_from_model("gpt-4o")?;
for result in results {
    let actual_tokens = tokenizer.encode_ordinary(&result.content_core.content).len();
    assert!(actual_tokens <= max_tokens.unwrap());
}
```

### 3. Process Chunks Efficiently

```rust
// Process in parallel for better performance
use rayon::prelude::*;

let embeddings: Vec<_> = results
    .par_iter()
    .map(|result| generate_embedding(&result.content_core.content))
    .collect();
```

### 4. Preserve Context

When splitting for QA, include headings for context:

```rust
fn format_chunk_with_context(result: &ExtractionResult) -> String {
    let mut output = String::new();
    
    if let Some(headings_json) = &result.content_core.headings_json {
        if let Ok(headings) = serde_json::from_str::<Vec<String>>(headings_json) {
            output.push_str(&headings.join(" > "));
            output.push_str("\n\n");
        }
    }
    
    output.push_str(&result.content_core.content);
    output
}
```

## Migration from Older Versions

If upgrading from versions before 0.7.7:

### Key Changes

1. **Stricter token limits**: Chunks will never exceed max_tokens
2. **Better sentence handling**: Less mid-sentence splits
3. **Consistent behavior**: No more random splitting

### Code Changes

```rust
// Old API (if using internal functions)
let docs = output_doc(&document, None, max_tokens)?;

// New API
let docs = output_doc_new_schema(&document, None, max_tokens, source_id, source_type)?;
```

## Performance Considerations

1. **Tokenization is cached**: The two-phase approach minimizes tokenizer calls
2. **Efficient for large documents**: Processes documents in a single pass
3. **Memory usage**: Proportional to chunk size, not document size

## Error Handling

```rust
use pdf_extract::parse_pdf;

match parse_pdf(file_path, 1, "pdf", None, None, None, Some(500)) {
    Ok(results) => {
        println!("Extracted {} chunks", results.len());
    }
    Err(e) => {
        eprintln!("Extraction failed: {}", e);
        // Handle specific error types
        if e.to_string().contains("Failed to load PDF") {
            // File not found or invalid PDF
        }
    }
}
```

## Example: Building a RAG Pipeline

```rust
use pdf_extract::{parse_pdf, ExtractionResult};

async fn index_pdf_for_rag(
    file_path: &str,
    vector_db: &VectorDB,
) -> Result<(), Box<dyn std::error::Error>> {
    // Extract with appropriate chunk size for embeddings
    let chunks = parse_pdf(file_path, 1, "pdf", None, None, None, Some(500))?;
    
    for (idx, chunk) in chunks.iter().enumerate() {
        // Generate embedding
        let embedding = generate_embedding(&chunk.content_core.content).await?;
        
        // Prepare metadata
        let metadata = json!({
            "source": file_path,
            "chunk_id": chunk.content_core.chunk_id,
            "chunk_index": idx,
            "headings": chunk.content_core.headings_json,
            "token_count": chunk.content_core.token_count,
        });
        
        // Store in vector database
        vector_db.insert(
            &chunk.content_core.chunk_id,
            embedding,
            &chunk.content_core.content,
            metadata,
        ).await?;
    }
    
    Ok(())
}
```

## Troubleshooting

### Chunks Still Exceeding Limit?

This should not happen in 0.7.7+. If it does:

1. Verify you're using the latest version
2. Check if you're modifying content after extraction
3. File an issue with a reproducible example

### Too Many Small Chunks?

The system prioritizes respecting limits over chunk size. Consider:

1. Increasing max_tokens if your use case allows
2. Using None for max_tokens if you just need paragraph breaks

### Missing Content?

The system never drops content. If content appears missing:

1. Check all chunks - it may be in the next chunk
2. Verify the PDF is valid and text is extractable
3. Enable OCR if dealing with scanned content

## Support

For issues or questions:
- GitHub: https://github.com/jrmuizel/pdf-extract
- Ensure you're using version 0.7.7 or later
- Include PDF samples when reporting issues