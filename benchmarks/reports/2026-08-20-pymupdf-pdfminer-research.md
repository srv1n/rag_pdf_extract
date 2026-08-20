# PyMuPDF and pdfminer.six: lessons for the fast text lane

Date: 2026-08-20  
Scope: upstream documentation/source plus bounded same-host probes; these are
directional measurements, not release-gate cross-library benchmarks.

## Bottom line

The current `extract_text_fast` design is already pointed at the same low-cost
part of the problem as the fast APIs in PyMuPDF: decode page text, avoid layout
and metadata, then do a small downstream split. The largest structural gap is
not sentence splitting; it is the amount of PDF interpretation work done before
text can be emitted. PyMuPDF gets its advantage from MuPDF's native C engine and
from keeping the simple `TEXT` / `BLOCKS` extraction path cheap. pdfminer.six is
useful as a control because it exposes a genuine no-layout switch, but remains a
pure-Python interpreter and still builds layout objects around characters.

The local result is directional only: the 21-PDF corpus measured the Rust fast
lane at 12,463.6 ms total, versus 20,480.1 ms for the existing structured
no-layout path and 45,658.1 ms for layout (`all_texts=false`). A follow-up probe
on the same host used three small corpus PDFs, one warmup, and one measured run
per variant:

| Variant | Documents | Total wall ms | Median document ms | Characters |
|---|---:|---:|---:|---:|
| **Rust `extract_text_fast`** | 3 | **407.8** | **122.5** | 111,402 |
| PyMuPDF 1.28.2 `get_text("text", sort=False)` | 3 | 134.7 | 29.4 | 120,591 |
| PyMuPDF 1.28.2 `get_text("text", sort=True)` | 3 | 718.4 | 138.4 | 123,529 |
| pdfminer.six default `extract_text()` | 3 | 1,142.8 | 388.7 | 123,722 |
| pdfminer.six `extract_text_to_fp(..., laparams=None)` | 3 | 790.3 | 312.9 | 118,251 |

On this subset, PyMuPDF creator-order text was about 3.0x faster than the Rust
fast lane; the Rust lane was about 1.8x faster than PyMuPDF's coordinate sort,
1.9x faster than pdfminer.six's no-layout path, and 2.8x faster than its default
layout path. The 21-PDF Rust/PyMuPDF probe points in the same direction:
12,463.6 ms versus 4,760.4 ms for PyMuPDF `sort=False`; PyMuPDF `sort=True`
took 22,353.4 ms. These runs do not establish quality equivalence: character
totals differ because each engine handles spacing, ligatures, malformed objects,
and ordering differently. The PyMuPDF loop was sequential while the Rust fast
lane uses Rayon page workers, so CPU/RSS and process-model measurements are still
needed. See [the local Rust benchmark](2026-08-20-fast-text.md).

## Extraction paths and knobs

| Path | What it emits / orders | Main cost controls | Parallelism and OCR |
|---|---|---|---|
| **This crate: `extract_text_fast`** | `lopdf` page content is processed on Rayon workers; segments are concatenated in page order, cleaned, then split on `.`, `!`, and `?`. No locations, fonts, layout grouping, headings, tables, or OCR. See [`fast_text_chunks`](../../src/lib.rs#L6915-L7002) and [`split_fast_sentences`](../../src/lib.rs#L7005-L7033). | `max_tokens` is a cheap word-count estimate; `ExtractionOptions.max_output_bytes` and `max_recursion_depth` bound work/output. There is no public per-call Rayon worker-count knob. | Page-level `par_iter`; OCR is intentionally skipped in this override. |
| **PyMuPDF** | `Page.get_text("text")` returns plain text in the PDF creator's order; `sort=True` reorders top-left to bottom-right. `"blocks"` and `"words"` are explicitly high-speed outputs; richer `dict`/`rawdict` add span/character metadata. This is not a sentence parser or a zero-geometry path: the underlying `TextPage` still represents blocks, lines, spans, and characters. See [Page.get_text](https://github.com/pymupdf/PyMuPDF/blob/main/docs/page.rst) and [Appendix 1](https://github.com/pymupdf/PyMuPDF/blob/main/docs/app1.rst#plain-text). | Choose the cheapest output; pass flags that omit images and unnecessary features. `TEXT_INHIBIT_SPACES` changes spacing; `clip` restricts the page region. Reuse one `TextPage` for repeated formats/searches; the docs say this can cut repeated extraction by >50% and up to 95%. A `DisplayList` can also be reused. See [text flags/performance](https://github.com/pymupdf/PyMuPDF/blob/main/docs/app1.rst#text-extraction-flags-defaults) and [DisplayList/TextPage](https://pymupdf.readthedocs.io/en/latest/coop_low.html). | PyMuPDF does not support concurrent threads; official guidance is multiprocessing with one independently opened document per process and page ranges. OCR is `get_textpage_ocr(flags, language, dpi, full, tessdata)` and invokes Tesseract; it is a separate, much slower path. See [multiprocessing](https://pymupdf.readthedocs.io/en/latest/recipes-multiprocessing.html) and [OCR API](https://github.com/pymupdf/PyMuPDF/blob/main/docs/page.rst). |
| **pdfminer.six** | `extract_text()` creates `LAParams()` when none is supplied, so the convenient API performs layout analysis. For a real no-layout path, use `extract_text_to_fp(..., output_type="text", laparams=None)`: `PDFLayoutAnalyzer.end_page()` calls `LTPage.analyze()` only when `laparams` is non-`None`. Even then the interpreter/device still builds `LTPage`/`LTChar` structures and traverses them. See [high_level.py](https://raw.githubusercontent.com/pdfminer/pdfminer.six/master/pdfminer/high_level.py#L136-L173) and [converter.py](https://raw.githubusercontent.com/pdfminer/pdfminer.six/master/pdfminer/converter.py#L58-L82). | `LAParams`: `line_overlap`, `char_margin`, `word_margin`, `line_margin`, `boxes_flow`, `detect_vertical`, `all_texts`. `boxes_flow=None` disables advanced textbox ordering, but does **not** remove line/word grouping. High-level controls include `page_numbers`, `maxpages`, `caching` / `disable_caching`, and `codec`. See [`LAParams`](https://raw.githubusercontent.com/pdfminer/pdfminer.six/master/pdfminer/layout.py#L45-L87) and [high-level API](https://pdfminersix.readthedocs.io/en/latest/reference/highlevel.html). | The documented high-level loop is sequential; no built-in page worker knob is exposed. The bundled hOCR converter is for explicit PDF text and explicitly does not handle images/diagrams, so scanned-page OCR must be supplied outside this path. See [converter.py](https://raw.githubusercontent.com/pdfminer/pdfminer.six/master/pdfminer/converter.py#L791-L805). |

## What is verified versus inferred

### Verified from upstream sources

- PyMuPDF is a Python binding over the MuPDF C engine; its README describes the
  engine as lightweight/fast and offers plain text as the fast, lightweight
  output. ([README](https://github.com/pymupdf/PyMuPDF/blob/main/README.md#why-pymupdf))
- PyMuPDF's own extraction appendix reports that excluding images reduced its
  2,700-page example from 160 s to 77 s, and gives relative costs for TEXT,
  BLOCKS, WORDS, XML, HTML, DICT, and RAWDICT. Those are vendor measurements on
  a specified corpus, not a claim about this repository. ([Appendix 1 performance](https://github.com/pymupdf/PyMuPDF/blob/main/docs/app1.rst#performance))
- Reusing a `TextPage` avoids rebuilding the page text representation when
  multiple extraction formats/searches are needed; the docs quantify a >50% to
  95% reduction for repeated extraction depending on output. ([Page API](https://github.com/pymupdf/PyMuPDF/blob/main/docs/page.rst), [low-level API](https://pymupdf.readthedocs.io/en/latest/coop_low.html#textpage))
- pdfminer.six's layout analyzer has three geometric grouping stages (characters
  to lines, lines to boxes, boxes hierarchically), controlled by `LAParams`.
  ([layout-analysis docs](https://pdfminersix.readthedocs.io/en/latest/topic/converting_pdf_to_text.html#layout-analysis-algorithm))
- pdfminer.six is written entirely in Python and exposes a modular interpreter /
  device pipeline. ([upstream README](https://github.com/pdfminer/pdfminer.six#features))
- `extract_text()` and `extract_text_to_fp()` differ materially: the former
  inserts default `LAParams()`; the latter preserves `laparams=None`, which the
  converter uses to skip `LTPage.analyze()`. ([high_level.py](https://raw.githubusercontent.com/pdfminer/pdfminer.six/master/pdfminer/high_level.py#L136-L173), [converter.py](https://raw.githubusercontent.com/pdfminer/pdfminer.six/master/pdfminer/converter.py#L76-L82))

### Inferences for this repository (not upstream guarantees)

1. **Do not add layout just to get sentence boundaries.** Neither PyMuPDF nor
   pdfminer.six promises sentence segmentation; they expose text/line/word
   structures. A cheap splitter after extraction is the right layer for the
   fast contract.
2. **The highest-value optimization target is decode/representation overhead.**
   PyMuPDF's native core and pdfminer.six's no-layout behavior suggest measuring
   the time spent in `Processor::process_stream`, glyph/font decoding, and
   `Segment` allocation before changing the splitter or adding heuristics.
3. **A bounded page-worker knob could help throughput, but it is not free.** The
   current Rayon `par_iter` is already parallel. Exposing worker count should be
   justified by CPU/RSS measurements and a deployment need; otherwise it is API
   surface without evidence.
4. **A parse-once/reuse contract matters for non-fast callers.** If future code
   requests multiple outputs from the same page, retain the decoded page
   representation and derive text/metadata from it, analogous to PyMuPDF's
   `TextPage` reuse. Do not add a cache to the one-pass fast path.

### Current-checkout hotspot (verified by source inspection)

The fast lane bypasses layout grouping, but it still calls the shared
`Processor::process_stream`. That function performs a full content decode and a
`content_text_layer_stats` pass, then—per glyph—does width lookup, Unicode
decoding, CTM/device transforms, visibility checks, and segment bookkeeping
before `extract_text_fast` reads only `segment.content`. See
[`process_stream`](../../src/lib.rs#L3727-L3806), the `TJ` glyph loop
([`src/lib.rs`](../../src/lib.rs#L4096-L4138)), and the analogous `Tj` loop
([`src/lib.rs`](../../src/lib.rs#L4320-L4357)). This is the concrete gap to
measure against PyMuPDF TEXT and pdfminer.six's `laparams=None` path.

The smallest plausible experiment is a text-only processor mode that preserves
font decoding, PDF operator semantics, Form XObject recursion, and all resource
limits, while skipping geometry/visibility/metadata work that the fast output
cannot expose. It should be benchmarked as separate toggles (stats pass,
transforms/widths, per-glyph metadata) before becoming production code; this
report does not implement that experiment.

### Layout/context hotspots in the current checkout

The largest opportunities are not all in the glyph sorter:

1. **The product fallback runs a second complete extraction.** `parse_pdf` first
   computes layout results, then—unless `LayoutFallbackPolicy::Disabled`—runs
   the entire document again with `laparams=None` before deciding whether to
   keep the first result ([`processing.rs`](../../src/document/processing.rs#L2407-L2443)).
   On the 21-PDF benchmark, layout with the default fallback was 45,658 ms while
   structured no-layout was 20,480 ms; that is consistent with roughly one
   extra full pass. Use `LAParams::diagnostic_layout()` for latency-sensitive
   callers, or preflight only suspicious pages before paying for the second
   pass. This is the highest-confidence large lever.
2. **Layout duplicates and clones per-glyph state.** A `Glyph` carries several
   owned strings and style/source metadata. Layout sorts it and previously cloned
   every glyph into line vectors just to preserve a snapshot for optional
   vertical detection ([`lib.rs`](../../src/lib.rs#L5106-L5200)). The normal path
   now moves the sorted vector; the second snapshot exists only when
   `detect_vertical=true`. On the 21-document corpus this produced byte-identical
   normalized output for all 21 files, including the older Supreme Court PDFs,
   with a measured 10.56% wall-time improvement (41,551.8 ms -> 37,164.4 ms) in
   the gated run. Keep the opt-in vertical snapshot until a reference-based
   representation is proven against rotated-text fixtures.
3. **Paragraph/box grouping is a separate performance cliff.** With
   `PDF_EXTRACT_LA_BOXES=1`, the three-document sample took about 3.92 s for
   layout versus 1.86 s with the default line-only path; the same 21-document
   run did not finish in roughly three minutes. Keep it opt-in and profile the
   merge/plane algorithm before making it a default.
4. **Contextual post-processing is sequential and repeatedly copies text.**
   After page-parallel extraction, the pipeline performs continuation,
   hyphenation, title, header/footer, tables, sorting, heading classification,
   and chunk accumulation in order ([`processing.rs`](../../src/document/processing.rs#L1260-L1450)).
   Header/footer alone was not material in the three-document probe. The likely
   medium-sized cost is tokenization: `ChunkAccumulator::can_add_segment`
   encodes each incoming segment and may encode the accumulated chunk again. The
   document path now reuses the existing `OUTPUT_TOKENIZER` singleton
   ([`processing.rs`](../../src/document/processing.rs#L1408-L1430)). Structured
   chunk decisions now use a 1.3 tokens/whitespace-word estimate with a 15%
   limit margin by default; exact BPE remains opt-in through
   `LAParams::exact_tokens()` or `PDF_EXTRACT_EXACT_TOKENS=1`. Exact hard-cap
   splitting remains at the final emitted-output boundary. The public
   `extract_text_fast` override still avoids layout and exact tokenization
   entirely.

   On the 21-document corpus, the approximate default completed in 36,426.9 ms
   versus 38,369.0 ms for the exact opt-in and 41,551.8 ms for the pre-change
   baseline. The approximate output changed chunk boundaries on two documents
   (Puttaswamy and Gayatri Balasamy) but the legal quality gate remained usable;
   exact opt-in restored all 21 normalized output hashes.

   The boundary change was not a text-loss event in the direct export check:
   Puttaswamy's approximate and exact exports had 125,906 identical
   alphanumeric tokens in the same order. Approximate had 28 additional
   punctuation marks from rewrapping, but no missing lexical token; the two
   legal quality-gate documents both retained 100% location coverage and 0.95
   usable quality. Aggregate emitted characters were 3,271,456 approximate
   versus 3,271,444 exact across all 21 documents (+12 characters total).

Do not spend the first optimization pass on sentence splitting, header/footer
regexes, or a public Rayon worker knob. The evidence points to duplicate full
passes, owned glyph cloning, and exact tokenization as the large or easiest
targets; the rest should earn its complexity through profiling.

### Still unknown after the bounded probe

- Peak RSS, CPU time, and scaling on the full corpus. The wall-clock probe is
  enough to establish direction, not a deployment capacity claim.
- Whether PyMuPDF's `sort=False` output is acceptable for the same corpus, and
  what quality loss occurs versus this crate's page/segment order.
- Whether a Rayon worker limit, parser resource cache, or fewer temporary
  `Segment` allocations moves the wall-clock needle on representative PDFs.
- How scanned PDFs, Form XObjects, multi-column pages, malformed PDFs, and
  Unicode-heavy fonts change the ranking. OCR runs should be reported
  separately from text-only extraction.

## Recommended next benchmark (no implementation implied)

Run all three implementations on the same corpus, host, and page selection:

1. Rust `extract_text_fast` (current baseline).
2. PyMuPDF `page.get_text("text", sort=False)` with images excluded; add a
   separate `TextPage`-reuse case only when more than one output is requested.
3. pdfminer.six `extract_text_to_fp(..., output_type="text", laparams=None)`
   and default `extract_text(..., laparams=LAParams())` as separate variants.

Record warmup plus at least three measured runs, wall time, CPU time, peak RSS,
page/character counts, output byte counts, and an ordering/quality diff. Keep
OCR and sentence splitting out of the extraction timing, then measure each as a
separate downstream stage. Do not use PyMuPDF's vendor Appendix 1 timings as a
cross-library result.
