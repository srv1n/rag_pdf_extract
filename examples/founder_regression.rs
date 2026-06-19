use pdf_extract::founder_regression::{render_markdown_summary, run_founder_regression};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let args: Vec<String> = env::args().collect();
    let manifest_path = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("eval/manifests/founder_regression_01.json"));

    let report = match run_founder_regression(&manifest_path) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("founder regression failed: {}", err);
            process::exit(1);
        }
    };

    let run_dir = prepare_run_dir();
    let json_path = run_dir.join("founder_regression.json");
    let md_path = run_dir.join("founder_regression.md");
    let latest_json = Path::new("eval/runs/latest_founder_regression.json");
    let latest_md = Path::new("eval/runs/latest_founder_regression.md");

    let json = serde_json::to_string_pretty(&report).expect("serialize report");
    let markdown = render_markdown_summary(&report);

    fs::write(&json_path, &json).expect("write report json");
    fs::write(&md_path, &markdown).expect("write report markdown");
    fs::write(latest_json, &json).expect("write latest json");
    fs::write(latest_md, &markdown).expect("write latest markdown");

    println!("Founder regression report written to {}", run_dir.display());
    println!(
        "Summary: passed={}, warnings={}, hard_failures={}",
        report.passed, report.warnings, report.hard_failures
    );
    println!();
    println!("{}", markdown);

    if !report.overall_ok() {
        process::exit(2);
    }
}

fn prepare_run_dir() -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let dir = Path::new("eval/runs").join(format!("founder_regression_{}", ts));
    fs::create_dir_all(&dir).expect("create run dir");
    dir
}
