# PDF Extraction Evaluation System

This document describes the evaluation framework for measuring and improving PDF text extraction quality.

## Overview

The evaluation system allows you to:
1. **Measure extraction quality** against ground truth markdown files
2. **Track regressions** across code changes
3. **Compare against baselines** (MarkItDown, Gemini OCR, Claude OCR)
4. **Get actionable diagnostics** for autonomous iteration

## Quick Start

```bash
# 1. Add test PDFs by category
cp your_legal_doc.pdf eval/corpus/legal/
cp your_paper.pdf eval/corpus/scientific/

# 2. Create expected markdown (ground truth)
# Option A: Run extraction and manually review/fix
./target/release/examples/extract_markdown your_doc.pdf > eval/corpus/legal/your_doc.md
# Option B: Use a baseline tool as starting point
python -c "from eval.tools.baselines import extract_with_tool; print(extract_with_tool('markitdown', 'path/to/doc.pdf'))"

# 3. Run evaluation
python eval/eval.py

# 4. Iterate: make code changes, re-run, check for regressions
python eval/eval.py --diagnose
```

## Directory Structure

```
eval/
├── corpus/                 # Test documents organized by category
│   ├── legal/             # Legal documents (contracts, court filings)
│   │   ├── contract.pdf
│   │   └── contract.md    # Ground truth (expected output)
│   ├── scientific/        # Academic papers
│   ├── financial/         # Financial reports, tables
│   ├── scanned/           # OCR-required documents
│   └── mixed/             # Other document types
├── runs/                   # Evaluation history
│   ├── run_YYYYMMDD_HHMMSS.json
│   ├── output_YYYYMMDD_HHMMSS/  # Extraction outputs per run
│   └── latest.json
├── tools/
│   ├── __init__.py
│   └── baselines.py       # Baseline extraction tools
├── eval.py                # Main evaluation script
└── README.md              # Detailed usage docs
```

## Metrics

Based on established PDF extraction benchmarks ([Bast & Korzen 2017](https://ad-publications.cs.uni-freiburg.de/benchmark.pdf), [gipplab/pdf-benchmark](https://github.com/gipplab/pdf-benchmark)).

### Primary Metrics

| Metric | Threshold | Description |
|--------|-----------|-------------|
| **Word Error Rate (WER)** | < 15% | Word-level edit distance / total words |
| **Character Error Rate (CER)** | < 10% | Character-level Levenshtein distance |
| **Empty Section Rate** | < 10% | Headings with no content following |
| **Heading F1** | > 60% | Precision/recall of heading detection |
| **Content Coverage** | > 80% | % of expected words found in output |

### Metric Definitions

**WER (Word Error Rate)**
```
WER = (Insertions + Deletions + Substitutions) / Total Words
```
- Good: < 5%
- Acceptable: 5-15%
- Poor: > 15%

**CER (Character Error Rate)**
```
CER = Levenshtein_Distance(expected, actual) / len(expected)
```
- Good: < 2%
- Acceptable: 2-10%
- Poor: > 10%

**Empty Section Rate**
Percentage of headings that have no text content before the next heading. This catches the "empty chunk" problem where extraction detects headers but misses body text.

## CLI Usage

```bash
# Full evaluation across all categories
python eval/eval.py

# Single category
python eval/eval.py --category legal
python eval/eval.py -c scientific

# Detailed diagnostics (for debugging)
python eval/eval.py --diagnose

# View evaluation history
python eval/eval.py --history

# Compare against baseline tools
python eval/eval.py --compare markitdown
python eval/eval.py --compare gemini    # Requires GEMINI_API_KEY
python eval/eval.py --compare claude    # Requires ANTHROPIC_API_KEY

# List available tools
python eval/eval.py --list-tools

# Don't save run to history
python eval/eval.py --no-save
```

## Baseline Tools

| Tool | Type | Setup |
|------|------|-------|
| `ours` | Non-OCR | Built automatically |
| `markitdown` | Non-OCR | `pip install markitdown` |
| `gemini` | OCR | `pip install google-generativeai` + `GEMINI_API_KEY` |
| `claude` | OCR | `pip install anthropic` + `ANTHROPIC_API_KEY` |

### Running Comparisons

```bash
# Check tool availability
python eval/eval.py --list-tools

# Compare extraction quality
python eval/eval.py --compare markitdown

# Output shows WER and coverage for each tool:
#   ours:      WER=5.2%, coverage=92.1%
#   markitdown: WER=8.7%, coverage=88.3%
#   Winner: ours <-
```

## Diagnostic Output

When running with `--diagnose`, failures include actionable information:

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
| `failure_reasons` | Which thresholds were violated |
| `likely_causes` | Pattern-based root cause analysis |
| `suggested_fixes` | Code files to investigate |
| `empty_sections` | List of headings with no content |
| `missing_headings` | Expected headings not detected |
| `sample_errors` | Word-level diff examples |

### Code Locations for Common Issues

| Issue | Files to Check |
|-------|----------------|
| Empty sections | `src/chunk_accumulator.rs`, `src/document/processing.rs` |
| Text encoding | `src/lib.rs` (process_stream, font handling) |
| Heading detection | `src/document/processing.rs` (font size thresholds) |
| Over-filtering | `src/document/header_footer.rs` |

## Autonomous Iteration Workflow

For AI agents iterating on extraction quality:

```python
# Pseudo-code workflow
while True:
    # 1. Run evaluation
    result = run("python eval/eval.py --diagnose")

    # 2. Check if passing
    if "RESULT: PASS" in result:
        break

    # 3. Parse diagnostics
    diagnostics = parse_diagnostics(result)

    # 4. Identify the issue type
    if "Empty section rate" in diagnostics:
        # Check empty_sections list
        # Look at suggested_fixes for code location
        # Make targeted fix

    elif "WER" in diagnostics:
        # Look at sample_errors
        # Check for encoding issues

    # 5. Make code changes
    # 6. Re-run to verify fix + no regressions
```

## Run History

The system tracks metrics over time to detect regressions:

```bash
python eval/eval.py --history

EVALUATION HISTORY
======================================================================
Timestamp            Git        Pass Rate    Empty Rate   WER
----------------------------------------------------------------------
20241201_100000      abc123         60%          15.2%      8.3%
20241201_110000      def456         80%          10.1%      6.2%
20241201_120000      ghi789         90%           5.3%      4.1%
```

## Creating Ground Truth Files

### Option 1: Manual Creation
Best for critical test cases. View PDF, manually write expected markdown.

### Option 2: From Our Extraction (Bootstrap)
```bash
# Run extraction
./target/release/examples/extract_markdown doc.pdf > eval/corpus/legal/doc.md

# Review and fix obvious errors
vim eval/corpus/legal/doc.md
```

### Option 3: From Baseline Tool
```bash
# Use MarkItDown as starting point
python -c "
from eval.tools.baselines import extract_with_tool
from pathlib import Path
print(extract_with_tool('markitdown', Path('corpus/legal/doc.pdf')))
" > eval/corpus/legal/doc.md

# Review and fix
```

## CI Integration

```bash
# In CI pipeline
python eval/eval.py --no-save

# Exit codes:
# 0 = all tests passed
# 1 = one or more failures
```

## Configuration

Edit thresholds in `eval/eval.py`:

```python
THRESHOLDS = {
    "word_error_rate": 0.15,        # Fail if WER > 15%
    "char_error_rate": 0.10,        # Fail if CER > 10%
    "empty_section_rate": 0.10,     # Fail if > 10% empty sections
    "heading_f1": 0.60,             # Fail if heading F1 < 60%
    "content_coverage": 0.80,       # Fail if < 80% content found
}
```

## Legacy Benchmarks

The `benchmarks/` folder contains an older evaluation approach focused on:
- Academic datasets (DocLayNet, PubLayNet, PMC OA)
- Bounding box / layout accuracy
- No text ground truth

This was useful for layout analysis but not for text extraction quality. The new `eval/` system is recommended for RAG use cases where text accuracy matters.

## References

- [Bast & Korzen: A Benchmark and Evaluation for Text Extraction from PDF (2017)](https://ad-publications.cs.uni-freiburg.de/benchmark.pdf)
- [gipplab/pdf-benchmark](https://github.com/gipplab/pdf-benchmark) - Multi-task PDF evaluation
- [ckorzen/pdf-text-extraction-benchmark](https://github.com/ckorzen/pdf-text-extraction-benchmark) - Semantic extraction benchmark
- [DocLayNet](https://github.com/DS4SD/DocLayNet) - Document layout dataset
- [PubLayNet](https://github.com/ibm-aur-nlp/PubLayNet) - Scientific document layout
