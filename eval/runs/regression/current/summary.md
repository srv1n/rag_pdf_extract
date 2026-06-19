# founder-legal-regression-pack

Mixed local fixture and founder/legal regression pack for catching extraction collapse, chunk collapse, and obvious regressions.

- Manifest: `eval/regressions/founder_legal_pack.json`
- Output dir: `eval/runs/regression/current`
- Totals: 5 passed, 0 warned, 1 failed, 0 skipped

| Status | Group | Document | Pages | Chunks | Chars | Note |
| --- | --- | --- | ---: | ---: | ---: | --- |
| failed | repo-fixture | documents_stack.pdf | 1 | 0 | 0 | chunk count 0 below expected floor 1 |
| passed | repo-fixture | embeded-core-fonts.pdf | 28 | 73 | 99426 | extracted 73 chunks / 28 pages / 99426 chars |
| passed | repo-fixture | 10.pdf | 47 | 350 | 338915 | extracted 350 chunks / 47 pages / 338915 chars |
| passed | founder-legal | Energy Watchdog v CERC | 65 | 155 | 224615 | extracted 155 chunks / 65 pages / 224615 chars |
| passed | founder-legal | Puttaswamy v UoI | 547 | 1380 | 1631593 | extracted 1380 chunks / 547 pages / 1631593 chars |
| passed | founder-legal | Vishaka v State of Rajasthan | 10 | 50 | 45213 | extracted 50 chunks / 10 pages / 45213 chars |
