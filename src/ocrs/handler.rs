use image::RgbImage;
use log::{debug, info};
use std::error::Error;

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

        let detection_model_path = config
            .detection_model
            .as_deref()
            .ok_or("Detection model path is required")?;
        let recognition_model_path = config
            .recognition_model
            .as_deref()
            .ok_or("Recognition model path is required")?;

        debug!(
            "Model paths - detection: {}, recognition: {}",
            detection_model_path, recognition_model_path
        );

        let processor = OcrProcessor::new(detection_model_path, recognition_model_path)?;
        info!("OCR handler created successfully");
        Ok(Self { processor })
    }

    pub fn process_image(&self, img: &RgbImage) -> Result<String, Box<dyn Error>> {
        let (width, height) = img.dimensions();
        debug!("Processing image for OCR: {}x{}", width, height);

        // Apply Goldilocks sizing as per architect's advice
        let mut processed_img = img.clone();
        let (mut w, mut h) = processed_img.dimensions();
        let max_side = w.max(h);

        if max_side > 1600 {
            let scale = 1600.0 / max_side as f32;
            w = (w as f32 * scale) as u32;
            h = (h as f32 * scale) as u32;
            processed_img = image::imageops::resize(
                &processed_img,
                w,
                h,
                image::imageops::FilterType::Lanczos3,
            );
            debug!("Downscaled from {}x{} to {}x{}", width, height, w, h);
        }

        if w < 240 || h < 240 {
            // Up-scale tiny stamps
            let scale = 240.0 / w.min(h) as f32;
            w = (w as f32 * scale) as u32;
            h = (h as f32 * scale) as u32;
            processed_img = image::imageops::resize(
                &processed_img,
                w,
                h,
                image::imageops::FilterType::CatmullRom,
            );
            debug!("Upscaled image from {}x{} to {}x{}", width, height, w, h);
        }

        // Process with OCR
        let text = self.processor.extract_text(&processed_img)?;
        debug!("OCR completed, extracted {} characters", text.len());

        Ok(text)
    }
}
