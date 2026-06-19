use image::RgbImage;
use std::error::Error;

#[derive(Debug)]
pub struct OcrConfig {
    pub detection_model: Option<String>,
    pub recognition_model: Option<String>,
}

pub struct OcrHandler;

impl OcrHandler {
    pub fn new(_config: &OcrConfig) -> Result<Self, Box<dyn Error>> {
        Err("OCR support is disabled; rebuild with feature `ocr` or `ocr-ocrs`".into())
    }

    pub fn process_image(&self, _img: &RgbImage) -> Result<String, Box<dyn Error>> {
        Err("OCR support is disabled; rebuild with feature `ocr` or `ocr-ocrs`".into())
    }
}
