use image::{ImageFormat, RgbImage};
use log::{debug, error, warn};
use std::error::Error;

use crate::{PdfImage, Transform, apply_transform_to_image, decode_stream};

impl<'a> PdfImage<'a> {
    /// Convert PDF image to RGB using the image crate for robust format handling
    pub fn to_rgb_image(&self, transform: Option<&Transform>) -> Result<RgbImage, Box<dyn Error>> {
        debug!("Converting PDF image - {}x{}, color_space={:?}, filters={:?}", 
               self.width, self.height, self.color_space, self.filters);
        
        // 1. Get decoded bitmap bytes (handle predictors, filters, etc.)
        let bytes = decode_stream(self)?;
        debug!("Decoded {} bytes from PDF stream", bytes.len());
        
        
        // 2. Pick the container format for the image crate
        let format = match self.filters.as_ref().and_then(|f| f.first()).map(|s| s.as_str()) {
            Some("DCTDecode") => ImageFormat::Jpeg,
            Some("JPXDecode") => {
                // JPEG2000 not supported by image crate version we're using
                // Fall back to raw construction
                return self.construct_raw_image(&bytes, transform);
            }
            Some("CCITTFaxDecode") => ImageFormat::Tiff,
            _ => {
                // For raw data, we'll need to construct the image manually
                return self.construct_raw_image(&bytes, transform);
            }
        };
        
        debug!("Using image format: {:?}", format);
        
        // 3. Let image crate handle ALL color space conversions
        let dyn_img = image::load_from_memory_with_format(&bytes, format)?;
        let mut rgb_img = dyn_img.to_rgb8();
        
        
        // 4. Handle /Decode array flips
        let decode_mirror = self.origin_dict
            .get(b"Decode")
            .ok()
            .and_then(|o| o.as_array().ok())
            .map(|a| {
                if a.len() >= 2 {
                    matches!(
                        (a.get(0).and_then(|o| o.as_i64().ok()), 
                         a.get(1).and_then(|o| o.as_i64().ok())),
                        (Some(1), Some(0))
                    )
                } else {
                    false
                }
            })
            .unwrap_or(false);
        
        if decode_mirror {
            debug!("/Decode [1 0] detected but NOT applying flip to avoid double-mirror");
            // DISABLED: This was causing double-flip with CTM-based mirror detection
            // rgb_img = image::imageops::flip_vertical(&rgb_img);
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
}