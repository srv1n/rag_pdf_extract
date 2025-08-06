use image::RgbImage;
use std::error::Error;
use log::{debug, info};

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
        
        debug!("Model paths - detection: {}, recognition: {}", 
               detection_model_path, recognition_model_path);
            
        let processor = OcrProcessor::new(detection_model_path, recognition_model_path)?;
        info!("OCR handler created successfully");
        Ok(Self { processor })
    }
    
    pub fn process_image(&self, img: &RgbImage) -> Result<String, Box<dyn Error>> {
        let (width, height) = img.dimensions();
        debug!("Processing image for OCR: {}x{}", width, height);
        
        // Always save debug image before OCR processing
        self.save_debug_image(img, "before_ocr")?;
        
        // Process with OCR - NO PREPROCESSING, EXACTLY LIKE CLI
        let text = self.processor.extract_text(img)?;
        debug!("OCR completed, extracted {} characters", text.len());
        
        Ok(text)
    }
    
    /// Save debug image
    fn save_debug_image(&self, img: &RgbImage, stage: &str) -> Result<(), Box<dyn Error>> {
        use std::time::SystemTime;
        
        let debug_dir = "debug_images";
        std::fs::create_dir_all(debug_dir)?;
        
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_millis();
        
        let filename = format!("{}/{}_{}x{}_{}.png", 
                             debug_dir, stage, img.width(), img.height(), timestamp);
        
        img.save(&filename)?;
        debug!("Saved OCR debug image: {}", filename);
        
        Ok(())
    }
}