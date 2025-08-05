use image::{imageops, RgbImage};
use std::error::Error;
use log::{debug, info, warn};

use super::OcrProcessor;

// OCR engine configuration
#[derive(Debug)]
pub struct OcrConfig {
    pub detection_model: Option<String>,
    pub recognition_model: Option<String>,
}

// OCR handler that wraps the processor
pub struct OcrHandler {
    processor: OcrProcessor,
}

impl OcrHandler {
    pub fn new(config: &OcrConfig) -> Result<Self, Box<dyn Error>> {
        debug!("Creating OCR handler with config: {:?}", config);
        
        let detection_model_path = config.detection_model.as_deref()
            .ok_or("Detection model path is required")?;
        let recognition_model_path = config.recognition_model.as_deref()
            .ok_or("Recognition model path is required")?;
        
        println!("🔥 PATHS: detection={}, recognition={}", detection_model_path, recognition_model_path);
            
        let processor = OcrProcessor::new(detection_model_path, recognition_model_path)?;
        println!("🔥 OCR HANDLER CREATED SUCCESSFULLY");
        Ok(Self { processor })
    }
    
    pub fn process_image(&self, img: &RgbImage) -> Result<String, Box<dyn Error>> {
        println!("🚀 PROCESS_IMAGE CALLED with image {}x{}", img.dimensions().0, img.dimensions().1);
        
        // CRITICAL: The OCRS CLI uses image::open() which might do something different
        // Let's save and reload the image EXACTLY like the CLI does
        
        let debug_dir = "./debug_images";
        if let Err(e) = std::fs::create_dir_all(debug_dir) {
            println!("🔍 OCR DEBUG: Failed to create debug directory: {}", e);
        }
        
        // First rotate the image to correct orientation
        println!("🔍 OCR: Rotating image 90 degrees clockwise to correct orientation");
        let rotated = imageops::rotate90(img);
        
        // Save it with unique filename to avoid race conditions
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let thread_id = std::thread::current().id();
        let temp_filename = format!("{}/temp_for_ocr_{}_{:?}.png", debug_dir, timestamp, thread_id);
        rotated.save(&temp_filename)?;
        println!("🔍 OCR DEBUG: Saved rotated image to: {}", temp_filename);
        
        // Now load it EXACTLY like OCRS CLI does
        let reloaded_image = image::open(&temp_filename)?;
        println!("🔍 OCR DEBUG: Reloaded image using image::open(), format: {:?}", reloaded_image.color());
        
        // Convert to RGB8 exactly like CLI
        let rgb_image = reloaded_image.into_rgb8();
        println!("🔍 OCR DEBUG: Converted to RGB8, dimensions: {}x{}", rgb_image.width(), rgb_image.height());
        
        // Save the final image for debugging
        let final_filename = format!("{}/last_before_ocr.png", debug_dir);
        rgb_image.save(&final_filename)?;
        println!("🔍 OCR DEBUG: ✅ Saved final image: {}", final_filename);
        
        println!("SENT-TO-OCR  w={}  h={}", rgb_image.width(), rgb_image.height());
        
        // Use the reloaded image
        let text = self.processor.extract_text(&rgb_image)?;
        println!("🚀 PROCESS_IMAGE COMPLETED with text length: {}", text.len());
        
        // Clean up temp file
        let _ = std::fs::remove_file(&temp_filename);
        
        Ok(text)
    }
}