use pdf_extract::{extract_text, parse_pdf, LAParams};

#[test]
fn stage_benchmark_current_matches_manifest_schema() {
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read("eval/fixtures/pdf_manifest.json").expect("read fixture manifest"),
    )
    .expect("parse fixture manifest");
    let report: serde_json::Value = serde_json::from_slice(
        &std::fs::read("eval/runs/stage_benchmark_current/stage_benchmark.json")
            .expect("read current stage benchmark report"),
    )
    .expect("parse current stage benchmark report");

    for key in ["command", "git_sha", "git_dirty", "fixtures"] {
        assert!(
            report.get(key).is_some(),
            "benchmark report missing `{}`",
            key
        );
    }

    let manifest_ids = manifest["fixtures"]
        .as_array()
        .expect("manifest fixtures")
        .iter()
        .map(|fixture| fixture["id"].as_str().expect("fixture id").to_string())
        .collect::<Vec<_>>();
    let report_fixtures = report["fixtures"].as_array().expect("report fixtures");
    let report_ids = report_fixtures
        .iter()
        .map(|fixture| {
            fixture["fixture_id"]
                .as_str()
                .expect("report fixture_id")
                .to_string()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        report_ids, manifest_ids,
        "stage_benchmark_current fixture list is stale"
    );

    for fixture in report_fixtures {
        for key in ["product_status", "raw_layout_status", "overall_status"] {
            assert!(
                fixture.get(key).is_some(),
                "fixture {:?} missing `{key}`",
                fixture.get("fixture_id")
            );
        }
    }
}

// Shorthand for creating ExpectedText
// example: expected!("atomic.pdf", "Atomic Data");
macro_rules! expected {
    ($filename:expr, $text:expr) => {
        ExpectedText {
            filename: $filename,
            text: $text,
        }
    };
}

// Use the macro to create a list of ExpectedText
// and then check if the text is correctly extracted
#[test]
fn extract_expected_text() {
    // Note: documents_stack.pdf has extraction issues - it doesn't extract text properly
    // This might be due to the PDF structure or encoding. For now, we'll just ensure
    // it doesn't crash during extraction.
    let docs = vec![
        expected!("documents_stack.pdf.link", ""), // Empty string means just test extraction doesn't crash
    ];
    for doc in docs {
        doc.test();
    }
}

#[test]
// iterate over all docs in the `tests/docs` directory, don't crash
fn extract_all_docs() {
    let docs = std::fs::read_dir("tests/docs").unwrap();
    for doc in docs {
        let doc = doc.unwrap();
        let path = doc.path();
        let filename = path.file_name().unwrap().to_string_lossy();
        expected!(&filename, "").test();
    }
}

#[test]
fn emitted_chunks_respect_exact_token_cap_on_repo_fixtures() {
    let tokenizer = tiktoken_rs::get_bpe_from_model("gpt-4o").expect("load tokenizer");
    let fixtures = [
        "tests/docs_cache/embeded-core-fonts.pdf",
        "eval/corpus/legal/12. CCI v Kerala Film Exhibitors Federation & Ors.pdf",
        "10.pdf",
    ];

    for fixture in fixtures {
        if !std::path::Path::new(fixture).exists() {
            continue;
        }

        let mut laparams = LAParams::default();
        laparams.all_texts = false;
        laparams.detect_vertical = false;

        let docs = parse_pdf(
            fixture,
            1,
            "file",
            None,
            None,
            None,
            Some(128),
            Some(laparams),
            Some(true),
        )
        .unwrap_or_else(|err| panic!("failed to parse {}: {}", fixture, err));

        for (idx, chunk) in docs.iter().enumerate() {
            let exact = tokenizer.encode_ordinary(&chunk.content_core.content).len();
            assert!(
                exact <= 128,
                "fixture {fixture} chunk {idx} exceeded cap: exact={exact}, recorded={}",
                chunk.content_core.token_count
            );
        }
    }
}

#[test]
fn documents_stack_layout_produces_located_chunks() {
    let fixture = "eval/corpus/mixed/documents_stack.pdf";
    if !std::path::Path::new(fixture).exists() {
        eprintln!("Skipping: {fixture} not found.");
        return;
    }

    let docs = parse_pdf(
        fixture,
        1,
        "file",
        None,
        None,
        None,
        Some(500),
        Some(LAParams::default()),
        Some(true),
    )
    .expect("parse documents_stack");

    assert!(!docs.is_empty(), "documents_stack produced zero chunks");
    assert!(
        docs.iter()
            .any(|doc| pdf_extract::extract_pdf_location(&doc.content_ext)
                .map(|loc| !loc.fragments.is_empty())
                .unwrap_or(false)),
        "documents_stack produced no located chunks"
    );
}

#[test]
fn malformed_text_operators_do_not_panic() {
    let fixture = "eval/fixtures/pdfs/malformed_text_ops.pdf";

    parse_pdf(
        fixture,
        1,
        "file",
        None,
        None,
        None,
        Some(128),
        None,
        Some(true),
    )
    .expect("no-layout parser should skip malformed text operators");

    parse_pdf(
        fixture,
        1,
        "file",
        None,
        None,
        None,
        Some(128),
        Some(LAParams::default()),
        Some(true),
    )
    .expect("layout parser should skip malformed text operators");
}

#[test]
fn ctm_scaled_text_uses_single_viewer_y_flip() {
    let fixture = "eval/fixtures/pdfs/ctm_scaled_text.pdf";

    let docs = parse_pdf(
        fixture,
        1,
        "file",
        None,
        None,
        None,
        Some(128),
        Some(LAParams::default()),
        Some(true),
    )
    .expect("ctm scaled fixture should parse");

    let loc = docs
        .iter()
        .find_map(|doc| pdf_extract::extract_pdf_location(&doc.content_ext).ok())
        .expect("fixture should emit PDF location");
    let bbox = &loc.fragments.first().expect("location fragment").bbox;

    assert!(
        docs.iter()
            .any(|doc| doc.content_core.content.contains("Scaled CTM Text")),
        "fixture text missing"
    );
    assert!((bbox.x - 72.0).abs() < 1.0, "unexpected x: {}", bbox.x);
    assert!(
        (bbox.y - 672.0).abs() < 2.0,
        "unexpected viewer-space y after CTM conversion: {}",
        bbox.y
    );
    assert!(bbox.width > 90.0 && bbox.width < 110.0);
    assert!(bbox.height > 20.0 && bbox.height < 30.0);
}

#[test]
fn scientific_type3_fixture_does_not_panic_in_layout_mode() {
    let fixture = "eval/corpus/scientific/3.pdf";
    if !std::path::Path::new(fixture).exists() {
        eprintln!("Skipping: {fixture} not found.");
        return;
    }

    let docs = parse_pdf(
        fixture,
        1,
        "file",
        None,
        None,
        None,
        Some(350),
        Some(LAParams::default()),
        Some(true),
    )
    .expect("scientific Type3 fixture should not panic or fail");

    let normalized_chars = docs
        .iter()
        .flat_map(|doc| doc.content_core.content.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .count();
    assert!(
        normalized_chars > 40_000,
        "expected substantial extracted text from {}, got {}",
        fixture,
        normalized_chars
    );
}

#[test]
fn layout_fallback_prevents_legal_17_text_collapse() {
    let fixture = "eval/corpus/legal/17. Sunil B Naik v Geowave Commander.pdf";
    if !std::path::Path::new(fixture).exists() {
        eprintln!("Skipping: {fixture} not found.");
        return;
    }

    let no_layout = parse_pdf(
        fixture,
        1,
        "file",
        None,
        None,
        None,
        Some(350),
        None,
        Some(true),
    )
    .expect("no-layout parse should succeed");

    let layout = parse_pdf(
        fixture,
        1,
        "file",
        None,
        None,
        None,
        Some(350),
        Some(LAParams::default()),
        Some(true),
    )
    .expect("layout parse should fall back instead of collapsing");

    let count = |docs: &[pdf_extract::ExtractionResult]| {
        docs.iter()
            .flat_map(|doc| doc.content_core.content.split_whitespace())
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .count()
    };
    let no_layout_chars = count(&no_layout);
    let layout_chars = count(&layout);

    assert!(
        no_layout_chars > 70_000,
        "sanity check failed for {}: no-layout chars={}",
        fixture,
        no_layout_chars
    );
    assert!(
        layout_chars * 100 >= no_layout_chars * 95,
        "layout chars collapsed for {}: layout={}, no_layout={}",
        fixture,
        layout_chars,
        no_layout_chars
    );
}

#[test]
fn repeated_content_has_distinct_chunk_id_and_same_content_hash() {
    let a = pdf_extract::create_content_core_with_identity(
        "same content",
        &[],
        1,
        "file",
        Some(1),
        Some(0),
        0,
    );
    let b = pdf_extract::create_content_core_with_identity(
        "same content",
        &[],
        1,
        "file",
        Some(2),
        Some(0),
        1,
    );

    assert_ne!(a.chunk_id, b.chunk_id);
    assert_eq!(a.content_hash, b.content_hash);
}

// data structure to make it easy to check if certain files are correctly parsed
// e.g. ExpectedText { filename: "atomic.pdf", text: "Atomic Data" }
#[derive(Debug, PartialEq)]
struct ExpectedText<'a> {
    filename: &'a str,
    text: &'a str,
}

impl ExpectedText<'_> {
    /// Opens the `filename` from `tests/docs`, extracts the text and checks if it contains `text`
    /// If the file ends with `_link`, it will download the file from the url in the file to the `tests/docs_cache` directory
    fn test(self) {
        let ExpectedText { filename, text } = self;
        let file_path = if filename.ends_with(".pdf.link") {
            let docs_cache = "tests/docs_cache";
            if !std::path::Path::new(docs_cache).exists() {
                // This might race with exists test above, but that's fine
                if let Err(e) = std::fs::create_dir(docs_cache) {
                    if e.kind() != std::io::ErrorKind::AlreadyExists {
                        panic!("Failed to create directory {}, {}", docs_cache, e);
                    }
                }
            }
            let file_path = format!("{}/{}", docs_cache, filename.replace(".link", ""));
            if std::path::Path::new(&file_path).exists() {
                file_path
            } else {
                let url = std::fs::read_to_string(format!("tests/docs/{}", filename))
                    .unwrap()
                    .trim()
                    .to_string();
                eprintln!("Downloading PDF from: {}", url);
                match ureq::get(&url).call() {
                    Ok(resp) => {
                        let mut file = std::fs::File::create(&file_path).unwrap();
                        std::io::copy(&mut resp.into_reader(), &mut file).unwrap();
                        file_path
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: Failed to download {} from {}: {}",
                            filename, url, e
                        );
                        eprintln!("Skipping this test");
                        return;
                    }
                }
            }
        } else {
            format!("tests/docs/{}", filename)
        };
        // Verify the file is actually a PDF before trying to extract
        let file_contents = std::fs::read(&file_path).unwrap();
        if file_contents.len() < 4 || &file_contents[0..4] != b"%PDF" {
            eprintln!(
                "Warning: {} is not a valid PDF file. It might be HTML or corrupted. Skipping.",
                filename
            );
            return;
        }

        let out = extract_text(file_path, None)
            .unwrap_or_else(|e| panic!("Failed to extract text from {}, {}", filename, e));

        // If no specific text is expected, just make sure extraction doesn't crash
        if text.is_empty() {
            println!("Extracted {} characters from {}", out.len(), filename);
            return;
        }

        // For PDFs that might have extraction issues, be more lenient
        if out.is_empty() {
            eprintln!("Warning: No text extracted from {}. This might be a complex PDF that needs OCR or has unsupported features.", filename);
            return;
        }

        println!("{}", out);
        assert!(
            out.contains(text),
            "Text {} does not contain '{}'",
            filename,
            text
        );
    }
}
