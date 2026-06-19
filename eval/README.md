# PDF Extraction Evaluation System

A comprehensive evaluation framework for iterating on PDF extraction quality, based on established benchmarks like [Bast & Korzen](https://ad-publications.cs.uni-freiburg.de/benchmark.pdf) and [gipplab/pdf-benchmark](https://github.com/gipplab/pdf-benchmark).

## Quick Start

```bash
# 1. Add test PDFs by category
cp legal_doc.pdf eval/corpus/legal/
cp paper.pdf eval/corpus/scientific/

# 2. Create expected markdown (golden files)
# Option A: Manually create the expected output
# Option B: Use a baseline tool then review
python -c "from tools.baselines import extract_with_tool; print(extract_with_tool('markitdown', 'corpus/legal/legal_doc.pdf'))" > corpus/legal/legal_doc.md

# 3. Run evaluation
python eval.py

# 4. Iterate: make changes, re-run, check for regressions
python eval.py --diagnose  # Detailed failure analysis
```

## Regression Pack

For the smaller, repeatable regression loop used to catch extraction collapse
on repo fixtures and founder/legal PDFs, see:

- [`REGRESSION_PACK.md`](./REGRESSION_PACK.md)
- [`regressions/archetype_benchmark_01.json`](./regressions/archetype_benchmark_01.json)

The practical three-layer stack is:

1. `examples/founder_regression.rs` for the founder gate.
2. `examples/regression_pack.rs` for fixed archetype packs.
3. `eval.py` for golden-output and baseline-comparison work.

The regression-pack runner also understands two things that matter in practice:

- linked repo fixtures via `tests/docs/*.pdf.link`, cached into `tests/docs_cache/`
- explicit OCR cases via `use_ocr: true`, using `models/text-detection.rten` and `models/text-recognition.rten` when present

## Folder Structure

```
eval/
├── corpus/                 # Test documents by category
│   ├── legal/             # Legal documents (contracts, court filings)
│   │   ├── doc1.pdf
│   │   └── doc1.md        # Expected output (golden file)
│   ├── scientific/        # Academic papers (figures, citations)
│   ├── financial/         # Financial reports (tables, numbers)
│   ├── scanned/           # Scanned/OCR documents
│   └── mixed/             # Other document types
├── runs/                   # Evaluation history
│   ├── run_20241201_*.json
│   └── latest.json
├── tools/                  # Baseline extraction tools
│   └── baselines.py
└── eval.py                # Main evaluation script
```

## Metrics (Based on PDF Extraction Benchmarks)

| Metric | Threshold | Description | Source |
|--------|-----------|-------------|--------|
| **WER** | < 15% | Word Error Rate (edit distance) | OCR benchmarks |
| **CER** | < 10% | Character Error Rate | OCR benchmarks |
| **Empty Section Rate** | < 10% | Headings with no content | Custom |
| **Heading F1** | > 60% | Heading detection accuracy | Document AI |
| **Content Coverage** | > 80% | Expected words found in output | Custom |

### Word Error Rate (WER)
Standard metric from speech/OCR evaluation. Measures word-level edit distance:
```
WER = (Insertions + Deletions + Substitutions) / Total Words
```
- **Good**: < 5%
- **Acceptable**: 5-15%
- **Poor**: > 15%

### Character Error Rate (CER)
Character-level accuracy from OCR benchmarks:
```
CER = Levenshtein Distance / Total Characters
```
- **Good**: < 2%
- **Acceptable**: 2-10%
- **Poor**: > 10%

## Usage

### Basic Evaluation
```bash
python eval.py                      # Full evaluation
python eval.py -c legal             # Single category
python eval.py --diagnose           # Detailed diagnostics
python eval.py --history            # Show improvement over time
```

### Comparing Against Baselines
```bash
# Check available tools
python tools/baselines.py

# Compare our extraction vs MarkItDown
python eval.py --compare markitdown

# Compare against OCR baseline (needs API key)
export GEMINI_API_KEY=your_key
python eval.py --compare gemini
```

### Recommended PDF Ownership Loop

```bash
# 1. Fast founder gate
cargo run --example founder_regression

# 2. Fixed archetype pack with hard-cap metrics
cargo run --release --example regression_pack -- \
  --manifest eval/regressions/archetype_benchmark_01.json \
  --out-dir eval/runs/archetype_benchmark/current

# 3. Golden-output benchmark comparisons
python eval.py --diagnose
```

### Available Baseline Tools

| Tool | Type | Description |
|------|------|-------------|
| `ours` | Non-OCR | Our Rust extraction |
| `markitdown` | Non-OCR | Microsoft MarkItDown (PyMuPDF) |
| `gemini` | OCR | Google Gemini API |
| `claude` | OCR | Anthropic Claude API |

## Diagnostic Output (For Agent Iteration)

When evaluation fails, the system provides actionable diagnostics:

```
FAILURES:

  legal/contract.pdf
    - Empty section rate 25.0% > 10%
    Likely cause: Consecutive empty headings - content may be filtered out
    Suggested fix: Check header/footer filtering in processing.rs
    Empty sections: ['ARTICLE I', 'ARTICLE II', 'ARTICLE III']
```

### Diagnostic Fields

| Field | Purpose |
|-------|---------|
| `failure_reasons` | What threshold was violated |
| `likely_causes` | Pattern-based root cause analysis |
| `suggested_fixes` | Specific code locations to investigate |
| `empty_sections` | List of problematic headings |
| `missing_headings` | Expected headings not found |
| `sample_errors` | Word-level diff examples |

## Iteration Workflow

### For an AI Agent

```python
# Pseudo-code for autonomous iteration

while True:
    # 1. Run evaluation
    result = run("python eval.py --diagnose")

    # 2. Check if passing
    if "RESULT: PASS" in result:
        break

    # 3. Parse diagnostics
    diagnostics = parse_diagnostics(result)

    # 4. Identify the issue
    if "Empty section rate" in diagnostics.failure_reasons:
        # Look at empty_sections list
        # Check suggested_fixes for code location
        fix_empty_sections(diagnostics)

    elif "WER" in diagnostics.failure_reasons:
        # Look at sample_errors for specific issues
        # Check likely_causes for encoding/font issues
        fix_text_extraction(diagnostics)

    # 5. Make code changes
    # 6. Re-run to verify fix didn't break other things
```

### Key Files to Check

| Issue | Files to Investigate |
|-------|---------------------|
| Empty sections | `src/chunk_accumulator.rs`, `src/document/processing.rs` |
| Text encoding | `src/lib.rs` (process_stream, font handling) |
| Heading detection | `src/document/processing.rs` (heading detection logic) |
| Header/footer over-filtering | `src/document/header_footer.rs` |

## Creating Ground Truth (Expected Files)

### Option 1: Manual Creation
Best for important test cases. Open PDF, manually create markdown.

### Option 2: From Baseline Tool
```bash
# Use MarkItDown as starting point
python -c "
from tools.baselines import extract_with_tool
from pathlib import Path
print(extract_with_tool('markitdown', Path('corpus/legal/doc.pdf')))
" > corpus/legal/doc.md

# Review and fix the output
vim corpus/legal/doc.md
```

### Option 3: From Our Tool (Bootstrap)
```bash
# After initial extraction looks reasonable
python eval.py  # Generates output in runs/output_*/

# Promote good outputs to expected
cp runs/output_*/legal/doc.md corpus/legal/doc.md
```

## Run History

The system tracks metrics over time:

```bash
python eval.py --history

EVALUATION HISTORY
======================================================================
Timestamp            Git        Pass Rate    Empty Rate   WER
----------------------------------------------------------------------
20241201_100000      abc123         60%          15.2%      8.3%
20241201_110000      def456         80%          10.1%      6.2%
20241201_120000      ghi789         90%           5.3%      4.1%
```

This helps verify that fixes improve metrics without regressions.

## CI Integration

```bash
# In your CI pipeline
python eval.py --no-save

# Exit codes:
# 0 = all tests passed
# 1 = one or more failures
```

## Extending

### Adding a New Category
```bash
mkdir -p eval/corpus/invoices
# Add PDFs and expected markdown files
```

### Adding a Regression Case
Add a row to `eval/regressions/founder_legal_pack.json` with a path, group,
and floors for `expected_min_chars` / `expected_min_chunks`. The runner will
record the actual counts and flag missing or collapsed extraction.

### Adding a New Baseline Tool
Edit `tools/baselines.py`:
```python
def extract_with_newtool(pdf_path: Path) -> str:
    # Your implementation
    pass

EXTRACTORS["newtool"] = extract_with_newtool
AVAILABLE_TOOLS["newtool"] = "Description of new tool"
```

### Adjusting Thresholds
Edit `THRESHOLDS` in `eval.py`:
```python
THRESHOLDS = {
    "word_error_rate": 0.10,  # Stricter: 10%
    "empty_section_rate": 0.05,  # Stricter: 5%
    ...
}
```

## References

- [Bast & Korzen Benchmark (2017)](https://ad-publications.cs.uni-freiburg.de/benchmark.pdf) - Foundational PDF extraction benchmark
- [gipplab/pdf-benchmark](https://github.com/gipplab/pdf-benchmark) - Multi-task PDF evaluation framework
- [ckorzen/pdf-text-extraction-benchmark](https://github.com/ckorzen/pdf-text-extraction-benchmark) - Semantic extraction benchmark
