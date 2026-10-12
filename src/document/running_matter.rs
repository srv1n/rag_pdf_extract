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
    // key -> (page, y, font, page number at either end of the line)
    let mut recurring: HashMap<String, Vec<(u32, f64, f64, Option<i64>)>> = HashMap::new();
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
        recurring.entry(running_key(&text)).or_default().push((
            *page,
            *y,
            font,
            edge_number(&text),
        ));
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
        // A masked number must be the page number: it moves with the page by one
        // constant offset. "1 Overview" on pages 2, 5 and 9 does not.
        let offsets = places
            .iter()
            .map(|place| place.3.map(|number| number - i64::from(place.0)))
            .collect::<HashSet<_>>();
        if offsets.len() > 1 {
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
        .map(|(_, _, _, indices)| {
            let line = indices
                .iter()
                .map(|i| segments[*i].content.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            running_key(&line)
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

/// The comparison key for a running line: only a page number at either end is
/// masked, so "Section 1 Overview" and "Section 2 Overview" stay distinct, and
/// all brackets compare alike, so OCR noise ("(1988] 1 S.C.R." beside
/// "[1988] 1 S.C.R.") still matches.
fn running_key(text: &str) -> String {
    edge_number_text(text)
        .chars()
        .map(|c| if "()[]{}".contains(c) { '|' } else { c })
        .collect()
}

/// The number at either end of a line, if any (the candidate page number).
fn edge_number(text: &str) -> Option<i64> {
    let mut words = text.split_whitespace();
    let first = words.next()?;
    let last = words.last().unwrap_or(first);
    [first, last]
        .into_iter()
        .find(|word| word.chars().all(|c| c.is_ascii_digit()))
        .and_then(|word| word.parse().ok())
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

/// (segment index, letter, x, y, glued to a body line)
type RailEntry = (usize, char, f64, f64, bool);

/// Removes reporter line-letter rails (A, B, C ... beside the text column),
/// including a letter the layout glued to the end of a body line.
fn remove_margin_rails(segments: &mut [TextSegment], remove: &mut HashSet<usize>) {
    let bodies = body_columns(segments, remove);
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
                    .push((index, letter, segment.x, segment.y, false));
            } else if segment.x - gap > *right {
                margins
                    .entry((segment.page_num, false))
                    .or_default()
                    .push((index, letter, segment.x, segment.y, false));
            }
            continue;
        }
        let end = segment.x + segment.width;
        if let Some(letter) = glued_trailing_letter(text) {
            if end > right + gap && end <= right + segment.font_size * 4.0 {
                margins
                    .entry((segment.page_num, false))
                    .or_default()
                    .push((index, letter, end, segment.y, true));
            }
        } else if let Some(letter) = glued_leading_letter(&segment.content) {
            if segment.x < left - segment.font_size && segment.x >= left - segment.font_size * 6.0 {
                margins
                    .entry((segment.page_num, true))
                    .or_default()
                    .push((index, letter, segment.x, segment.y, true));
            }
        }
    }
    // A rail is four or more capitals in order, aligned on one side of a page...
    let rails = margins
        .into_iter()
        .filter(|(_, rail)| {
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
            // A reporter rail runs A, B, C, D... down the page.
            let mut by_height = rail.iter().collect::<Vec<_>>();
            by_height.sort_by(|a, b| a.3.total_cmp(&b.3));
            let in_order = by_height.windows(2).all(|pair| pair[0].1 < pair[1].1);
            letters.len() >= 4 && in_order && high - low <= font.max(1.0) * 1.5
        })
        .collect::<Vec<_>>();
    // A reporter rail runs A..H down a full page; a four-item outline never shows a fifth.
    if !rails.iter().any(|(_, rail)| {
        rail.iter()
            .map(|entry| entry.1)
            .collect::<HashSet<_>>()
            .len()
            >= 5
    }) {
        return;
    }
    // ...whose letters sit at the same heights on other pages too. A printed
    // line-letter rail repeats its A..H positions page after page; an A/B/C
    // outline does not.
    let mut heights: HashMap<char, Vec<(u32, f64)>> = HashMap::new();
    for ((page, _), rail) in &rails {
        for entry in rail {
            heights.entry(entry.1).or_default().push((*page, entry.3));
        }
    }
    for ((page, left_side), rail) in rails {
        let recurring = rail
            .into_iter()
            .filter(|entry| {
                let tolerance = segments[entry.0].font_size.max(1.0) * 2.0;
                heights[&entry.1]
                    .iter()
                    .filter(|(_, y)| (y - entry.3).abs() <= tolerance)
                    .map(|(page, _)| *page)
                    .collect::<HashSet<_>>()
                    .len()
                    >= 3
            })
            .collect::<Vec<_>>();
        if recurring.len() < 3 {
            continue;
        }
        let Some(&(left, right)) = bodies.get(&page) else {
            continue;
        };
        for (index, _, _, _, glued) in recurring {
            if !glued {
                remove.insert(index);
            } else if left_side {
                trim_leading_letter(&mut segments[index], left);
            } else {
                trim_glued_letter(&mut segments[index], right);
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

/// Keeps the located spans of `keep` (start, len) ranges of the old text, laid end
/// to end in the new text.
fn keep_spans(
    located: &crate::document::LocatedText,
    keep: &[(usize, usize)],
    text: &str,
) -> crate::document::LocatedText {
    let mut spans = Vec::new();
    let mut offset = 0;
    for (start, len) in keep {
        for mut span in located.slice_chars(*start, *len, String::new()).spans {
            span.output_start += offset;
            span.output_end += offset;
            spans.push(span);
        }
        offset += len;
    }
    crate::document::LocatedText {
        text: text.to_string(),
        spans,
    }
}

/// Drops a leading rail letter and its space. The paragraph prefix stays; the
/// source range starts past the letter, as the trailing trim ends before it.
fn trim_leading_letter(segment: &mut TextSegment, body_left: f64) {
    let rest = segment.content.trim_start();
    let prefix_chars = segment.content.chars().count() - rest.chars().count();
    let body = rest[2..].trim_start();
    let body_chars = body.chars().count();
    let removed = rest.chars().count() - body_chars;
    let content = format!(
        "{}{body}",
        &segment.content[..segment.content.len() - rest.len()]
    );
    segment.located_text = segment.located_text.as_ref().map(|located| {
        keep_spans(
            located,
            &[(0, prefix_chars), (prefix_chars + removed, body_chars)],
            &content,
        )
    });
    segment.char_start = (segment.char_start + removed).min(segment.char_end);
    let end = segment.x + segment.width;
    segment.x = segment.x.max(body_left);
    segment.width = (end - segment.x).max(0.0);
    segment.word_count = content.split_whitespace().count();
    segment.content = content;
}

/// Drops a trailing rail letter and the space before it; trailing whitespace stays.
fn trim_glued_letter(segment: &mut TextSegment, body_right: f64) {
    let total = segment.content.chars().count();
    let trimmed = segment.content.trim_end();
    let tail = segment.content[trimmed.len()..].to_string();
    let tail_chars = tail.chars().count();
    let head = trimmed[..trimmed.len() - 1].trim_end().to_string();
    let head_chars = head.chars().count();
    let removed = total - head_chars - tail_chars;
    // A continuation line keeps its trailing space; this one is synthetic.
    let content = format!("{head}{}", if tail.is_empty() { " " } else { &tail });
    segment.located_text = segment.located_text.as_ref().map(|located| {
        keep_spans(
            located,
            &[(0, head_chars), (total - tail_chars, tail_chars)],
            &content,
        )
    });
    if tail.is_empty() {
        // The retained source text now ends before the letter.
        segment.char_end = segment.char_end.saturating_sub(removed);
    }
    segment.width = segment.width.min((body_right - segment.x).max(0.0));
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
        // A footnote block opens with a superscript number, carries text, and is
        // cited by the same superscript number in the body. A lone small page
        // number or a numbered caption has no matching body mark.
        // A footnote mark sits on its footnote's first line, just left of its text.
        let labels = |opener: &TextSegment| {
            mark(opener)
                && foot.iter().any(|index| {
                    let segment = &segments[*index];
                    !mark(segment)
                        && segment.word_count >= 2
                        && (segment.y - opener.y).abs() <= body * 0.6
                        && segment.x >= opener.x
                        && segment.x <= opener.x + opener.width + body * 1.5
                })
        };
        if !labels(&segments[*first]) {
            continue;
        }
        // A small page number in the foot labels nothing, so it stays in the page flow.
        let foot = foot
            .iter()
            .copied()
            .filter(|index| !mark(&segments[*index]) || labels(&segments[*index]))
            .collect::<Vec<_>>();
        let numbers = foot
            .iter()
            .filter(|index| mark(&segments[**index]))
            .map(|index| segments[*index].content.trim().to_string())
            .collect::<HashSet<_>>();
        let references = indices
            .iter()
            .copied()
            .filter(|index| {
                let segment = &segments[*index];
                segment.y <= last_body && mark(segment) && numbers.contains(segment.content.trim())
            })
            .collect::<Vec<_>>();
        if references.is_empty() {
            continue;
        }
        for index in references {
            found.insert(index, Footnote::Reference);
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
            for (i, letter) in ["A", "B", "C", "D", "E"].iter().enumerate() {
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
                .any(|word| ["A", "B", "C", "D", "E"].contains(&word)),
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
            for (i, letter) in ["A", "B", "C", "D", "E"].iter().enumerate() {
                rows.push(segment(letter, page, 20.0, 100.0 + i as f64 * 55.0, 5.0));
                rows.push(segment(
                    &format!(
                        "A body paragraph {} with statute (B) intact",
                        ["one", "two", "three"][page as usize - 1]
                    ),
                    page,
                    100.0,
                    100.0 + i as f64 * 55.0,
                    300.0,
                ));
            }
            rows.push(segment("A", page, 100.0, 400.0, 5.0));
        }
        let (kept, _) =
            remove_running_matter(rows, &HashMap::from([(1, 800.0), (2, 800.0), (3, 800.0)]));
        assert_eq!(
            kept.len(),
            21,
            "{:?}",
            kept.iter()
                .map(|s| (s.page_num, s.x, s.y, s.content.as_str()))
                .collect::<Vec<_>>()
        );
        for page in 1..=3 {
            assert_eq!(kept.iter().filter(|s| s.page_num == page).count(), 7);
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
        for (i, letter) in ["A", "B", "C", "D", "E"].iter().enumerate() {
            let y = 150.0 + i as f64 * 72.0;
            let word = ["first", "second", "third", "fourth", "fifth", "sixth"][page as usize - 1];
            let body = format!("Body line {i} of the {word} page keeps the statute (B) words");
            if verso {
                rows.push(segment(letter, page, 144.0, y, 8.0));
                rows.push(segment(&body, page, left, y, right - left));
            } else if i % 2 == 0 {
                // The layout glues a right-hand letter onto the body line; a
                // line may itself open with the article "A".
                let article = if i == 2 { "A " } else { "" };
                rows.push(segment(
                    &format!("{article}{body} {letter}\n"),
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
            30
        );
        assert_eq!(
            kept.iter()
                .filter(|s| s.content
                    == format!(
                        "A Body line 2 of the {} page keeps the statute (B) words\n",
                        if s.page_num == 1 {
                            "first"
                        } else if s.page_num == 3 {
                            "third"
                        } else {
                            "fifth"
                        }
                    ))
                .count(),
            3
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

    fn located(text: &str, page: u32) -> crate::document::LocatedText {
        use crate::document::span_map::{OutputSpan, SpanSource};
        crate::document::LocatedText {
            text: text.into(),
            spans: (0..text.chars().count())
                .map(|i| OutputSpan {
                    output_start: i,
                    output_end: i + 1,
                    source: SpanSource::Pdf {
                        page,
                        char_start: i,
                        char_end: i + 1,
                        bbox: crate::BoundingBox {
                            x: i as f64,
                            y: 0.0,
                            width: 1.0,
                            height: 1.0,
                        },
                    },
                })
                .collect(),
        }
    }

    fn source_start(span: &crate::document::span_map::OutputSpan) -> usize {
        match &span.source {
            crate::document::span_map::SpanSource::Pdf { char_start, .. } => *char_start,
            _ => panic!("expected a PDF span"),
        }
    }

    #[test]
    fn rail_letters_glued_to_line_ends_are_trimmed_with_their_spans() {
        let mut rows = Vec::new();
        for page in 1..=3 {
            let word = ["first", "second", "third"][page as usize - 1];
            for (i, letter) in ["A", "B", "C", "D", "E"].iter().enumerate() {
                let y = 150.0 + i as f64 * 72.0;
                rows.push(segment(
                    &format!("Plain {word} body line number {i} here"),
                    page,
                    163.0,
                    y,
                    311.0,
                ));
                for extra in 1..=3 {
                    rows.push(segment(
                        &format!("More {word} plain text {i} line {extra} here"),
                        page,
                        163.0,
                        y + 40.0 + extra as f64 * 13.0,
                        311.0,
                    ));
                }
                let start = format!("\n{letter} the glued {word} line {i} here");
                let mut glued = segment(&start, page, 144.0, y + 8.0, 331.0);
                // The parser leaves spans unset; page 3 keeps that shape.
                glued.located_text = (page < 3).then(|| located(&start, page));
                rows.push(glued);
                let end = format!("ends the {word} line {i} here {letter}\n");
                let mut glued = segment(&end, page, 163.0, y + 30.0, 320.0);
                glued.located_text = Some(located(&end, page));
                rows.push(glued);
            }
        }
        let (kept, _) = remove_running_matter(rows, &heights(3));
        let starts = kept
            .iter()
            .filter(|s| s.content.contains("the glued"))
            .collect::<Vec<_>>();
        assert_eq!(starts.len(), 15);
        for segment in starts {
            // The source range starts past the letter and its space.
            assert_eq!(segment.char_start, 2);
            if segment.page_num == 3 {
                assert!(segment.located_text.is_none());
                continue;
            }
            assert!(
                segment.content.starts_with("\nthe glued"),
                "{:?}",
                segment.content
            );
            assert_eq!(segment.x, 163.0);
            let spans = &segment.located_text.as_ref().unwrap().spans;
            assert_eq!(segment.located_text.as_ref().unwrap().text, segment.content);
            assert_eq!(spans.len(), segment.content.chars().count());
            assert_eq!(source_start(&spans[0]), 0, "kept paragraph prefix");
            assert_eq!((spans[1].output_start, source_start(&spans[1])), (1, 3));
        }
        let ends = kept
            .iter()
            .filter(|s| s.content.starts_with("ends the"))
            .collect::<Vec<_>>();
        assert_eq!(ends.len(), 15);
        for segment in ends {
            assert!(
                segment.content.ends_with(" here\n"),
                "{:?}",
                segment.content
            );
            let spans = &segment.located_text.as_ref().unwrap().spans;
            assert_eq!(spans.len(), segment.content.chars().count());
            let total = segment.content.chars().count() + 2;
            assert_eq!(
                source_start(spans.last().unwrap()),
                total - 1,
                "kept newline"
            );
        }
    }

    #[test]
    fn an_outline_and_numbered_section_heads_are_not_furniture() {
        let mut rows = Vec::new();
        for page in 1..=3 {
            let word = ["first", "second", "third"][page as usize - 1];
            rows.push(segment(
                &format!("Section {page} Overview"),
                page,
                100.0,
                110.0,
                120.0,
            ));
            for (i, letter) in ["A", "B", "C"].iter().enumerate() {
                // Outline labels beside their clauses, at different heights per page.
                let y = 200.0 + page as f64 * 40.0 + i as f64 * 30.0;
                rows.push(segment(letter, page, 80.0, y, 7.0));
                rows.push(segment(
                    &format!("The {word} clause {i} of this outline reads"),
                    page,
                    100.0,
                    y,
                    300.0,
                ));
                rows.push(segment(
                    &format!("and its {word} continuation {i} runs on"),
                    page,
                    100.0,
                    y + 13.0,
                    300.0,
                ));
            }
        }
        let (kept, _) = remove_running_matter(rows, &heights(3));
        assert_eq!(
            kept.iter()
                .filter(|s| s.content.ends_with("Overview"))
                .count(),
            3
        );
        assert_eq!(
            kept.iter()
                .filter(|s| ["A", "B", "C"].contains(&s.content.as_str()))
                .count(),
            9
        );
    }

    #[test]
    fn a_small_page_number_is_not_a_footnote() {
        let mut rows = Vec::new();
        for page in 1..=3 {
            let word = ["first", "second", "third"][page as usize - 1];
            for i in 0..6 {
                rows.push(sized(
                    &format!("Body sentence {i} on the {word} page runs on"),
                    page,
                    150.0,
                    150.0 + i as f64 * 13.0,
                    300.0,
                    12.0,
                ));
            }
            rows.push(sized(&format!("{page}"), page, 300.0, 760.0, 6.0, 8.0));
            rows.push(sized(
                "A small caption under the figure",
                page,
                150.0,
                600.0,
                200.0,
                8.0,
            ));
        }
        let (kept, footnotes) = remove_running_matter(rows, &heights(3));
        assert!(footnotes.is_empty(), "{footnotes:?}");
        assert_eq!(
            kept.iter()
                .filter(|s| s.content.starts_with("A small caption"))
                .count(),
            3
        );
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

    #[test]
    fn a_numbered_head_that_does_not_track_the_page_stays() {
        let mut rows = Vec::new();
        for (n, page) in [(1, 2), (2, 5), (3, 9)] {
            rows.push(segment(&format!("{n} Overview"), page, 100.0, 110.0, 120.0));
            rows.push(segment(
                &format!("Body text for part {n} of the report"),
                page,
                100.0,
                300.0,
                300.0,
            ));
        }
        let (kept, _) = remove_running_matter(rows, &heights(9));
        assert_eq!(
            kept.iter()
                .filter(|s| s.content.ends_with("Overview"))
                .count(),
            3
        );
    }

    #[test]
    fn a_four_label_outline_at_fixed_heights_is_not_a_rail() {
        let mut rows = Vec::new();
        for page in 1..=3 {
            let word = ["first", "second", "third"][page as usize - 1];
            for (i, letter) in ["A", "B", "C", "D"].iter().enumerate() {
                let y = 200.0 + i as f64 * 100.0;
                rows.push(segment(letter, page, 80.0, y, 7.0));
                rows.push(segment(
                    &format!("The {word} clause {i} of this outline reads"),
                    page,
                    100.0,
                    y,
                    300.0,
                ));
                rows.push(segment(
                    &format!("and its {word} continuation {i} runs on"),
                    page,
                    100.0,
                    y + 13.0,
                    300.0,
                ));
            }
        }
        let (kept, _) = remove_running_matter(rows, &heights(3));
        assert_eq!(
            kept.iter()
                .filter(|s| ["A", "B", "C", "D"].contains(&s.content.as_str()))
                .count(),
            12
        );
    }

    #[test]
    fn a_page_number_beside_a_caption_does_not_claim_a_body_marker() {
        let mut rows = Vec::new();
        for i in 0..6 {
            rows.push(sized(
                &format!("Body sentence {i} of the only page runs on"),
                1,
                150.0,
                150.0 + i as f64 * 13.0,
                300.0,
                12.0,
            ));
        }
        // A small list marker in the body, a small centred page number, then a caption.
        rows.push(sized("1", 1, 140.0, 163.0, 5.0, 7.0));
        rows.push(sized("1", 1, 300.0, 700.0, 5.0, 7.0));
        rows.push(sized(
            "A caption for the plate",
            1,
            150.0,
            720.0,
            200.0,
            8.0,
        ));
        let (kept, footnotes) = remove_running_matter(rows, &heights(1));
        assert!(footnotes.is_empty(), "{footnotes:?}");
        assert!(kept.iter().any(|s| s.content == "1" && s.y == 163.0));
        assert!(kept.iter().any(|s| s.content.starts_with("A caption")));
    }

    #[test]
    fn a_footer_page_number_is_not_a_footnote_label() {
        let mut rows = Vec::new();
        for i in 0..6 {
            rows.push(sized(
                &format!("Body sentence {i} of the only page runs on"),
                1,
                150.0,
                150.0 + i as f64 * 13.0,
                300.0,
                12.0,
            ));
        }
        // Footnote 1 is cited; the superscript 2 in x² is not a footnote mark.
        rows.push(sized("1", 1, 400.0, 150.0, 5.0, 7.0));
        rows.push(sized("2", 1, 420.0, 176.0, 5.0, 7.0));
        rows.push(sized("1", 1, 150.0, 650.0, 5.0, 7.0));
        rows.push(sized(
            "A note on the cited case",
            1,
            157.0,
            650.0,
            150.0,
            9.0,
        ));
        rows.push(sized("2", 1, 300.0, 760.0, 5.0, 7.0));
        let (kept, footnotes) = remove_running_matter(rows, &heights(1));
        let notes = footnotes
            .iter()
            .map(|s| s.content.trim())
            .collect::<Vec<_>>();
        assert_eq!(notes, ["1", "A note on the cited case"]);
        assert!(kept.iter().any(|s| s.content == "2" && s.y == 176.0));
        assert!(!kept.iter().any(|s| s.content == "1"));
    }
}
