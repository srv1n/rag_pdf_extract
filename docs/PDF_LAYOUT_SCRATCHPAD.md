PDF Layout Scratchpad

Purpose: Working notes while implementing PDFMiner‑style layout. Use this to log parameters, edge cases, and decisions.

Candidate Defaults (tune per corpus)
- char_margin: 2.0  (chars on same line if gap ≤ 2× char width)
- word_margin: 0.10 (inject space if gap > 0.1× char width)
- line_overlap: 0.5 (≥50% vertical overlap → same line)
- line_margin: 0.5  (vert gap < 0.5× line height → same paragraph)
- boxes_flow: 0.5   (balance top‑down and left‑to‑right)
- detect_vertical: false

Open Questions
- Horizontal overlap threshold for lines→box (0.2 vs 0.3). Check impact on indented first lines.
- Handling ligatures: rely on ToUnicode only or fallback splitting heuristics if single glyph → multi‑char.
- Multi‑script pages: per‑script margins or unified parameters?

Edge‑Case Corpus (local files)
- 9.pdf, 10.pdf (multi‑column/headings)
- ocr.pdf (OCR fallback interplay)
- White‑on‑dark banners (verify near‑white suppression off by default)
- Rotated table headers (test detect_vertical)

Instrumentation ideas
- Counters: glyphs seen, glyphs kept, spaces inserted, lines formed, boxes formed.
- Dump top‑N pages with most boxes for quick visual check.

Validation Checks
- No empty lines/boxes after trimming.
- Word boundary accuracy: <1% concatenated words on sampled pages.
- Reading order sanity: left column before right on two‑column pages.

Decision Log
- 2025‑09‑08: Disabled default near‑white suppression; rely on Tr for invisibility.
- 2025‑09‑08: Flush on font‑size change to avoid losing partial lines.

