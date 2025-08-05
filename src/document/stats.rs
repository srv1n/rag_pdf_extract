use std::collections::HashMap;
use ordered_float::OrderedFloat;
use crate::TextSegment;
use log::debug;

#[derive(Debug)]
pub struct FontStats {
    pub sizes: HashMap<OrderedFloat<f64>, usize>,
    pub transformed_sizes: HashMap<OrderedFloat<f64>, usize>,
    pub size_to_transformed: HashMap<OrderedFloat<f64>, f64>,
    pub total_chars: usize,
}

#[derive(Debug)]
pub struct DocumentStats {
    pub font_stats: HashMap<String, FontStats>,
    pub body_font: String,
    pub body_size: f64,
    pub body_transformed_size: f64,
    pub transformed_thresholds: Vec<f64>,
    pub font_heading_thresholds: HashMap<String, Vec<f64>>,
}

pub fn calculate_document_stats(lines: &[TextSegment]) -> DocumentStats {
    let mut font_stats: HashMap<String, FontStats> = HashMap::new();

    // Collect statistics

    // Collect statistics
    for segment in lines {
        let font_stat = font_stats
            .entry(segment.font_name.clone())
            .or_insert_with(|| FontStats {
                sizes: HashMap::new(),
                transformed_sizes: HashMap::new(),
                size_to_transformed: HashMap::new(),
                total_chars: 0,
            });

        *font_stat
            .sizes
            .entry(OrderedFloat(segment.font_size))
            .or_insert(0) += segment.content.len();
        *font_stat
            .transformed_sizes
            .entry(OrderedFloat(segment.transformed_font_size))
            .or_insert(0) += segment.content.len();
    }
    let (body_font, body_size, body_transformed_size) = font_stats
        .iter()
        .flat_map(|(font_name, stats)| {
            stats
                .transformed_sizes
                .iter()
                .map(move |(&size, &count)| (font_name, size, count))
        })
        .max_by_key(|&(_, _, count)| count)
        .map(|(font, size, _)| (font.clone(), size, size))
        .unwrap_or((
            "".to_string(),
            ordered_float::OrderedFloat(0.0),
            ordered_float::OrderedFloat(0.0),
        ));

    // Find the body size (most common size across all fonts)

    debug!("body_transformed_size: {}", body_transformed_size);
    // Calculate six threshold levels based on transformed sizes
    let all_transformed_sizes: Vec<f64> = font_stats
        .values()
        .flat_map(|stats| stats.transformed_sizes.keys().cloned())
        .map(|ordered_float| ordered_float.into_inner())
        .collect();
    let transformed_thresholds =
        calculate_heading_thresholds(&all_transformed_sizes, *body_transformed_size);
    debug!("transformed_thresholds: {:?}", transformed_thresholds);

    // Calculate font-specific thresholds
    let mut font_heading_thresholds = HashMap::new();
    for (font_name, stats) in font_stats.iter() {
        let font_thresholds: Vec<f64> = transformed_thresholds
            .iter()
            .filter_map(|&transformed_size| {
                stats
                    .size_to_transformed
                    .iter()
                    .find(|&(_, &t)| (t - transformed_size).abs() < 0.01)
                    .map(|(&original_size, _)| original_size.into_inner())
            })
            .collect();
        font_heading_thresholds.insert(font_name.clone(), font_thresholds);
    }

    DocumentStats {
        font_stats,
        body_font,
        body_size: *body_size,
        body_transformed_size: *body_transformed_size,
        transformed_thresholds,
        font_heading_thresholds,
    }
}

pub fn calculate_mode(numbers: &[f64]) -> f64 {
    let mut counts = HashMap::new();
    for &num in numbers {
        *counts.entry((num * 100.0).round() as i64).or_insert(0) += 1;
    }
    let mode = counts
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .unwrap()
        .0;
    mode as f64 / 100.0
}

pub fn calculate_heading_thresholds(sizes: &[f64], body_size: f64) -> Vec<f64> {
    let mut thresholds: Vec<f64> = sizes
        .iter()
        .filter(|&&size| size > body_size * 1.1)
        .cloned()
        .collect();

    thresholds.sort_by(|a, b| b.partial_cmp(a).unwrap()); // Sort in descending order
    thresholds.dedup(); // Remove duplicates

    if thresholds.len() <= 6 {
        thresholds
    } else {
        // Group into 6 levels
        let step = thresholds.len() / 6;
        let mut result: Vec<f64> = (0..6).map(|i| thresholds[i * step]).collect();
        result.dedup(); // Remove any potential duplicates after grouping
        result
    }
}