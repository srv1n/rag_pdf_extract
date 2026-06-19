# founder-legal-regression-pack

Mixed local fixture and founder/legal regression pack for catching extraction collapse, chunk collapse, and obvious regressions.

- Manifest: `eval/regressions/founder_legal_pack.json`
- Output dir: `eval/runs/regression/founder_legal_current`
- Totals: 5 passed, 1 warned, 0 failed, 0 skipped

| Status | Group | Document | Pages | Chunks | Chars | Max Tokens | Over Cap | Garbage | Dup Line Ratio | Note |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| passed | repo-fixture | documents_stack.pdf | 1 | 0 | 0 | 0 | 0 | 0 | 0.000 | extracted 0 chunks / 1 pages / 0 chars / max_chunk_tokens=0 |
| passed | repo-fixture | embeded-core-fonts.pdf | 28 | 71 | 99308 | 443 | 0 | 0 | 0.048 | extracted 71 chunks / 28 pages / 99308 chars / max_chunk_tokens=443 |
| passed | repo-fixture | 10.pdf | 47 | 333 | 333851 | 500 | 0 | 0 | 0.003 | extracted 333 chunks / 47 pages / 333851 chars / max_chunk_tokens=500 |
| passed | founder-legal | Energy Watchdog v CERC | 65 | 164 | 227539 | 497 | 0 | 0 | 0.005 | extracted 164 chunks / 65 pages / 227539 chars / max_chunk_tokens=497 |
| warned | founder-legal | Puttaswamy v UoI | 547 | 1099 | 1489291 | 500 | 0 | 97 | 0.004 | garbage-like chunks detected (97) |
| passed | founder-legal | Vishaka v State of Rajasthan | 10 | 45 | 44987 | 500 | 0 | 0 | 0.000 | extracted 45 chunks / 10 pages / 44987 chars / max_chunk_tokens=500 |
