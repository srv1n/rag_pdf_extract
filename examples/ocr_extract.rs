use pdf_extract::*;
use std::env;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    // Handle help
    if args.len() > 1 && (args[1] == "--help" || args[1] == "-h") {
        println!("Usage: {} [PDF_FILE]", args[0]);
        println!("  PDF_FILE: Path to PDF file (default: 9.pdf)");
        println!("  This example uses local OCRS models for text extraction");
        process::exit(0);
    }

    // Default file
    let default_file = "ocr.pdf".to_string();
    let file = args.get(1).unwrap_or(&default_file);

    println!("=== OCRS-Enhanced PDF Extraction: {} ===", file);
    println!("Using local OCRS models for improved text recognition");
    println!();

    // Configure OCR with local model files
    let detection_model_path = "models/text-detection-ssfbcj81.rten";
    let recognition_model_path = "models/text-rec-checkpoint-s52qdbqt.rten";

    // Check if model files exist
    if !std::path::Path::new(detection_model_path).exists() {
        eprintln!(
            "Error: Detection model not found at {}",
            detection_model_path
        );
        eprintln!("Please run: cargo run --example extract");
        eprintln!("This will download the required models.");
        process::exit(1);
    }

    if !std::path::Path::new(recognition_model_path).exists() {
        eprintln!(
            "Error: Recognition model not found at {}",
            recognition_model_path
        );
        eprintln!("Please run: cargo run --example extract");
        eprintln!("This will download the required models.");
        process::exit(1);
    }

    println!("✓ Detection model: {}", detection_model_path);
    println!("✓ Recognition model: {}", recognition_model_path);
    println!();

    // Configure OCR settings
    let ocr_config = OcrConfig {
        detection_model: Some(detection_model_path.to_string()),
        recognition_model: Some(recognition_model_path.to_string()),
    };

    // Parse the PDF with OCR enabled
    let docs = match parse_pdf(
        file,
        1,
        "file",
        Some(ocr_config),
        None,      // ocr_cache
        None,      // resume
        Some(500), // Max tokens per chunk
    ) {
        Ok(docs) => docs,
        Err(e) => {
            eprintln!("Error parsing PDF with OCR: {}", e);
            process::exit(1);
        }
    };

    println!(
        "🎉 Successfully extracted {} chunks using OCRS!",
        docs.len()
    );
    println!();

    // Show some sample content and search for OCR-extracted content
    for (idx, result) in docs.iter().take(5).enumerate() {
        let content_core = &result.content_core;

        println!("{}", "=".repeat(60));
        println!("SAMPLE CHUNK #{}", idx + 1);
        println!("{}", "=".repeat(60));

        // Extract headings from JSON
        let headings: Vec<String> = if let Some(headings_json) = &content_core.headings_json {
            serde_json::from_str(headings_json).unwrap_or_default()
        } else {
            vec![]
        };

        if !headings.is_empty() {
            println!("📑 HEADING: {}", headings.join(" > "));
        }

        // Extract and display PDF location metadata
        if let Ok(pdf_location) = extract_pdf_location(&result.content_ext) {
            let pages: Vec<u32> = pdf_location.fragments.iter().map(|f| f.page).collect();
            let min_page = pages.iter().min().unwrap_or(&1);
            let max_page = pages.iter().max().unwrap_or(&1);

            if min_page != max_page {
                println!("📄 PAGES: {} to {}", min_page, max_page);
            } else {
                println!("📄 PAGE: {}", min_page);
            }
        }

        println!("📝 CONTENT ({} chars):", content_core.content.len());
        println!("{}", "-".repeat(40));

        // Show first 300 chars of content
        let preview = if content_core.content.len() > 300 {
            format!("{}...", &content_core.content[..300])
        } else {
            content_core.content.clone()
        };
        println!("{}", preview);
        println!();
    }

    // Summary statistics
    println!("{}", "=".repeat(60));
    println!("OCRS EXTRACTION SUMMARY");
    println!("{}", "=".repeat(60));
    println!("✓ Total chunks extracted: {}", docs.len());

    let total_chars: usize = docs
        .iter()
        .map(|result| result.content_core.content.len())
        .sum();
    println!("✓ Total characters: {}", total_chars);

    let pages: std::collections::HashSet<u32> = docs
        .iter()
        .filter_map(|result| {
            extract_pdf_location(&result.content_ext)
                .ok()
                .map(|loc| loc.fragments.iter().map(|f| f.page).collect::<Vec<_>>())
        })
        .flatten()
        .collect();
    println!("✓ Pages processed: {} pages", pages.len());

    // Look for potential OCR content by searching for specific patterns
    let mut ocr_indicators = 0;
    let mut has_image_markers = false;
    
    for result in &docs {
        let content = &result.content_core.content;
        // Look for signs this might contain OCR text
        if content.contains("Image") || content.to_lowercase().contains("extracted") {
            has_image_markers = true;
        }
        // Count chunks with very short content that might be OCR fragments
        if content.len() < 50 && !content.trim().is_empty() {
            ocr_indicators += 1;
        }
    }
    
    if has_image_markers {
        println!("✓ Found potential image-related content markers");
    }
    if ocr_indicators > 0 {
        println!("✓ Found {} short text segments (potential OCR fragments)", ocr_indicators);
    }

    println!("\n🎯 OCRS models successfully used for enhanced text recognition!");
    println!("   Detection model: {}", detection_model_path);
    println!("   Recognition model: {}", recognition_model_path);
}
