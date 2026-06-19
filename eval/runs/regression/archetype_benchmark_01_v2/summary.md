# pdf-archetype-benchmark-01

Fixed PDF archetype pack covering legal, Form XObject, OCR-heavy, multi-column, table-heavy, long-document, rendering-edge, and garbage-prone cases.

- Manifest: `eval/regressions/archetype_benchmark_01.json`
- Output dir: `eval/runs/regression/archetype_benchmark_01_v2`
- Totals: 6 passed, 2 warned, 0 failed, 1 skipped

| Status | Group | Document | Pages | Chunks | Chars | Max Tokens | Over Cap | Garbage | Dup Line Ratio | Note |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| passed | native-digital-legal | Energy Watchdog v CERC | 65 | 239 | 227455 | 347 | 0 | 0 | 0.004 | extracted 239 chunks / 65 pages / 227455 chars / max_chunk_tokens=347 |
| warned | long-book-report | Puttaswamy v UoI | 547 | 1518 | 1488153 | 350 | 0 | 115 | 0.004 | garbage-like chunks detected (115) |
| passed | garbage-prone-legal | Vishaka v State of Rajasthan | 10 | 66 | 44950 | 350 | 0 | 0 | 0.000 | extracted 66 chunks / 10 pages / 44950 chars / max_chunk_tokens=350 |
| warned | form-xobject-legal | CCI v Kerala Film Exhibitors Federation & Ors | 68 | 193 | 199399 | 348 | 0 | 3 | 0.026 | garbage-like chunks detected (3) |
| passed | multi-column | Scientific 6.pdf | 15 | 48 | 38902 | 344 | 0 | 0 | 0.009 | extracted 48 chunks / 15 pages / 38902 chars / max_chunk_tokens=344 |
| passed | table-form-heavy | 10.pdf | 47 | 491 | 333506 | 350 | 0 | 0 | 0.003 | extracted 491 chunks / 47 pages / 333506 chars / max_chunk_tokens=350 |
| passed | ocr-heavy | ocr.pdf | 5 | 7 | 5620 | 324 | 0 | 0 | 0.034 | extracted 7 chunks / 5 pages / 5620 chars / max_chunk_tokens=324 |
| skipped | rendering-edge-case | alternate-color-space.pdf | - | - | - | - | - | - | - | fixture is not a valid PDF; skipped |
| passed | garbage-glyph-soup | documents_stack.pdf | 1 | 0 | 0 | 0 | 0 | 0 | 0.000 | extracted 0 chunks / 1 pages / 0 chars / max_chunk_tokens=0 |
