# PDF Stage Benchmark

- command: `target/debug/examples/stage_benchmark --manifest eval/fixtures/pdf_manifest.json --out-dir eval/runs/stage_benchmark_current`
- git_sha: `de635f7cc694e4398ab51aa2381e2914fc232500`
- git_dirty: `true`
- product_passed: 10/10
- raw_layout_passed: 10/10
- fallback_used: 0/10

| Fixture | Product | Raw Layout | Overall | Fallback | Chunks | Max Tokens | Over Cap | Max ContentExt c/u bytes | No Layout Chars | Layout Chars | Fragments | Span Coverage | Highlight IoU p50/p95 | Reference | Failures |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `ctm_scaled_text` | `passed` | `passed` | `passed` | false | 1 | 4 | 0 | 533/3063 | 15 | 15 | 3 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `invisible_ocr_only` | `passed` | `passed` | `passed` | false | 1 | 4 | 0 | 601/4334 | 24 | 24 | 4 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `long_split_locations` | `passed` | `passed` | `passed` | false | 2 | 440 | 0 | 20115/473552 | 5134 | 5134 | 711 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `span_bbox_two_lines` | `passed` | `passed` | `passed` | false | 1 | 6 | 0 | 739/6721 | 36 | 36 | 6 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `nonzero_mediabox` | `passed` | `passed` | `passed` | false | 1 | 5 | 0 | 581/4035 | 21 | 21 | 3 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `cropbox_smaller` | `passed` | `passed` | `passed` | false | 1 | 3 | 0 | 474/2311 | 12 | 12 | 2 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `rotate_90` | `passed` | `passed` | `passed` | false | 1 | 4 | 0 | 509/2708 | 14 | 14 | 3 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `rotate_180` | `passed` | `passed` | `passed` | false | 1 | 4 | 0 | 524/2842 | 15 | 15 | 3 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `rotate_270` | `passed` | `passed` | `passed` | false | 1 | 4 | 0 | 524/2842 | 15 | 15 | 3 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `nonzero_form_xobject` | `passed` | `passed` | `passed` | false | 1 | 3 | 0 | 497/2737 | 13 | 13 | 3 | 1.00 | 1.00/1.00 | poppler: `available` | - |
