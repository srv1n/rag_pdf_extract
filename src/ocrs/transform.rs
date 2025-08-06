use image::{imageops, RgbImage};

// Import Transform type from the parent module
use crate::Transform;
use log::debug;

/// Apply transformation matrix using mathematical classification (improved approach)
pub fn apply_transform_to_image(img: RgbImage, transform: &Transform) -> RgbImage {
    let det = transform.m11 * transform.m22 - transform.m12 * transform.m21;
    
    // Detect if there's a Y-flip in the transformation (m22 < 0 typically indicates Y-flip)
    let has_y_flip = transform.m22 < 0.0;
    
    // If there's a Y-flip from the PDF viewer coordinates, we need to compensate
    // by applying our own Y-flip to get the image right-side up
    let compensate_y_flip = has_y_flip;
    
    // Calculate rotation angle from the transformation matrix
    // We need to account for the Y-flip when calculating the angle
    let (m11, m12) = if has_y_flip {
        (transform.m11, -transform.m12)  // Adjust for Y-flip
    } else {
        (transform.m11, transform.m12)
    };
    
    let angle_rad = m12.atan2(m11);
    let angle_deg_raw = angle_rad.to_degrees();
    let mut deg = angle_deg_raw.rem_euclid(360.0);
    deg = (deg / 90.0).round() * 90.0; // snap to multiples of 90

    // Check if additional mirroring is needed (beyond Y-flip compensation)
    // As per architect's advice: ignore the fixed viewer flip
    let needs_x_mirror = (det.signum() != -1.0) && has_y_flip;
    

    // Step 3: Apply rotation first
    let mut out = match deg as i32 {
        0   => img,
        90  => imageops::rotate90(&img),
        180 => imageops::rotate180(&img),
        270 => imageops::rotate270(&img),
        _   => img,
    };
    
    // Step 4: Apply Y-flip compensation if needed (to counteract PDF's Y-flip)
    if compensate_y_flip {
        out = imageops::flip_vertical(&out);
    }
    
    // Step 5: Apply X-mirror if needed
    if needs_x_mirror {
        out = imageops::flip_horizontal(&out);
    }

    // OPTIONAL FIX: Normalize scale to avoid distorted debug images
    let scale_x = (transform.m11.powi(2) + transform.m12.powi(2)).sqrt();
    let scale_y = (transform.m21.powi(2) + transform.m22.powi(2)).sqrt();
    if (scale_x - 1.0).abs() > 0.05 || (scale_y - 1.0).abs() > 0.05 {
        let new_width = (out.width() as f64 * scale_x).round() as u32;
        let new_height = (out.height() as f64 * scale_y).round() as u32;
        
        // Only resize if dimensions are reasonable (avoid extreme scaling)
        if new_width > 0 && new_width < 10000 && new_height > 0 && new_height < 10000 {
            out = imageops::resize(
                &out,
                new_width,
                new_height,
                imageops::Lanczos3,
            );
        }
    }
    out
}