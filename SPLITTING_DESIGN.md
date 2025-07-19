# Text Splitting Design for PDF Extraction

## Requirements
- Split ContentOutput chunks to max 300 tokens (configurable)
- Use tiktoken for accurate token counting
- Preserve page tracking metadata when splitting
- Enable accurate search highlighting even after splitting

## Proposed Design

### 1. Post-Processing Approach
Split after extraction to maintain separation of concerns:
```rust
// Extract full chunks
let docs = parse_pdf(file, None, None, None)?;

// Split to desired size
let split_docs = split_content_outputs(docs, ChunkConfig {
    max_tokens: 300,
    tokenizer: cl100k_base()?,
    preserve_headings: true,
})?;
```

### 2. Metadata Handling

When splitting a ContentOutput:

```rust
Original: {
    paragraph: "This is a long text with 500 tokens...",
    page: 15,
    end_page: Some(17),
    page_char_start: Some(100),
    page_char_end: Some(850),
    page_positions: [
        PagePosition { page: 15, char_start: 100, char_end: 400, bbox: {...} },
        PagePosition { page: 16, char_start: 0, char_end: 350, bbox: {...} },
        PagePosition { page: 17, char_start: 0, char_end: 100, bbox: {...} },
    ]
}

Split into:
Chunk 1 (300 tokens): {
    paragraph: "This is a long text...",
    page: 15,
    end_page: Some(16),  // Updated based on content
    page_char_start: Some(100),
    page_char_end: Some(200),  // Estimated
    page_positions: [
        PagePosition { page: 15, char_start: 100, char_end: 400, bbox: {...} },
        PagePosition { page: 16, char_start: 0, char_end: 200, bbox: {...} }, // Partial
    ]
}

Chunk 2 (200 tokens): {
    paragraph: "...remaining text",
    page: 16,
    end_page: Some(17),
    page_char_start: Some(200),  // Continues from chunk 1
    page_char_end: Some(100),
    page_positions: [
        PagePosition { page: 16, char_start: 200, char_end: 350, bbox: {...} }, // Partial
        PagePosition { page: 17, char_start: 0, char_end: 100, bbox: {...} },
    ]
}
```

### 3. Character Position Estimation

Since we're splitting by tokens, not characters, we need to estimate character positions:

```rust
fn estimate_char_positions(
    original_text: &str,
    split_texts: &[&str],
    original_start: usize,
    original_end: usize,
) -> Vec<(usize, usize)> {
    let total_chars = original_text.len();
    let mut positions = Vec::new();
    let mut current_pos = original_start;
    
    for split_text in split_texts {
        let split_ratio = split_text.len() as f64 / total_chars as f64;
        let char_count = ((original_end - original_start) as f64 * split_ratio) as usize;
        positions.push((current_pos, current_pos + char_count));
        current_pos += char_count;
    }
    
    // Adjust last position to match original_end
    if let Some(last) = positions.last_mut() {
        last.1 = original_end;
    }
    
    positions
}
```

### 4. Bounding Box Estimation

For split chunks, estimate smaller bounding boxes:

```rust
fn estimate_bbox_for_split(
    original_bbox: &BoundingBox,
    text_offset: f64,  // 0.0 to 1.0 (position in original text)
    text_length: f64,  // 0.0 to 1.0 (portion of original text)
) -> BoundingBox {
    // Assume text flows top-to-bottom
    BoundingBox {
        x: original_bbox.x,
        y: original_bbox.y + (original_bbox.height * text_offset),
        width: original_bbox.width,
        height: original_bbox.height * text_length,
    }
}
```

### 5. Implementation Plan

1. Add dependencies to Cargo.toml:
   ```toml
   text-splitter = "0.27.0"
   tiktoken-rs = "0.6.0"
   ```

2. Create text splitting module:
   ```rust
   // src/text_splitting.rs
   pub struct ChunkConfig {
       pub max_tokens: usize,
       pub preserve_headings: bool,
       pub overlap_tokens: usize,  // For context continuity
   }
   
   pub fn split_content_outputs(
       outputs: Vec<ContentOutput>,
       config: ChunkConfig,
   ) -> Result<Vec<ContentOutput>, Box<dyn Error>> {
       // Implementation
   }
   ```

3. Add helper functions for metadata preservation

4. Update examples to show usage

## Questions for Discussion

1. **Overlap between chunks?** - Should we have 10-20 token overlap for context?

2. **Heading handling?** - Should headings be:
   - Duplicated in all chunks from that section?
   - Only in the first chunk?
   - Configurable?

3. **Empty chunks?** - What if preprocessing removes all content?

4. **Performance?** - Tokenization is expensive. Should we cache token counts?

5. **Search implications?** - How does splitting affect search accuracy?
   - Maybe store original ContentOutput ID in split chunks?
   - Add a `parent_chunk_id` field?

Please let me know your preferences on these design decisions!