# Lazy layout fallback — 2026-08-20

## What we took from PDF Inspector

[Firecrawl PDF Inspector](https://github.com/firecrawl/pdf-inspector) separates
cheap document classification from the expensive extraction route. Its Rust
pipeline loads a document once, samples content streams with early-exit or
page-sample strategies, and reuses page analysis rather than analyzing a page
twice. See the [detector implementation](https://raw.githubusercontent.com/firecrawl/pdf-inspector/main/src/detector.rs)
and [single-load pipeline](https://raw.githubusercontent.com/firecrawl/pdf-inspector/main/src/lib.rs).

We did not add the dependency or replace `lopdf`. The transferable idea here is
the cost boundary: make the decision from the result already produced, and
only pay for a second representation when that result is suspicious.

## Change shipped

`parse_pdf` now treats `LAParams::default()` (`OnSuspiciousVolume`) as a lazy
fallback:

- Healthy layout output returns immediately; no no-layout extraction runs.
- Empty, low-quality, repeated, or suspiciously decoded layout output still
  runs the no-layout comparison and can fall back exactly as before.
- `OnTextLoss` remains the explicit conservative mode and still always runs
  the comparison.
- `Disabled` still performs no fallback extraction.

No downstream call-site change is required. The existing product layout
defaults now avoid the duplicate pass for healthy documents. Telemetry reports
the no-layout character count only when that probe actually ran.

## Evidence

Focused checks:

```text
cargo test --lib layout_ --quiet                              PASS (4 tests)
cargo test --test tests layout_fallback_prevents_legal_17_text_collapse -- --nocapture  PASS
cargo test --test legal_supreme_court_pdf_quality -- --nocapture              PASS
```

The old Supreme Court regression and the two representative quality fixtures
remain green. The custom-font/CMap decoder and glyph path were not removed.

The 21-PDF benchmark used one warmup and one measured run:

```text
cargo run --release --example fast_text_benchmark -- \
  --corpus-dir benchmarks/corpus --runs 2 --max-tokens 350 \
  --output /tmp/lazy-layout-fallback-2026-08-20.json
```

| variant | total wall ms | median document ms | successes | chars | chunks |
|---|---:|---:|---:|---:|---:|
| layout, `all_texts=false` | 15,104.2 | 377.0 | 21/21 | 3,271,398 | 3,439 |
| layout, `all_texts=true` | 15,536.8 | 396.6 | 21/21 | 3,271,456 | 3,439 |
| structured no-layout | 19,309.8 | 481.3 | 21/21 | 3,270,289 | 3,932 |
| fast text | 10,827.8 | 282.7 | 21/21 | 3,277,696 | 3,569 |

The prior double-pass layout run on the same host measured 25,957.2 ms and
25,837.7 ms for the two layout variants. This change therefore removed about
42% of layout wall time in this directional run, while preserving the exact
layout character/chunk totals from that baseline. Repeat on the deployment
host with multiple measured runs before treating the percentage as a capacity
contract.

## Guidance for the downstream team

Keep using `LAParams::default()` if you want layout plus the quality net: it is
now lazy. Use `LayoutFallbackPolicy::OnTextLoss` only when every layout result
must be compared against no-layout, and use `Disabled` for a deliberately
single-pass diagnostic/fast lane where fallback quality protection is not
wanted. The tokenizer and glyph/CMap safeguards are unchanged.
