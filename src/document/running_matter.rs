//! Remove document running matter before layout merges and chunking.
use super::header_footer::HeaderFooterDetector;
use crate::TextSegment;
use std::collections::{HashMap, HashSet};

pub(crate) fn remove_running_matter(
    segments: Vec<TextSegment>,
    page_heights: &HashMap<u32, f64>,
) -> Vec<TextSegment> {
    if std::env::var("PDF_EXTRACT_SKIP_HEADER_FOOTER").is_ok() {
        return segments;
    }
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
        if segment.y > height * 0.1 && segment.y < height * 0.9 {
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
        detector.add_occurrence(&text, *page, *y, font, *height);
        texts.push(HeaderFooterDetector::normalize_text(&text));
    }
    let repeated = detector.analyze();
    let mut remove = HashSet::new();
    for ((_, _, _, indices), text) in edge_lines.iter().zip(texts) {
        if repeated.contains(&text) {
            remove.extend(indices.iter().copied());
        }
    }

    // Infer the body column from substantive runs, never from single letters.
    let mut bodies: HashMap<u32, (f64, f64)> = HashMap::new();
    for (index, segment) in segments.iter().enumerate() {
        if remove.contains(&index) || segment.content.split_whitespace().count() < 4 {
            continue;
        }
        if !segment.x.is_finite() || !segment.width.is_finite() || segment.width <= 0.0 {
            continue;
        }
        let body = bodies
            .entry(segment.page_num)
            .or_insert((segment.x, segment.x + segment.width));
        body.0 = body.0.min(segment.x);
        body.1 = body.1.max(segment.x + segment.width);
    }
    let mut margins: HashMap<(u32, bool), Vec<(usize, char)>> = HashMap::new();
    for (index, segment) in segments.iter().enumerate() {
        let text = segment.content.trim();
        let mut chars = text.chars();
        let Some(letter) = chars.next() else { continue };
        if chars.next().is_some() || !letter.is_ascii_uppercase() {
            continue;
        }
        let Some((left, right)) = bodies.get(&segment.page_num) else {
            continue;
        };
        let gap = segment.font_size * 2.0;
        if !gap.is_finite() || gap <= 0.0 {
            continue;
        }
        let outside_left = segment.x + segment.width + gap < *left;
        let outside_right = segment.x - gap > *right;
        if outside_left || outside_right {
            margins
                .entry((segment.page_num, outside_left))
                .or_default()
                .push((index, letter));
        }
    }
    // Require a recurring alphabet rail, not one isolated initial or list marker.
    for rail in margins.values() {
        let letters = rail
            .iter()
            .map(|(_, letter)| *letter)
            .collect::<HashSet<_>>();
        if letters.len() >= 3 {
            remove.extend(rail.iter().map(|(index, _)| *index));
        }
    }
    segments
        .into_iter()
        .enumerate()
        .filter_map(|(i, segment)| (!remove.contains(&i)).then_some(segment))
        .collect()
}

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
            for (i, letter) in ["A", "B", "C"].iter().enumerate() {
                let y = 700 - i as i64 * 70;
                text(letter.to_string(), 20, y);
                text(
                    format!("Document {page} body paragraph {i} with statute (B) intact."),
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
                    "A body paragraph with statute (B) intact",
                    page,
                    100.0,
                    100.0 + i as f64 * 70.0,
                    300.0,
                ));
            }
            rows.push(segment("A", page, 100.0, 400.0, 5.0));
        }
        let kept =
            remove_running_matter(rows, &HashMap::from([(1, 800.0), (2, 800.0), (3, 800.0)]));
        assert_eq!(kept.len(), 15);
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
            remove_running_matter(rows, &HashMap::from([(1, 800.0)])).len(),
            3
        );
    }
}
