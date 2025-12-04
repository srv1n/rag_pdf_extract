use pdf_extract::*;
use std::collections::HashSet;
use std::env;
use std::process;
use tiktoken_rs::get_bpe_from_model;

fn main() {
    // Enable logging from the library when RUST_LOG is set
    let _ = env_logger::try_init();
    let args: Vec<String> = env::args().collect();

    // Handle help
    if args.len() > 1 && (args[1] == "--help" || args[1] == "-h") {
        println!(
            "Usage: {} [PDF_FILE] [MAX_TOKENS] [--ocr DETECTION_MODEL RECOGNITION_MODEL] [--no-layout] [--raw]",
            args[0]
        );
        println!("  PDF_FILE: Path to PDF file (default: 12. CCI v Kerala...)");
        println!("  MAX_TOKENS: Maximum tokens per chunk (default: 500)");
        println!("  --ocr: Enable OCR with detection and recognition models");
        println!("  --no-layout: Disable layout analysis (enabled by default)");
        println!("  --raw: Disable text cleaning (preserves exact extraction with all whitespace/special chars)");
        process::exit(0);
    }

    // Default file and options
    let default_file = "eval/corpus/legal/12. CCI v Kerala Film Exhibitors Federation & Ors.pdf".to_string();
    let file = args.get(1).unwrap_or(&default_file);
    let max_tokens = args
        .get(2)
        .and_then(|s| s.parse::<usize>().ok())
        .or(Some(500));

    // Check for OCR flag
    let ocr_config = if args.len() > 3 && args[3] == "--ocr" && args.len() >= 6 {
        Some(OcrConfig {
            detection_model: Some(args[4].clone()),
            recognition_model: Some(args[5].clone()),
        })
    } else {
        None
    };

    println!("=== Extracting PDF: {} ===", file);
    if let Some(tokens) = max_tokens {
        println!("Max tokens per chunk: {}", tokens);
    } else {
        println!("No token limit (natural paragraph chunking)");
    }
    if ocr_config.is_some() {
        println!("OCR enabled with models");
    }
    println!();

    // Use layout analysis by default (matches extract_markdown.rs behavior)
    // Can be disabled with --no-layout flag
    let disable_layout = args.iter().any(|a| a == "--no-layout");
    let mut lp = if disable_layout { None } else { Some(LAParams::default()) };
    // Enable all_texts to include text inside Form XObjects (matches extract_markdown.rs)
    if let Some(ref mut p) = lp {
        p.all_texts = true;
    }

    // Text cleaning for indexing (enabled by default, can be disabled with --raw)
    let clean_text = !args.iter().any(|a| a == "--raw");
    // LAParams tuning flags
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--la-all-texts" => {
                if let Some(ref mut p) = lp { p.all_texts = true; }
            }
            "--la-detect-vertical" => {
                if let Some(ref mut p) = lp { p.detect_vertical = true; }
            }
            "--la-boxes-flow" => {
                if i + 1 < args.len() {
                    if let Ok(v) = args[i+1].parse::<f32>() {
                        if let Some(ref mut p) = lp { p.boxes_flow = v; }
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let laparams = lp;

    // Parse the PDF
    let docs = match parse_pdf(
        file,
        1,
        "file",
        ocr_config,
        None,
        None,
        max_tokens,
        laparams,
        Some(clean_text),
    ) {
        Ok(docs) => docs,
        Err(e) => {
            eprintln!("Error parsing PDF: {}", e);
            process::exit(1);
        }
    };

    println!("Total chunks extracted: {}\n", docs.len());

    // Initialize tokenizer for verification
    let bpe = get_bpe_from_model("gpt-4o").unwrap();

    for (idx, result) in docs.iter().enumerate() {
        let content_core = &result.content_core;

        println!("{}", "=".repeat(80));
        println!("CHUNK #{}", idx);
        println!("{}", "=".repeat(80));

        // Extract actual headings from content (markdown format)
        let mut headings = Vec::new();
        for line in content_core.content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("##") {
                // Remove the ## prefix and trim
                let heading_text = trimmed.trim_start_matches('#').trim();
                if !heading_text.is_empty() {
                    headings.push(heading_text.to_string());
                }
            }
        }

        // Heading information
        if !headings.is_empty() {
            println!("📑 HEADING: {}", headings.join(" > "));
        }

        // Show raw compressed metadata size
        println!(
            "💾 COMPRESSED METADATA SIZE: {} bytes",
            result.content_ext.ext_json.len()
        );

        // Show full decompressed metadata structure
        if let Ok(json_data) = decompress_content_ext(&result.content_ext) {
            println!("🔍 DECOMPRESSED METADATA STRUCTURE:");
            let pretty_json = serde_json::to_string_pretty(&json_data).unwrap_or_default();
            // Show first 500 chars of pretty JSON
            println!("{}", pretty_json);
        }

        // Extract and display PDF location metadata
        if let Ok(pdf_location) = extract_pdf_location(&result.content_ext) {
            // Get page range from fragments
            let pages: Vec<u32> = pdf_location.fragments.iter().map(|f| f.page).collect();
            let min_page = pages.iter().min().unwrap_or(&1);
            let max_page = pages.iter().max().unwrap_or(&1);

            if min_page != max_page {
                println!("📄 PAGES: {} to {} (multi-page chunk)", min_page, max_page);
            } else {
                println!("📄 PAGE: {}", min_page);
            }

            // Fragment information
            println!("📊 FRAGMENTS: {} items", pdf_location.fragments.len());
            for (frag_idx, fragment) in pdf_location.fragments.iter().enumerate() {
                println!(
                    "   Fragment {}: page {}, chars {}-{}",
                    frag_idx, fragment.page, fragment.char_range.start, fragment.char_range.end
                );

                // Show bounding box
                println!(
                    "     Bbox: x={:.2}, y={:.2}, w={:.2}, h={:.2}",
                    fragment.bbox.x, fragment.bbox.y, fragment.bbox.width, fragment.bbox.height
                );
            }

            // Text flow info removed - not needed for basic highlighting
        }

        // Also show how this would be converted to DocumentRange format (as used in rznapp)
        if let Ok(pdf_location) = extract_pdf_location(&result.content_ext) {
            println!("\n🔄 DOCUMENT RANGE CONVERSION (for rznapp):");

            // Group fragments by page
            let mut page_fragments: std::collections::HashMap<u32, Vec<_>> =
                std::collections::HashMap::new();
            for fragment in &pdf_location.fragments {
                let page = fragment.page;
                page_fragments
                    .entry(page)
                    .or_insert_with(Vec::new)
                    .push(fragment);
            }

            println!("  {} pages with content", page_fragments.len());
            for (page, fragments) in page_fragments.iter() {
                let mut min_char = usize::MAX;
                let mut max_char = 0;
                let mut bbox_count = 0;

                for fragment in fragments {
                    min_char = min_char.min(fragment.char_range.start);
                    max_char = max_char.max(fragment.char_range.end);
                    bbox_count += 1; // One bbox per fragment
                }

                println!(
                    "  Page {}: chars {}-{}, {} bounding boxes",
                    page, min_char, max_char, bbox_count
                );
            }
        }

        // Content preview and stats
        // Match the library’s counting (encode_ordinary)
        let token_count = bpe.encode_ordinary(&content_core.content).len();

        println!("📊 CORE DATA:");
        println!("  chunk_id: {}", &content_core.chunk_id[..12]);
        println!("  source_id: {}", content_core.source_id);
        println!("  estimated_tokens: {}", content_core.token_count);
        println!("  actual_tokens: {}", token_count);

        println!(
            "\n📝 CONTENT ({} chars, {} tokens):",
            content_core.content.len(),
            token_count
        );
        if max_tokens.is_some() && token_count > max_tokens.unwrap() {
            println!(
                "⚠️  WARNING: Chunk exceeds max tokens ({} > {})",
                token_count,
                max_tokens.unwrap()
            );
        }
        println!("{}", "-".repeat(40));

        // Show the actual content with proper line breaks
        println!("{}", content_core.content);

        println!("\n");
    }

    // Summary statistics
    println!("{}", "=".repeat(80));
    println!("SUMMARY:");
    println!("{}", "=".repeat(80));
    println!("Total chunks: {}", docs.len());

    let total_chars: usize = docs
        .iter()
        .map(|result| result.content_core.content.len())
        .sum();
    println!("Total characters: {}", total_chars);

    // Token statistics
    let allowed_special = HashSet::new();
    let token_counts: Vec<usize> = docs
        .iter()
        .map(|result| {
            let (tokens, _) = bpe.encode(&result.content_core.content, &allowed_special);
            tokens.len()
        })
        .collect();
    let total_tokens: usize = token_counts.iter().sum();
    let max_chunk_tokens = token_counts.iter().max().unwrap_or(&0);
    let min_chunk_tokens = token_counts.iter().min().unwrap_or(&0);
    let avg_chunk_tokens = if !token_counts.is_empty() {
        total_tokens / token_counts.len()
    } else {
        0
    };

    println!("Total tokens: {}", total_tokens);
    println!(
        "Token stats: min={}, max={}, avg={}",
        min_chunk_tokens, max_chunk_tokens, avg_chunk_tokens
    );

    if let Some(max_t) = max_tokens {
        let chunks_over_limit = token_counts.iter().filter(|&&t| t > max_t).count();
        if chunks_over_limit > 0 {
            println!(
                "⚠️  {} chunks exceed the {} token limit!",
                chunks_over_limit, max_t
            );
        }
    }

    let pages: std::collections::HashSet<u32> = docs
        .iter()
        .filter_map(|result| {
            extract_pdf_location(&result.content_ext)
                .ok()
                .map(|loc| loc.fragments.iter().map(|f| f.page).collect::<Vec<_>>())
        })
        .flatten()
        .collect();
    println!("Pages covered: {:?}", pages);

    let multi_page_chunks = docs
        .iter()
        .filter(|result| {
            extract_pdf_location(&result.content_ext)
                .map(|loc| {
                    let pages: std::collections::HashSet<u32> =
                        loc.fragments.iter().map(|f| f.page).collect();
                    pages.len() > 1
                })
                .unwrap_or(false)
        })
        .count();
    println!("Multi-page chunks: {}", multi_page_chunks);

    let total_compressed_size: usize = docs
        .iter()
        .map(|result| result.content_ext.ext_json.len())
        .sum();
    println!(
        "Total compressed metadata size: {} bytes",
        total_compressed_size
    );
}
