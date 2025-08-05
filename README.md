# PDF Extract - Advanced PDF Content Extraction Library

[![Build Status](https://github.com/jrmuizel/pdf-extract/actions/workflows/rust.yml/badge.svg)](https://github.com/jrmuizel/pdf-extract/actions)
[![crates.io](https://img.shields.io/crates/v/pdf-extract.svg)](https://crates.io/crates/pdf-extract)
[![Documentation](https://docs.rs/pdf-extract/badge.svg)](https://docs.rs/pdf-extract)

A powerful Rust library for extracting structured content from PDF files with precise positioning data and intelligent chunking for RAG (Retrieval-Augmented Generation) applications.

## 🚀 Features

- **Advanced Text Extraction** - Extract text with precise positioning and font information
- **Intelligent Chunking** - Token-aware splitting optimized for LLM consumption
- **Header/Footer Detection** - Automatic identification and filtering of repetitive content
- **Multi-page Support** - Track content spanning multiple pages with detailed fragments
- **OCR Integration** - Built-in OCR support for scanned documents
- **Structured Schema** - Modern schema with compressed metadata for efficient storage
- **Search Highlighting** - Precise bounding boxes and quads for visual highlighting
- **Production Ready** - Optimized for high-throughput document processing

## 📦 Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
pdf-extract = "0.7.7"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

## 🔧 Quick Start

### Basic Usage

```rust
use pdf_extract::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Extract content from PDF file
    let results = parse_pdf("document.pdf", 1, "file", None, None, None, Some(500))?;
    
    for result in results {
        println!("Content: {}", result.content_core.content);
        println!("Tokens: {}", result.content_core.token_count);
        
        // Extract PDF-specific location data
        let pdf_location = extract_pdf_location(&result.content_ext)?;
        println!("Pages: {} to {}", 
            pdf_location.page_range.start, 
            pdf_location.page_range.end);
    }
    
    Ok(())
}
```

### With OCR Support

```rust
use pdf_extract::*;

let results = parse_pdf(
    "scanned_document.pdf",
    1,                    // source_id
    "file",              // source_type
    Some(true),          // enable OCR
    None,                // detection model (uses default)
    None,                // recognition model (uses default)
    Some(1000)           // max tokens per chunk
)?;
```

## 📋 Schema Overview

The library returns structured data in two main components:

### ContentCore
The primary content structure:

```rust
pub struct ContentCore {
    pub chunk_id: String,         // blake3 hash of content
    pub source_id: i64,           // your source identifier
    pub source_type: String,      // "file" | "web" | "api"
    pub content: String,          // extracted text
    pub token_count: i32,         // estimated token count
    pub headings_json: Option<String>, // hierarchical headings
    pub status: String,           // extraction status
    pub schema_version: i32,      // for future compatibility
    pub created_at: i64,          // unix timestamp
}
```

### ContentExt
Compressed metadata with positioning information:

```rust
pub struct ContentExt {
    pub chunk_id: String,
    pub ext_json: Vec<u8>,        // zstd compressed metadata
}
```

The compressed metadata includes:
- **PDF Location Data** - Page ranges, character positions, bounding boxes
- **Fragment Information** - Per-page positioning with quads for highlighting
- **Text Flow** - Reading order and layout type detection

## 🎯 Advanced Features

### Precise Positioning

Extract exact locations for search highlighting:

```rust
let pdf_location = extract_pdf_location(&result.content_ext)?;

for fragment in pdf_location.fragments {
    println!("Page {}: chars {}-{}", 
        fragment.page, 
        fragment.char_range.start, 
        fragment.char_range.end);
    
    // Use quads for precise highlighting
    for quad in fragment.quads {
        println!("Highlight area: ({},{}) to ({},{})", 
            quad.x1, quad.y1, quad.x3, quad.y3);
    }
}
```

### Token-Aware Chunking

Intelligent splitting respects token limits:

```rust
// Chunks will be optimally split to stay under 500 tokens
let results = parse_pdf("large_document.pdf", 1, "file", None, None, None, Some(500))?;

for result in results {
    assert!(result.content_core.token_count <= 500);
}
```

### Hierarchical Headings

Access document structure:

```rust
if let Some(headings_json) = &result.content_core.headings_json {
    let headings: Vec<String> = serde_json::from_str(headings_json)?;
    println!("Section: {}", headings.join(" > "));
}
```

## 🔍 Metadata Extraction

Decompress and analyze metadata:

```rust
let metadata = decompress_content_ext(&result.content_ext)?;

if let Some(extraction_meta) = metadata.get("extraction_metadata") {
    if let Some(bbox) = extraction_meta.get("bbox") {
        println!("Bounding box: {:?}", bbox);
    }
}
```

## ⚡ Performance Tips

1. **Batch Processing** - Process multiple files in parallel
2. **Token Limits** - Use appropriate token limits for your use case
3. **OCR Selective** - Only enable OCR for scanned documents
4. **Compression** - Metadata is automatically compressed with zstd

## 🛠️ Integration Examples

### Database Storage

```rust
// Store in your database
struct DocumentChunk {
    id: String,
    source_id: i64,
    content: String,
    token_count: i32,
    metadata: Vec<u8>,  // compressed ContentExt.ext_json
    created_at: i64,
}

let chunk = DocumentChunk {
    id: result.content_core.chunk_id,
    source_id: result.content_core.source_id,
    content: result.content_core.content,
    token_count: result.content_core.token_count,
    metadata: result.content_ext.ext_json,
    created_at: result.content_core.created_at,
};
```

### Vector Search

```rust
// Prepare for embedding
let chunks: Vec<String> = results.iter()
    .map(|r| r.content_core.content.clone())
    .collect();

// Generate embeddings and store with chunk_id as reference
```

## 📚 API Reference

### Main Functions

- `parse_pdf()` - Main extraction function
- `extract_pdf_location()` - Extract PDF positioning data
- `decompress_content_ext()` - Decompress metadata
- `create_content_core()` - Create ContentCore structure
- `create_content_ext()` - Create ContentExt structure

### Data Structures

- `ExtractionResult` - Combined content and metadata
- `ContentCore` - Primary content structure
- `ContentExt` - Compressed metadata
- `PdfLocation` - PDF-specific positioning
- `PageFragment` - Per-page position data
- `FormatLocation` - Multi-format location enum

## 🔧 Configuration

### OCR Settings

```rust
let results = parse_pdf(
    "document.pdf",
    1,
    "file",
    Some(true),                           // enable OCR
    Some("path/to/detection.onnx".to_string()), // custom detection model
    Some("path/to/recognition.onnx".to_string()), // custom recognition model
    Some(500)
)?;
```

### Chunking Options

- `Some(500)` - Split at ~500 tokens
- `Some(1000)` - Split at ~1000 tokens  
- `None` - Natural paragraph boundaries

## 🤝 Contributing

We welcome contributions! Please see our [Integration Guide](INTEGRATION.md) for production deployment examples.

## 📄 License

MIT License - see [LICENSE](LICENSE) file for details.

## 🔗 Related Projects

- [PDFExtract](https://github.com/elacin/PDFExtract/) - Alternative PDF extraction
- [pdfminer](https://github.com/euske/pdfminer) - Python PDF mining tool
- [marker](https://github.com/VikParuchuri/marker) - PDF to markdown converter
- [layout-parser](https://github.com/Layout-Parser/layout-parser) - Document layout analysis