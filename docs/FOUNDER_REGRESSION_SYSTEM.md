# Founder PDF Regression System

This is the local guardrail for the PDFs that keep kicking us in the teeth.

## Goal

Keep a small, high-signal founder/legal pack that tells us when extraction quality regresses, without pretending every PDF bug needs a giant benchmark first.

## What It Covers

- hard regression cases:
  - `Puttaswamy`
  - `Energy Watchdog`
- warn-only watch case:
  - `Vishaka`

The warn-only lane is deliberate. `Vishaka` is currently suspicious, and lying with a green check would be stupid.

## Files

- manifest:
  - `/Users/sarav/Downloads/side/rzn/rag_pdf_extract/eval/manifests/founder_regression_01.json`
- broader mixed regression pack:
  - `/Users/sarav/Downloads/side/rzn/rag_pdf_extract/eval/regressions/founder_legal_pack.json`
  - `/Users/sarav/Downloads/side/rzn/rag_pdf_extract/eval/REGRESSION_PACK.md`
- runner:
  - `/Users/sarav/Downloads/side/rzn/rag_pdf_extract/examples/founder_regression.rs`
- local regression test:
  - `/Users/sarav/Downloads/side/rzn/rag_pdf_extract/tests/founder_regression.rs`

## Run It

```bash
cd /Users/sarav/Downloads/side/rzn/rag_pdf_extract
cargo run --example founder_regression
```

Optional corpus root override:

```bash
RAG_PDF_FOUNDER_ROOT=/absolute/path/to/corpus cargo run --example founder_regression
```

Run the broader mixed pack:

```bash
cd /Users/sarav/Downloads/side/rzn/rag_pdf_extract
cargo run --release --example regression_pack -- --manifest eval/regressions/founder_legal_pack.json
```

## Output

Each run writes:

- `eval/runs/founder_regression_<timestamp>/founder_regression.json`
- `eval/runs/founder_regression_<timestamp>/founder_regression.md`
- `eval/runs/latest_founder_regression.json`
- `eval/runs/latest_founder_regression.md`

The report captures:

- extracted character count
- chunk count
- unique page count
- max emitted chunk tokens
- over-cap chunk count
- garbage-like chunk count
- duplicate-line ratio
- expected-term hits
- pass / warn / fail reasons

## Policy

- `hard` cases must pass
- `warn` cases may stay noisy, but they must remain visible
- if a `warn` case gets fixed, tighten the thresholds and upgrade it later

## Why This Exists

PDF extraction has a bad habit of “improving” one document while quietly wrecking another one. This pack is the minimum viable bullshit detector.
