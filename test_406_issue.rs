use pdf_extract::*;
use std::path::Path;

fn main() {
    env_logger::init();
    
    // Test with the specific PDF that shows 406 token chunks
    let path = Path::new("/Users/sarav/Downloads/Dad case/6. Additonal Affidavit dated 03.07.2025.docx.pdf");
    
    println!("Testing PDF: {:?}", path);
    
    let doc = lopdf::Document::load(path).expect("Failed to load PDF");
    
    // Call output_doc with 350 token limit
    match output_doc(&doc, None, Some(350)) {
        Ok(outputs) => {
            println!("Extracted {} chunks", outputs.len());
            
            // Check each chunk's token count
            let tokenizer = tiktoken_rs::get_bpe_from_model("gpt-4o").unwrap();
            let mut oversized = Vec::new();
            
            for (idx, output) in outputs.iter().enumerate() {
                let tokens = tokenizer.encode_ordinary(&output.paragraph);
                let token_count = tokens.len();
                
                if token_count > 350 {
                    oversized.push((idx, token_count, output.paragraph.len()));
                    
                    // Print details about oversized chunks
                    if token_count == 406 {
                        println!("\n🔴 Found 406 token chunk at index {}:", idx);
                        println!("   Page: {}", output.page);
                        println!("   Text preview: {}", &output.paragraph[..100.min(output.paragraph.len())]);
                        println!("   Full text length: {} chars", output.paragraph.len());
                    }
                }
            }
            
            if !oversized.is_empty() {
                println!("\n⚠️  Found {} oversized chunks (>350 tokens):", oversized.len());
                for (idx, tokens, chars) in &oversized {
                    println!("   Chunk {}: {} tokens ({} chars)", idx, tokens, chars);
                }
            } else {
                println!("\n✅ All chunks within 350 token limit!");
            }
        }
        Err(e) => {
            eprintln!("Error extracting text: {}", e);
        }
    }
}