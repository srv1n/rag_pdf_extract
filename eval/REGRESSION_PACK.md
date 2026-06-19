# PDF Extraction Regression Pack

This pack is the small, repeatable regression loop for extraction quality.
It covers stable repo fixtures and the current founder/legal PDFs that have
shown collapse or suspiciously low extraction.

## What it records

For each PDF the runner records:

- extracted character count
- page count
- chunk count
- exact token count
- max emitted chunk token count
- over-cap chunk count
- garbage-like chunk count
- duplicate-line ratio
- completion notes
- pass / warn / fail / skip status

## Current manifest

The default manifest lives at:

- [`eval/regressions/founder_legal_pack.json`](./regressions/founder_legal_pack.json)
- [`eval/regressions/archetype_benchmark_01.json`](./regressions/archetype_benchmark_01.json)

It mixes:

- repo fixtures that should stay boring
- founder/legal PDFs that should not regress
- broader archetypes that catch layout/pathology classes before product QA does

## Run it

From the crate root:

```bash
cargo run --release --example regression_pack -- \
  --manifest eval/regressions/founder_legal_pack.json \
  --out-dir eval/runs/regression

cargo run --release --example regression_pack -- \
  --manifest eval/regressions/archetype_benchmark_01.json \
  --out-dir eval/runs/archetype_benchmark/current
```

If you omit `--out-dir`, the runner writes into a timestamped directory under
`eval/runs/regression/`.

## Outputs

Each run writes:

- `report.json` - machine-readable summary
- `summary.md` - human-readable table
- `manifest.snapshot.json` - the exact manifest used for the run

## How to extend it

Add a new entry to the manifest with:

- `id`
- `label`
- `path`
- `group`
- minimum floors for `expected_min_chars` and `expected_min_chunks`
- `use_ocr: true` only when the fixture is supposed to exercise the OCR path

Fixture resolution is deliberate, not magical:

- if `path` is a real PDF, the runner uses it
- if `path` resolves to a bad local placeholder, the runner will fall back to a matching `tests/docs/<name>.pdf.link` fixture when one exists
- linked fixtures are cached under `tests/docs_cache/`
- non-PDF placeholders get `skipped`, not misreported as extractor failures

OCR cases are also explicit:

- `use_ocr: true` tells the runner to initialize OCR with `models/text-detection.rten` and `models/text-recognition.rten` unless the manifest overrides those paths
- if those models are missing, the case is `skipped`
- if `use_ocr` is absent or `false`, the runner never silently enables OCR

Use `group` as the PDF archetype name. That field is the benchmark taxonomy, not decoration.

If a local founder PDF is missing on your machine, the runner marks it as
`skipped` instead of pretending the suite failed.

## Strong Take

The runner is only useful if it measures contract violations, not just output volume.

```text
chars alone can lie
chunk counts alone can lie
exact over-cap counts do not lie
```
