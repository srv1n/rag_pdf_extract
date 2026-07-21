#!/usr/bin/env python3
"""Generate small PDFs with page-local transforms outside the page origin."""

import os

from reportlab.pdfgen import canvas


HERE = os.path.dirname(os.path.abspath(__file__))
TEXT = "Page-relative bbox origin regression text."


def render(path, translate):
    pdf = canvas.Canvas(path, pagesize=(612, 792))
    pdf.saveState()
    pdf.translate(*translate)
    pdf.setFont("Helvetica", 12)
    pdf.drawString(72, 700, TEXT)
    pdf.restoreState()
    pdf.showPage()
    pdf.save()


render(os.path.join(HERE, "negative_x_origin.pdf"), (-220, 0))
render(os.path.join(HERE, "negative_y_origin.pdf"), (0, 600))
print("generated negative_x_origin.pdf negative_y_origin.pdf")
