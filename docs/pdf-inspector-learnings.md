# PDF Inspector (firecrawl/pdf-inspector) — consolidated learnings

Consolidated from two independent evaluations (2026-08-04). Research pin:
commit `3fb545284bec8bfd86cf2a1445083b8e2832625b` (Rust crate 0.1.7, MIT).

## Verdict (both evaluations agree)

**Do not adopt as a dependency — for extraction, fallback, or detection.**

- It duplicates ~80% of what this repo already does (extraction, ToUnicode/CID,
  font-ratio headings, tables, columns, markdown), and this fork is more mature
  there (hyphenation repair, header/footer stripping, visibility, quality metrics).
- It adds **no parser diversity**: it is another lopdf consumer (0.42), so it
  shares common-mode failures with us. For a true malformed-PDF fallback,
  Poppler/MuPDF would be the strategic choice, not a second lopdf stack.
- Its output contract (agent-oriented Markdown + TextItems) does not match the
  RZN chunk/location/output-span contract; adapting it is a partial rewrite.
- It provides no isolation, no resource budgets, no cancellation.
- Open correctness issues in exactly the areas we'd want it for: no xref
  rebuild (#228), contradictory OCR verdicts between its own APIs (#227),
  column detection without reading-order fix (#219), Arabic/RTL routing and
  tail-latency problems (#213).
- Release hygiene is weak: three uncoordinated version streams, no GitHub
  releases, doc/code drift (README says full-scan default; code defaults to
  `Sample(8)`), mixed 0-based/1-based page indexing across result types.

The only defensible direct use is a **pinned, detect-only shadow classifier**
inside an isolated worker, telemetry-only, for calibration against our corpus.

## What to port (MIT permits; retain attribution; audit fixture/CMap provenance separately)

Ranked by value:

1. **Fast pre-classification (detector.rs concepts, not the module).**
   Sample content streams (default: 8 pages — first/last/middle spread) and
   count operators: `Tj`/`TJ`/`'`/`"` (text), `Tf` (fonts), `Do` + XObject
   analysis (images), path ops (vector). Field-tested thresholds: ≥3 text ops
   per page; 60% text-page ratio ⇒ TextBased; 1000+ path ops at >200× text ops
   ⇒ vector-outlined text; 500K px area ⇒ full-page template image.
   We currently decide OCR *after* full extraction via `quality.rs`; this moves
   routing upfront and makes empty extractions attributable (continues the
   work in 34ce18b / 25b112c).

2. **One canonical per-page assessment with reason codes.**
   `scanned` / `no_text` / `vector_text` / `suspected_garbled_text` + confidence,
   reused by both classification and extraction. Their issue #227 (two APIs
   disagreeing on OCR for the same page) is the failure mode to design against:
   compute the verdict once, thread it everywhere.

3. **Text-quality checks (`quality.rs`).** Replacement-char runs,
   private-use/C1 characters, broken-CMap artifacts, symbol-dominant output,
   substitution-cipher-like text. Caveat: some thresholds assume Latin/English
   frequency — recalibrate against our corpus before trusting them.

4. **Regression methodology.** Paired baseline/candidate runs over the same
   corpus revision, per-document deltas, rejection of missing predictions, a
   max per-document regression threshold, and an independent-engine (MuPDF)
   diagnostic probe that never becomes a runtime dependency. Adopt the
   discipline; our `benchmarks/` harness is the natural home.

5. **`estimate_page_count_from_bytes`** — when parsing fails entirely, scan raw
   bytes for `/Type /Page` to still report a page count. Cheap; pairs with the
   pre-classifier for attribution.

6. **Typed error taxonomy.** `NotAPdf` / `Encrypted` / `InvalidStructure` /
   `Parse` / `Io` beats string errors; extend with phase, page, retryability,
   and whether a repair was attempted.

7. **Bounded, auditable container repairs.** BOM/leading-garbage stripping,
   truncated `%%EOF` repair, bare structure-name normalization — deterministic,
   recorded (original hash + repair applied). Not general xref rebuilding.

8. **Encrypted-PDF ergonomics.** Optional password, empty-password fallback for
   owner-only protection, password-redacted Debug, and distinct outcomes for
   missing vs wrong password vs unsupported handler.

**Design reference only** (port a piece only after it wins a targeted fixture
category): the full font subsystem, column/reading-order engine, table
reconstruction, markdown renderer, structure-tree handling, region extraction,
multi-language packaging.

## Pre-existing issues surfaced by the evaluation (higher priority than any port)

### This repo (rag_pdf_extract)

- **lopdf 0.38 is vulnerable to RUSTSEC-2026-0187**: a ~21KB PDF with deeply
  nested arrays/dicts causes an uncatchable stack-overflow SIGABRT. Fixed in
  lopdf ≥0.42. Upgrade (pdf-inspector's own upgrade commit
  `1c32e4bd691bde83778ffef235019c8feac0c0c5` is a reference) and add the
  deep-nesting fixture to tests. **This is action #1.**

### RZN backend (crates/rzn-extract — consumer of this repo)

- Same lopdf 0.38 exposure via its lockfile.
- The declared 20,000-page gate is **inert** on the PDF path: the processor
  leaves `page_count` at default/None, and the gate only checks that field.
- Worker containment is timeout+kill only — no CPU/RSS/thread/PID/output
  budgets; worker stdout/stderr read into unbounded buffers; the 50M-char
  output check runs only after extraction.

## Consolidated action order

1. Upgrade lopdf to ≥0.42 here (and in the backend); add the RUSTSEC-2026-0187
   nesting fixture.
2. Backend: fix the inert page gate; add hard worker resource budgets.
3. Build the pre-classifier in this repo: sampling scan + canonical per-page
   assessment with reason codes + byte-level page-count fallback.
4. Adopt the paired-regression discipline in `benchmarks/`.
5. Typed errors + observability fields (engine SHA, phase timings, repair/
   routing metadata).
6. Optional: pinned detect-only shadow run of pdf-inspector inside the worker
   for threshold calibration — telemetry only, never canonical output.
