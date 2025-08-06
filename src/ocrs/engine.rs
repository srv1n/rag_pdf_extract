use anyhow::{Context, Result};
use image::RgbImage;
use ocrs::{DecodeMethod, DimOrder, ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;
use rten_tensor::prelude::*;
use rten_tensor::NdTensor;
use log::{debug, trace};

pub struct OcrProcessor {
    engine: OcrEngine,
}

impl OcrProcessor {
    /// Create OCR processor - EXACT CLI replica
    pub fn new(detection_model_path: &str, recognition_model_path: &str) -> Result<Self> {
        // Load models EXACTLY like CLI does
        let detection_model = Model::load_file(detection_model_path)
            .with_context(|| format!("Failed to load text detection model from {}", detection_model_path))?;
        
        let recognition_model = Model::load_file(recognition_model_path)
            .with_context(|| format!("Failed to load text recognition model from {}", recognition_model_path))?;

        // Initialize OCR engine EXACTLY like CLI
        #[allow(clippy::needless_update)]
        let engine = OcrEngine::new(OcrEngineParams {
            detection_model: Some(detection_model),
            recognition_model: Some(recognition_model),
            debug: false,
            alphabet: None,
            decode_method: DecodeMethod::Greedy,
            allowed_chars: None,
            ..Default::default()
        })?;

        Ok(Self { engine })
    }

    /// Extract text - EXACT CLI replica
    pub fn extract_text(&self, input_image: &RgbImage) -> Result<String> {
        // EXACT COPY FROM CLI main.rs - Read image into HWC tensor
        let image = input_image;  // CLI does image.into_rgb8() but we already have RgbImage
        let (width, height) = image.dimensions();
        let in_chans = 3;
        let color_img: NdTensor<u8, 3> = NdTensor::from_data(
            [height as usize, width as usize, in_chans],
            image.clone().into_vec(),
        );

        // EXACT COPY FROM CLI main.rs - Preprocess image for use with OCR engine
        let color_img_source = ImageSource::from_tensor(color_img.view(), DimOrder::Hwc)?;
        let ocr_input = self.engine.prepare_input(color_img_source)?;

        // EXACT COPY FROM CLI main.rs
        let word_rects = self.engine.detect_words(&ocr_input)?;
        let line_rects = self.engine.find_text_lines(&ocr_input, &word_rects);
        let line_texts = self.engine.recognize_text(&ocr_input, &line_rects)?;

        // Format using the output module format_text_output
        let content = format_text_output(&line_texts);
        
        Ok(content)
    }
}

/// Format OCR outputs as plain text - EXACT COPY from CLI
fn format_text_output(text_lines: &[Option<ocrs::TextLine>]) -> String {
    let lines: Vec<String> = text_lines
        .iter()
        .flatten()
        .map(|line| line.to_string())
        .collect();
    lines.join("\n")
}
