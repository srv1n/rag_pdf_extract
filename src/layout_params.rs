#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutFallbackPolicy {
    Disabled,
    OnTextLoss,
    OnSuspiciousVolume,
}

/// Token counting used while building structured chunks.
///
/// Approximate counting is the product default. Exact BPE counting remains
/// available for callers that need precise token accounting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TokenCountMode {
    #[default]
    Approximate,
    Exact,
}

#[derive(Clone, Debug)]
pub struct LAParams {
    /// Max horizontal gap (× char width) to group into same word/line
    pub char_margin: f32,
    /// Threshold to inject a space when inter‑glyph gap exceeds (× char width)
    pub word_margin: f32,
    /// Min vertical overlap (fraction of height) for same line
    pub line_overlap: f32,
    /// Max vertical gap (× line height) to group lines into a text box
    pub line_margin: f32,
    /// Reading‑order bias: −1 top‑down, +1 left‑to‑right; 0.5 ~ PDFMiner default
    pub boxes_flow: f32,
    /// Detect and group vertical/rotated text
    pub detect_vertical: bool,
    /// Include text inside figures/forms (LTFigure) like pdfminer all_texts
    pub all_texts: bool,
    /// Product fallback policy for layout extraction. Eval should set this to Disabled.
    pub layout_fallback_policy: LayoutFallbackPolicy,
    /// Chunk-budget counting policy. Approximate is intentionally the default.
    pub token_count_mode: TokenCountMode,
}

impl Default for LAParams {
    fn default() -> Self {
        Self::product_layout()
    }
}

impl LAParams {
    pub fn product_layout() -> Self {
        Self {
            char_margin: 2.0,
            word_margin: 0.10,
            line_overlap: 0.5,
            line_margin: 0.5,
            boxes_flow: 0.5,
            detect_vertical: false,
            all_texts: false,
            layout_fallback_policy: LayoutFallbackPolicy::OnSuspiciousVolume,
            token_count_mode: TokenCountMode::Approximate,
        }
    }

    pub fn diagnostic_layout() -> Self {
        let mut params = Self::product_layout();
        params.layout_fallback_policy = LayoutFallbackPolicy::Disabled;
        params
    }

    /// Opt into exact BPE accounting for structured chunks.
    pub fn exact_tokens(mut self) -> Self {
        self.token_count_mode = TokenCountMode::Exact;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{LAParams, TokenCountMode};

    #[test]
    fn approximate_tokens_are_default_and_exact_is_opt_in() {
        assert_eq!(
            LAParams::default().token_count_mode,
            TokenCountMode::Approximate
        );
        assert_eq!(
            LAParams::default().exact_tokens().token_count_mode,
            TokenCountMode::Exact
        );
    }
}
