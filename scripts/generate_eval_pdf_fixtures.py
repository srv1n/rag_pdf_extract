#!/usr/bin/env python3
"""Generate tiny deterministic PDF fixtures for parser regression tests."""

from pathlib import Path
from typing import List, Optional


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "eval" / "fixtures" / "pdfs"


def pdf_string(value: str) -> str:
    return value.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")


def write_pdf(
    path: Path,
    operations: str,
    media_box: str = "0 0 612 792",
    crop_box: Optional[str] = None,
    rotate: Optional[int] = None,
) -> None:
    stream = operations.encode("ascii")
    page_attrs = [f"/MediaBox [{media_box}]"]
    if crop_box is not None:
        page_attrs.append(f"/CropBox [{crop_box}]")
    if rotate is not None:
        page_attrs.append(f"/Rotate {rotate}")
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        (
            f"<< /Type /Page /Parent 2 0 R {' '.join(page_attrs)} "
            f"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
        ).encode("ascii"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Length %d >>\nstream\n%s\nendstream" % (len(stream), stream),
    ]

    write_objects(path, objects)


def write_objects(path: Path, objects: List[bytes]) -> None:
    chunks = [b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n"]
    offsets = []
    for idx, obj in enumerate(objects, start=1):
        offsets.append(sum(len(chunk) for chunk in chunks))
        chunks.append(f"{idx} 0 obj\n".encode("ascii"))
        chunks.append(obj)
        chunks.append(b"\nendobj\n")

    xref_offset = sum(len(chunk) for chunk in chunks)
    chunks.append(f"xref\n0 {len(objects) + 1}\n".encode("ascii"))
    chunks.append(b"0000000000 65535 f \n")
    for offset in offsets:
        chunks.append(f"{offset:010d} 00000 n \n".encode("ascii"))
    chunks.append(
        (
            f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\n"
            f"startxref\n{xref_offset}\n%%EOF\n"
        ).encode("ascii")
    )

    path.write_bytes(b"".join(chunks))


def write_form_xobject_pdf(path: Path) -> None:
    form_stream = (
        "q\n1 0 0 1 20 20 cm\nBT\n/F1 12 Tf\n0 0 Td\n(Form Box Text) Tj\nET\nQ\n"
    ).encode("ascii")
    page_stream = b"q\n1 0 0 1 72 650 cm\n/Fm1 Do\nQ\n"
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [36 36 648 828] "
            b"/Resources << /Font << /F1 4 0 R >> /XObject << /Fm1 6 0 R >> >> "
            b"/Contents 5 0 R >>"
        ),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Length %d >>\nstream\n%s\nendstream" % (len(page_stream), page_stream),
        (
            b"<< /Type /XObject /Subtype /Form /BBox [0 0 200 60] "
            b"/Resources << /Font << /F1 4 0 R >> >> /Length %d >>\nstream\n%s\nendstream"
            % (len(form_stream), form_stream)
        ),
    ]
    write_objects(path, objects)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)

    write_pdf(
        OUT / "ctm_scaled_text.pdf",
        "q\n2 0 0 2 72 120 cm\nBT\n/F1 12 Tf\n0 0 Td\n(Scaled CTM Text) Tj\nET\nQ\n",
    )

    write_pdf(
        OUT / "invisible_ocr_only.pdf",
        "BT\n/F1 12 Tf\n3 Tr\n72 720 Td\n(Invisible OCR Layer Text) Tj\nET\n",
    )

    long_sentences = [
        "Split provenance paragraph sentence %03d with stable source text." % idx
        for idx in range(1, 80)
    ]
    long_ops = ["BT", "/F1 7 Tf", "54 760 Td"]
    for idx in range(0, len(long_sentences), 2):
        if idx > 0:
            long_ops.append("0 -14 Td")
        line = " ".join(long_sentences[idx : idx + 2])
        long_ops.append("(%s) Tj" % pdf_string(line))
    long_ops.append("ET")
    write_pdf(
        OUT / "long_split_locations.pdf",
        "\n".join(long_ops) + "\n",
    )

    write_pdf(
        OUT / "span_bbox_two_lines.pdf",
        "\n".join(
            [
                "BT",
                "/F1 12 Tf",
                "72 720 Td",
                "(First visual line) Tj",
                "0 -24 Td",
                "(Second visual line) Tj",
                "ET",
                "",
            ]
        ),
    )

    write_pdf(
        OUT / "nonzero_mediabox.pdf",
        "BT\n/F1 12 Tf\n72 720 Td\n(Nonzero MediaBox Text) Tj\nET\n",
        media_box="36 36 648 828",
    )

    write_pdf(
        OUT / "cropbox_smaller.pdf",
        "BT\n/F1 12 Tf\n72 720 Td\n(CropBox Text) Tj\nET\n",
        media_box="0 0 612 792",
        crop_box="36 72 576 720",
    )

    for rotation in (90, 180, 270):
        write_pdf(
            OUT / f"rotate_{rotation}.pdf",
            f"BT\n/F1 12 Tf\n72 720 Td\n(Rotate {rotation} Text) Tj\nET\n",
            rotate=rotation,
        )

    write_form_xobject_pdf(OUT / "nonzero_form_xobject.pdf")

    write_pdf(
        OUT / "malformed_text_ops.pdf",
        "\n".join(
            [
                "q",
                "1 0 cm",
                "BT",
                "72 720 Td",
                "(Text before font should be skipped) Tj",
                "123 Tj",
                "1 0 0 1 72 Tm",
                "1 2 TD",
                "ET",
                "Q",
                "",
            ]
        ),
    )


if __name__ == "__main__":
    main()
