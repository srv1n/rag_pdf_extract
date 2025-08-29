use super::stats::DocumentStats;
use crate::{TextSegment, NUMBERED_HEADING};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum TextLevel {
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Body,
    SubBody,
}

pub fn is_heading(segment: &TextSegment, doc_stats: &DocumentStats) -> TextLevel {
    // let fg_color = segment.fill_color.unwrap_or((0, 0, 0)); // default black
    // let bg_color = segment.stroke_color.unwrap_or((255, 255, 255)); // default white

    // // Calculate contrast ratio
    // let cr = contrast_ratio(fg_color, bg_color);

    // // Filter low-contrast text (adjust threshold as needed)
    // if cr < 1.5 {
    //     // Catches near-invisible text but allows light gray
    //     return TextLevel::Body;
    // }

    // Additional heading checks

    // if segment.content first char is lower case then it is not a heading

    let is_short = segment.content.split_whitespace().count() < 10;
    // let not_short = segment.content.len() > 4;
    let starts_with_alphanum = segment
        .content
        .trim()
        .chars()
        .next()
        .map_or(false, |c| c.is_alphanumeric());

    let is_valid_short_content = |content: &str| {
        let trimmed = content.trim();
        if trimmed.len() >= 4 {
            true
        } else {
            // Check if it's a number or Roman numeral
            trimmed.parse::<u32>().is_ok() || is_roman_numeral(trimmed)
        }
    };

    // let is_short = segment.content.split_whitespace().count() < 10 && is_valid_short_content(&segment.content);

    fn is_roman_numeral(s: &str) -> bool {
        let valid_chars = ['I', 'V', 'X'];
        !s.is_empty() && s.chars().all(|c| valid_chars.contains(&c))
    }

    let is_numbered = NUMBERED_HEADING.is_match(&segment.content);

    let is_potential_heading = is_short
        && (starts_with_alphanum && is_valid_short_content(&segment.content) || is_numbered);

    // Check if the segment matches the body font and size
    if segment.font_name == doc_stats.body_font
        && (segment.transformed_font_size - doc_stats.body_transformed_size).abs() < 0.1
    {
        // If it matches body font and size, check if it's bold
        if segment.is_bold && is_potential_heading {
            return TextLevel::H6;
        } else {
            return TextLevel::Body;
        }
    }
    // If it's smaller than the body text, consider it sub-body
    else if segment.transformed_font_size < doc_stats.body_transformed_size {
        return TextLevel::SubBody;
    } else if (segment.transformed_font_size - doc_stats.body_transformed_size).abs() < 0.1 {
        if segment.is_bold {
            if !segment.content.chars().next().unwrap().is_lowercase() && is_potential_heading {
                return TextLevel::H6;
            } else {
                return TextLevel::Body;
            }
        } else {
            return TextLevel::Body;
        }
    }
    // Find the closest heading level
    else if is_potential_heading {
        let closest_threshold = match doc_stats.transformed_thresholds.iter().min_by(|&&a, &&b| {
            (a - segment.transformed_font_size)
                .abs()
                .partial_cmp(&(b - segment.transformed_font_size).abs())
                .unwrap()
        }) {
            Some(threshold) => threshold,
            None => return TextLevel::Body,
        };

        let index = doc_stats
            .transformed_thresholds
            .iter()
            .position(|&r| r == *closest_threshold)
            .unwrap();

        return match index {
            0 => TextLevel::H1,
            1 => TextLevel::H2,
            2 => TextLevel::H3,
            3 => TextLevel::H4,
            4 => TextLevel::H5,
            _ => TextLevel::H6,
        };
    } else {
        // If we can't determine a specific level, return Body as fallback
        return TextLevel::Body;
    }

    // if is_numbered {
    //     return TextLevel::H1;
    // } else {
    //     return TextLevel::Body;
    // }
}
