# PDF Parity Sync Protocol

This is the rule for keeping the canonical extractor and the vendored app copy aligned without cargo-culting copy/paste.

## Repos

| Role | Path |
| --- | --- |
| Canonical extractor | `/Users/sarav/Downloads/side/rzn/rag_pdf_extract` |
| Vendored app copy | `/Users/sarav/Downloads/side/rzn/rznapp/crates/pdf-extract` |

## Hard Rule

If a change affects extraction semantics, layout analysis, heading detection, chunking, token accounting, or public example entrypoints, it is not done until parity is checked against the vendored copy.

## What Must Stay In Lockstep

- `Cargo.toml`
- `README.md`
- `INTEGRATION.md`
- `examples/`
- `src/`
- extractor tests that verify chunk/token contracts

Benchmark manifests and run artifacts stay in the canonical repo. The vendored app copy is a crate mirror, not the benchmark home.

## Workflow

```text
edit canonical -> run founder gate -> run regression pack -> check vendored parity -> sync vendored copy -> re-check parity
```

## Commands

Check parity only:

```bash
cd /Users/sarav/Downloads/side/rzn/rag_pdf_extract
python3 scripts/sync_vendored_pdf_extract.py --mode check
```

Sync the vendored copy:

```bash
cd /Users/sarav/Downloads/side/rzn/rag_pdf_extract
python3 scripts/sync_vendored_pdf_extract.py --mode sync
python3 scripts/sync_vendored_pdf_extract.py --mode check
```

## Release Gate

Before telling the app team to consume an update:

1. `cargo run --example founder_regression`
2. `cargo run --release --example regression_pack -- --manifest eval/regressions/founder_legal_pack.json --out-dir eval/runs/regression/current`
3. `python3 scripts/sync_vendored_pdf_extract.py --mode check`

If step 3 fails, parity is broken. Fix it before calling anything “ready.”

## Why This Exists

The previous failure mode was simple:

```text
canonical improved
vendored copy drifted
app kept shipping the drift
product QA rediscovered the same PDF bugs
```

That loop is dead. Keep it dead.
