//! Export parsed PDF chunks to a machine-readable JSON file.
//!
//! Stage 1 of the offline extraction bench: parse a PDF once and cache the
//! chunks, so later stages never re-parse. Each chunk gets a dense 1-based
//! integer `id` (the token the model cites) alongside the extractor's stable
//! `chunk_id` (for traceability back into the pipeline).
//!
//! Usage:
//!   export_chunks <PDF_PATH> <MAX_TOKENS> <OUTPUT_JSON>
//!
//! Location fields are emitted as `null` when the extractor genuinely produced
//! no location data for a chunk. They are never defaulted: a missing page
//! silently becoming 0 would poison every downstream rehydration.

use pdf_extract::{
    decompress_content_ext, parse_pdf, ChunkLocation, ContentExt, ExtractionResult, LAParams,
    PageFragment,
};
use serde::Serialize;
use std::error::Error;
use std::path::Path;

#[derive(Serialize)]
struct ExportedDocument {
    source_pdf: String,
    max_tokens: usize,
    chunk_count: usize,
    chunks: Vec<ExportedChunk>,
}

#[derive(Serialize)]
struct ExportedChunk {
    /// Dense, 1-based, ordered. This is the identifier the model cites.
    id: usize,
    /// Extractor's stable blake3 identity for the same chunk.
    chunk_id: String,
    text: String,
    token_count: i32,
    /// Every page this chunk touches, ascending. `null` when unknown.
    pages: Option<Vec<u32>>,
    /// Character offsets within `text`. `null` when unknown.
    char_range: Option<ExportedCharRange>,
    /// Union boxes in PDF user space, tagged with their page. `null` when unknown.
    bboxes: Option<Vec<ExportedBox>>,
}

#[derive(Serialize)]
struct ExportedCharRange {
    start: usize,
    end: usize,
}

#[derive(Serialize)]
struct ExportedBox {
    page: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// The location facts recovered from one chunk's compressed extension payload.
struct ChunkPlacement {
    pages: Option<Vec<u32>>,
    char_range: Option<ExportedCharRange>,
    bboxes: Option<Vec<ExportedBox>>,
}

fn usage(program: &str) -> String {
    format!("Usage: {} <PDF_PATH> <MAX_TOKENS> <OUTPUT_JSON>", program)
}

fn main() -> Result<(), Box<dyn Error>> {
    let _ = env_logger::try_init();

    let args: Vec<String> = std::env::args().collect();
    let program = args
        .first()
        .map(String::as_str)
        .unwrap_or("export_chunks")
        .to_string();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage(&program));
        return Ok(());
    }
    if args.len() != 4 {
        return Err(format!(
            "expected 3 arguments, got {}\n{}",
            args.len().saturating_sub(1),
            usage(&program)
        )
        .into());
    }

    let pdf_path = &args[1];
    let max_tokens: usize = args[2]
        .parse()
        .map_err(|e| format!("MAX_TOKENS '{}' is not a positive integer: {}", args[2], e))?;
    if max_tokens == 0 {
        return Err("MAX_TOKENS must be greater than zero".into());
    }
    let output_path = Path::new(&args[3]);

    let source_pdf = Path::new(pdf_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| format!("PDF_PATH '{}' has no file name component", pdf_path))?;

    // Same configuration as `examples/extract.rs` in its default mode:
    // layout analysis on, text cleaning on, no OCR.
    let results: Vec<ExtractionResult> = parse_pdf(
        pdf_path,
        1,
        "file",
        None,
        None,
        None,
        Some(max_tokens),
        Some(LAParams::default()),
        Some(true),
        Default::default(),
    )?;

    let mut chunks = Vec::with_capacity(results.len());
    for (index, result) in results.iter().enumerate() {
        let placement = placement_for(&result.content_ext).map_err(|e| {
            format!(
                "chunk {} ({}): failed to read location metadata: {}",
                index + 1,
                result.content_core.chunk_id,
                e
            )
        })?;

        chunks.push(ExportedChunk {
            id: index + 1,
            chunk_id: result.content_core.chunk_id.clone(),
            text: result.content_core.content.clone(),
            token_count: result.content_core.token_count,
            pages: placement.pages,
            char_range: placement.char_range,
            bboxes: placement.bboxes,
        });
    }

    let document = ExportedDocument {
        source_pdf,
        max_tokens,
        chunk_count: chunks.len(),
        chunks,
    };

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = std::fs::File::create(output_path)?;
    let writer = std::io::BufWriter::new(file);
    serde_json::to_writer_pretty(writer, &document)?;

    println!(
        "{} -> {} ({} chunks, max_tokens={})",
        pdf_path,
        output_path.display(),
        document.chunk_count,
        max_tokens
    );
    Ok(())
}

/// Decompress a chunk's `ContentExt` once and recover its placement.
///
/// A corrupt or unreadable payload is an error. A payload that simply carries
/// no location data yields `None` fields, never zeros.
fn placement_for(content_ext: &ContentExt) -> Result<ChunkPlacement, Box<dyn Error>> {
    let payload = decompress_content_ext(content_ext)?;

    let fragments = read_fragments(&payload)?;
    let chunk_locations = read_chunk_locations(&payload)?;

    // Character offsets are only meaningful from fragments, which are documented
    // as offsets into the emitted chunk text. `page_char_start`/`page_char_end`
    // are offsets into the source page and must not be substituted here.
    let char_range = fragments
        .iter()
        .map(|fragment| (fragment.char_range.start, fragment.char_range.end))
        .reduce(|(a_start, a_end), (b_start, b_end)| (a_start.min(b_start), a_end.max(b_end)))
        .map(|(start, end)| ExportedCharRange { start, end });

    // `chunk_locations` is the extractor's purpose-built per-page union summary;
    // fall back to per-fragment boxes when it is absent.
    let mut boxes: Vec<ExportedBox> = Vec::new();
    if !chunk_locations.is_empty() {
        for location in &chunk_locations {
            for bbox in &location.bboxes {
                if let Some(exported) = exported_box(location.page, bbox) {
                    boxes.push(exported);
                }
            }
        }
    } else {
        for fragment in &fragments {
            if let Some(exported) = exported_box(fragment.page, &fragment.bbox) {
                boxes.push(exported);
            }
        }
    }

    let mut pages: Vec<u32> = chunk_locations
        .iter()
        .map(|location| location.page)
        .chain(fragments.iter().map(|fragment| fragment.page))
        .collect();
    pages.sort_unstable();
    pages.dedup();

    Ok(ChunkPlacement {
        pages: if pages.is_empty() { None } else { Some(pages) },
        char_range,
        bboxes: if boxes.is_empty() { None } else { Some(boxes) },
    })
}

/// Drop boxes carrying non-finite coordinates: JSON has no representation for
/// them, and a clamped substitute would be a fabricated location.
fn exported_box(page: u32, bbox: &pdf_extract::BoundingBox) -> Option<ExportedBox> {
    if !bbox.x.is_finite()
        || !bbox.y.is_finite()
        || !bbox.width.is_finite()
        || !bbox.height.is_finite()
    {
        return None;
    }
    Some(ExportedBox {
        page,
        x: bbox.x,
        y: bbox.y,
        width: bbox.width,
        height: bbox.height,
    })
}

fn read_fragments(payload: &serde_json::Value) -> Result<Vec<PageFragment>, Box<dyn Error>> {
    let Some(format_location) = payload.get("format_location") else {
        return Ok(Vec::new());
    };

    // Current shape: internally tagged, `{"format": "Pdf", "fragments": [...]}`.
    let fragments = if format_location.get("format").and_then(|v| v.as_str()) == Some("Pdf") {
        format_location.get("fragments")
    } else {
        // Legacy shape: externally tagged, `{"Pdf": {"fragments": [...]}}`.
        format_location
            .get("Pdf")
            .and_then(|pdf| pdf.get("fragments"))
    };

    match fragments {
        None => Ok(Vec::new()),
        Some(value) if value.is_null() => Ok(Vec::new()),
        Some(value) => Ok(serde_json::from_value(value.clone())?),
    }
}

fn read_chunk_locations(payload: &serde_json::Value) -> Result<Vec<ChunkLocation>, Box<dyn Error>> {
    match payload.get("chunk_locations") {
        None => Ok(Vec::new()),
        Some(value) if value.is_null() => Ok(Vec::new()),
        Some(value) => Ok(serde_json::from_value(value.clone())?),
    }
}
