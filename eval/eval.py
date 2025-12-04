#!/usr/bin/env python3
"""
PDF Extraction Evaluation System

Evaluates extraction quality using established metrics from PDF extraction benchmarks:
- Character Error Rate (CER) / Word Error Rate (WER)
- Structure accuracy (headings, paragraphs)
- Empty section detection
- Reading order preservation

Organized by document category for targeted analysis.

Usage:
    python eval.py                      # Full evaluation
    python eval.py --category legal     # Single category
    python eval.py --diagnose           # Detailed diagnostics for debugging
    python eval.py --history            # Show improvement over runs
"""

import os
import sys
import json
import subprocess
import argparse
import difflib
import re
import hashlib
from pathlib import Path
from dataclasses import dataclass, field, asdict
from typing import List, Dict, Optional, Tuple, Any
from datetime import datetime
from collections import defaultdict


# =============================================================================
# Configuration
# =============================================================================

EVAL_DIR = Path(__file__).parent
CORPUS_DIR = EVAL_DIR / "corpus"
RUNS_DIR = EVAL_DIR / "runs"
TOOLS_DIR = EVAL_DIR / "tools"

PROJECT_ROOT = EVAL_DIR.parent
EXTRACT_BINARY = PROJECT_ROOT / "target" / "release" / "examples" / "extract_markdown"

# Document categories
CATEGORIES = ["legal", "scientific", "financial", "scanned", "mixed"]

# Thresholds (based on established benchmarks)
THRESHOLDS = {
    "word_error_rate": 0.15,        # WER < 15% (benchmarks consider <5% good for OCR)
    "char_error_rate": 0.10,        # CER < 10%
    "empty_section_rate": 0.10,     # < 10% empty sections
    "heading_f1": 0.60,             # F1 > 60% for heading detection
    "structure_similarity": 0.70,   # Structure match > 70%
    "content_coverage": 0.80,       # > 80% of expected content present
}


# =============================================================================
# Metrics Implementation (from PDF extraction benchmarks)
# =============================================================================

def levenshtein_distance(s1: str, s2: str) -> int:
    """Compute Levenshtein edit distance between two strings."""
    if len(s1) < len(s2):
        return levenshtein_distance(s2, s1)
    if len(s2) == 0:
        return len(s1)

    previous_row = range(len(s2) + 1)
    for i, c1 in enumerate(s1):
        current_row = [i + 1]
        for j, c2 in enumerate(s2):
            insertions = previous_row[j + 1] + 1
            deletions = current_row[j] + 1
            substitutions = previous_row[j] + (c1 != c2)
            current_row.append(min(insertions, deletions, substitutions))
        previous_row = current_row

    return previous_row[-1]


def calculate_cer(expected: str, actual: str) -> float:
    """
    Character Error Rate: edit_distance / len(expected)
    Standard metric from OCR benchmarks.
    """
    if not expected:
        return 0.0 if not actual else 1.0

    distance = levenshtein_distance(expected, actual)
    return distance / len(expected)


def calculate_wer(expected: str, actual: str) -> float:
    """
    Word Error Rate: word-level edit distance / word count
    Standard metric from speech/OCR benchmarks.
    """
    expected_words = expected.split()
    actual_words = actual.split()

    if not expected_words:
        return 0.0 if not actual_words else 1.0

    # Use difflib for word-level comparison
    matcher = difflib.SequenceMatcher(None, expected_words, actual_words)

    # Count operations needed
    insertions = deletions = substitutions = 0
    for tag, i1, i2, j1, j2 in matcher.get_opcodes():
        if tag == 'replace':
            substitutions += max(i2 - i1, j2 - j1)
        elif tag == 'delete':
            deletions += i2 - i1
        elif tag == 'insert':
            insertions += j2 - j1

    return (insertions + deletions + substitutions) / len(expected_words)


def extract_structure(markdown: str) -> Dict[str, Any]:
    """
    Extract structural elements from markdown for comparison.
    Returns headings, paragraph count, and structure fingerprint.
    """
    lines = markdown.split('\n')

    headings = []
    paragraphs = []
    current_para = []

    for line in lines:
        if line.startswith('#'):
            # Save current paragraph
            if current_para:
                paragraphs.append(' '.join(current_para))
                current_para = []

            # Extract heading with level
            match = re.match(r'^(#+)\s*(.+)$', line)
            if match:
                level = len(match.group(1))
                text = match.group(2).strip()
                headings.append({'level': level, 'text': text})
        elif line.strip():
            current_para.append(line.strip())
        elif current_para:
            paragraphs.append(' '.join(current_para))
            current_para = []

    if current_para:
        paragraphs.append(' '.join(current_para))

    return {
        'headings': headings,
        'paragraph_count': len(paragraphs),
        'heading_count': len(headings),
        'structure_hash': hashlib.md5(
            json.dumps([h['text'][:50] for h in headings]).encode()
        ).hexdigest()[:8]
    }


def calculate_heading_f1(expected_headings: List[Dict], actual_headings: List[Dict]) -> Tuple[float, float, float]:
    """
    Calculate precision, recall, F1 for heading detection.
    Uses fuzzy matching on heading text.
    """
    if not expected_headings and not actual_headings:
        return 1.0, 1.0, 1.0
    if not expected_headings:
        return 0.0, 1.0, 0.0
    if not actual_headings:
        return 1.0, 0.0, 0.0

    def normalize(text):
        return re.sub(r'\s+', ' ', text.lower().strip())

    expected_texts = [normalize(h['text']) for h in expected_headings]
    actual_texts = [normalize(h['text']) for h in actual_headings]

    # Find matches using fuzzy comparison
    matched_expected = set()
    matched_actual = set()

    for i, exp in enumerate(expected_texts):
        for j, act in enumerate(actual_texts):
            if j in matched_actual:
                continue
            ratio = difflib.SequenceMatcher(None, exp, act).ratio()
            if ratio > 0.7:
                matched_expected.add(i)
                matched_actual.add(j)
                break

    precision = len(matched_actual) / len(actual_texts) if actual_texts else 0
    recall = len(matched_expected) / len(expected_texts) if expected_texts else 0
    f1 = 2 * precision * recall / (precision + recall) if (precision + recall) > 0 else 0

    return precision, recall, f1


def find_empty_sections(markdown: str) -> List[Dict]:
    """
    Find headings with no content following them.
    Returns details about each empty section for debugging.
    """
    lines = markdown.split('\n')
    empty_sections = []

    i = 0
    while i < len(lines):
        line = lines[i]
        if line.startswith('#'):
            match = re.match(r'^(#+)\s*(.+)$', line)
            if match:
                level = len(match.group(1))
                heading_text = match.group(2).strip()

                # Look for content before next heading
                has_content = False
                next_heading_line = None
                for j in range(i + 1, len(lines)):
                    next_line = lines[j].strip()
                    if next_line.startswith('#'):
                        next_heading_line = j
                        break
                    if next_line:
                        has_content = True
                        break

                if not has_content:
                    empty_sections.append({
                        'line': i + 1,
                        'level': level,
                        'heading': heading_text,
                        'next_heading_line': next_heading_line
                    })
        i += 1

    return empty_sections


def calculate_content_coverage(expected: str, actual: str) -> float:
    """
    What percentage of expected words appear in actual output?
    More lenient than WER - doesn't penalize extra content.
    """
    def get_words(text):
        return set(re.findall(r'\b\w+\b', text.lower()))

    expected_words = get_words(expected)
    actual_words = get_words(actual)

    if not expected_words:
        return 1.0

    found = expected_words & actual_words
    return len(found) / len(expected_words)


# =============================================================================
# Diagnostic Output (for agent iteration)
# =============================================================================

@dataclass
class DiagnosticInfo:
    """Detailed diagnostic information for debugging extraction issues."""

    # Basic info
    pdf_name: str
    category: str

    # Metrics
    wer: Optional[float] = None
    cer: Optional[float] = None
    heading_f1: Optional[float] = None
    empty_section_rate: float = 0.0
    content_coverage: Optional[float] = None

    # Detailed issues
    empty_sections: List[Dict] = field(default_factory=list)
    missing_headings: List[str] = field(default_factory=list)
    extra_headings: List[str] = field(default_factory=list)
    sample_errors: List[Dict] = field(default_factory=list)

    # Suggestions for fixing
    likely_causes: List[str] = field(default_factory=list)
    suggested_fixes: List[str] = field(default_factory=list)

    # Pass/fail
    passed: bool = True
    failure_reasons: List[str] = field(default_factory=list)


def generate_diagnostics(
    pdf_name: str,
    category: str,
    actual_markdown: str,
    expected_markdown: Optional[str] = None
) -> DiagnosticInfo:
    """
    Generate detailed diagnostics for debugging.
    This is what the agent uses to understand and fix issues.
    """
    diag = DiagnosticInfo(pdf_name=pdf_name, category=category)

    # Analyze structure
    actual_structure = extract_structure(actual_markdown)
    empty_sections = find_empty_sections(actual_markdown)

    diag.empty_sections = empty_sections
    diag.empty_section_rate = (
        len(empty_sections) / actual_structure['heading_count']
        if actual_structure['heading_count'] > 0 else 0.0
    )

    # Check empty section threshold
    if diag.empty_section_rate > THRESHOLDS['empty_section_rate']:
        diag.passed = False
        diag.failure_reasons.append(
            f"Empty section rate {diag.empty_section_rate:.1%} > {THRESHOLDS['empty_section_rate']:.0%}"
        )

        # Analyze patterns in empty sections
        if empty_sections:
            levels = [e['level'] for e in empty_sections]
            if levels.count(1) > len(levels) * 0.5:
                diag.likely_causes.append("Many H1 headings are empty - possible title/header detection issue")
                diag.suggested_fixes.append("Check heading detection logic in chunk_accumulator.rs")

            consecutive = sum(1 for i in range(len(empty_sections)-1)
                            if empty_sections[i+1]['line'] - empty_sections[i]['line'] < 5)
            if consecutive > 2:
                diag.likely_causes.append("Consecutive empty headings - content may be filtered out")
                diag.suggested_fixes.append("Check header/footer filtering in processing.rs")

    # Compare against expected if available
    if expected_markdown:
        expected_structure = extract_structure(expected_markdown)

        # Calculate metrics
        diag.wer = calculate_wer(expected_markdown, actual_markdown)
        diag.cer = calculate_cer(expected_markdown, actual_markdown)
        diag.content_coverage = calculate_content_coverage(expected_markdown, actual_markdown)

        # Heading comparison
        precision, recall, f1 = calculate_heading_f1(
            expected_structure['headings'],
            actual_structure['headings']
        )
        diag.heading_f1 = f1

        # Find missing/extra headings
        expected_heading_texts = {h['text'].lower() for h in expected_structure['headings']}
        actual_heading_texts = {h['text'].lower() for h in actual_structure['headings']}

        diag.missing_headings = list(expected_heading_texts - actual_heading_texts)[:5]
        diag.extra_headings = list(actual_heading_texts - expected_heading_texts)[:5]

        # Check thresholds
        if diag.wer and diag.wer > THRESHOLDS['word_error_rate']:
            diag.passed = False
            diag.failure_reasons.append(f"WER {diag.wer:.1%} > {THRESHOLDS['word_error_rate']:.0%}")
            diag.likely_causes.append("High word error rate - text extraction or encoding issue")
            diag.suggested_fixes.append("Check font encoding in process_stream()")

        if diag.heading_f1 and diag.heading_f1 < THRESHOLDS['heading_f1']:
            diag.passed = False
            diag.failure_reasons.append(f"Heading F1 {diag.heading_f1:.1%} < {THRESHOLDS['heading_f1']:.0%}")
            if diag.missing_headings:
                diag.likely_causes.append(f"Missing headings like: {diag.missing_headings[0]}")
                diag.suggested_fixes.append("Check heading detection thresholds (font size ratio)")

        if diag.content_coverage and diag.content_coverage < THRESHOLDS['content_coverage']:
            diag.passed = False
            diag.failure_reasons.append(
                f"Content coverage {diag.content_coverage:.1%} < {THRESHOLDS['content_coverage']:.0%}"
            )
            diag.likely_causes.append("Significant content missing from extraction")
            diag.suggested_fixes.append("Check if content is being filtered or skipped")

        # Generate sample errors (word-level diff)
        if diag.wer and diag.wer > 0.05:
            expected_words = expected_markdown.split()[:100]
            actual_words = actual_markdown.split()[:100]

            matcher = difflib.SequenceMatcher(None, expected_words, actual_words)
            for tag, i1, i2, j1, j2 in matcher.get_opcodes():
                if tag != 'equal' and len(diag.sample_errors) < 3:
                    diag.sample_errors.append({
                        'type': tag,
                        'expected': ' '.join(expected_words[i1:i2])[:50],
                        'actual': ' '.join(actual_words[j1:j2])[:50]
                    })

    return diag


# =============================================================================
# Extraction Runner
# =============================================================================

def ensure_binary():
    """Build extraction binary if needed."""
    if not EXTRACT_BINARY.exists():
        print("Building extract_markdown...")
        result = subprocess.run(
            ["cargo", "build", "--release", "--example", "extract_markdown"],
            cwd=PROJECT_ROOT,
            capture_output=True,
            text=True
        )
        if result.returncode != 0:
            raise RuntimeError(f"Build failed: {result.stderr}")


def run_extraction(pdf_path: Path) -> str:
    """Run extraction and return markdown."""
    ensure_binary()

    result = subprocess.run(
        [str(EXTRACT_BINARY), str(pdf_path), "--max-tokens", "500"],
        capture_output=True,
        text=True,
        cwd=PROJECT_ROOT
    )

    if result.returncode != 0:
        raise RuntimeError(f"Extraction failed: {result.stderr}")

    return result.stdout


# =============================================================================
# Corpus Management
# =============================================================================

def find_reference_file(pdf_path: Path) -> Optional[Path]:
    """
    Find the best available reference file for a PDF.

    Priority order:
    1. document.md (manually curated ground truth)
    2. document.gemini.md (Gemini API extraction)
    3. document.markitdown.md (MarkItDown extraction)
    """
    # Check in priority order
    candidates = [
        pdf_path.with_suffix('.md'),           # Manual ground truth
        pdf_path.with_suffix('.gemini.md'),    # Gemini reference
        pdf_path.with_suffix('.markitdown.md'), # MarkItDown reference
    ]

    for candidate in candidates:
        if candidate.exists():
            return candidate

    return None


def get_corpus_files() -> Dict[str, List[Tuple[Path, Optional[Path]]]]:
    """
    Get all PDFs organized by category.
    Returns: {category: [(pdf_path, reference_md_path or None), ...]}

    Reference files are found in priority order:
    1. document.md (manual)
    2. document.gemini.md
    3. document.markitdown.md
    """
    corpus = defaultdict(list)

    for category in CATEGORIES:
        category_dir = CORPUS_DIR / category
        if not category_dir.exists():
            continue

        for pdf_path in sorted(category_dir.glob("*.pdf")):
            ref_path = find_reference_file(pdf_path)
            corpus[category].append((pdf_path, ref_path))

    return dict(corpus)


# =============================================================================
# Run Management
# =============================================================================

@dataclass
class RunResult:
    """Results from a single evaluation run."""
    timestamp: str
    git_hash: Optional[str]

    total_files: int
    passed_files: int
    failed_files: int

    by_category: Dict[str, Dict[str, Any]]
    diagnostics: List[DiagnosticInfo]

    # Aggregate metrics
    avg_wer: Optional[float] = None
    avg_empty_rate: float = 0.0
    avg_heading_f1: Optional[float] = None


def get_git_hash() -> Optional[str]:
    """Get current git commit hash."""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=PROJECT_ROOT,
            capture_output=True,
            text=True
        )
        return result.stdout.strip() if result.returncode == 0 else None
    except:
        return None


def save_run(run: RunResult):
    """Save run results for history tracking."""
    RUNS_DIR.mkdir(exist_ok=True)

    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    run_file = RUNS_DIR / f"run_{timestamp}.json"

    # Convert to serializable format
    run_dict = {
        'timestamp': run.timestamp,
        'git_hash': run.git_hash,
        'total_files': run.total_files,
        'passed_files': run.passed_files,
        'failed_files': run.failed_files,
        'by_category': run.by_category,
        'avg_wer': run.avg_wer,
        'avg_empty_rate': run.avg_empty_rate,
        'avg_heading_f1': run.avg_heading_f1,
        'diagnostics': [asdict(d) for d in run.diagnostics]
    }

    run_file.write_text(json.dumps(run_dict, indent=2))

    # Also save as latest
    (RUNS_DIR / "latest.json").write_text(json.dumps(run_dict, indent=2))

    return run_file


def load_previous_run() -> Optional[Dict]:
    """Load the most recent run for comparison."""
    latest = RUNS_DIR / "latest.json"
    if latest.exists():
        return json.loads(latest.read_text())
    return None


# =============================================================================
# Main Evaluation
# =============================================================================

def evaluate_file(
    pdf_path: Path,
    expected_path: Optional[Path],
    category: str,
    output_dir: Path
) -> DiagnosticInfo:
    """Evaluate a single PDF file."""

    # Run extraction
    try:
        actual_markdown = run_extraction(pdf_path)
    except Exception as e:
        diag = DiagnosticInfo(
            pdf_name=pdf_path.name,
            category=category,
            passed=False,
            failure_reasons=[f"Extraction error: {str(e)}"]
        )
        return diag

    # Save output
    output_file = output_dir / category / f"{pdf_path.stem}.md"
    output_file.parent.mkdir(parents=True, exist_ok=True)
    output_file.write_text(actual_markdown)

    # Load expected if available
    expected_markdown = None
    if expected_path and expected_path.exists():
        expected_markdown = expected_path.read_text()

    # Generate diagnostics
    return generate_diagnostics(
        pdf_path.name,
        category,
        actual_markdown,
        expected_markdown
    )


def run_evaluation(
    categories: Optional[List[str]] = None,
    verbose: bool = False,
    diagnose: bool = False
) -> RunResult:
    """Run full evaluation across corpus."""

    corpus = get_corpus_files()

    if categories:
        corpus = {k: v for k, v in corpus.items() if k in categories}

    if not corpus:
        print("No PDF files found in corpus/")
        print(f"Add PDFs to: {CORPUS_DIR}/<category>/")
        print(f"Categories: {', '.join(CATEGORIES)}")
        sys.exit(1)

    # Create output directory for this run
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    output_dir = RUNS_DIR / f"output_{timestamp}"

    all_diagnostics = []
    by_category = {}

    total_files = sum(len(files) for files in corpus.values())
    print(f"Evaluating {total_files} files across {len(corpus)} categories...\n")

    for category, files in corpus.items():
        if not files:
            continue

        print(f"[{category}]")
        category_diagnostics = []

        for pdf_path, expected_path in files:
            print(f"  {pdf_path.name}...", end=" ", flush=True)

            diag = evaluate_file(pdf_path, expected_path, category, output_dir)
            category_diagnostics.append(diag)
            all_diagnostics.append(diag)

            status = "PASS" if diag.passed else "FAIL"
            print(status)

            if diagnose and not diag.passed:
                print(f"    Reasons: {', '.join(diag.failure_reasons)}")
                if diag.likely_causes:
                    print(f"    Likely causes: {diag.likely_causes[0]}")

        # Category summary
        passed = sum(1 for d in category_diagnostics if d.passed)
        by_category[category] = {
            'total': len(files),
            'passed': passed,
            'failed': len(files) - passed,
            'avg_empty_rate': sum(d.empty_section_rate for d in category_diagnostics) / len(category_diagnostics),
            'avg_wer': None,
            'avg_heading_f1': None
        }

        # Calculate averages for metrics that have values
        wer_values = [d.wer for d in category_diagnostics if d.wer is not None]
        if wer_values:
            by_category[category]['avg_wer'] = sum(wer_values) / len(wer_values)

        f1_values = [d.heading_f1 for d in category_diagnostics if d.heading_f1 is not None]
        if f1_values:
            by_category[category]['avg_heading_f1'] = sum(f1_values) / len(f1_values)

        print()

    # Build run result
    passed_files = sum(1 for d in all_diagnostics if d.passed)

    run = RunResult(
        timestamp=timestamp,
        git_hash=get_git_hash(),
        total_files=total_files,
        passed_files=passed_files,
        failed_files=total_files - passed_files,
        by_category=by_category,
        diagnostics=all_diagnostics,
        avg_empty_rate=sum(d.empty_section_rate for d in all_diagnostics) / len(all_diagnostics) if all_diagnostics else 0
    )

    # Calculate overall averages
    wer_values = [d.wer for d in all_diagnostics if d.wer is not None]
    if wer_values:
        run.avg_wer = sum(wer_values) / len(wer_values)

    f1_values = [d.heading_f1 for d in all_diagnostics if d.heading_f1 is not None]
    if f1_values:
        run.avg_heading_f1 = sum(f1_values) / len(f1_values)

    return run


def print_report(run: RunResult, previous: Optional[Dict] = None):
    """Print evaluation report with comparison to previous run."""

    print("=" * 70)
    print("PDF EXTRACTION EVALUATION REPORT")
    print("=" * 70)
    print(f"Timestamp: {run.timestamp}")
    if run.git_hash:
        print(f"Git commit: {run.git_hash}")
    print()

    # Overall results
    pass_rate = run.passed_files / run.total_files * 100 if run.total_files > 0 else 0
    print(f"OVERALL: {run.passed_files}/{run.total_files} passed ({pass_rate:.0f}%)")

    # Comparison with previous
    if previous:
        prev_pass_rate = previous['passed_files'] / previous['total_files'] * 100
        delta = pass_rate - prev_pass_rate
        indicator = "+" if delta > 0 else ""
        print(f"  vs previous: {indicator}{delta:.0f}% ({previous['git_hash'] or 'unknown'})")

    print()

    # Metrics summary
    print("METRICS:")
    print(f"  Empty section rate: {run.avg_empty_rate:.1%}", end="")
    if previous and 'avg_empty_rate' in previous:
        delta = run.avg_empty_rate - previous['avg_empty_rate']
        print(f" ({'+' if delta > 0 else ''}{delta:.1%})", end="")
    print()

    if run.avg_wer is not None:
        print(f"  Word Error Rate: {run.avg_wer:.1%}", end="")
        if previous and previous.get('avg_wer'):
            delta = run.avg_wer - previous['avg_wer']
            print(f" ({'+' if delta > 0 else ''}{delta:.1%})", end="")
        print()

    if run.avg_heading_f1 is not None:
        print(f"  Heading F1: {run.avg_heading_f1:.1%}", end="")
        if previous and previous.get('avg_heading_f1'):
            delta = run.avg_heading_f1 - previous['avg_heading_f1']
            print(f" ({'+' if delta > 0 else ''}{delta:.1%})", end="")
        print()

    print()

    # By category
    print("BY CATEGORY:")
    for category, stats in run.by_category.items():
        status = "PASS" if stats['failed'] == 0 else "FAIL"
        print(f"  {category}: {stats['passed']}/{stats['total']} [{status}]")
        print(f"    empty_rate={stats['avg_empty_rate']:.1%}", end="")
        if stats['avg_wer'] is not None:
            print(f", wer={stats['avg_wer']:.1%}", end="")
        if stats['avg_heading_f1'] is not None:
            print(f", heading_f1={stats['avg_heading_f1']:.1%}", end="")
        print()

    print()

    # Failed files with diagnostics
    failed = [d for d in run.diagnostics if not d.passed]
    if failed:
        print("FAILURES:")
        for diag in failed:
            print(f"\n  {diag.category}/{diag.pdf_name}")
            for reason in diag.failure_reasons:
                print(f"    - {reason}")
            if diag.likely_causes:
                print(f"    Likely cause: {diag.likely_causes[0]}")
            if diag.suggested_fixes:
                print(f"    Suggested fix: {diag.suggested_fixes[0]}")
            if diag.empty_sections:
                print(f"    Empty sections: {[e['heading'][:30] for e in diag.empty_sections[:3]]}")

    print()
    print("=" * 70)

    return 0 if run.failed_files == 0 else 1


def print_history():
    """Show evaluation history over time."""
    run_files = sorted(RUNS_DIR.glob("run_*.json"))

    if not run_files:
        print("No evaluation history found.")
        return

    print("EVALUATION HISTORY")
    print("=" * 70)
    print(f"{'Timestamp':<20} {'Git':<10} {'Pass Rate':<12} {'Empty Rate':<12} {'WER':<10}")
    print("-" * 70)

    for run_file in run_files[-10:]:  # Last 10 runs
        data = json.loads(run_file.read_text())
        pass_rate = data['passed_files'] / data['total_files'] * 100

        print(f"{data['timestamp']:<20} "
              f"{(data.get('git_hash') or 'unknown'):<10} "
              f"{pass_rate:>6.0f}%      "
              f"{data.get('avg_empty_rate', 0)*100:>6.1f}%      "
              f"{(data.get('avg_wer') or 0)*100:>6.1f}%")


# =============================================================================
# Baseline Comparison
# =============================================================================

def run_comparison(tool: str, categories: Optional[List[str]] = None):
    """Compare our extraction against a baseline tool."""
    try:
        from tools.baselines import extract_with_tool, check_tool_available, AVAILABLE_TOOLS
    except ImportError:
        print("Error: Could not import baseline tools")
        sys.exit(1)

    # Check tool availability
    available, status = check_tool_available(tool)
    if not available:
        print(f"Tool '{tool}' not available: {status}")
        sys.exit(1)

    corpus = get_corpus_files()
    if categories:
        corpus = {k: v for k, v in corpus.items() if k in categories}

    if not corpus:
        print("No PDF files found in corpus/")
        sys.exit(1)

    print(f"Comparing 'ours' vs '{tool}' ({AVAILABLE_TOOLS.get(tool, '')})")
    print("=" * 70)

    results = []

    for category, files in corpus.items():
        if not files:
            continue

        print(f"\n[{category}]")

        for pdf_path, expected_path in files:
            print(f"  {pdf_path.name}:")

            # Run both extractors
            try:
                ours_md = run_extraction(pdf_path)
                baseline_md = extract_with_tool(tool, pdf_path)
            except Exception as e:
                print(f"    Error: {e}")
                continue

            # Compare against expected if available
            if expected_path and expected_path.exists():
                expected = expected_path.read_text()

                ours_wer = calculate_wer(expected, ours_md)
                baseline_wer = calculate_wer(expected, baseline_md)

                ours_coverage = calculate_content_coverage(expected, ours_md)
                baseline_coverage = calculate_content_coverage(expected, baseline_md)

                # Determine winner
                ours_score = (1 - ours_wer) + ours_coverage
                baseline_score = (1 - baseline_wer) + baseline_coverage

                winner = "ours" if ours_score > baseline_score else tool
                winner_icon = "<-" if winner == "ours" else "->"

                print(f"    ours:     WER={ours_wer:.1%}, coverage={ours_coverage:.1%}")
                print(f"    {tool}:  WER={baseline_wer:.1%}, coverage={baseline_coverage:.1%}")
                print(f"    Winner: {winner} {winner_icon}")

                results.append({
                    'file': pdf_path.name,
                    'category': category,
                    'ours_wer': ours_wer,
                    'baseline_wer': baseline_wer,
                    'ours_coverage': ours_coverage,
                    'baseline_coverage': baseline_coverage,
                    'winner': winner
                })
            else:
                # No expected file - just compare lengths
                print(f"    ours:     {len(ours_md)} chars, {len(ours_md.split())} words")
                print(f"    {tool}:  {len(baseline_md)} chars, {len(baseline_md.split())} words")
                print(f"    (No expected file for comparison)")

    # Summary
    if results:
        print("\n" + "=" * 70)
        print("COMPARISON SUMMARY")
        print("=" * 70)

        ours_wins = sum(1 for r in results if r['winner'] == 'ours')
        baseline_wins = len(results) - ours_wins

        print(f"  ours wins: {ours_wins}/{len(results)}")
        print(f"  {tool} wins: {baseline_wins}/{len(results)}")

        avg_ours_wer = sum(r['ours_wer'] for r in results) / len(results)
        avg_baseline_wer = sum(r['baseline_wer'] for r in results) / len(results)

        print(f"\n  Average WER:")
        print(f"    ours:     {avg_ours_wer:.1%}")
        print(f"    {tool}:  {avg_baseline_wer:.1%}")


def list_tools():
    """List available baseline tools."""
    try:
        from tools.baselines import list_available_tools
        list_available_tools()
    except ImportError:
        print("Available tools: ours, markitdown, gemini, claude")
        print("Run 'python tools/baselines.py' for detailed status")


# =============================================================================
# CLI
# =============================================================================

def main():
    parser = argparse.ArgumentParser(description="PDF Extraction Evaluation")
    parser.add_argument("--category", "-c", choices=CATEGORIES, help="Evaluate single category")
    parser.add_argument("--diagnose", "-d", action="store_true", help="Show detailed diagnostics")
    parser.add_argument("--verbose", "-v", action="store_true", help="Verbose output")
    parser.add_argument("--history", action="store_true", help="Show evaluation history")
    parser.add_argument("--no-save", action="store_true", help="Don't save run results")
    parser.add_argument("--compare", metavar="TOOL", help="Compare against baseline (markitdown, gemini, claude)")
    parser.add_argument("--list-tools", action="store_true", help="List available baseline tools")

    args = parser.parse_args()

    if args.list_tools:
        list_tools()
        return 0

    if args.history:
        print_history()
        return 0

    if args.compare:
        categories = [args.category] if args.category else None
        run_comparison(args.compare, categories)
        return 0

    # Run evaluation
    categories = [args.category] if args.category else None
    run = run_evaluation(categories, args.verbose, args.diagnose)

    # Load previous for comparison
    previous = load_previous_run()

    # Save this run
    if not args.no_save:
        save_run(run)

    # Print report
    exit_code = print_report(run, previous)

    sys.exit(exit_code)


if __name__ == "__main__":
    main()
