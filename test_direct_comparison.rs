use std::process::Command;
use image::open;
use pdf_extract::{OcrConfig, OcrHandler};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Test with the exact image from debug_images
    let test_image = "debug_images/1.png";
    
    println!("=== Testing image: {} ===\n", test_image);
    
    // 1. Run OCRS CLI and capture output
    println!("1. OCRS CLI Output:");
    println!("-------------------");
    let output = Command::new("ocrs")
        .arg(test_image)
        .output()?;
    let cli_text = String::from_utf8_lossy(&output.stdout);
    println!("{}", cli_text.lines().take(20).collect::<Vec<_>>().join("\n"));
    
    // 2. Run our integration
    println!("\n2. Our Integration Output:");
    println!("--------------------------");
    let config = OcrConfig {
        detection_model: None,  // Use default models
        recognition_model: None,
    };
    let handler = OcrHandler::new(&config)?;
    
    let img = open(test_image)?;
    let rgb = img.to_rgb8();
    println!("Image size: {}x{}", rgb.width(), rgb.height());
    
    let our_text = handler.process_image(&rgb)?;
    println!("{}", our_text.lines().take(20).collect::<Vec<_>>().join("\n"));
    
    // 3. Compare
    println!("\n=== Comparison ===");
    let cli_words: Vec<&str> = cli_text.split_whitespace().collect();
    let our_words: Vec<&str> = our_text.split_whitespace().collect();
    
    println!("CLI word count: {}", cli_words.len());
    println!("Our word count: {}", our_words.len());
    
    // Check first few words
    println!("\nFirst 10 words comparison:");
    println!("CLI: {:?}", &cli_words[..10.min(cli_words.len())]);
    println!("Our: {:?}", &our_words[..10.min(our_words.len())]);
    
    Ok(())
}