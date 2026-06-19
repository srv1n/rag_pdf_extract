#!/usr/bin/env python3
from __future__ import annotations

import argparse
import filecmp
import shutil
from pathlib import Path


SYNC_PATHS = [
    "Cargo.toml",
    "README.md",
    "INTEGRATION.md",
    "examples/extract.rs",
    "examples/extract_markdown.rs",
    "examples/ocr_extract.rs",
    "examples/founder_regression.rs",
    "examples/regression_pack.rs",
    "src/chunk_accumulator.rs",
    "src/core_fonts.rs",
    "src/document/mod.rs",
    "src/document/processing.rs",
    "src/document/analysis.rs",
    "src/document/columns.rs",
    "src/document/header_footer.rs",
    "src/document/hyphenation.rs",
    "src/document/lists.rs",
    "src/document/stats.rs",
    "src/document/tables.rs",
    "src/form.rs",
    "src/founder_regression.rs",
    "src/glyphlist-export.py",
    "src/glyphlist-extended.txt",
    "src/glyphnames.rs",
    "src/heading_hierarchy.rs",
    "src/layout_params.rs",
    "src/lib.rs",
    "src/pdf_image.rs",
    "src/text_splitting.rs",
    "src/zapfglyphnames.rs",
    "tests/founder_regression.rs",
    "tests/tests.rs",
]


def compare_file(src: Path, dst: Path) -> str:
    if not dst.exists():
        return "missing"
    return "same" if filecmp.cmp(src, dst, shallow=False) else "different"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Check or sync the canonical pdf-extract crate into the vendored rznapp copy."
    )
    parser.add_argument(
        "--canonical-root",
        default="/Users/sarav/Downloads/side/rzn/rag_pdf_extract",
    )
    parser.add_argument(
        "--vendored-root",
        default="/Users/sarav/Downloads/side/rzn/rznapp/crates/pdf-extract",
    )
    parser.add_argument(
        "--mode",
        choices=["check", "sync"],
        default="check",
    )
    args = parser.parse_args()

    canonical_root = Path(args.canonical_root)
    vendored_root = Path(args.vendored_root)

    changes: list[tuple[str, str]] = []
    for rel in SYNC_PATHS:
        src = canonical_root / rel
        dst = vendored_root / rel
        status = compare_file(src, dst)
        if status != "same":
            changes.append((rel, status))

    if args.mode == "sync":
        for rel, _ in changes:
            src = canonical_root / rel
            dst = vendored_root / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)
        if not changes:
            print("pdf-extract parity: already clean")
            return 0

        print(f"pdf-extract parity: synced {len(changes)} path(s)")
        for rel, status in changes:
            print(f" - {status} -> synced: {rel}")
        return 0

    if not changes:
        print("pdf-extract parity: clean")
        return 0

    print(f"pdf-extract parity: {len(changes)} path(s) out of sync")
    for rel, status in changes:
        print(f" - {status}: {rel}")

    return 0 if args.mode == "sync" else 1


if __name__ == "__main__":
    raise SystemExit(main())
