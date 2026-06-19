use pdf_extract::founder_regression::{load_manifest, resolve_corpus_root, run_founder_regression};
use std::path::Path;

#[test]
fn founder_regression_hard_cases_hold() {
    let manifest_path = Path::new("eval/manifests/founder_regression_01.json");
    let manifest = load_manifest(manifest_path).expect("load founder manifest");
    let corpus_root = resolve_corpus_root(&manifest);

    if !corpus_root.exists() {
        eprintln!(
            "Skipping founder regression: corpus root not found at {}",
            corpus_root.display()
        );
        return;
    }

    let report = run_founder_regression(manifest_path).expect("run founder regression");
    let hard_failures = report
        .cases
        .iter()
        .filter(|case| {
            matches!(
                case.severity,
                pdf_extract::founder_regression::RegressionSeverity::Hard
            ) && !matches!(
                case.status,
                pdf_extract::founder_regression::RegressionStatus::Passed
            )
        })
        .map(|case| format!("{} => {:?}", case.id, case.failure_reasons))
        .collect::<Vec<_>>();

    assert!(
        hard_failures.is_empty(),
        "hard founder regression failures: {}",
        hard_failures.join(" | ")
    );
}
