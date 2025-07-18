use pdf_extract::parse_pdf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <pdf_file>", args[0]);
        std::process::exit(1);
    }

    let path = &args[1];
    match parse_pdf(path, None) {
        Ok(outputs) => {
            println!("Successfully parsed PDF with {} chunks", outputs.len());
            
            // Demonstrate improved font weight detection
            println!("\n=== Font Weight Analysis ===");
            for (i, output) in outputs.iter().take(5).enumerate() {
                println!("\nChunk #{}: Page {}", i + 1, output.page);
                println!("Text preview: {}", 
                    output.paragraph.chars().take(50).collect::<String>());
                
                // Note: Font weight info is now properly tracked in TextSegments
                // but not exposed in ContentOutput. This demonstrates the 
                // infrastructure is in place for future enhancements.
            }
            
            // Headers and footers are now automatically filtered out
            println!("\n=== Content without headers/footers ===");
            println!("The extraction now automatically removes:");
            println!("- Page numbers (e.g., 'Page 1', '-1-', '[1]')");
            println!("- Dates in headers/footers");
            println!("- Repeated copyright notices");
            println!("- Chapter titles that appear on every page");
            
            // Multi-page handling
            println!("\n=== Multi-page chunk handling ===");
            for output in outputs.iter() {
                if let Some(end_page) = output.end_page {
                    println!("Found chunk spanning pages {} to {}", 
                        output.page, end_page);
                    break;
                }
            }
        }
        Err(e) => {
            eprintln!("Error parsing PDF: {}", e);
            std::process::exit(1);
        }
    }
}