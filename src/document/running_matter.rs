//! Remove document running matter before layout merges and chunking.
use super::header_footer::HeaderFooterDetector;
use crate::TextSegment;
use std::collections::{HashMap, HashSet};

/// Returns the page-flow segments and, separately, the footnotes. The caller
/// appends footnotes after the last page so a sentence that crosses a page
/// break is not cut by the footnotes at the foot of that page.
pub(crate) fn remove_running_matter(
    segments: Vec<TextSegment>,
    page_heights: &HashMap<u32, f64>,
) -> (Vec<TextSegment>, Vec<TextSegment>) {
    if std::env::var("PDF_EXTRACT_SKIP_HEADER_FOOTER").is_ok() {
        return (segments, Vec::new());
    }
    let mut segments = segments;
    // Footnote marks at the page foot look like page numbers, so find footnotes first.
    let footnotes = footnote_blocks(&segments);
    let mut remove = running_lines(&segments, page_heights, &footnotes);
    remove_margin_rails(&mut segments, &mut remove);
    let mut kept = Vec::with_capacity(segments.len());
    let mut moved = Vec::new();
    for (index, segment) in segments.into_iter().enumerate() {
        if remove.contains(&index) {
            continue;
        }
        match footnotes.get(&index) {
            Some(Footnote::Body) => moved.push(segment),
            Some(Footnote::Reference) => {}
            None => kept.push(segment),
        }
    }
    (kept, moved)
}

/// Indices of whole visual lines that repeat as running heads or feet.
fn running_lines(
    segments: &[TextSegment],
    page_heights: &HashMap<u32, f64>,
    footnotes: &HashMap<usize, Footnote>,
) -> HashSet<usize> {
    let mut detector = HeaderFooterDetector::new(page_heights.len());
    // A reporter line can have many font runs. Detect its whole visual line,
    // before continuation/title merges attach it to body text.
    let mut edge_lines: Vec<(u32, f64, f64, Vec<usize>)> = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let Some(height) = page_heights.get(&segment.page_num) else {
            continue;
        };
        if !height.is_finite() || *height <= 0.0 || !segment.y.is_finite() {
            continue;
        }
        if footnotes.contains_key(&index) {
            continue;
        }
        if segment.y > height * EDGE_BAND && segment.y < height * (1.0 - EDGE_BAND) {
            continue;
        }
        if let Some(line) = edge_lines
            .iter_mut()
            .rev()
            .take_while(|line| line.0 == segment.page_num)
            .find(|line| {
                line.0 == segment.page_num && (line.1 - segment.y).abs() <= segment.font_size * 0.3
            })
        {
            line.3.push(index);
        } else {
            edge_lines.push((segment.page_num, segment.y, *height, vec![index]));
        }
    }
    let mut texts = Vec::with_capacity(edge_lines.len());
    let mut recurring: HashMap<String, Vec<(u32, f64, f64)>> = HashMap::new();
    for (page, y, height, indices) in &mut edge_lines {
        indices.sort_by(|a, b| segments[*a].x.total_cmp(&segments[*b].x));
        let text = indices
            .iter()
            .map(|i| segments[*i].content.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let font = indices
            .first()
            .map(|i| segments[*i].font_size)
            .unwrap_or(0.0);
        // The fixed-band detector keeps its own outer-band rules for short documents.
        if *y <= *height * 0.1 || *y >= *height * 0.9 {
            detector.add_occurrence(&text, *page, *y, font, *height);
        }
        let normalized = HeaderFooterDetector::normalize_text(&text);
        recurring
            .entry(running_key(&text, *y < *height / 2.0))
            .or_default()
            .push((*page, *y, font));
        texts.push(normalized);
    }
    // Alternating recto/verso heads and per-section heads repeat on a share of the
    // pages, not most of them. The same short line at the same height on three
    // pages is running matter.
    let mut recurring_lines = HashSet::new();
    for (text, places) in recurring {
        let pages = places.iter().map(|place| place.0).collect::<HashSet<_>>();
        if pages.len() < 3 || text.split_whitespace().count() > 24 {
            continue;
        }
        let mut heights = places.iter().map(|place| place.1).collect::<Vec<_>>();
        heights.sort_by(f64::total_cmp);
        let median = heights[heights.len() / 2];
        if places
            .iter()
            // Scanned pages drift by more than a line; the edge-stack rule below
            // keeps body text out, so the height check only rejects strays.
            .all(|place| (place.1 - median).abs() <= place.2.max(1.0) * 2.0)
        {
            recurring_lines.insert(text);
        }
    }
    let repeated = detector.analyze();
    let line_texts = edge_lines
        .iter()
        .map(|(_, y, height, indices)| {
            let line = indices
                .iter()
                .map(|i| segments[*i].content.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            running_key(&line, *y < height / 2.0)
        })
        .collect::<Vec<_>>();
    // A recurring line counts only in the unbroken stack of recurring lines
    // nearest the page edge; a lookalike below a non-recurring line is body text.
    let mut by_page: HashMap<(u32, bool), Vec<usize>> = HashMap::new();
    for (line, (page, y, height, _)) in edge_lines.iter().enumerate() {
        by_page
            .entry((*page, *y < height / 2.0))
            .or_default()
            .push(line);
    }
    let mut stacked = HashSet::new();
    for ((_, top), mut lines) in by_page {
        lines.sort_by(|a, b| {
            let order = edge_lines[*a].1.total_cmp(&edge_lines[*b].1);
            if top {
                order
            } else {
                order.reverse()
            }
        });
        stacked.extend(
            lines
                .into_iter()
                .take_while(|line| recurring_lines.contains(&line_texts[*line])),
        );
    }
    let mut remove = HashSet::new();
    for (line, ((_, _, _, indices), text)) in edge_lines.iter().zip(texts).enumerate() {
        if repeated.contains(&text) || stacked.contains(&line) {
            remove.extend(indices.iter().copied());
        }
    }
    remove
}

/// The comparison key for a running line. A head masks every digit and treats
/// all brackets alike, so OCR noise ("(1988] 1 S.C.R." beside "[1988] 1 S.C.R.")
/// still matches. A foot keeps its inner digits, because footnotes at the foot
/// differ only in their numbers.
fn running_key(text: &str, top: bool) -> String {
    if !top {
        return edge_number_text(text);
    }
    HeaderFooterDetector::normalize_text(text)
        .chars()
        .map(|c| if "()[]{}".contains(c) { '|' } else { c })
        .collect()
}

/// Lowercased, space-collapsed text with only a page number at either end masked.
/// Other digits stay, so footnote citations such as "(1990) 3 SCC 645" and
/// "(1976) 2 SCC 123" never collapse into one repeated line.
fn edge_number_text(text: &str) -> String {
    let mut words = text
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    let page_number = |word: &str| !word.is_empty() && word.chars().all(|c| c.is_ascii_digit());
    if let Some(first) = words.first_mut() {
        if page_number(first) {
            *first = "#".into();
        }
    }
    if let Some(last) = words.last_mut() {
        if page_number(last) {
            *last = "#".into();
        }
    }
    words.join(" ")
}

/// The page's substantive text column from the most common line start and end,
/// so a margin letter glued to a body line cannot widen it.
fn body_columns(segments: &[TextSegment], remove: &HashSet<usize>) -> HashMap<u32, (f64, f64)> {
    // Per page: (starts, ends) of lines without a leading capital, then of all lines.
    type Edges = (HashMap<i64, usize>, HashMap<i64, usize>);
    let mut edges: HashMap<u32, (Edges, Edges)> = HashMap::new();
    for (index, segment) in segments.iter().enumerate() {
        if remove.contains(&index) || segment.content.split_whitespace().count() < 4 {
            continue;
        }
        if !segment.x.is_finite() || !segment.width.is_finite() || segment.width <= 0.0 {
            continue;
        }
        let page = edges.entry(segment.page_num).or_default();
        let start = segment.x.round() as i64;
        let end = (segment.x + segment.width).round() as i64;
        // A rail letter glued to the line start would pull the column into the margin.
        if glued_leading_letter(&segment.content).is_none() {
            *page.0 .0.entry(start).or_default() += 1;
            *page.0 .1.entry(end).or_default() += 1;
        }
        *page.1 .0.entry(start).or_default() += 1;
        *page.1 .1.entry(end).or_default() += 1;
    }
    let mode = |counts: &HashMap<i64, usize>, prefer_low: bool| {
        counts
            .iter()
            .max_by(|a, b| {
                a.1.cmp(b.1).then(if prefer_low {
                    b.0.cmp(a.0)
                } else {
                    a.0.cmp(b.0)
                })
            })
            .map(|(edge, _)| *edge as f64)
    };
    edges
        .into_iter()
        .filter_map(|(page, (plain, all))| {
            let (lefts, rights) = if plain.0.is_empty() { all } else { plain };
            // Paragraph outdents start left of the common start, so take the leftmost
            // start that recurs; ragged lines end short of the common end.
            let left = lefts
                .iter()
                .filter(|(_, count)| **count >= 2)
                .map(|(edge, _)| *edge as f64)
                .fold(None, |low: Option<f64>, edge| {
                    Some(low.map_or(edge, |low| low.min(edge)))
                })
                .or_else(|| mode(&lefts, true))?;
            Some((page, (left, mode(&rights, false)?)))
        })
        .collect()
}

/// (segment index, letter, x, glued to a body line)
type RailEntry = (usize, char, f64, bool);

/// Removes reporter line-letter rails (A, B, C ... beside the text column),
/// including a letter the layout glued to the end of a body line.
fn remove_margin_rails(segments: &mut [TextSegment], remove: &mut HashSet<usize>) {
    let bodies = body_columns(segments, remove);
    // (page, left side) -> (index, letter, x, glued to a body line)
    let mut margins: HashMap<(u32, bool), Vec<RailEntry>> = HashMap::new();
    for (index, segment) in segments.iter().enumerate() {
        if remove.contains(&index) {
            continue;
        }
        let Some((left, right)) = bodies.get(&segment.page_num) else {
            continue;
        };
        // Scanned lines end raggedly, a few points either side of the column.
        let gap = segment.font_size * 0.3;
        if !gap.is_finite() || gap <= 0.0 {
            continue;
        }
        let text = segment.content.trim();
        let mut chars = text.chars();
        if let (Some(letter), None) = (chars.next(), chars.next()) {
            if !letter.is_ascii_uppercase() {
                continue;
            }
            // A lone glyph is at most about one em wide, even when the layout
            // reports a wider merged box for it.
            if segment.x + segment.width.min(segment.font_size) + gap < *left {
                margins
                    .entry((segment.page_num, true))
                    .or_default()
                    .push((index, letter, segment.x, false));
            } else if segment.x - gap > *right {
                margins
                    .entry((segment.page_num, false))
                    .or_default()
                    .push((index, letter, segment.x, false));
            }
            continue;
        }
        let end = segment.x + segment.width;
        if let Some(letter) = glued_trailing_letter(text) {
            if end > right + gap && end <= right + segment.font_size * 4.0 {
                margins
                    .entry((segment.page_num, false))
                    .or_default()
                    .push((index, letter, end, true));
            }
        } else if let Some(letter) = glued_leading_letter(&segment.content) {
            if segment.x < left - segment.font_size && segment.x >= left - segment.font_size * 6.0 {
                margins
                    .entry((segment.page_num, true))
                    .or_default()
                    .push((index, letter, segment.x, true));
            }
        }
    }
    // Require a recurring, vertically aligned alphabet rail, not one isolated
    // initial or list marker.
    for ((page, _), rail) in margins {
        let letters = rail.iter().map(|entry| entry.1).collect::<HashSet<_>>();
        let font = rail
            .iter()
            .map(|entry| segments[entry.0].font_size)
            .fold(0.0_f64, f64::max);
        let (low, high) = rail
            .iter()
            .fold((f64::MAX, f64::MIN), |(low, high), entry| {
                (low.min(entry.2), high.max(entry.2))
            });
        if letters.len() < 3 || high - low > font.max(1.0) * 1.5 {
            continue;
        }
        let Some(&(left, right)) = bodies.get(&page) else {
            continue;
        };
        for (index, _, _, glued) in rail {
            if !glued {
                remove.insert(index);
            } else if glued_leading_letter(&segments[index].content).is_some() {
                trim_leading_letter(&mut segments[index], left);
            } else {
                trim_glued_letter(&mut segments[index], Some(right));
            }
        }
    }
}

/// `"... and, A"` -> `Some('A')` when a lone capital ends the line after a space.
fn glued_trailing_letter(text: &str) -> Option<char> {
    let mut chars = text.chars().rev();
    let letter = chars.next()?;
    (letter.is_ascii_uppercase() && chars.next()? == ' ' && chars.next().is_some_and(|c| c != ' '))
        .then_some(letter)
}

/// `"C the cardholder"` -> `Some('C')` when a lone capital opens the line.
fn glued_leading_letter(content: &str) -> Option<char> {
    let mut chars = content.trim_start().chars();
    let letter = chars.next()?;
    (letter.is_ascii_uppercase() && chars.next()? == ' ' && chars.next().is_some_and(|c| c != ' '))
        .then_some(letter)
}

fn trim_leading_letter(segment: &mut TextSegment, body_left: f64) {
    let rest = segment.content.trim_start();
    let prefix = segment.content[..segment.content.len() - rest.len()].to_string();
    let prefix_chars = prefix.chars().count();
    // Skip the letter and the space after it.
    let body = rest[2..].trim_start();
    let skipped = rest.chars().count() - body.chars().count();
    let content = format!("{prefix}{body}");
    segment.located_text = segment.located_text.as_ref().map(|located| {
        let start = prefix_chars + skipped;
        let mut spans = located
            .slice_chars(start, body.chars().count(), String::new())
            .spans;
        for span in &mut spans {
            span.output_start += prefix_chars;
            span.output_end += prefix_chars;
        }
        crate::document::LocatedText {
            text: content.clone(),
            spans,
        }
    });
    segment.char_start = segment.char_start.saturating_add(skipped);
    let end = segment.x + segment.width;
    segment.x = segment.x.max(body_left);
    segment.width = (end - segment.x).max(0.0);
    segment.word_count = content.split_whitespace().count();
    segment.content = content;
}

fn trim_glued_letter(segment: &mut TextSegment, body_right: Option<f64>) {
    let trimmed = segment.content.trim_end();
    let tail = segment.content[trimmed.len()..].to_string();
    let head = trimmed[..trimmed.len() - 1].trim_end().to_string();
    let head_chars = head.chars().count();
    let removed = segment.content.chars().count() - head_chars - tail.chars().count();
    let content = format!("{head}{}", if tail.is_empty() { " " } else { &tail });
    segment.located_text = segment.located_text.as_ref().map(|located| {
        let spans = located.slice_chars(0, head_chars, String::new()).spans;
        crate::document::LocatedText {
            text: content.clone(),
            spans,
        }
    });
    segment.char_end = segment.char_end.saturating_sub(removed);
    if let Some(right) = body_right {
        segment.width = segment.width.min((right - segment.x).max(0.0));
    }
    segment.word_count = content.split_whitespace().count();
    segment.content = content;
}

enum Footnote {
    /// Footnote text at the page foot; moved out of the page flow.
    Body,
    /// A superscript reference mark in the body text; dropped.
    Reference,
}

/// Finds footnote blocks: smaller-type lines below the last body line of a page
/// that open with a superscript number, plus the matching marks in the body.
fn footnote_blocks(segments: &[TextSegment]) -> HashMap<usize, Footnote> {
    let mut pages: HashMap<u32, Vec<usize>> = HashMap::new();
    for (index, segment) in segments.iter().enumerate() {
        if segment.font_size.is_finite() && segment.y.is_finite() {
            pages.entry(segment.page_num).or_default().push(index);
        }
    }
    let mut found = HashMap::new();
    for indices in pages.values() {
        // The body size is the size carrying the most characters on the page.
        let mut sizes: HashMap<i64, usize> = HashMap::new();
        for index in indices {
            let segment = &segments[*index];
            *sizes
                .entry((segment.font_size * 10.0).round() as i64)
                .or_default() += segment.content.chars().count();
        }
        let Some(body) = sizes
            .into_iter()
            .max_by_key(|entry| entry.1)
            .map(|entry| entry.0 as f64 / 10.0)
        else {
            continue;
        };
        let is_body = |segment: &TextSegment| {
            (segment.font_size - body).abs() < body * 0.05 && segment.word_count >= 2
        };
        let last_body = indices
            .iter()
            .map(|index| &segments[*index])
            .filter(|segment| is_body(segment))
            .map(|segment| segment.y)
            .fold(f64::MIN, f64::max);
        let mark = |segment: &TextSegment| {
            segment.font_size < body * 0.75
                && !segment.content.trim().is_empty()
                && segment.content.trim().chars().all(|c| c.is_ascii_digit())
        };
        let mut foot = indices
            .iter()
            .copied()
            .filter(|index| {
                let segment = &segments[*index];
                segment.y > last_body && segment.font_size < body * 0.9
            })
            .collect::<Vec<_>>();
        foot.sort_by(|a, b| segments[*a].y.total_cmp(&segments[*b].y));
        let Some(first) = foot.first() else { continue };
        if !mark(&segments[*first]) {
            continue;
        }
        let numbers = foot
            .iter()
            .filter(|index| mark(&segments[**index]))
            .map(|index| segments[*index].content.trim().to_string())
            .collect::<HashSet<_>>();
        for index in indices {
            let segment = &segments[*index];
            if segment.y <= last_body && mark(segment) && numbers.contains(segment.content.trim()) {
                found.insert(*index, Footnote::Reference);
            }
        }
        for index in foot {
            found.insert(index, Footnote::Body);
        }
    }
    found
}

/// Running heads sit in the outer fifth of the page; reporter heads often sit
/// lower than a fixed 10% band.
const EDGE_BAND: f64 = 0.2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_removes_running_matter_and_retains_pdf_page_locations() {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Object, Stream};
        let mut doc = lopdf::Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(
            dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica"},
        );
        let resources = doc.add_object(dictionary! {"Font" => dictionary! {"F1" => font_id}});
        let mut kids = Vec::new();
        for page in 1..=3 {
            let mut ops = Vec::new();
            let mut text = |value: String, x: i64, y: i64| {
                ops.push(Operation::new("BT", vec![]));
                ops.push(Operation::new("Tf", vec!["F1".into(), 10.into()]));
                ops.push(Operation::new(
                    "Tm",
                    vec![1.into(), 0.into(), 0.into(), 1.into(), x.into(), y.into()],
                ));
                ops.push(Operation::new("Tj", vec![Object::string_literal(value)]));
                ops.push(Operation::new("ET", vec![]));
            };
            text(format!("{page} REPEATED REPORT TITLE"), 100, 780);
            let word = ["first", "second", "third"][page - 1];
            for (i, letter) in ["A", "B", "C"].iter().enumerate() {
                let y = 700 - i as i64 * 70;
                text(letter.to_string(), 20, y);
                text(
                    format!("Document {word} body paragraph {i} with statute (B) intact."),
                    100,
                    y,
                );
            }
            let stream = Content { operations: ops }
                .encode()
                .expect("encode test content");
            let content_id = doc.add_object(Stream::new(dictionary! {}, stream));
            let page_id = doc.add_object(dictionary! {"Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 800.into()],
            "Resources" => resources, "Contents" => content_id});
            kids.push(page_id.into());
        }
        doc.objects.insert(
            pages_id,
            dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => 3}.into(),
        );
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        doc.trailer.set("Root", catalog);
        let results = super::super::processing::output_doc_new_schema(
            &doc,
            None,
            Some(512),
            1,
            "file",
            Some(&crate::LAParams {
                all_texts: true,
                ..Default::default()
            }),
            true,
            crate::ExtractionOptions::default(),
        )
        .expect("parse generated PDF");
        let text = results
            .iter()
            .map(|r| r.content_core.content.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!text.contains("REPEATED REPORT TITLE"), "{}", text);
        assert!(
            !text
                .split_whitespace()
                .any(|word| ["A", "B", "C"].contains(&word)),
            "{}",
            text
        );
        assert!(text.contains("statute (B) intact"), "{}", text);
        let pages = results
            .iter()
            .flat_map(|r| crate::extract_chunk_locations(&r.content_ext).expect("locations"))
            .map(|location| location.page)
            .collect::<HashSet<_>>();
        assert_eq!(pages, HashSet::from([1, 2, 3]));
    }

    fn segment(text: &str, page: u32, x: f64, y: f64, width: f64) -> TextSegment {
        TextSegment {
            content: text.into(),
            font_size: 10.0,
            transformed_font_size: 10.0,
            x,
            y,
            width,
            height: 10.0,
            page_num: page,
            is_bold: false,
            font_name: String::new(),
            font_weight: crate::FontWeight::Regular,
            is_italic: false,
            cutat: String::new(),
            fill_color: None,
            stroke_color: None,
            char_start: 0,
            char_end: text.len(),
            word_count: text.split_whitespace().count(),
            located_text: None,
        }
    }
    #[test]
    fn removes_split_numbered_headers_and_margin_rail_preserving_body_and_pages() {
        let mut rows = Vec::new();
        for page in 1..=3 {
            rows.push(segment(&page.to_string(), page, 80.0, 20.0, 10.0));
            rows.push(segment("REPEATED REPORT TITLE", page, 100.0, 20.0, 180.0));
            rows.push(segment("REPEATED REPORT TITLE", page, 100.0, 300.0, 180.0));
            for (i, letter) in ["A", "B", "C"].iter().enumerate() {
                rows.push(segment(letter, page, 20.0, 100.0 + i as f64 * 70.0, 5.0));
                rows.push(segment(
                    &format!(
                        "A body paragraph {} with statute (B) intact",
                        ["one", "two", "three"][page as usize - 1]
                    ),
                    page,
                    100.0,
                    100.0 + i as f64 * 70.0,
                    300.0,
                ));
            }
            rows.push(segment("A", page, 100.0, 400.0, 5.0));
        }
        let (kept, _) =
            remove_running_matter(rows, &HashMap::from([(1, 800.0), (2, 800.0), (3, 800.0)]));
        assert_eq!(
            kept.len(),
            15,
            "{:?}",
            kept.iter()
                .map(|s| (s.page_num, s.x, s.y, s.content.as_str()))
                .collect::<Vec<_>>()
        );
        for page in 1..=3 {
            assert_eq!(kept.iter().filter(|s| s.page_num == page).count(), 5);
            assert!(kept.iter().any(|s| s.page_num == page
                && s.content == "REPEATED REPORT TITLE"
                && s.y == 300.0));
            assert!(kept
                .iter()
                .any(|s| s.page_num == page && s.content == "A" && s.x == 100.0));
        }
    }
    #[test]
    fn isolated_margin_initial_and_nonrepeating_heading_survive() {
        let rows = vec![
            segment("A", 1, 20.0, 100.0, 5.0),
            segment("Body text with many words", 1, 100.0, 100.0, 300.0),
            segment("ACTUAL HEADING", 1, 100.0, 20.0, 100.0),
        ];
        assert_eq!(
            remove_running_matter(rows, &HashMap::from([(1, 800.0)]))
                .0
                .len(),
            3
        );
    }

    fn sized(text: &str, page: u32, x: f64, y: f64, width: f64, font: f64) -> TextSegment {
        TextSegment {
            font_size: font,
            transformed_font_size: font,
            ..segment(text, page, x, y, width)
        }
    }

    fn heights(pages: u32) -> HashMap<u32, f64> {
        (1..=pages).map(|page| (page, 792.0)).collect()
    }

    /// Reporter layout: verso pages carry "<n> SUPREME COURT REPORTS", recto pages the
    /// case title, both about 14% down the page; line letters sit beside the column.
    fn reporter_page(page: u32, rows: &mut Vec<TextSegment>) {
        let verso = page % 2 == 0;
        if verso {
            rows.push(segment(
                &format!("{} ", 500 + page),
                page,
                139.0,
                114.0,
                16.0,
            ));
            rows.push(segment(
                "SUPREME COURT REPORTS [2021] 13 S.C.R.",
                page,
                204.0,
                115.0,
                268.0,
            ));
        } else {
            rows.push(segment(
                &format!("{} ", 500 + page),
                page,
                472.0,
                114.0,
                16.0,
            ));
            rows.push(segment(
                "STATE v. RESPONDENT [A. JUDGE, J.]",
                page,
                161.0,
                117.0,
                287.0,
            ));
        }
        let (left, right) = if verso {
            (191.0, 475.0)
        } else {
            (177.0, 461.0)
        };
        for (i, letter) in ["A", "B", "C", "D"].iter().enumerate() {
            let y = 150.0 + i as f64 * 72.0;
            let word = ["first", "second", "third", "fourth", "fifth", "sixth"][page as usize - 1];
            let body = format!("Body line {i} of the {word} page keeps the statute (B) words");
            if verso {
                rows.push(segment(letter, page, 144.0, y, 8.0));
                rows.push(segment(&body, page, left, y, right - left));
            } else if i % 2 == 0 {
                // The layout glues a right-hand letter onto the body line.
                rows.push(segment(
                    &format!("{body} {letter}\n"),
                    page,
                    left,
                    y,
                    480.0 - left,
                ));
            } else {
                rows.push(segment(&body, page, left, y, right - left));
                rows.push(segment(letter, page, 472.7, y, 8.0));
            }
            rows.push(segment(
                &format!("Following line {i} of the {word} page continues"),
                page,
                left,
                y + 13.0,
                right - left,
            ));
        }
    }

    #[test]
    fn alternating_reporter_heads_and_rails_are_removed_body_kept() {
        let mut rows = Vec::new();
        for page in 1..=6 {
            reporter_page(page, &mut rows);
        }
        let (kept, footnotes) = remove_running_matter(rows, &heights(6));
        assert!(footnotes.is_empty());
        let text = kept
            .iter()
            .map(|s| s.content.as_str())
            .collect::<Vec<_>>()
            .join("|");
        assert!(!text.contains("SUPREME COURT REPORTS"), "{text}");
        assert!(!text.contains("RESPONDENT [A. JUDGE"), "{text}");
        assert!(
            !kept.iter().any(|s| s.content.trim().parse::<u32>().is_ok()),
            "{text}"
        );
        assert!(
            !kept
                .iter()
                .any(|s| ["A", "B", "C", "D"].contains(&s.content.trim())),
            "{text}"
        );
        assert!(
            !kept
                .iter()
                .any(|s| s.content.trim_end().ends_with(" A")
                    || s.content.trim_end().ends_with(" C")),
            "{text}"
        );
        assert_eq!(
            kept.iter()
                .filter(|s| s.content.contains("statute (B) words"))
                .count(),
            24
        );
        let glued = kept
            .iter()
            .find(|s| s.page_num == 1 && s.content.starts_with("Body line 0"))
            .expect("glued body line kept");
        assert_eq!(
            glued.content,
            "Body line 0 of the first page keeps the statute (B) words\n"
        );
        assert!(glued.x + glued.width <= 461.0 + 0.5);
    }

    #[test]
    fn rail_letter_glued_to_line_start_is_trimmed() {
        let mut rows = Vec::new();
        for (i, letter) in ["A", "B", "C"].iter().enumerate() {
            let y = 150.0 + i as f64 * 72.0;
            rows.push(segment(
                &format!("Plain body line number {i} here"),
                1,
                163.0,
                y,
                311.0,
            ));
            rows.push(segment(
                &format!("{letter} the glued body line {i} here"),
                1,
                144.0,
                y + 8.0,
                331.0,
            ));
        }
        let (kept, _) = remove_running_matter(rows, &heights(1));
        for (i, segment) in kept
            .iter()
            .filter(|s| s.content.contains("glued"))
            .enumerate()
        {
            assert_eq!(segment.content, format!("the glued body line {i} here"));
            assert_eq!(segment.x, 163.0);
        }
    }

    #[test]
    fn footnotes_leave_the_page_flow_and_reference_marks_drop() {
        let mut rows = Vec::new();
        for page in 1..=3 {
            for i in 0..6 {
                let y = 150.0 + i as f64 * 13.0;
                let word = ["first", "second", "third"][page as usize - 1];
                rows.push(sized(
                    &format!("Body sentence {i} on the {word} page runs on"),
                    page,
                    150.0,
                    y,
                    300.0,
                    11.0,
                ));
            }
            rows.push(sized(&format!("{page}"), page, 400.0, 160.0, 5.0, 6.4));
            rows.push(sized(&format!("\n{page}\n"), page, 150.0, 662.0, 6.0, 5.2));
            // Distinct citations share a shape once digits are masked.
            rows.push(sized(
                &format!("(199{page}) {page} SCC 64{page} "),
                page,
                157.0,
                665.0,
                66.0,
                9.0,
            ));
        }
        let (kept, footnotes) = remove_running_matter(rows, &heights(3));
        assert_eq!(kept.len(), 18, "{kept:?}");
        assert!(kept.iter().all(|s| s.font_size == 11.0));
        let notes = footnotes
            .iter()
            .map(|s| s.content.trim())
            .collect::<Vec<_>>();
        assert_eq!(
            notes,
            [
                "1",
                "(1991) 1 SCC 641",
                "2",
                "(1992) 2 SCC 642",
                "3",
                "(1993) 3 SCC 643"
            ]
        );
    }

    #[test]
    fn edge_number_text_masks_only_line_end_numbers() {
        assert_eq!(
            edge_number_text("528 SUPREME COURT REPORTS [2021] 13 S.C.R."),
            "# supreme court reports [2021] 13 s.c.r."
        );
        assert_eq!(edge_number_text("(1990) 3 SCC 645"), "(1990) 3 scc #");
        assert_ne!(
            edge_number_text("(1990) 3 SCC 645"),
            edge_number_text("(1976) 2 SCC 123")
        );
    }
}
