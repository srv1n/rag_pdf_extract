use lopdf::Document;
use pdf_extract::{output_doc_new_schema, LAParams, OcrConfig, OcrHandler};
use serde::{Deserialize, Serialize};
use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Deserialize)]
struct RegressionManifest {
    suite_name: String,
    description: Option<String>,
    base_dir: Option<String>,
    documents: Vec<RegressionCase>,
}

#[derive(Debug, Deserialize)]
struct RegressionCase {
    id: String,
    label: String,
    path: String,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    source_id: Option<i64>,
    #[serde(default)]
    source_type: Option<String>,
    #[serde(default)]
    max_tokens: Option<usize>,
    #[serde(default)]
    all_texts: Option<bool>,
    #[serde(default)]
    detect_vertical: Option<bool>,
    #[serde(default)]
    clean_text: Option<bool>,
    #[serde(default)]
    use_ocr: Option<bool>,
    #[serde(default)]
    ocr_detection_model: Option<String>,
    #[serde(default)]
    ocr_recognition_model: Option<String>,
    #[serde(default)]
    expected_min_chars: Option<usize>,
    #[serde(default)]
    expected_min_chunks: Option<usize>,
    #[serde(default)]
    expected_min_pages: Option<usize>,
}

#[derive(Debug, Serialize)]
struct RegressionReport {
    suite_name: String,
    description: Option<String>,
    manifest_path: String,
    output_dir: String,
    started_at_unix: u64,
    finished_at_unix: u64,
    totals: RegressionTotals,
    documents: Vec<DocumentReport>,
}

#[derive(Debug, Default, Serialize)]
struct RegressionTotals {
    passed: usize,
    warned: usize,
    failed: usize,
    skipped: usize,
}

#[derive(Debug, Serialize)]
struct DocumentReport {
    id: String,
    label: String,
    group: Option<String>,
    path: String,
    resolved_path: Option<String>,
    status: String,
    completion_note: String,
    page_count: Option<usize>,
    chunk_count: Option<usize>,
    extracted_char_count: Option<usize>,
    extracted_token_count: Option<usize>,
    max_chunk_token_count: Option<usize>,
    over_cap_chunk_count: Option<usize>,
    garbage_chunk_count: Option<usize>,
    duplicate_line_ratio: Option<f64>,
    expected_min_pages: Option<usize>,
    expected_min_chunks: Option<usize>,
    expected_min_chars: Option<usize>,
    warnings: Vec<String>,
    error: Option<String>,
    notes: Option<String>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        eprintln!(
            "Usage: {} --manifest <manifest.json> [--out-dir <dir>]",
            args[0]
        );
        eprintln!();
        eprintln!("Runs a manifest-driven PDF extraction regression pack.");
        return Ok(());
    }

    let manifest_path = read_flag_value(&args, "--manifest")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("eval/regressions/founder_legal_pack.json"));
    let out_dir = read_flag_value(&args, "--out-dir")
        .map(PathBuf::from)
        .unwrap_or_else(default_output_dir);

    fs::create_dir_all(&out_dir)?;

    let manifest_text = fs::read_to_string(&manifest_path)?;
    let manifest: RegressionManifest = serde_json::from_str(&manifest_text)?;
    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let base_dir = manifest
        .base_dir
        .as_deref()
        .map(|base| manifest_dir.join(base))
        .unwrap_or_else(|| manifest_dir.to_path_buf());

    let started_at = unix_now();
    let mut totals = RegressionTotals::default();
    let mut documents = Vec::new();

    let total_cases = manifest.documents.len();
    for (idx, case) in manifest.documents.into_iter().enumerate() {
        println!(
            "[{}/{}] {} ({})",
            idx + 1,
            total_cases,
            case.label,
            case.path
        );
        let report = run_case(&base_dir, &case);
        println!("    -> {}: {}", report.status, report.completion_note);
        match report.status.as_str() {
            "passed" => totals.passed += 1,
            "warned" => totals.warned += 1,
            "failed" => totals.failed += 1,
            "skipped" => totals.skipped += 1,
            _ => {}
        }
        documents.push(report);
    }

    let finished_at = unix_now();
    let report = RegressionReport {
        suite_name: manifest.suite_name,
        description: manifest.description,
        manifest_path: manifest_path.display().to_string(),
        output_dir: out_dir.display().to_string(),
        started_at_unix: started_at,
        finished_at_unix: finished_at,
        totals,
        documents,
    };

    let report_json_path = out_dir.join("report.json");
    fs::write(&report_json_path, serde_json::to_vec_pretty(&report)?)?;

    let report_md_path = out_dir.join("summary.md");
    fs::write(&report_md_path, render_markdown(&report))?;

    let manifest_snapshot_path = out_dir.join("manifest.snapshot.json");
    fs::write(&manifest_snapshot_path, manifest_text)?;

    println!("Suite: {}", report.suite_name);
    if let Some(desc) = &report.description {
        println!("{}", desc);
    }
    println!(
        "Totals: {} passed, {} warned, {} failed, {} skipped",
        report.totals.passed, report.totals.warned, report.totals.failed, report.totals.skipped
    );
    println!("Report JSON: {}", report_json_path.display());
    println!("Report MD: {}", report_md_path.display());

    if report.totals.failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}

fn run_case(base_dir: &Path, case: &RegressionCase) -> DocumentReport {
    let fixture = match resolve_fixture_path(base_dir, &case.path) {
        Ok(fixture) => fixture,
        Err(err) => {
            return DocumentReport {
                id: case.id.clone(),
                label: case.label.clone(),
                group: case.group.clone(),
                path: case.path.clone(),
                resolved_path: None,
                status: "failed".to_string(),
                completion_note: "fixture resolution failed".to_string(),
                page_count: None,
                chunk_count: None,
                extracted_char_count: None,
                extracted_token_count: None,
                max_chunk_token_count: None,
                over_cap_chunk_count: None,
                garbage_chunk_count: None,
                duplicate_line_ratio: None,
                expected_min_pages: case.expected_min_pages,
                expected_min_chunks: case.expected_min_chunks,
                expected_min_chars: case.expected_min_chars,
                warnings: Vec::new(),
                error: Some(err.to_string()),
                notes: case.notes.clone(),
            };
        }
    };
    let resolved_path = fixture.path;

    if !resolved_path.exists() {
        return DocumentReport {
            id: case.id.clone(),
            label: case.label.clone(),
            group: case.group.clone(),
            path: case.path.clone(),
            resolved_path: Some(resolved_path.display().to_string()),
            status: "skipped".to_string(),
            completion_note: "fixture missing; skipped".to_string(),
            page_count: None,
            chunk_count: None,
            extracted_char_count: None,
            extracted_token_count: None,
            max_chunk_token_count: None,
            over_cap_chunk_count: None,
            garbage_chunk_count: None,
            duplicate_line_ratio: None,
            expected_min_pages: case.expected_min_pages,
            expected_min_chunks: case.expected_min_chunks,
            expected_min_chars: case.expected_min_chars,
            warnings: vec!["missing fixture".to_string()],
            error: None,
            notes: case.notes.clone(),
        };
    }

    if !fixture.is_pdf {
        return DocumentReport {
            id: case.id.clone(),
            label: case.label.clone(),
            group: case.group.clone(),
            path: case.path.clone(),
            resolved_path: Some(resolved_path.display().to_string()),
            status: "skipped".to_string(),
            completion_note: "fixture is not a valid PDF; skipped".to_string(),
            page_count: None,
            chunk_count: None,
            extracted_char_count: None,
            extracted_token_count: None,
            max_chunk_token_count: None,
            over_cap_chunk_count: None,
            garbage_chunk_count: None,
            duplicate_line_ratio: None,
            expected_min_pages: case.expected_min_pages,
            expected_min_chunks: case.expected_min_chunks,
            expected_min_chars: case.expected_min_chars,
            warnings: vec!["invalid pdf fixture".to_string()],
            error: None,
            notes: case.notes.clone(),
        };
    }

    let doc = match Document::load(&resolved_path) {
        Ok(doc) => doc,
        Err(err) => {
            return DocumentReport {
                id: case.id.clone(),
                label: case.label.clone(),
                group: case.group.clone(),
                path: case.path.clone(),
                resolved_path: Some(resolved_path.display().to_string()),
                status: "failed".to_string(),
                completion_note: "failed to load pdf".to_string(),
                page_count: None,
                chunk_count: None,
                extracted_char_count: None,
                extracted_token_count: None,
                max_chunk_token_count: None,
                over_cap_chunk_count: None,
                garbage_chunk_count: None,
                duplicate_line_ratio: None,
                expected_min_pages: case.expected_min_pages,
                expected_min_chunks: case.expected_min_chunks,
                expected_min_chars: case.expected_min_chars,
                warnings: Vec::new(),
                error: Some(err.to_string()),
                notes: case.notes.clone(),
            };
        }
    };
    let page_count = doc.get_pages().len();

    let mut lp = LAParams::default();
    lp.all_texts = case.all_texts.unwrap_or(false);
    lp.detect_vertical = case.detect_vertical.unwrap_or(false);

    let max_tokens = case.max_tokens.unwrap_or(500);
    let clean_text = case.clean_text.unwrap_or(true);
    let source_id = case.source_id.unwrap_or(1);
    let source_type = case.source_type.as_deref().unwrap_or("file");
    let mut warnings = fixture.notes;
    let ocr_handler = match resolve_ocr_config(case) {
        OcrResolution::Enabled(config) => match OcrHandler::new(&config) {
            Ok(handler) => Some(handler),
            Err(err) => {
                return DocumentReport {
                    id: case.id.clone(),
                    label: case.label.clone(),
                    group: case.group.clone(),
                    path: case.path.clone(),
                    resolved_path: Some(resolved_path.display().to_string()),
                    status: "skipped".to_string(),
                    completion_note: "ocr fixture skipped; OCR handler unavailable".to_string(),
                    page_count: Some(page_count),
                    chunk_count: None,
                    extracted_char_count: None,
                    extracted_token_count: None,
                    max_chunk_token_count: None,
                    over_cap_chunk_count: None,
                    garbage_chunk_count: None,
                    duplicate_line_ratio: None,
                    expected_min_pages: case.expected_min_pages,
                    expected_min_chunks: case.expected_min_chunks,
                    expected_min_chars: case.expected_min_chars,
                    warnings: vec![format!("failed to initialize OCR: {err}")],
                    error: None,
                    notes: case.notes.clone(),
                };
            }
        },
        OcrResolution::Disabled => None,
        OcrResolution::Unavailable(reason) => {
            return DocumentReport {
                id: case.id.clone(),
                label: case.label.clone(),
                group: case.group.clone(),
                path: case.path.clone(),
                resolved_path: Some(resolved_path.display().to_string()),
                status: "skipped".to_string(),
                completion_note: "ocr fixture skipped; OCR models unavailable".to_string(),
                page_count: Some(page_count),
                chunk_count: None,
                extracted_char_count: None,
                extracted_token_count: None,
                max_chunk_token_count: None,
                over_cap_chunk_count: None,
                garbage_chunk_count: None,
                duplicate_line_ratio: None,
                expected_min_pages: case.expected_min_pages,
                expected_min_chunks: case.expected_min_chunks,
                expected_min_chars: case.expected_min_chars,
                warnings: vec![reason],
                error: None,
                notes: case.notes.clone(),
            };
        }
    };

    match output_doc_new_schema(
        &doc,
        ocr_handler.as_ref(),
        Some(max_tokens),
        source_id,
        source_type,
        Some(&lp),
        clean_text,
    ) {
        Ok(chunks) => {
            let chunk_count = chunks.len();
            let extracted_char_count = chunks
                .iter()
                .map(|chunk| chunk.content_core.content.chars().count())
                .sum::<usize>();
            let tokenizer = tiktoken_rs::get_bpe_from_model("gpt-4o").expect("load tokenizer");
            let exact_token_counts = chunks
                .iter()
                .map(|chunk| tokenizer.encode_ordinary(&chunk.content_core.content).len())
                .collect::<Vec<_>>();
            let extracted_token_count = exact_token_counts.iter().sum::<usize>();
            let max_chunk_token_count = exact_token_counts.iter().copied().max().unwrap_or(0);
            let over_cap_chunk_count = exact_token_counts
                .iter()
                .filter(|count| **count > max_tokens)
                .count();
            let garbage_chunk_count = chunks
                .iter()
                .filter(|chunk| looks_like_garbage(&chunk.content_core.content))
                .count();
            let duplicate_line_ratio = compute_duplicate_line_ratio(&chunks);

            let mut status = "passed".to_string();
            let mut completion_note = format!(
                "extracted {} chunks / {} pages / {} chars / max_chunk_tokens={}",
                chunk_count, page_count, extracted_char_count, max_chunk_token_count
            );

            if let Some(expected_pages) = case.expected_min_pages {
                if page_count < expected_pages {
                    status = "failed".to_string();
                    completion_note = format!(
                        "page count {} below expected floor {}",
                        page_count, expected_pages
                    );
                    warnings.push("page floor not met".to_string());
                }
            }
            if status == "passed" {
                if let Some(expected_chunks) = case.expected_min_chunks {
                    if chunk_count < expected_chunks {
                        status = "failed".to_string();
                        completion_note = format!(
                            "chunk count {} below expected floor {}",
                            chunk_count, expected_chunks
                        );
                        warnings.push("chunk floor not met".to_string());
                    }
                }
            }
            if status == "passed" {
                if let Some(expected_chars) = case.expected_min_chars {
                    if extracted_char_count < expected_chars {
                        status = "failed".to_string();
                        completion_note = format!(
                            "char count {} below expected floor {}",
                            extracted_char_count, expected_chars
                        );
                        warnings.push("char floor not met".to_string());
                    }
                }
            }

            if status == "passed" {
                if over_cap_chunk_count > 0 {
                    status = "failed".to_string();
                    completion_note = format!(
                        "{} chunks exceeded hard cap {}",
                        over_cap_chunk_count, max_tokens
                    );
                    warnings.push("hard token cap violated".to_string());
                }
            }

            if status == "passed" {
                if page_count > 1 && chunk_count == 1 {
                    status = "warned".to_string();
                    completion_note = format!(
                        "single chunk on multi-page PDF ({} pages, {} chars)",
                        page_count, extracted_char_count
                    );
                    warnings.push("suspicious multi-page collapse".to_string());
                } else if page_count > 1 && extracted_char_count < 500 {
                    status = "warned".to_string();
                    completion_note = format!(
                        "low extraction volume for multi-page PDF ({} pages, {} chars)",
                        page_count, extracted_char_count
                    );
                    warnings.push("low extraction volume".to_string());
                } else if garbage_chunk_count > 0 {
                    status = "warned".to_string();
                    completion_note =
                        format!("garbage-like chunks detected ({})", garbage_chunk_count);
                    warnings.push("garbage text detected".to_string());
                }
            }

            DocumentReport {
                id: case.id.clone(),
                label: case.label.clone(),
                group: case.group.clone(),
                path: case.path.clone(),
                resolved_path: Some(resolved_path.display().to_string()),
                status,
                completion_note,
                page_count: Some(page_count),
                chunk_count: Some(chunk_count),
                extracted_char_count: Some(extracted_char_count),
                extracted_token_count: Some(extracted_token_count),
                max_chunk_token_count: Some(max_chunk_token_count),
                over_cap_chunk_count: Some(over_cap_chunk_count),
                garbage_chunk_count: Some(garbage_chunk_count),
                duplicate_line_ratio: Some(duplicate_line_ratio),
                expected_min_pages: case.expected_min_pages,
                expected_min_chunks: case.expected_min_chunks,
                expected_min_chars: case.expected_min_chars,
                warnings,
                error: None,
                notes: case.notes.clone(),
            }
        }
        Err(err) => DocumentReport {
            id: case.id.clone(),
            label: case.label.clone(),
            group: case.group.clone(),
            path: case.path.clone(),
            resolved_path: Some(resolved_path.display().to_string()),
            status: "failed".to_string(),
            completion_note: "failed during extraction".to_string(),
            page_count: Some(page_count),
            chunk_count: None,
            extracted_char_count: None,
            extracted_token_count: None,
            max_chunk_token_count: None,
            over_cap_chunk_count: None,
            garbage_chunk_count: None,
            duplicate_line_ratio: None,
            expected_min_pages: case.expected_min_pages,
            expected_min_chunks: case.expected_min_chunks,
            expected_min_chars: case.expected_min_chars,
            warnings: vec!["extraction error".to_string()],
            error: Some(err.to_string()),
            notes: case.notes.clone(),
        },
    }
}

fn resolve_case_path(base_dir: &Path, raw_path: &str) -> PathBuf {
    let path = Path::new(raw_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

#[derive(Debug)]
struct ResolvedFixture {
    path: PathBuf,
    is_pdf: bool,
    notes: Vec<String>,
}

fn resolve_fixture_path(base_dir: &Path, raw_path: &str) -> io::Result<ResolvedFixture> {
    let initial = resolve_case_path(base_dir, raw_path);
    let mut notes = Vec::new();

    if let Some(path) = resolve_fixture_candidate(&initial, &mut notes)? {
        let is_pdf = looks_like_pdf_file(&path)?;
        return Ok(ResolvedFixture {
            path,
            is_pdf,
            notes,
        });
    }

    let basename = initial
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(raw_path);
    let tests_link = Path::new("tests/docs").join(format!("{basename}.link"));
    if let Some(path) = resolve_fixture_candidate(&tests_link, &mut notes)? {
        let is_pdf = looks_like_pdf_file(&path)?;
        return Ok(ResolvedFixture {
            path,
            is_pdf,
            notes,
        });
    }

    let legacy_pdf = Path::new("benchmarks_legacy/pdfs").join(basename);
    if let Some(path) = resolve_fixture_candidate(&legacy_pdf, &mut notes)? {
        let is_pdf = looks_like_pdf_file(&path)?;
        return Ok(ResolvedFixture {
            path,
            is_pdf,
            notes,
        });
    }

    Ok(ResolvedFixture {
        path: initial,
        is_pdf: false,
        notes,
    })
}

fn resolve_fixture_candidate(path: &Path, notes: &mut Vec<String>) -> io::Result<Option<PathBuf>> {
    if path.exists() {
        if path.extension().and_then(|ext| ext.to_str()) == Some("link") {
            let downloaded = ensure_linked_pdf_cached(path)?;
            if looks_like_pdf_file(&downloaded)? {
                notes.push(format!(
                    "resolved linked fixture {}",
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("unknown")
                ));
                return Ok(Some(downloaded));
            }
            notes.push(format!(
                "linked fixture {} did not resolve to a valid PDF",
                path.display()
            ));
            return Ok(None);
        }

        if looks_like_pdf_file(path)? {
            return Ok(Some(path.to_path_buf()));
        }

        if let Some(linked) = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| Path::new("tests/docs").join(format!("{name}.link")))
            .filter(|linked| linked.exists())
        {
            let downloaded = ensure_linked_pdf_cached(&linked)?;
            if looks_like_pdf_file(&downloaded)? {
                notes.push(format!(
                    "replaced invalid fixture {} with linked source {}",
                    path.display(),
                    linked.display()
                ));
                return Ok(Some(downloaded));
            }
            notes.push(format!(
                "linked fallback {} was not a valid PDF",
                linked.display()
            ));
        }

        return Ok(Some(path.to_path_buf()));
    }

    let linked_path = PathBuf::from(format!("{}.link", path.display()));
    if linked_path.exists() {
        let downloaded = ensure_linked_pdf_cached(&linked_path)?;
        if looks_like_pdf_file(&downloaded)? {
            notes.push(format!("resolved linked fixture {}", linked_path.display()));
            return Ok(Some(downloaded));
        }
        notes.push(format!(
            "linked fixture {} did not resolve to a valid PDF",
            linked_path.display()
        ));
    }

    Ok(None)
}

fn ensure_linked_pdf_cached(link_path: &Path) -> io::Result<PathBuf> {
    let docs_cache = Path::new("tests/docs_cache");
    fs::create_dir_all(docs_cache)?;

    let target_name = link_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid link fixture name"))?
        .trim_end_matches(".link")
        .to_string();
    let cached_path = docs_cache.join(target_name);
    if cached_path.exists() && looks_like_pdf_file(&cached_path)? {
        return Ok(cached_path);
    }

    let url = fs::read_to_string(link_path)?.trim().to_string();
    let response = ureq::get(&url)
        .call()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, format!("download failed: {err}")))?;
    let mut reader = response.into_reader();
    let mut file = fs::File::create(&cached_path)?;
    std::io::copy(&mut reader, &mut file)?;
    Ok(cached_path)
}

fn looks_like_pdf_file(path: &Path) -> io::Result<bool> {
    let file = fs::read(path)?;
    Ok(file.windows(4).take(1024).any(|window| window == b"%PDF"))
}

enum OcrResolution {
    Enabled(OcrConfig),
    Disabled,
    Unavailable(String),
}

fn resolve_ocr_config(case: &RegressionCase) -> OcrResolution {
    if !case.use_ocr.unwrap_or(false) {
        return OcrResolution::Disabled;
    }

    let detection_model = case
        .ocr_detection_model
        .clone()
        .or_else(|| env::var("PDF_EXTRACT_OCR_DETECTION_MODEL").ok())
        .unwrap_or_else(|| "models/text-detection.rten".to_string());
    let recognition_model = case
        .ocr_recognition_model
        .clone()
        .or_else(|| env::var("PDF_EXTRACT_OCR_RECOGNITION_MODEL").ok())
        .unwrap_or_else(|| "models/text-recognition.rten".to_string());

    if !Path::new(&detection_model).exists() || !Path::new(&recognition_model).exists() {
        return OcrResolution::Unavailable(format!(
            "missing OCR models: detection={} recognition={}",
            detection_model, recognition_model
        ));
    }

    OcrResolution::Enabled(OcrConfig {
        detection_model: Some(detection_model),
        recognition_model: Some(recognition_model),
    })
}

fn default_output_dir() -> PathBuf {
    let stamp = unix_now();
    PathBuf::from(format!("eval/runs/regression/run_{}", stamp))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|idx| args.get(idx + 1))
        .cloned()
}

fn render_markdown(report: &RegressionReport) -> String {
    let mut md = String::new();
    md.push_str(&format!("# {}\n\n", report.suite_name));
    if let Some(desc) = &report.description {
        md.push_str(desc);
        md.push_str("\n\n");
    }
    md.push_str(&format!(
        "- Manifest: `{}`\n- Output dir: `{}`\n- Totals: {} passed, {} warned, {} failed, {} skipped\n\n",
        report.manifest_path,
        report.output_dir,
        report.totals.passed,
        report.totals.warned,
        report.totals.failed,
        report.totals.skipped
    ));
    md.push_str("| Status | Group | Document | Pages | Chunks | Chars | Max Tokens | Over Cap | Garbage | Dup Line Ratio | Note |\n");
    md.push_str("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for doc in &report.documents {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            doc.status,
            doc.group.as_deref().unwrap_or("-"),
            escape_md(&doc.label),
            doc.page_count
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
            doc.chunk_count
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
            doc.extracted_char_count
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
            doc.max_chunk_token_count
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
            doc.over_cap_chunk_count
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
            doc.garbage_chunk_count
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
            doc.duplicate_line_ratio
                .map(|v| format!("{v:.3}"))
                .unwrap_or_else(|| "-".to_string()),
            escape_md(&doc.completion_note)
        ));
    }
    md
}

fn escape_md(input: &str) -> String {
    input.replace('|', "\\|").replace('\n', " ")
}

fn compute_duplicate_line_ratio(chunks: &[pdf_extract::ExtractionResult]) -> f64 {
    use std::collections::HashMap;

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut total = 0usize;
    for chunk in chunks {
        for line in chunk.content_core.content.lines() {
            let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
            if normalized.len() < 20 {
                continue;
            }
            total += 1;
            *counts.entry(normalized).or_insert(0) += 1;
        }
    }

    if total == 0 {
        return 0.0;
    }

    let duplicated = counts
        .values()
        .filter(|count| **count > 1)
        .map(|count| count - 1)
        .sum::<usize>();
    duplicated as f64 / total as f64
}

fn looks_like_garbage(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 40 {
        return false;
    }

    let alpha = trimmed.chars().filter(|c| c.is_alphabetic()).count();
    let digits = trimmed.chars().filter(|c| c.is_ascii_digit()).count();
    let symbols = trimmed
        .chars()
        .filter(|c| !c.is_alphanumeric() && !c.is_whitespace())
        .count();
    let total = trimmed.chars().count().max(1);

    let alpha_ratio = alpha as f64 / total as f64;
    let symbol_ratio = symbols as f64 / total as f64;
    let digit_ratio = digits as f64 / total as f64;

    alpha_ratio < 0.25 && symbol_ratio > 0.35 && digit_ratio < 0.35
}
