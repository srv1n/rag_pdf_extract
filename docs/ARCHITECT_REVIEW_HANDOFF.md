# PDF Parser Review Handoff

Date: 2026-05-28

## Scope

Implemented the review pack under `artifacts/pdf_parser_review_tasks` as production code plus gates. The implementation keeps the existing `ContentCore` / `ContentExt` schema path and current lopdf extraction path, while avoiding an experimental backend-selection surface.

## Implemented

| Task | Status | Evidence |
| --- | --- | --- |
| 01 eval harness | Done | `examples/stage_benchmark.rs`, `eval/fixtures/pdf_manifest.json`, `eval/runs/stage_benchmark_current/` |
| 02 header/footer | Done | `src/document/header_footer.rs`; unit tests cover one-page, two-page, repeated header, variance |
| 03 visibility policy | Done | `src/document/visibility.rs`; layout and no-layout use the same policy path |
| 04 Form XObject CTM | Done | `StreamContext` + explicit child initial CTM in `src/lib.rs`; recursion depth guard |
| 05 vertical dedup | Done | vertical grouping gated by page-level signal and overlap suppression in `src/lib.rs` |
| 06 location fidelity | Done for current backend | heading locations use heading source segments; table markdown keeps source ranges; chunk split locations are subset; grouped fragments default; `content_hash` split from source-stable `chunk_id` |
| 07 backend spike | Removed from ship branch | Experimental backend-selection API and native-runtime dependency were cut to keep the crate surface coherent |
| 08 heading/context | Done for known bug | pending headings preserve their own source locations; context remains distinguishable through location metadata notes |
| 09 tables/columns/order | Done for current backend | table cells retain source char ranges; same-y sort includes x; benchmark emits table/location-sensitive metrics |
| 10 packaging/license | Done as package contract | OCR remains feature-gated via `ocr` / `ocr-ocrs`; legacy `ocr-tesseract` is a compatibility alias only; no native parser runtime is required by default |
| 11 acceptance gates | Done | `cargo test`; `stage_benchmark`; founder regression |

## Validation

```text
cargo fmt
cargo check --all-targets
  PASS

cargo run --example stage_benchmark -- --manifest eval/fixtures/pdf_manifest.json --out-dir eval/runs/stage_benchmark_current
  PASS: product_passed=10/10, raw_layout_passed=10/10, fallback_used=0/10

cargo run --example stage_benchmark -- --manifest eval/manifests/production_broad_corpus.json --out-dir eval/runs/production_broad_corpus
  PASS: product_passed=9/9, raw_layout_passed=9/9, fallback_used=1/9

cargo run --example founder_regression
  PASS: passed=3, warnings=0, hard_failures=0

cargo test
  PASS: 70 passed, 6 suites

./scripts/check_warning_budget.sh
  PASS: warning budget ok: 0 <= 64
```

## Review Notes

- The current extractor still has segment-level source spans for normal text. `span_map` keeps the span model local to the current pipeline without exposing a backend-selection API.
- `all_texts` remains fixture/workload-specific. Founder legal regression uses it because that corpus needs Form XObject text; general examples and the stage harness default to safer settings unless a fixture opts in.
