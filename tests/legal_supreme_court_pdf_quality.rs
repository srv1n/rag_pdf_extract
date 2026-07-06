use pdf_extract::{assess_parse_quality, parse_pdf, LAParams, ParseQualityStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct Manifest {
    base_dir: String,
    documents: Vec<ManifestDocument>,
}

#[derive(Debug, Deserialize)]
struct ManifestDocument {
    id: String,
    label: String,
    path: String,
    sha256: String,
    size_bytes: u64,
    expected_min_pages: usize,
    expected_min_chars: usize,
    expected_min_chunks: usize,
    expected_quality_status: String,
}

#[derive(Debug, Serialize)]
struct Diagnostic {
    id: String,
    label: String,
    path: String,
    sha256: String,
    parser_layout_mode: String,
    layout_fallback_policy: String,
    page_count: usize,
    extracted_chars: usize,
    normalized_chars: usize,
    chunk_count: usize,
    chunks_with_location: usize,
    location_coverage_ratio: f64,
    repeated_line_ratio: f64,
    unique_token_ratio: f64,
    top_repeated_lines: Vec<pdf_extract::RepeatedLine>,
    quality_status: ParseQualityStatus,
    quality_score: f64,
}

#[test]
fn legal_supreme_court_pdf_quality() {
    let manifest_path = Path::new("eval/manifests/local_supreme_court_pdf_quality.json");
    let manifest_text = fs::read_to_string(manifest_path).expect("read local manifest");
    let manifest: Manifest = serde_json::from_str(&manifest_text).expect("parse local manifest");
    let base_dir = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&manifest.base_dir);
    let paths = manifest
        .documents
        .iter()
        .map(|doc| base_dir.join(&doc.path))
        .collect::<Vec<_>>();
    let present = paths.iter().filter(|path| path.exists()).count();

    if present == 0 {
        eprintln!(
            "Skipping local Supreme Court PDF quality regression: ignored artifact PDFs are absent"
        );
        return;
    }

    assert_eq!(
        present,
        paths.len(),
        "partial local Supreme Court PDF artifact bundle; expected all manifest PDFs"
    );

    let mut diagnostics = Vec::new();
    for (case, path) in manifest.documents.iter().zip(paths.iter()) {
        verify_file(path, case);

        let pdf = lopdf::Document::load(path).expect("load pdf for page count");
        let page_count = pdf.get_pages().len();
        let docs = parse_pdf(
            path.to_str().expect("utf-8 pdf path"),
            1,
            "file",
            None,
            None,
            None,
            Some(500),
            Some(LAParams::product_layout()),
            Some(true),
            Default::default(),
        )
        .unwrap_or_else(|err| panic!("parse {}: {}", case.id, err));
        let metrics = assess_parse_quality(&docs);

        assert!(
            page_count >= case.expected_min_pages,
            "{} page_count {} below floor {}",
            case.id,
            page_count,
            case.expected_min_pages
        );
        assert!(
            metrics.normalized_chars >= case.expected_min_chars,
            "{} normalized_chars {} below floor {}",
            case.id,
            metrics.normalized_chars,
            case.expected_min_chars
        );
        assert!(
            metrics.chunk_count >= case.expected_min_chunks,
            "{} chunk_count {} below floor {}",
            case.id,
            metrics.chunk_count,
            case.expected_min_chunks
        );
        assert_eq!(
            quality_status_name(&metrics.status),
            case.expected_quality_status,
            "{} quality status regressed",
            case.id
        );
        assert_ne!(
            metrics.status,
            ParseQualityStatus::MostlyBoilerplate,
            "{} must not be classified as mostly_boilerplate",
            case.id
        );

        diagnostics.push(Diagnostic {
            id: case.id.clone(),
            label: case.label.clone(),
            path: case.path.clone(),
            sha256: case.sha256.clone(),
            parser_layout_mode: "parse_pdf + LAParams::product_layout".to_string(),
            layout_fallback_policy: "on_suspicious_volume".to_string(),
            page_count,
            extracted_chars: metrics.extracted_chars,
            normalized_chars: metrics.normalized_chars,
            chunk_count: metrics.chunk_count,
            chunks_with_location: metrics.chunks_with_location,
            location_coverage_ratio: metrics.location_coverage_ratio,
            repeated_line_ratio: metrics.repeated_line_ratio,
            unique_token_ratio: metrics.unique_token_ratio,
            top_repeated_lines: metrics.top_repeated_lines,
            quality_status: metrics.status,
            quality_score: metrics.score,
        });
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&diagnostics).expect("serialize diagnostics")
    );
}

fn verify_file(path: &Path, case: &ManifestDocument) {
    let metadata = fs::metadata(path).unwrap_or_else(|err| panic!("metadata {}: {}", case.id, err));
    assert_eq!(
        metadata.len(),
        case.size_bytes,
        "{} size changed for {}",
        case.id,
        display(path)
    );
    let bytes = fs::read(path).unwrap_or_else(|err| panic!("read {}: {}", case.id, err));
    let hash = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(hash, case.sha256, "{} sha256 mismatch", case.id);
}

fn quality_status_name(status: &ParseQualityStatus) -> &'static str {
    match status {
        ParseQualityStatus::Usable => "usable",
        ParseQualityStatus::Empty => "empty",
        ParseQualityStatus::TooShort => "too_short",
        ParseQualityStatus::MostlyBoilerplate => "mostly_boilerplate",
        ParseQualityStatus::LikelyGarbled => "likely_garbled",
    }
}

fn display(path: &Path) -> String {
    path.display().to_string()
}
