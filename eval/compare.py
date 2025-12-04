#!/usr/bin/env python3
"""
Compare extractions from different tools side-by-side.

Usage:
    python compare.py                           # Compare all, show metrics table
    python compare.py --pdf "1.pdf"             # Single file comparison
    python compare.py --diff ours gemini        # Show diff between two tools
    python compare.py --category legal          # Single category
"""

import argparse
import sys
import difflib
from pathlib import Path
from collections import defaultdict

EVAL_DIR = Path(__file__).parent
CORPUS_DIR = EVAL_DIR / "corpus"
CATEGORIES = ["legal", "scientific", "financial", "scanned", "mixed"]
TOOLS = ["ours", "gemini", "markitdown"]


def get_extraction_path(pdf_path: Path, tool: str) -> Path:
    """Get the extraction file path for a PDF and tool."""
    return pdf_path.with_suffix(f".{tool}.md")


def load_extraction(pdf_path: Path, tool: str) -> str | None:
    """Load extraction content, stripping metadata header."""
    ext_path = get_extraction_path(pdf_path, tool)
    if not ext_path.exists():
        return None

    content = ext_path.read_text(encoding="utf-8")

    # Strip metadata header if present
    if content.startswith("<!--"):
        end_comment = content.find("-->")
        if end_comment != -1:
            content = content[end_comment + 3:].strip()

    return content


def calculate_wer(text1: str, text2: str) -> float:
    """Word Error Rate between two texts."""
    words1 = text1.split()
    words2 = text2.split()

    if not words1:
        return 0.0 if not words2 else 1.0

    matcher = difflib.SequenceMatcher(None, words1, words2)
    insertions = deletions = substitutions = 0

    for tag, i1, i2, j1, j2 in matcher.get_opcodes():
        if tag == 'replace':
            substitutions += max(i2 - i1, j2 - j1)
        elif tag == 'delete':
            deletions += i2 - i1
        elif tag == 'insert':
            insertions += j2 - j1

    return (insertions + deletions + substitutions) / len(words1)


def count_empty_sections(markdown: str) -> tuple[int, int]:
    """Count total sections and empty sections."""
    lines = markdown.split('\n')
    total = 0
    empty = 0

    for i, line in enumerate(lines):
        if line.startswith('#'):
            total += 1
            # Check if next non-empty line is another heading
            has_content = False
            for j in range(i + 1, len(lines)):
                next_line = lines[j].strip()
                if next_line.startswith('#'):
                    break
                if next_line:
                    has_content = True
                    break
            if not has_content:
                empty += 1

    return total, empty


def get_stats(content: str) -> dict:
    """Get basic stats for content."""
    if content is None:
        return {"chars": 0, "words": 0, "sections": 0, "empty_sections": 0}

    sections, empty = count_empty_sections(content)
    return {
        "chars": len(content),
        "words": len(content.split()),
        "sections": sections,
        "empty_sections": empty,
        "empty_rate": f"{empty/sections*100:.1f}%" if sections > 0 else "N/A"
    }


def compare_single_pdf(pdf_path: Path, verbose: bool = False):
    """Compare all extractions for a single PDF."""
    print(f"\n{pdf_path.name}")
    print("-" * 60)

    extractions = {}
    for tool in TOOLS:
        content = load_extraction(pdf_path, tool)
        if content is not None:
            extractions[tool] = content

    if not extractions:
        print("  No extractions found")
        return

    # Print stats for each
    print(f"{'Tool':<12} {'Chars':>10} {'Words':>10} {'Sections':>10} {'Empty':>10}")
    print(f"{'-'*12} {'-'*10} {'-'*10} {'-'*10} {'-'*10}")

    for tool in TOOLS:
        if tool in extractions:
            stats = get_stats(extractions[tool])
            print(f"{tool:<12} {stats['chars']:>10,} {stats['words']:>10,} "
                  f"{stats['sections']:>10} {stats['empty_rate']:>10}")
        else:
            print(f"{tool:<12} {'(missing)':>10}")

    # WER comparison (if we have at least 2)
    if len(extractions) >= 2:
        print(f"\nWER (Word Error Rate):")
        tools_with_content = list(extractions.keys())
        for i, t1 in enumerate(tools_with_content):
            for t2 in tools_with_content[i+1:]:
                wer = calculate_wer(extractions[t1], extractions[t2])
                print(f"  {t1} vs {t2}: {wer:.1%}")


def compare_all(categories: list[str] | None = None):
    """Compare all PDFs, show summary table."""
    print("EXTRACTION COMPARISON")
    print("=" * 80)

    results = []

    for category in (categories or CATEGORIES):
        category_dir = CORPUS_DIR / category
        if not category_dir.exists():
            continue

        for pdf_path in sorted(category_dir.glob("*.pdf")):
            row = {"category": category, "pdf": pdf_path.name}

            for tool in TOOLS:
                content = load_extraction(pdf_path, tool)
                stats = get_stats(content)
                row[f"{tool}_chars"] = stats["chars"]
                row[f"{tool}_words"] = stats["words"]
                row[f"{tool}_empty"] = stats["empty_rate"]

            # Calculate WER between ours and gemini (if both exist)
            ours = load_extraction(pdf_path, "ours")
            gemini = load_extraction(pdf_path, "gemini")
            if ours and gemini:
                row["wer_vs_gemini"] = calculate_wer(gemini, ours)
            else:
                row["wer_vs_gemini"] = None

            results.append(row)

    # Print table
    print(f"\n{'PDF':<45} {'Ours':>12} {'Gemini':>12} {'MarkIt':>12} {'WER':>8}")
    print(f"{'':<45} {'(words)':>12} {'(words)':>12} {'(words)':>12} {'vs Gem':>8}")
    print("-" * 95)

    for row in results:
        pdf_name = row['pdf'][:42] + "..." if len(row['pdf']) > 45 else row['pdf']
        wer_str = f"{row['wer_vs_gemini']:.1%}" if row['wer_vs_gemini'] is not None else "N/A"

        print(f"{pdf_name:<45} "
              f"{row['ours_words']:>12,} "
              f"{row['gemini_words']:>12,} "
              f"{row['markitdown_words']:>12,} "
              f"{wer_str:>8}")

    # Summary
    print("-" * 95)
    ours_total = sum(r['ours_words'] for r in results)
    gemini_total = sum(r['gemini_words'] for r in results)
    markitdown_total = sum(r['markitdown_words'] for r in results)

    print(f"{'TOTAL':<45} {ours_total:>12,} {gemini_total:>12,} {markitdown_total:>12,}")

    # Average WER
    wer_values = [r['wer_vs_gemini'] for r in results if r['wer_vs_gemini'] is not None]
    if wer_values:
        avg_wer = sum(wer_values) / len(wer_values)
        print(f"\nAverage WER (ours vs gemini): {avg_wer:.1%}")


def show_diff(pdf_path: Path, tool1: str, tool2: str, context: int = 3):
    """Show unified diff between two extractions."""
    content1 = load_extraction(pdf_path, tool1)
    content2 = load_extraction(pdf_path, tool2)

    if content1 is None:
        print(f"No extraction found for {tool1}")
        return
    if content2 is None:
        print(f"No extraction found for {tool2}")
        return

    lines1 = content1.splitlines(keepends=True)
    lines2 = content2.splitlines(keepends=True)

    diff = difflib.unified_diff(
        lines1, lines2,
        fromfile=f"{pdf_path.stem}.{tool1}.md",
        tofile=f"{pdf_path.stem}.{tool2}.md",
        n=context
    )

    diff_text = ''.join(diff)
    if diff_text:
        print(diff_text)
    else:
        print("Files are identical")


def main():
    parser = argparse.ArgumentParser(description="Compare extractions from different tools")
    parser.add_argument("--pdf", "-p", help="Compare single PDF file")
    parser.add_argument("--category", "-c", choices=CATEGORIES, help="Single category")
    parser.add_argument("--diff", nargs=2, metavar=("TOOL1", "TOOL2"),
                       help="Show diff between two tools (requires --pdf)")
    parser.add_argument("--context", type=int, default=3,
                       help="Lines of context for diff (default: 3)")

    args = parser.parse_args()

    if args.diff:
        if not args.pdf:
            print("Error: --diff requires --pdf")
            sys.exit(1)

        # Find the PDF
        pdf_path = None
        for category in CATEGORIES:
            candidate = CORPUS_DIR / category / args.pdf
            if candidate.exists():
                pdf_path = candidate
                break

        if not pdf_path:
            print(f"PDF not found: {args.pdf}")
            sys.exit(1)

        show_diff(pdf_path, args.diff[0], args.diff[1], args.context)

    elif args.pdf:
        # Find and compare single PDF
        pdf_path = None
        for category in CATEGORIES:
            candidate = CORPUS_DIR / category / args.pdf
            if candidate.exists():
                pdf_path = candidate
                break

        if not pdf_path:
            print(f"PDF not found: {args.pdf}")
            sys.exit(1)

        compare_single_pdf(pdf_path, verbose=True)

    else:
        # Compare all
        categories = [args.category] if args.category else None
        compare_all(categories)


if __name__ == "__main__":
    main()
