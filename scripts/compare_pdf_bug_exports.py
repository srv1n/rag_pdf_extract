#!/usr/bin/env python3
"""Compare export_chunks JSON against Poppler without retaining book text.

Run examples/export_chunks on each original first, then:
python3 scripts/compare_pdf_bug_exports.py --originals DIR --exports DIR --output JSON
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess


def words(text):
    # Match the shelf report's counting convention, including Portuguese text.
    return re.findall(r"[A-Za-z']+", text)


def compare(originals, exports):
    results = []
    for path in sorted(exports.glob("bug-*-current.json")):
        export = json.loads(path.read_text())
        original = originals / export["source_pdf"]
        reference = subprocess.run(
            ["pdftotext", str(original), "-"], check=True, capture_output=True
        ).stdout.decode("utf-8", errors="replace")
        text = "\n".join(chunk["text"] for chunk in export["chunks"])
        actual_words, reference_words = words(text), words(reference)
        results.append({
            "source_pdf": original.name,
            "sha256": hashlib.sha256(original.read_bytes()).hexdigest(),
            "chunks": len(export["chunks"]),
            "extracted_words": len(actual_words),
            "poppler_words": len(reference_words),
            "word_ratio": len(actual_words) / max(len(reference_words), 1),
            "extracted_long_word_fraction": sum(len(w) >= 20 for w in actual_words)
                / max(len(actual_words), 1),
            "poppler_long_word_fraction": sum(len(w) >= 20 for w in reference_words)
                / max(len(reference_words), 1),
            "extracted_single_letter_fraction": sum(len(w) == 1 for w in actual_words)
                / max(len(actual_words), 1),
            "poppler_single_letter_fraction": sum(len(w) == 1 for w in reference_words)
                / max(len(reference_words), 1),
            "pages_with_content": len({
                page for chunk in export["chunks"] for page in chunk["pages"] or []
            }),
        })
    if not results:
        raise ValueError("no bug-*-current.json exports found")
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--originals", required=True, type=Path)
    parser.add_argument("--exports", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    assert words("Oneofthemostcomprehensive story") == [
        "Oneofthemostcomprehensive", "story"
    ]
    results = compare(args.originals, args.exports)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(results, indent=2) + "\n")
    for result in results:
        print(f'{result["source_pdf"]}: {result["extracted_words"]}/'
              f'{result["poppler_words"]} words ({result["word_ratio"]:.3%}), '
              f'{result["pages_with_content"]} pages')


if __name__ == "__main__":
    main()
