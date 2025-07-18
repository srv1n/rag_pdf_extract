use pdf_extract::parse_pdf;

fn main() {
    // Example to demonstrate multi-page chunk handling
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <pdf_file>", args[0]);
        std::process::exit(1);
    }

    let path = &args[1];
    let outputs = parse_pdf(path, None).expect("Failed to parse PDF");

    println!("Total chunks extracted: {}", outputs.len());
    println!("\nChunks that span multiple pages:");
    
    for (i, output) in outputs.iter().enumerate() {
        if let Some(end_page) = output.end_page {
            println!("\nChunk #{} (multi-page):", i + 1);
            println!("  Start page: {}", output.page);
            println!("  End page: {}", end_page);
            println!("  Text preview: {}", 
                output.paragraph.chars().take(100).collect::<String>());
            
            if let Some(bbox) = &output.bbox {
                println!("  Bounding box on start page: x={:.2}, y={:.2}, width={:.2}, height={:.2}",
                    bbox.x, bbox.y, bbox.width, bbox.height);
            }
            
            if let (Some(start), Some(end)) = (output.page_char_start, output.page_char_end) {
                println!("  Character positions: {} to {}", start, end);
            }
        }
    }
    
    println!("\n\nSingle-page chunks:");
    for (i, output) in outputs.iter().enumerate() {
        if output.end_page.is_none() {
            println!("\nChunk #{} (single page):", i + 1);
            println!("  Page: {}", output.page);
            println!("  Text preview: {}", 
                output.paragraph.chars().take(100).collect::<String>());
        }
    }
}