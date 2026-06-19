use crate::MediaBox;
use euclid::{Point2D, Transform2D};

use crate::Space;

#[derive(Debug, Clone)]
pub struct TextVisibilityPolicy {
    pub keep_invisible_if_only_text_layer: bool,
    pub drop_generated_non_space: bool,
    pub drop_sentinel_unicode: bool,
    pub enforce_cropbox: bool,
    pub drop_near_white_text: bool,
    pub keep_clip_visible_modes: bool,
    pub invisible_text_ratio_to_keep: f64,
}

impl Default for TextVisibilityPolicy {
    fn default() -> Self {
        Self {
            keep_invisible_if_only_text_layer: true,
            drop_generated_non_space: true,
            drop_sentinel_unicode: true,
            enforce_cropbox: std::env::var("PDF_EXTRACT_ENFORCE_VISIBILITY").is_ok(),
            drop_near_white_text: std::env::var("PDF_EXTRACT_DROP_NEAR_WHITE").is_ok(),
            keep_clip_visible_modes: true,
            invisible_text_ratio_to_keep: std::env::var("PDF_EXTRACT_INVISIBLE_RATIO_TO_KEEP")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.30),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityDecision {
    Keep,
    Drop { reason: VisibilityDropReason },
}

impl VisibilityDecision {
    pub fn keep(self) -> bool {
        matches!(self, VisibilityDecision::Keep)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VisibilityDropReason {
    RenderModeInvisibleRedundant,
    RenderModeClipOnly,
    GeneratedNonSpace,
    SentinelUnicode,
    OutsideCropBox,
    TransparentOrNearWhite,
    ZeroAreaBox,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PageTextLayerStats {
    pub visible_useful_chars: usize,
    pub invisible_useful_chars: usize,
    pub generated_non_space_chars: usize,
    pub sentinel_chars: usize,
}

impl PageTextLayerStats {
    pub fn should_keep_invisible(self, policy: &TextVisibilityPolicy) -> bool {
        if !policy.keep_invisible_if_only_text_layer {
            return false;
        }
        if self.visible_useful_chars == 0 && self.invisible_useful_chars > 0 {
            return true;
        }
        let total = self.visible_useful_chars + self.invisible_useful_chars;
        total > 0
            && (self.invisible_useful_chars as f64 / total as f64)
                >= policy.invisible_text_ratio_to_keep
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TextVisibilityInput<'a> {
    pub transform: &'a Transform2D<f64, Space, Space>,
    pub font_size: f64,
    pub color: (f64, f64, f64),
    pub media_box: &'a MediaBox,
    pub rendering_mode: i32,
    pub generated: Option<bool>,
    pub unicode: Option<char>,
    pub page_stats: Option<PageTextLayerStats>,
}

impl TextVisibilityPolicy {
    pub fn decide(&self, input: TextVisibilityInput<'_>) -> VisibilityDecision {
        if matches!(input.unicode, Some('\u{fffe}' | '\u{ffff}')) {
            return VisibilityDecision::Drop {
                reason: VisibilityDropReason::SentinelUnicode,
            };
        }

        if self.drop_generated_non_space
            && input.generated == Some(true)
            && !matches!(input.unicode, Some(' ' | '\n' | '\r' | '\t'))
        {
            return VisibilityDecision::Drop {
                reason: VisibilityDropReason::GeneratedNonSpace,
            };
        }

        match input.rendering_mode {
            0 | 1 | 2 => {}
            3 => {
                let keep_invisible = input
                    .page_stats
                    .map(|stats| stats.should_keep_invisible(self))
                    .unwrap_or(false);
                if !keep_invisible {
                    return VisibilityDecision::Drop {
                        reason: VisibilityDropReason::RenderModeInvisibleRedundant,
                    };
                }
            }
            4 | 5 | 6 if self.keep_clip_visible_modes => {}
            7 => {
                return VisibilityDecision::Drop {
                    reason: VisibilityDropReason::RenderModeClipOnly,
                }
            }
            _ => {
                return VisibilityDecision::Drop {
                    reason: VisibilityDropReason::RenderModeClipOnly,
                }
            }
        }

        if input.font_size <= 0.0 || input.font_size.is_nan() {
            return VisibilityDecision::Drop {
                reason: VisibilityDropReason::ZeroAreaBox,
            };
        }

        if self.enforce_cropbox {
            let point = input.transform.transform_point(Point2D::new(0.0, 0.0));
            if point.x < input.media_box.llx
                || point.x > input.media_box.urx
                || point.y < input.media_box.lly
                || point.y > input.media_box.ury
            {
                return VisibilityDecision::Drop {
                    reason: VisibilityDropReason::OutsideCropBox,
                };
            }
        }

        if self.drop_near_white_text {
            const MIN_COLOR_DIFF: f64 = 0.1;
            if input.color.0 > 1.0 - MIN_COLOR_DIFF
                && input.color.1 > 1.0 - MIN_COLOR_DIFF
                && input.color.2 > 1.0 - MIN_COLOR_DIFF
            {
                return VisibilityDecision::Drop {
                    reason: VisibilityDropReason::TransparentOrNearWhite,
                };
            }
        }

        VisibilityDecision::Keep
    }
}
