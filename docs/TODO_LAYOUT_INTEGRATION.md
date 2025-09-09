PDF Layout Integration TODO (PDFMiner‑inspired)

Goal: Port the core layout heuristics (chars→lines, lines→text boxes, reading order) while keeping our page‑streaming and token‑aware chunking. Prioritize accuracy; keep knobs tunable.

Phase 0 — Baseline drop fixes [Completed]
- [x] Honor text rendering mode (Tr=3 invisible) instead of color‑based dropping by default.
- [x] Make near‑white color suppression optional via `PDF_EXTRACT_DROP_NEAR_WHITE`.
- [x] Always flush current_line on font size change (avoid silent loss).

Phase 1 — Config surface (LAParams)
- [ ] Define `LAParams` with: `char_margin f32`, `word_margin f32`, `line_overlap f32`, `line_margin f32`, `boxes_flow f32`, `detect_vertical bool`.
- [ ] Expose params via `parse_pdf(..., laparams: Option<LAParams>)` and examples/CLI flags.
- [ ] Add sane defaults: `char_margin=2.0`, `word_margin=0.1`, `line_overlap=0.5`, `line_margin=0.5`, `boxes_flow=0.5`, `detect_vertical=false`.

Phase 2 — Character capture + spacing
- [ ] Capture per‑glyph measurements: char, device‑space bbox, advance (w0×font_size×scale), rotation.
- [ ] Space insertion: insert space when inter‑glyph gap > `word_margin × max(prev_width, width)`.
- [ ] Normalize kerning from TJ arrays into gaps so rules are uniform across Tj/TJ.

Phase 3 — Group chars → lines
- [ ] Sort glyphs top‑to‑bottom, left‑to‑right (stable by Y then X).
- [ ] Group glyphs onto a line if vertical overlap ≥ `line_overlap × min(h1,h2)` and horizontal gap ≤ `char_margin × width`.
- [ ] Output `Line { glyphs, bbox, baseline_y, font_stats }`.

Phase 4 — Group lines → text boxes
- [ ] Compute vertical gap between consecutive lines; group when gap < `line_margin × line_height` AND horizontal projections overlap (≥ threshold, e.g. 0.2).
- [ ] Optional indent relaxation: allow first‑line indent within X pixels to join paragraph.
- [ ] Output `TextBox { lines, bbox, page }`.

Phase 5 — Reading order and chunking
- [ ] Order `TextBox` items by a key influenced by `boxes_flow` (balance Y vs X).
- [ ] Emit one chunk per TextBox, then apply token‑limit splitting.
- [ ] Aggregate PdfLocation fragments from contained lines/glyphs.

Phase 6 — Margins, headers, footers
- [ ] Add band filters (top/bottom ratios), repetition detection across pages.
- [ ] Config: `{header_band, footer_band, min_len, drop_repeated}`.

Phase 7 — Vertical text & rotations
- [ ] Detect rotated/vertical runs; when `detect_vertical`, group with swapped axes and include in output order.

Phase 8 — Tests, metrics, and docs
- [ ] Unit tests for char→line and line→box grouping.
- [ ] Integration tests: multi‑column, headings, white‑on‑dark, rotated labels.
- [ ] Metrics: count spaces inserted, lines/boxes per page, dropped glyphs.
- [ ] Update README and examples with LAParams usage.

Notes
- Keep performance acceptable: process per‑page; data structures should be lightweight (Vecs; optional spatial index later).
- Avoid changing existing public schema; extend via optional laparams and richer internals.

