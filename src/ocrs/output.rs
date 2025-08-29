use ocrs::TextLine;

/// Format text output exactly as the CLI does - EXACT COPY from CLI
pub fn format_text_output(line_texts: &[Option<TextLine>]) -> String {
    line_texts
        .iter()
        .filter_map(|opt_line| opt_line.as_ref())
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}
