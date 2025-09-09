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
}

impl Default for LAParams {
    fn default() -> Self {
        Self {
            char_margin: 2.0,
            word_margin: 0.10,
            line_overlap: 0.5,
            line_margin: 0.5,
            boxes_flow: 0.5,
            detect_vertical: false,
            all_texts: false,
        }
    }
}
