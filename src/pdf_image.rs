use image::{ImageFormat, RgbImage};
use log::{debug, error, warn};
use std::error::Error;
use lopdf::Object;

use crate::{PdfImage, Transform, apply_transform_to_image, decode_stream};

// PNG-Up predictor expansion function
fn png_up_predictor(bytes: &[u8], row: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());    // row + 1 per line
    for chunk in bytes.chunks(row + 1) {
        let (tag, line) = chunk.split_first().unwrap();
        match tag {
            0 => out.extend_from_slice(line),                 // None
            2 | 1 => {                                        // Up  (=2) or Sub (=1)
                let mut i = 0;
                while i < line.len() {
                    let prior = if *tag == 2 {
                        out[out.len() - row + i]              // prior row, same col
                    } else {
                        if i < 3 { 0 } else { out[out.len() - 3] } // Sub predictor
                    };
                    out.push(line[i].wrapping_add(prior));
                    i += 1;
                }
            }
            _ => panic!("Predictor {} not supported", tag),
        }
    }
    out
}

impl<'a> PdfImage<'a> {
    /// Convert PDF image to RGB using the image crate for robust format handling
    pub fn to_rgb_image(&self, transform: Option<&Transform>) -> Result<RgbImage, Box<dyn Error>> {
        debug!("Converting PDF image - {}x{}, color_space={:?}, filters={:?}", 
               self.width, self.height, self.color_space, self.filters);
        
        // 1. Get decoded bitmap bytes (handle predictors, filters, etc.)
        let mut bytes = decode_stream(self)?;
        debug!("Decoded {} bytes from PDF stream", bytes.len());
        
        // Handle PNG predictor if present
        if let Some(dp) = self.origin_dict.get(b"DecodeParms").ok().and_then(|o| o.as_dict().ok()) {
            let predictor_val = dp.get(b"Predictor").ok().and_then(|o| o.as_i64().ok());
            
            if matches!(predictor_val, Some(12)) {
                let row = (self.width as usize) * self.components.unwrap_or(3);   // 3 for RGB, 1 for Gray
                debug!("Applying PNG-Up predictor (Predictor=12) with row size {}", row);
                bytes = png_up_predictor(&bytes, row);
            }
        }
        
        
        // 2. Pick the container format for the image crate
        let format = match self.filters.as_ref().and_then(|f| f.first()).map(|s| s.as_str()) {
            Some("DCTDecode") => ImageFormat::Jpeg,
            Some("JPXDecode") => {
                // JPEG2000 not supported by image crate version we're using
                // Fall back to raw construction
                return self.construct_raw_image(&bytes, transform);
            }
            Some("CCITTFaxDecode") => {
                // CCITTFaxDecode is not a TIFF container, it's raw CCITT-compressed data
                // This is common in scanned documents but not directly supported
                debug!("CCITTFaxDecode format detected - falling back to raw construction");
                return self.construct_raw_image(&bytes, transform)
                    .or_else(|_| {
                        // If raw construction fails, create a placeholder image
                        warn!("Could not decode CCITTFaxDecode image, creating placeholder");
                        Ok(self.create_placeholder_image(transform))
                    });
            }
            _ => {
                // For raw data, we'll need to construct the image manually
                return self.construct_raw_image(&bytes, transform);
            }
        };
        
        debug!("Using image format: {:?}", format);
        
        // 3. Let image crate handle ALL color space conversions
        let dyn_img = image::load_from_memory_with_format(&bytes, format)?;
        let mut rgb_img = dyn_img.to_rgb8();  // This guarantees sRGB as per architect's advice
        
        
        // 4. Handle /Decode array flips - as per architect's specific fix
        let needs_vflip = matches!(
            self.origin_dict.get(b"Decode").ok(),
            Some(Object::Array(arr)) if arr.len() >= 2 &&
                matches!(arr.get(0).and_then(|o| o.as_i64().ok()), Some(1)) &&
                matches!(arr.get(1).and_then(|o| o.as_i64().ok()), Some(0))
        );
        
        if needs_vflip {
            debug!("/Decode [1 0] detected - applying vertical flip for OCR");
            rgb_img = image::imageops::flip_vertical(&rgb_img);
        }
        
        // 5. Apply transformation if provided
        if let Some(t) = transform {
            debug!("Applying transformation matrix");
            rgb_img = apply_transform_to_image(rgb_img, t);
        }
        
        
        Ok(rgb_img)
    }
    
    /// Construct raw image when no standard format is detected
    fn construct_raw_image(&self, bytes: &[u8], transform: Option<&Transform>) -> Result<RgbImage, Box<dyn Error>> {
        debug!("Constructing raw image from decoded bytes");
        
        let components = self.components.unwrap_or_else(|| {
            match self.color_space.as_deref() {
                Some("DeviceGray") => 1,
                Some("DeviceRGB") => 3,
                Some("DeviceCMYK") => 4,
                _ => 3, // Default to RGB
            }
        });
        
        let bits_per_component = self.bits_per_component.unwrap_or(8) as usize;
        let bytes_per_pixel = (bits_per_component * components + 7) / 8;
        let expected_size = self.width as usize * self.height as usize * bytes_per_pixel;
        
        if bytes.len() < expected_size {
            warn!("Image data size mismatch: expected {}, got {}", expected_size, bytes.len());
        }
        
        let mut img = RgbImage::new(self.width as u32, self.height as u32);
        
        match (self.color_space.as_deref(), components) {
            (Some("DeviceGray") | None, 1) => {
                debug!("Converting grayscale image");
                for y in 0..self.height as u32 {
                    for x in 0..self.width as u32 {
                        let idx = (y * self.width as u32 + x) as usize;
                        if idx < bytes.len() {
                            let gray = bytes[idx];
                            img.put_pixel(x, y, image::Rgb([gray, gray, gray]));
                        }
                    }
                }
            }
            (Some("DeviceRGB"), 3) => {
                debug!("Converting RGB image");
                for y in 0..self.height as u32 {
                    for x in 0..self.width as u32 {
                        let idx = ((y * self.width as u32 + x) * 3) as usize;
                        if idx + 2 < bytes.len() {
                            img.put_pixel(x, y, image::Rgb([bytes[idx], bytes[idx + 1], bytes[idx + 2]]));
                        }
                    }
                }
            }
            (Some("DeviceCMYK"), 4) => {
                debug!("Converting CMYK image");
                for y in 0..self.height as u32 {
                    for x in 0..self.width as u32 {
                        let idx = ((y * self.width as u32 + x) * 4) as usize;
                        if idx + 3 < bytes.len() {
                            // Simple CMYK to RGB conversion
                            let c = bytes[idx] as f32 / 255.0;
                            let m = bytes[idx + 1] as f32 / 255.0;
                            let y_val = bytes[idx + 2] as f32 / 255.0;
                            let k = bytes[idx + 3] as f32 / 255.0;
                            
                            let r = ((1.0 - c) * (1.0 - k) * 255.0) as u8;
                            let g = ((1.0 - m) * (1.0 - k) * 255.0) as u8;
                            let b = ((1.0 - y_val) * (1.0 - k) * 255.0) as u8;
                            
                            img.put_pixel(x, y, image::Rgb([r, g, b]));
                        }
                    }
                }
            }
            _ => {
                error!("Unsupported color space: {:?} with {} components", self.color_space, components);
                return Err("Unsupported color space".into());
            }
        }
        
        // Apply transformation if provided
        if let Some(t) = transform {
            debug!("Applying transformation to raw image");
            img = apply_transform_to_image(img, t);
        }
        
        
        Ok(img)
    }
    
    /// Create a placeholder image for unsupported formats
    fn create_placeholder_image(&self, transform: Option<&Transform>) -> RgbImage {
        debug!("Creating placeholder image for unsupported format");
        
        // Create a simple gray placeholder image
        let mut img = RgbImage::new(self.width as u32, self.height as u32);
        
        // Fill with light gray
        for pixel in img.pixels_mut() {
            *pixel = image::Rgb([200, 200, 200]);
        }
        
        // Apply transformation if provided
        if let Some(t) = transform {
            debug!("Applying transformation to placeholder image");
            return apply_transform_to_image(img, t);
        }
        
        img
    }
}