#!/usr/bin/env python3
"""Print OCR text geometry without retaining book text (requires installed pypdf)."""
import argparse
import hashlib
import json
from pathlib import Path

from pypdf import PdfReader
from pypdf.generic import ContentStream


def geometry(path, page_number):
    reader = PdfReader(path)
    page = reader.pages[page_number - 1]
    matrix = None
    scale = 100
    font_size = None
    samples = []
    shows = 0
    skewed_shows = 0
    for args, op in ContentStream(page.get_contents(), reader).operations:
        if op == b"Tm":
            matrix = [float(value) for value in args]
        elif op == b"Tz":
            scale = float(args[0])
        elif op == b"Tf":
            font_size = float(args[1])
        elif op in (b"Tj", b"TJ", b"'", b'"'):
            shows += 1
            if matrix is not None and (matrix[1] != 0 or matrix[2] != 0):
                skewed_shows += 1
                if len(samples) < 8:
                    samples.append({"text_matrix": matrix, "horizontal_scale_percent": scale,
                                    "font_size": font_size})
    return {"source_pdf": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "page": page_number, "text_show_operations": shows,
            "skewed_text_show_operations": skewed_shows, "first_skewed_samples": samples}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pdf", type=Path)
    parser.add_argument("page", type=int, help="1-based page number")
    args = parser.parse_args()
    if args.page < 1:
        parser.error("page must be positive")
    print(json.dumps(geometry(args.pdf, args.page), indent=2))


if __name__ == "__main__":
    main()
