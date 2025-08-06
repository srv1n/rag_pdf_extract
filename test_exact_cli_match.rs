// This test will exactly replicate what OCRS CLI does
use image::open;
use ocrs::{DecodeMethod, DimOrder, ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;
use rten_tensor::prelude::*;
use rten_tensor::NdTensor;

fn format_text_output(text_lines: &[Option<ocrs::TextLine>]) -> String {
    let lines: Vec<String> = text_lines
        .iter()
        .flatten()
        .map(|line| line.to_string())
        .collect();
    lines.join("\n")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Test image that works with CLI
    let test_path = "debug_images/before_ocr_1100x1600_1754416756209.png";
    
    println!("Loading image: {}", test_path);
    
    // EXACTLY like OCRS CLI main.rs line 319-329
    let color_img: NdTensor<u8, 3> = image::open(test_path)
        .map(|image| {
            let image = image.into_rgb8();
            let (width, height) = image.dimensions();
            println!("Image dimensions: {}x{}", width, height);
            let in_chans = 3;
            NdTensor::from_data(
                [height as usize, width as usize, in_chans],
                image.into_vec(),
            )
        })?;
    
    // Load models
    let detection_model = Model::load_file("models/text-detection-ssfbcj81.rten")?;
    let recognition_model = Model::load_file("models/text-rec-checkpoint-s52qdbqt.rten")?;
    
    // Create engine EXACTLY like CLI
    let engine = OcrEngine::new(OcrEngineParams {
        detection_model: Some(detection_model),
        recognition_model: Some(recognition_model),
        debug: false,
        alphabet: None,
        decode_method: DecodeMethod::Greedy,
        allowed_chars: None,
        ..Default::default()
    })?;
    
    // Process EXACTLY like CLI main.rs line 332-333
    let color_img_source = ImageSource::from_tensor(color_img.view(), DimOrder::Hwc)?;
    let ocr_input = engine.prepare_input(color_img_source)?;
    
    // Detect and recognize
    let word_rects = engine.detect_words(&ocr_input)?;
    println!("Detected {} word rectangles", word_rects.len());
    
    let line_rects = engine.find_text_lines(&ocr_input, &word_rects);
    println!("Found {} text lines", line_rects.len());
    
    let line_texts = engine.recognize_text(&ocr_input, &line_rects)?;
    
    // Format output
    let content = format_text_output(&line_texts);
    println!("\n=== OCR Output ===");
    println!("{}", content);
    
    Ok(())
}