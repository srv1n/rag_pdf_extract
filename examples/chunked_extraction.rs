// Example: Using token-based chunking for PDF extraction

use pdf_extract::*;

fn main() {
    let file = "1.pdf";
    
    // Extract with different chunk sizes
    println!("=== Extracting with 300 token chunks ===");
    let docs_300 = parse_pdf(file, None, None, None, Some(300)).unwrap();
    println!("Number of chunks: {}", docs_300.len());
    
    println!("\n=== Extracting with 500 token chunks ===");
    let docs_500 = parse_pdf(file, None, None, None, Some(500)).unwrap();
    println!("Number of chunks: {}", docs_500.len());
    
    println!("\n=== Extracting without chunking ===");
    let docs_unlimited = parse_pdf(file, None, None, None, None).unwrap();
    println!("Number of chunks: {}", docs_unlimited.len());
    
    // Show how headings are preserved across chunks
    println!("\n=== Heading preservation example ===");
    let mut last_heading = String::new();
    for (idx, doc) in docs_300.iter().take(10).enumerate() {
        let current_heading = doc.headings.first().unwrap_or(&String::new()).clone();
        
        if current_heading != last_heading && !current_heading.is_empty() {
            println!("\nNew section: {}", current_heading);
            last_heading = current_heading;
        }
        
        println!("  Chunk {}: {} chars, pages {}-{}", 
            idx, 
            doc.paragraph.len(),
            doc.page,
            doc.end_page.unwrap_or(doc.page)
        );
        
        // Show first 50 chars of content
        let preview = if doc.paragraph.len() > 50 {
            format!("{}...", &doc.paragraph[..50])
        } else {
            doc.paragraph.clone()
        };
        println!("    Preview: {}", preview.replace('\n', " "));
    }
    
    // Show accurate position tracking
    println!("\n=== Position tracking example ===");
    for (idx, doc) in docs_300.iter().take(3).enumerate() {
        println!("\nChunk {}: Pages {}-{}", 
            idx, 
            doc.page,
            doc.end_page.unwrap_or(doc.page)
        );
        
        if !doc.page_positions.is_empty() {
            println!("  Per-page positions:");
            for pos in &doc.page_positions {
                println!("    Page {}: chars {}-{}, bbox x={:.1} y={:.1} w={:.1} h={:.1}",
                    pos.page, 
                    pos.char_start, 
                    pos.char_end,
                    pos.bbox.x,
                    pos.bbox.y,
                    pos.bbox.width,
                    pos.bbox.height
                );
            }
        }
    }
}