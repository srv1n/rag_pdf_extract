// Example: How to use the improved ContentOutput for search highlighting

use pdf_extract::*;

fn main() {
    let file = "1.pdf";
    let search_term = "family court";
    
    let docs = parse_pdf(file, None, None, None).unwrap();
    
    for (idx, doc) in docs.iter().enumerate() {
        // Search in the paragraph
        if let Some(pos) = doc.paragraph.to_lowercase().find(&search_term.to_lowercase()) {
            println!("\n=== Found '{}' in chunk {} ===", search_term, idx);
            println!("Text: {}", &doc.paragraph[pos..pos.min(pos + 50, doc.paragraph.len())]);
            
            // Single page match
            if doc.end_page.is_none() {
                println!("Location: Page {}", doc.page);
                if let Some(bbox) = &doc.bbox {
                    println!("Highlight box: x={}, y={}, width={}, height={}", 
                        bbox.x, bbox.y, bbox.width, bbox.height);
                }
            } 
            // Multi-page match
            else {
                println!("Location: Pages {} to {}", doc.page, doc.end_page.unwrap());
                println!("\nPer-page highlight boxes:");
                for page_pos in &doc.page_positions {
                    println!("  Page {}: x={}, y={}, width={}, height={}", 
                        page_pos.page, 
                        page_pos.bbox.x, 
                        page_pos.bbox.y, 
                        page_pos.bbox.width, 
                        page_pos.bbox.height);
                }
            }
            
            // Character position info (useful for precise highlighting within the text)
            if let (Some(start), Some(end)) = (doc.page_char_start, doc.page_char_end) {
                println!("\nCharacter positions:");
                println!("  First page char range: {}-{}", start, end);
                
                // For multi-page content, show per-page character ranges
                if !doc.page_positions.is_empty() {
                    for page_pos in &doc.page_positions {
                        println!("  Page {} chars: {}-{}", 
                            page_pos.page, 
                            page_pos.char_start, 
                            page_pos.char_end);
                    }
                }
            }
        }
    }
}

// Example output:
// === Found 'family court' in chunk 42 ===
// Text: family Court to expedite disposal.
// Location: Pages 17 to 18
// 
// Per-page highlight boxes:
//   Page 17: x=354.68, y=581.88, width=636.89, height=98.88
//   Page 18: x=497.52, y=40.44, width=100.0, height=20.0
//
// Character positions:
//   First page char range: 749-16
//   Page 17 chars: 749-1259
//   Page 18 chars: 0-16