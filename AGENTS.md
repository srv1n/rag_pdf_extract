# Repository Guidelines

## Project Structure & Module Organization
- `src/` — Rust library crate; core modules in `lib.rs`, with helpers like `chunk_accumulator.rs`, `text_splitting.rs`, `heading_hierarchy.rs`, and PDF/OCR utilities.
- `examples/` — runnable examples: `extract.rs` (general extraction) and `ocr_extract.rs` (OCR).
- `tests/` — integration tests in `tests.rs` and fixtures under `tests/docs/` (downloads cached in `tests/docs_cache/`).
- `docs/` — technical design (see `TEXT_EXTRACTION_ARCHITECTURE.md`).
- `models/` — place OCRS `.rten` models here (not tracked).
- `target/` — build output (ignored). Root `*.pdf` and `*.txt` files are ignored by `.gitignore`.

## Build, Test, and Development Commands
- Build library and examples: `cargo build --release --examples`
- Run examples:
  - `cargo run --example extract -- 10.pdf 500`
  - `RUST_LOG=info cargo run --example ocr_extract`
- With OCR models: `cargo run --example extract -- 10.pdf 500 --ocr models/text-detection.rten models/text-recognition.rten`
- Tests: `cargo test -q` (show output with `cargo test -- --nocapture`)
- Lint/format: `cargo clippy --all-targets --all-features -D warnings && cargo fmt --all`

## Coding Style & Naming Conventions
- Rust 2018 edition; 4-space indentation.
- Names: `snake_case` for functions/modules, `CamelCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- Errors: prefer `anyhow` for app-level, `thiserror` for library errors. Log via `log`/`env_logger` (use `RUST_LOG`).

## Testing Guidelines
- Put integration tests in `tests/`. Keep fixtures small; prefer `.pdf.link` with cache in `tests/docs_cache/`.
- Ensure deterministic expectations; avoid network in unit tests unless guarded like current `.link` pattern.
- Validate token limits and page metadata where relevant.

## Commit & Pull Request Guidelines
- Commits: concise, imperative (“Fix token limit overflow”), group related changes. Reference issues when applicable.
- PRs: clear description, steps to reproduce/verify, before/after notes, and updated docs/tests. CI must pass (`cargo build/test` and semver checks in `.github/workflows/rust.yml`).

## Security & Configuration Tips
- Do not commit large PDFs or model files. Download OCRS models:
  - `curl -L https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.rten -o models/text-detection.rten`
  - `curl -L https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.rten -o models/text-recognition.rten`
- Review `docs/TEXT_EXTRACTION_ARCHITECTURE.md` before major changes.

