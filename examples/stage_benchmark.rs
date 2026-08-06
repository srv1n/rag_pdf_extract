use pdf_extract::{
    decompress_content_ext, decompress_content_ext_bytes, extract_pdf_location, parse_pdf,
    ExtractionOptions, ExtractionResult, LAParams,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Deserialize, Serialize)]
struct FixtureManifest {
    fixtures: Vec<Fixture>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Fixture {
    id: String,
    path: String,
    category: String,
    #[serde(default)]
    expected: Expected,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Expected {
    min_chunks: Option<usize>,
    must_contain: Option<Vec<String>>,
    max_over_cap_chunks: Option<usize>,
    location_required: Option<bool>,
    max_identical_location_sibling_count: Option<usize>,
    min_source_span_coverage_ratio: Option<f64>,
    min_output_span_coverage_ratio: Option<f64>,
    layout_to_no_layout_normalized_char_ratio_min: Option<f64>,
    layout_to_no_layout_normalized_char_ratio_max: Option<f64>,
    allow_overlapping_spans: Option<bool>,
    max_span_bbox_collapse_count: Option<usize>,
    max_identical_span_set_sibling_count: Option<usize>,
    max_highlight_replay_failures: Option<usize>,
    max_out_of_page_bbox_count: Option<usize>,
    min_highlight_replay_iou_p50: Option<f64>,
    min_highlight_replay_iou_p95: Option<f64>,
}

const DEFAULT_LAYOUT_TO_NO_LAYOUT_RATIO_MIN: f64 = 0.95;
const DEFAULT_LAYOUT_TO_NO_LAYOUT_MIN_ABSOLUTE_LOSS: usize = 512;

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    generated_at_epoch_ms: u128,
    command: String,
    git_sha: Option<String>,
    git_dirty: Option<bool>,
    manifest_path: String,
    fixtures: Vec<FixtureReport>,
}

#[derive(Debug, Serialize)]
struct FixtureReport {
    fixture_id: String,
    path: String,
    category: String,
    status: String,
    product_status: String,
    raw_layout_status: String,
    overall_status: String,
    stages: StageSet,
    references: ReferenceSet,
    chunks: ChunkMetrics,
    location: LocationMetrics,
    layout_fallback_used: bool,
    raw_layout_failures: Vec<String>,
    failures: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
struct StageSet {
    ours_no_layout_raw: Option<StageMetrics>,
    ours_layout_raw_without_fallback: Option<StageMetrics>,
    ours_layout_with_product_fallback: Option<StageMetrics>,
    ours_no_layout_post_filters: Option<StageMetrics>,
    ours_layout_post_filters: Option<StageMetrics>,
    ours_chunks: Option<StageMetrics>,
}

#[derive(Debug, Default, Serialize)]
struct ReferenceSet {
    poppler_pdftotext: ReferenceMetrics,
}

#[derive(Debug, Default, Serialize)]
struct ReferenceMetrics {
    status: String,
    normalized_chars: Option<usize>,
    delta_to_layout_chars: Option<i64>,
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct StageMetrics {
    normalized_chars: usize,
    raw_chars: usize,
    chunk_count: usize,
    duplicate_line_ratio: f64,
    layout_to_no_layout_char_ratio: Option<f64>,
    duration_ms: u128,
}

#[derive(Debug, Default, Serialize)]
struct ChunkMetrics {
    chunk_count: usize,
    max_tokens: usize,
    max_chunk_tokens: usize,
    over_cap_chunks: usize,
    empty_chunks: usize,
    chunks_without_location: usize,
    content_ext_max_compressed_bytes: usize,
    content_ext_max_uncompressed_bytes: usize,
    content_ext_total_compressed_bytes: usize,
    content_ext_total_uncompressed_bytes: usize,
}

#[derive(Debug, Default, Serialize)]
struct LocationMetrics {
    fragment_count: usize,
    invalid_bbox_count: usize,
    zero_area_bbox_count: usize,
    fragments_outside_page_count: usize,
    identical_location_sibling_count: usize,
    source_span_coverage_ratio: f64,
    output_span_coverage_ratio: f64,
    output_chars_total: usize,
    output_chars_with_any_span: usize,
    output_chars_pdf_backed: usize,
    output_chars_synthetic: usize,
    spanned_char_ratio: f64,
    pdf_backed_char_ratio: f64,
    pdf_backed_nonsynthetic_char_ratio: f64,
    chars_without_span: usize,
    chars_with_overlapping_spans: usize,
    wrong_page_span_count: usize,
    invalid_span_bbox_count: usize,
    out_of_page_bbox_count: usize,
    span_bbox_collapse_count: usize,
    identical_span_set_sibling_count: usize,
    synthetic_char_ratio: f64,
    overlapping_same_text_bbox_ratio: f64,
    form_text_ratio: f64,
    invisible_text_ratio: f64,
    vertical_duplicate_ratio: f64,
    fragment_density: f64,
    highlight_replay_failures: usize,
    highlight_replay_iou_p50: f64,
    highlight_replay_iou_p95: f64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    let manifest_path = read_flag(&args, "--manifest")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("eval/fixtures/pdf_manifest.json"));
    let out_dir = read_flag(&args, "--out-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("eval/runs/stage_benchmark_current"));
    let max_tokens = read_flag(&args, "--max-tokens")
        .and_then(|v| v.parse().ok())
        .unwrap_or(500usize);

    fs::create_dir_all(&out_dir)?;
    let raw = fs::read_to_string(&manifest_path)?;
    let manifest: FixtureManifest = serde_json::from_str(&raw)?;
    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));

    let mut fixture_reports = Vec::new();
    for fixture in manifest.fixtures {
        let path = resolve_path(manifest_dir, &fixture.path);
        fixture_reports.push(run_fixture(&fixture, &path, max_tokens));
    }

    let report = BenchmarkReport {
        generated_at_epoch_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        command: args.join(" "),
        git_sha: git_output(&["rev-parse", "HEAD"]),
        git_dirty: git_dirty(),
        manifest_path: manifest_path.display().to_string(),
        fixtures: fixture_reports,
    };

    fs::write(
        out_dir.join("stage_benchmark.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    fs::write(out_dir.join("stage_benchmark.md"), render_markdown(&report))?;

    let failed = report.fixtures.iter().any(|f| f.product_status == "failed");
    println!(
        "Report JSON: {}",
        out_dir.join("stage_benchmark.json").display()
    );
    println!(
        "Report MD: {}",
        out_dir.join("stage_benchmark.md").display()
    );
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

fn run_fixture(fixture: &Fixture, path: &Path, max_tokens: usize) -> FixtureReport {
    if !path.exists() {
        return FixtureReport {
            fixture_id: fixture.id.clone(),
            path: path.display().to_string(),
            category: fixture.category.clone(),
            status: "failed".to_string(),
            product_status: "failed".to_string(),
            raw_layout_status: "failed".to_string(),
            overall_status: "failed".to_string(),
            stages: StageSet::default(),
            references: ReferenceSet::default(),
            chunks: ChunkMetrics::default(),
            location: LocationMetrics::default(),
            layout_fallback_used: false,
            raw_layout_failures: Vec::new(),
            failures: vec!["fixture missing".to_string()],
        };
    }

    let no_layout = timed_parse(path, max_tokens, None);
    let mut lp = LAParams::diagnostic_layout();
    lp.all_texts = fixture.category == "form_xobject";
    lp.detect_vertical = fixture.category == "vertical";
    let layout_raw = timed_parse(path, max_tokens, Some(lp.clone()));
    lp.layout_fallback_policy = LAParams::product_layout().layout_fallback_policy;
    let layout_product = timed_parse(path, max_tokens, Some(lp));

    let mut failures = Vec::new();
    let mut raw_layout_failures = Vec::new();
    let mut stages = StageSet::default();
    let mut references = ReferenceSet::default();
    let mut chunks = ChunkMetrics {
        max_tokens,
        ..ChunkMetrics::default()
    };
    let mut location = LocationMetrics::default();
    if let Err(err) = &layout_raw {
        raw_layout_failures.push(format!("raw layout parse failed: {err}"));
    }

    if let Ok((docs, duration_ms)) = &no_layout {
        let metrics = stage_metrics(docs, *duration_ms, None);
        stages.ours_no_layout_raw = Some(metrics.clone());
        stages.ours_no_layout_post_filters = Some(metrics);
    }

    if let Ok((docs, duration_ms)) = &layout_raw {
        let no_layout_chars = stages
            .ours_no_layout_raw
            .as_ref()
            .map(|stage| stage.normalized_chars);
        stages.ours_layout_raw_without_fallback =
            Some(stage_metrics(docs, *duration_ms, no_layout_chars));
    }

    let mut layout_fallback_used = false;
    match layout_product {
        Ok((docs, duration_ms)) => {
            let no_layout_chars = stages
                .ours_no_layout_raw
                .as_ref()
                .map(|stage| stage.normalized_chars);
            let layout_stage = stage_metrics(&docs, duration_ms, no_layout_chars);
            chunks = chunk_metrics(&docs, max_tokens);
            location = location_metrics(&docs, &page_dimensions(path));
            stages.ours_chunks = Some(layout_stage.clone());
            stages.ours_layout_with_product_fallback = Some(layout_stage.clone());
            stages.ours_layout_post_filters = Some(layout_stage);
            layout_fallback_used = match (
                stages.ours_layout_raw_without_fallback.as_ref(),
                stages.ours_layout_with_product_fallback.as_ref(),
            ) {
                (Some(raw), Some(product)) => stage_fingerprint(raw) != stage_fingerprint(product),
                _ => false,
            };
            references = reference_metrics(path, &stages);
            apply_raw_layout_gates(fixture, &stages, &mut raw_layout_failures);
            apply_expectations(fixture, &docs, &chunks, &location, &stages, &mut failures);
        }
        Err(err) => failures.push(format!("layout parse failed: {err}")),
    }

    let product_status = if failures.is_empty() {
        "passed"
    } else {
        "failed"
    }
    .to_string();
    let raw_layout_status = if raw_layout_failures.is_empty() {
        "passed"
    } else {
        "failed"
    }
    .to_string();
    let overall_status = if product_status == "failed" {
        "failed".to_string()
    } else if raw_layout_status == "failed" {
        if layout_fallback_used {
            "passed_with_fallback".to_string()
        } else {
            "passed_with_raw_layout_failure".to_string()
        }
    } else {
        "passed".to_string()
    };

    FixtureReport {
        fixture_id: fixture.id.clone(),
        path: path.display().to_string(),
        category: fixture.category.clone(),
        status: overall_status.clone(),
        product_status,
        raw_layout_status,
        overall_status,
        stages,
        references,
        chunks,
        location,
        layout_fallback_used,
        raw_layout_failures,
        failures,
    }
}

fn timed_parse(
    path: &Path,
    max_tokens: usize,
    laparams: Option<LAParams>,
) -> Result<(Vec<ExtractionResult>, u128), Box<dyn Error>> {
    let started = Instant::now();
    let docs = parse_pdf(
        path.to_str().unwrap_or_default(),
        1,
        "file",
        None,
        None,
        None,
        Some(max_tokens),
        laparams,
        Some(true),
        ExtractionOptions {
            emit_output_spans: true,
            ..ExtractionOptions::default()
        },
    )?;
    Ok((docs, started.elapsed().as_millis()))
}

fn page_dimensions(path: &Path) -> HashMap<u32, (f64, f64)> {
    let mut dimensions = HashMap::new();
    let Ok(doc) = lopdf::Document::load(path) else {
        return dimensions;
    };
    for (page_num, object_id) in doc.get_pages() {
        let Ok(page_obj) = doc.get_object(object_id) else {
            continue;
        };
        let Ok(page_dict) = page_obj.as_dict() else {
            continue;
        };
        let Some(media_box) = inherited_number_array(&doc, page_dict, b"MediaBox") else {
            continue;
        };
        if media_box.len() >= 4 {
            let width = (media_box[2] - media_box[0]).abs();
            let height = (media_box[3] - media_box[1]).abs();
            dimensions.insert(page_num, (width, height));
        }
    }
    dimensions
}

fn inherited_number_array(
    doc: &lopdf::Document,
    dict: &lopdf::Dictionary,
    key: &[u8],
) -> Option<Vec<f64>> {
    if let Ok(object) = dict.get(key) {
        if let Some(numbers) = object_number_array(object) {
            return Some(numbers);
        }
    }
    let Ok(lopdf::Object::Reference(parent_id)) = dict.get(b"Parent") else {
        return None;
    };
    let parent = doc.get_object(*parent_id).ok()?.as_dict().ok()?;
    inherited_number_array(doc, parent, key)
}

fn object_number_array(object: &lopdf::Object) -> Option<Vec<f64>> {
    let lopdf::Object::Array(items) = object else {
        return None;
    };
    items
        .iter()
        .map(|item| match item {
            lopdf::Object::Integer(value) => Some(*value as f64),
            lopdf::Object::Real(value) => Some(*value as f64),
            _ => None,
        })
        .collect()
}

fn stage_fingerprint(stage: &StageMetrics) -> (usize, usize, usize) {
    (stage.normalized_chars, stage.raw_chars, stage.chunk_count)
}

fn stage_metrics(
    docs: &[ExtractionResult],
    duration_ms: u128,
    no_layout_chars: Option<usize>,
) -> StageMetrics {
    let text = docs
        .iter()
        .map(|doc| doc.content_core.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let normalized_chars = normalized_chars(&text);
    StageMetrics {
        normalized_chars,
        raw_chars: text.chars().count(),
        chunk_count: docs.len(),
        duplicate_line_ratio: duplicate_line_ratio(&text),
        layout_to_no_layout_char_ratio: no_layout_chars
            .filter(|chars| *chars > 0)
            .map(|chars| normalized_chars as f64 / chars as f64),
        duration_ms,
    }
}

fn chunk_metrics(docs: &[ExtractionResult], max_tokens: usize) -> ChunkMetrics {
    let tokenizer = tiktoken_rs::get_bpe_from_model("gpt-4o").ok();
    let mut out = ChunkMetrics {
        chunk_count: docs.len(),
        max_tokens,
        ..ChunkMetrics::default()
    };
    for doc in docs {
        if doc.content_core.content.trim().is_empty() {
            out.empty_chunks += 1;
        }
        let tokens = tokenizer
            .as_ref()
            .map(|tok| tok.encode_ordinary(&doc.content_core.content).len())
            .unwrap_or(doc.content_core.token_count as usize);
        out.max_chunk_tokens = out.max_chunk_tokens.max(tokens);
        if tokens > max_tokens {
            out.over_cap_chunks += 1;
        }
        if extract_pdf_location(&doc.content_ext)
            .map(|loc| loc.fragments.is_empty())
            .unwrap_or(true)
        {
            out.chunks_without_location += 1;
        }
        out.content_ext_max_compressed_bytes = out
            .content_ext_max_compressed_bytes
            .max(doc.content_ext.ext_json.len());
        out.content_ext_total_compressed_bytes += doc.content_ext.ext_json.len();
        if let Ok(bytes) = decompress_content_ext_bytes(&doc.content_ext) {
            out.content_ext_max_uncompressed_bytes =
                out.content_ext_max_uncompressed_bytes.max(bytes.len());
            out.content_ext_total_uncompressed_bytes += bytes.len();
        }
    }
    out
}

fn location_metrics(
    docs: &[ExtractionResult],
    page_dimensions: &HashMap<u32, (f64, f64)>,
) -> LocationMetrics {
    let mut out = LocationMetrics::default();
    let mut chunks_with_fragments = 0usize;
    let mut chunks_with_output_spans = 0usize;
    let mut highlight_ious = Vec::new();
    let mut seen_locations = HashSet::new();
    let mut seen_span_sets = HashSet::new();
    let mut full_text = String::new();
    for doc in docs {
        let output_chars = doc.content_core.content.chars().count();
        let content_chars = doc.content_core.content.chars().collect::<Vec<_>>();
        full_text.push_str(&doc.content_core.content);
        full_text.push('\n');
        out.output_chars_total += output_chars;
        let fragments = extract_pdf_location(&doc.content_ext)
            .map(|loc| loc.fragments)
            .unwrap_or_default();
        if !fragments.is_empty() {
            chunks_with_fragments += 1;
        }
        let key = serde_json::to_string(&fragments).unwrap_or_default();
        if !key.is_empty() && !seen_locations.insert(key) {
            out.identical_location_sibling_count += 1;
        }
        for frag in &fragments {
            out.fragment_count += 1;
            if !valid_bbox(&frag.bbox) {
                out.invalid_bbox_count += 1;
            }
            if frag.bbox.width == 0.0 || frag.bbox.height == 0.0 {
                out.zero_area_bbox_count += 1;
            }
            match page_dimensions.get(&frag.page) {
                Some(dimensions) => {
                    if bbox_outside_page(&frag.bbox, *dimensions) {
                        out.fragments_outside_page_count += 1;
                        out.out_of_page_bbox_count += 1;
                    }
                }
                None => out.wrong_page_span_count += 1,
            }
        }
        if let Ok(ext) = decompress_content_ext(&doc.content_ext) {
            if let Some(spans) = ext.get("output_spans").and_then(|value| value.as_array()) {
                if !spans.is_empty() {
                    chunks_with_output_spans += 1;
                }
                let span_key = serde_json::to_string(spans).unwrap_or_default();
                if !span_key.is_empty() && !seen_span_sets.insert(span_key) {
                    out.identical_span_set_sibling_count += 1;
                }
                let mut coverage = vec![0usize; output_chars];
                let mut pdf_coverage = vec![false; output_chars];
                let mut synthetic_coverage = vec![false; output_chars];
                for span in spans {
                    let start = span
                        .get("output_start")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0) as usize;
                    let end = span
                        .get("output_end")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(start as u64) as usize;
                    let start = start.min(output_chars);
                    let end = end.min(output_chars);
                    if start >= end {
                        continue;
                    }
                    let source = span.get("source");
                    let is_pdf = source.and_then(|source| source.get("Pdf")).is_some();
                    let is_synthetic = source.and_then(|source| source.get("Synthetic")).is_some();
                    for idx in start..end {
                        coverage[idx] += 1;
                        if is_pdf {
                            pdf_coverage[idx] = true;
                        }
                        if is_synthetic {
                            synthetic_coverage[idx] = true;
                        }
                    }
                    if is_pdf {
                        if let Some(pdf_source) = source.and_then(|source| source.get("Pdf")) {
                            let page = pdf_source
                                .get("page")
                                .and_then(|value| value.as_u64())
                                .unwrap_or(0) as u32;
                            if !page_dimensions.contains_key(&page) {
                                out.wrong_page_span_count += 1;
                            }
                            if content_chars[start..end].iter().any(|ch| *ch == '\n') {
                                out.span_bbox_collapse_count += 1;
                            }
                            if let Some(span_bbox) = pdf_source.get("bbox") {
                                match json_bbox(span_bbox) {
                                    Some(bbox) if valid_bbox(&bbox) => {
                                        if let Some(dimensions) = page_dimensions.get(&page) {
                                            if bbox_outside_page(&bbox, *dimensions) {
                                                out.out_of_page_bbox_count += 1;
                                            }
                                        }
                                        let best = fragments
                                            .iter()
                                            .filter(|frag| frag.page == page)
                                            .map(|frag| bbox_iou(&bbox, &frag.bbox))
                                            .fold(0.0, f64::max);
                                        highlight_ious.push(best);
                                        if best < 0.50 {
                                            out.highlight_replay_failures += 1;
                                        }
                                    }
                                    _ => out.invalid_span_bbox_count += 1,
                                }
                            } else {
                                out.invalid_span_bbox_count += 1;
                            }
                        }
                    }
                }
                out.output_chars_with_any_span +=
                    coverage.iter().filter(|count| **count > 0).count();
                out.output_chars_pdf_backed +=
                    pdf_coverage.iter().filter(|covered| **covered).count();
                out.output_chars_synthetic += synthetic_coverage
                    .iter()
                    .filter(|covered| **covered)
                    .count();
                out.chars_without_span += coverage.iter().filter(|count| **count == 0).count();
                out.chars_with_overlapping_spans +=
                    coverage.iter().filter(|count| **count > 1).count();
            }
        }
    }
    out.vertical_duplicate_ratio = duplicate_line_ratio(&full_text);
    if out.output_chars_total > 0 {
        out.overlapping_same_text_bbox_ratio =
            out.chars_with_overlapping_spans as f64 / out.output_chars_total as f64;
    }
    out.fragment_density = if docs.is_empty() {
        0.0
    } else {
        out.fragment_count as f64 / docs.len() as f64
    };
    out.source_span_coverage_ratio = if docs.is_empty() {
        0.0
    } else {
        chunks_with_fragments as f64 / docs.len() as f64
    };
    out.output_span_coverage_ratio = if docs.is_empty() {
        0.0
    } else {
        chunks_with_output_spans as f64 / docs.len() as f64
    };
    out.pdf_backed_char_ratio = if out.output_chars_total == 0 {
        0.0
    } else {
        out.output_chars_pdf_backed as f64 / out.output_chars_total as f64
    };
    out.spanned_char_ratio = if out.output_chars_total == 0 {
        0.0
    } else {
        out.output_chars_with_any_span as f64 / out.output_chars_total as f64
    };
    let nonsynthetic_chars = out
        .output_chars_total
        .saturating_sub(out.output_chars_synthetic);
    out.pdf_backed_nonsynthetic_char_ratio = if nonsynthetic_chars == 0 {
        0.0
    } else {
        out.output_chars_pdf_backed as f64 / nonsynthetic_chars as f64
    };
    out.synthetic_char_ratio = if out.output_chars_total == 0 {
        0.0
    } else {
        out.output_chars_synthetic as f64 / out.output_chars_total as f64
    };
    highlight_ious.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out.highlight_replay_iou_p50 = percentile(&highlight_ious, 0.50);
    out.highlight_replay_iou_p95 = percentile(&highlight_ious, 0.95);
    out
}

fn json_bbox(value: &serde_json::Value) -> Option<pdf_extract::BoundingBox> {
    Some(pdf_extract::BoundingBox {
        x: value.get("x")?.as_f64()?,
        y: value.get("y")?.as_f64()?,
        width: value.get("width")?.as_f64()?,
        height: value.get("height")?.as_f64()?,
    })
}

fn valid_bbox(bbox: &pdf_extract::BoundingBox) -> bool {
    bbox.x.is_finite()
        && bbox.y.is_finite()
        && bbox.width.is_finite()
        && bbox.height.is_finite()
        && bbox.width >= 0.0
        && bbox.height >= 0.0
}

fn bbox_outside_page(bbox: &pdf_extract::BoundingBox, dimensions: (f64, f64)) -> bool {
    let (width, height) = dimensions;
    let tolerance = 1.0;
    bbox.x < -tolerance
        || bbox.y < -tolerance
        || bbox.x + bbox.width > width + tolerance
        || bbox.y + bbox.height > height + tolerance
}

fn bbox_iou(a: &pdf_extract::BoundingBox, b: &pdf_extract::BoundingBox) -> f64 {
    let ax2 = a.x + a.width;
    let ay2 = a.y + a.height;
    let bx2 = b.x + b.width;
    let by2 = b.y + b.height;
    let ix = (ax2.min(bx2) - a.x.max(b.x)).max(0.0);
    let iy = (ay2.min(by2) - a.y.max(b.y)).max(0.0);
    let inter = ix * iy;
    let area_a = a.width.max(0.0) * a.height.max(0.0);
    let area_b = b.width.max(0.0) * b.height.max(0.0);
    let denom = area_a + area_b - inter;
    if denom <= 0.0 {
        0.0
    } else {
        inter / denom
    }
}

fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let idx = ((values.len() - 1) as f64 * p).round() as usize;
    values[idx.min(values.len() - 1)]
}

fn reference_metrics(path: &Path, stages: &StageSet) -> ReferenceSet {
    let layout_chars = stages
        .ours_layout_post_filters
        .as_ref()
        .map(|stage| stage.normalized_chars)
        .unwrap_or(0);
    ReferenceSet {
        poppler_pdftotext: poppler_reference(path, layout_chars),
    }
}

fn poppler_reference(path: &Path, layout_chars: usize) -> ReferenceMetrics {
    let output = std::process::Command::new("pdftotext")
        .arg("-layout")
        .arg(path)
        .arg("-")
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            let normalized = normalized_chars(&text);
            ReferenceMetrics {
                status: "available".to_string(),
                normalized_chars: Some(normalized),
                delta_to_layout_chars: Some(layout_chars as i64 - normalized as i64),
                note: None,
            }
        }
        Ok(output) => ReferenceMetrics {
            status: "missing_reference".to_string(),
            normalized_chars: None,
            delta_to_layout_chars: None,
            note: Some(format!("pdftotext exited with {}", output.status)),
        },
        Err(err) => ReferenceMetrics {
            status: "missing_reference".to_string(),
            normalized_chars: None,
            delta_to_layout_chars: None,
            note: Some(format!("pdftotext unavailable: {err}")),
        },
    }
}

fn apply_expectations(
    fixture: &Fixture,
    docs: &[ExtractionResult],
    chunks: &ChunkMetrics,
    location: &LocationMetrics,
    stages: &StageSet,
    failures: &mut Vec<String>,
) {
    if let Some(min_chunks) = fixture.expected.min_chunks {
        if chunks.chunk_count < min_chunks {
            failures.push(format!(
                "chunk_count {} < expected {}",
                chunks.chunk_count, min_chunks
            ));
        }
    }
    if let Some(max_over_cap) = fixture.expected.max_over_cap_chunks {
        if chunks.over_cap_chunks > max_over_cap {
            failures.push(format!(
                "over_cap_chunks {} > expected {}",
                chunks.over_cap_chunks, max_over_cap
            ));
        }
    }
    if fixture.expected.location_required.unwrap_or(false) && chunks.chunks_without_location > 0 {
        failures.push(format!(
            "{} chunks without location",
            chunks.chunks_without_location
        ));
    }
    if let Some(max_identical) = fixture.expected.max_identical_location_sibling_count {
        if location.identical_location_sibling_count > max_identical {
            failures.push(format!(
                "identical_location_sibling_count {} > expected {}",
                location.identical_location_sibling_count, max_identical
            ));
        }
    }
    if let Some(max_identical) = fixture.expected.max_identical_span_set_sibling_count {
        if location.identical_span_set_sibling_count > max_identical {
            failures.push(format!(
                "identical_span_set_sibling_count {} > expected {}",
                location.identical_span_set_sibling_count, max_identical
            ));
        }
    }
    if let Some(min) = fixture.expected.min_source_span_coverage_ratio {
        if location.source_span_coverage_ratio < min {
            failures.push(format!(
                "source_span_coverage_ratio {:.3} < {:.3}",
                location.source_span_coverage_ratio, min
            ));
        }
    }
    if let Some(min) = fixture.expected.min_output_span_coverage_ratio {
        if location.output_span_coverage_ratio < min {
            failures.push(format!(
                "output_span_coverage_ratio {:.3} < {:.3}",
                location.output_span_coverage_ratio, min
            ));
        }
    }
    for needle in fixture.expected.must_contain.as_deref().unwrap_or(&[]) {
        if !docs
            .iter()
            .any(|doc| doc.content_core.content.contains(needle))
        {
            failures.push(format!("missing required text `{needle}`"));
        }
    }
    if let (Some(layout), Some(no_layout)) = (
        stages.ours_layout_with_product_fallback.as_ref(),
        stages.ours_no_layout_raw.as_ref(),
    ) {
        if no_layout.normalized_chars > 0 {
            let ratio = layout.normalized_chars as f64 / no_layout.normalized_chars as f64;
            let min = fixture
                .expected
                .layout_to_no_layout_normalized_char_ratio_min
                .unwrap_or(DEFAULT_LAYOUT_TO_NO_LAYOUT_RATIO_MIN);
            let lost_chars = no_layout
                .normalized_chars
                .saturating_sub(layout.normalized_chars);
            if lost_chars >= DEFAULT_LAYOUT_TO_NO_LAYOUT_MIN_ABSOLUTE_LOSS && ratio < min {
                failures.push(format!("layout/no-layout char ratio {ratio:.3} < {min:.3}"));
            }
            if let Some(max) = fixture
                .expected
                .layout_to_no_layout_normalized_char_ratio_max
            {
                if ratio > max {
                    failures.push(format!("layout/no-layout char ratio {ratio:.3} > {max:.3}"));
                }
            }
        }
    }
    if location.invalid_bbox_count > 0 || location.zero_area_bbox_count > 0 {
        failures.push("invalid or zero-area bbox fragments found".to_string());
    }
    if location.invalid_span_bbox_count > 0 {
        failures.push(format!(
            "{} invalid span bboxes found",
            location.invalid_span_bbox_count
        ));
    }
    if location.wrong_page_span_count > 0 {
        failures.push(format!(
            "{} wrong-page spans/fragments found",
            location.wrong_page_span_count
        ));
    }
    let max_out_of_page = fixture.expected.max_out_of_page_bbox_count.unwrap_or(0);
    if location.out_of_page_bbox_count > max_out_of_page {
        failures.push(format!(
            "out_of_page_bbox_count {} > expected {}",
            location.out_of_page_bbox_count, max_out_of_page
        ));
    }
    let max_bbox_collapse = fixture.expected.max_span_bbox_collapse_count.unwrap_or(0);
    if location.span_bbox_collapse_count > max_bbox_collapse {
        failures.push(format!(
            "span_bbox_collapse_count {} > expected {}",
            location.span_bbox_collapse_count, max_bbox_collapse
        ));
    }
    if location.chars_without_span > 0 {
        failures.push(format!(
            "{} output chars without spans",
            location.chars_without_span
        ));
    }
    if !fixture.expected.allow_overlapping_spans.unwrap_or(false)
        && location.chars_with_overlapping_spans > 0
    {
        failures.push(format!(
            "{} output chars with overlapping spans",
            location.chars_with_overlapping_spans
        ));
    }
    if location.output_chars_total > 0 && location.spanned_char_ratio < 0.999 {
        failures.push(format!(
            "spanned_char_ratio {:.3} < 0.999",
            location.spanned_char_ratio
        ));
    }
    let nonsynthetic_chars = location
        .output_chars_total
        .saturating_sub(location.output_chars_synthetic);
    if nonsynthetic_chars > 0 && location.pdf_backed_nonsynthetic_char_ratio < 0.999 {
        failures.push(format!(
            "pdf_backed_nonsynthetic_char_ratio {:.3} < 0.999",
            location.pdf_backed_nonsynthetic_char_ratio
        ));
    }
    let max_highlight_failures = fixture.expected.max_highlight_replay_failures.unwrap_or(0);
    if location.highlight_replay_failures > max_highlight_failures {
        failures.push(format!(
            "highlight_replay_failures {} > expected {}",
            location.highlight_replay_failures, max_highlight_failures
        ));
    }
    if let Some(min) = fixture.expected.min_highlight_replay_iou_p50 {
        if location.highlight_replay_iou_p50 < min {
            failures.push(format!(
                "highlight_replay_iou_p50 {:.3} < {:.3}",
                location.highlight_replay_iou_p50, min
            ));
        }
    }
    if let Some(min) = fixture.expected.min_highlight_replay_iou_p95 {
        if location.highlight_replay_iou_p95 < min {
            failures.push(format!(
                "highlight_replay_iou_p95 {:.3} < {:.3}",
                location.highlight_replay_iou_p95, min
            ));
        }
    }
}

fn apply_raw_layout_gates(fixture: &Fixture, stages: &StageSet, failures: &mut Vec<String>) {
    let (Some(raw), Some(no_layout)) = (
        stages.ours_layout_raw_without_fallback.as_ref(),
        stages.ours_no_layout_raw.as_ref(),
    ) else {
        return;
    };
    if no_layout.normalized_chars == 0 {
        return;
    }

    let ratio = raw.normalized_chars as f64 / no_layout.normalized_chars as f64;
    let min = fixture
        .expected
        .layout_to_no_layout_normalized_char_ratio_min
        .unwrap_or(DEFAULT_LAYOUT_TO_NO_LAYOUT_RATIO_MIN);
    let lost_chars = no_layout
        .normalized_chars
        .saturating_sub(raw.normalized_chars);
    if lost_chars >= DEFAULT_LAYOUT_TO_NO_LAYOUT_MIN_ABSOLUTE_LOSS && ratio < min {
        failures.push(format!(
            "raw layout/no-layout char ratio {ratio:.3} < {min:.3}"
        ));
    }
    let max = fixture
        .expected
        .layout_to_no_layout_normalized_char_ratio_max
        .unwrap_or(1.25);
    if raw.normalized_chars > no_layout.normalized_chars && ratio > max {
        failures.push(format!(
            "raw layout/no-layout char ratio {ratio:.3} > {max:.3}"
        ));
    }
}

fn normalized_chars(text: &str) -> usize {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .count()
}

fn duplicate_line_ratio(text: &str) -> f64 {
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            line.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return 0.0;
    }
    let unique = lines.iter().collect::<std::collections::HashSet<_>>().len();
    1.0 - unique as f64 / lines.len() as f64
}

fn render_markdown(report: &BenchmarkReport) -> String {
    let mut out = String::from("# PDF Stage Benchmark\n\n");
    let product_passed = report
        .fixtures
        .iter()
        .filter(|fixture| fixture.product_status == "passed")
        .count();
    let raw_layout_passed = report
        .fixtures
        .iter()
        .filter(|fixture| fixture.raw_layout_status == "passed")
        .count();
    let fallback_used = report
        .fixtures
        .iter()
        .filter(|fixture| fixture.layout_fallback_used)
        .count();
    out.push_str(&format!(
        "- command: `{}`\n- git_sha: `{}`\n- git_dirty: `{}`\n- product_passed: {}/{}\n- raw_layout_passed: {}/{}\n- fallback_used: {}/{}\n\n",
        report.command,
        report.git_sha.as_deref().unwrap_or("unknown"),
        report
            .git_dirty
            .map(|dirty| dirty.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        product_passed,
        report.fixtures.len(),
        raw_layout_passed,
        report.fixtures.len(),
        fallback_used,
        report.fixtures.len()
    ));
    out.push_str("| Fixture | Product | Raw Layout | Overall | Fallback | Chunks | Max Tokens | Over Cap | Max ContentExt c/u bytes | No Layout Chars | Layout Chars | Fragments | Span Coverage | Highlight IoU p50/p95 | Reference | Failures |\n");
    out.push_str("| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |\n");
    for fixture in &report.fixtures {
        out.push_str(&format!(
            "| `{}` | `{}` | `{}` | `{}` | {} | {} | {} | {} | {}/{} | {} | {} | {} | {:.2} | {:.2}/{:.2} | poppler: `{}` | {} |\n",
            fixture.fixture_id,
            fixture.product_status,
            fixture.raw_layout_status,
            fixture.overall_status,
            fixture.layout_fallback_used,
            fixture.chunks.chunk_count,
            fixture.chunks.max_chunk_tokens,
            fixture.chunks.over_cap_chunks,
            fixture.chunks.content_ext_max_compressed_bytes,
            fixture.chunks.content_ext_max_uncompressed_bytes,
            fixture
                .stages
                .ours_no_layout_post_filters
                .as_ref()
                .map(|s| s.normalized_chars)
                .unwrap_or(0),
            fixture
                .stages
                .ours_layout_post_filters
                .as_ref()
                .map(|s| s.normalized_chars)
                .unwrap_or(0),
            fixture.location.fragment_count,
            fixture.location.output_span_coverage_ratio,
            fixture.location.highlight_replay_iou_p50,
            fixture.location.highlight_replay_iou_p95,
            fixture.references.poppler_pdftotext.status,
            if fixture.failures.is_empty() {
                "-".to_string()
            } else {
                fixture.failures.join("; ")
            }
        ));
    }
    out
}

fn resolve_path(manifest_dir: &Path, path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        manifest_dir.join(candidate)
    }
}

fn read_flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == name)
        .map(|window| window[1].clone())
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_dirty() -> Option<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(!output.stdout.is_empty())
}
