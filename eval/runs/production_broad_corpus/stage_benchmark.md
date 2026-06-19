# PDF Stage Benchmark

- command: `target/debug/examples/stage_benchmark --manifest eval/manifests/production_broad_corpus.json --out-dir eval/runs/production_broad_corpus`
- git_sha: `de635f7cc694e4398ab51aa2381e2914fc232500`
- git_dirty: `true`
- product_passed: 9/9
- raw_layout_passed: 9/9
- fallback_used: 1/9

| Fixture | Product | Raw Layout | Overall | Fallback | Chunks | Max Tokens | Over Cap | Max ContentExt c/u bytes | No Layout Chars | Layout Chars | Fragments | Span Coverage | Highlight IoU p50/p95 | Reference | Failures |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `large_legal_1` | `passed` | `passed` | `passed` | false | 70 | 476 | 0 | 24796/476044 | 111570 | 111379 | 16636 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `large_legal_17_form_heavy` | `passed` | `passed` | `passed` | false | 50 | 425 | 0 | 22133/414985 | 80930 | 80918 | 14177 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `prior_failure_mixed_4` | `passed` | `passed` | `passed` | false | 36 | 423 | 0 | 25292/512724 | 49138 | 49376 | 7935 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `scientific_3` | `passed` | `passed` | `passed` | false | 98 | 432 | 0 | 21142/440153 | 47853 | 48025 | 9239 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `scientific_6` | `passed` | `passed` | `passed` | true | 30 | 436 | 0 | 24700/430862 | 40205 | 40205 | 6496 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `multi_column_documents_stack` | `passed` | `passed` | `passed` | false | 6 | 229 | 0 | 7033/133795 | 1488 | 1546 | 194 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `form_xobject_fixture` | `passed` | `passed` | `passed` | false | 1 | 3 | 0 | 497/2737 | 13 | 13 | 3 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `ocr_only_invisible_text_fixture` | `passed` | `passed` | `passed` | false | 1 | 4 | 0 | 601/4334 | 24 | 24 | 4 | 1.00 | 1.00/1.00 | poppler: `available` | - |
| `table_repeated_text_legal_19` | `passed` | `passed` | `passed` | false | 48 | 425 | 0 | 24617/473448 | 85437 | 85354 | 13832 | 1.00 | 1.00/1.00 | poppler: `available` | - |
