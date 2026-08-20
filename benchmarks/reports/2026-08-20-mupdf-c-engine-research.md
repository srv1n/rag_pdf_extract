# MuPDF C engine: last-mile lessons for PDF extraction

Date: 2026-08-20  
Scope: primary MuPDF/PyMuPDF documentation and source, mapped to the current
Rust pipeline. The bounded P0 experiment described below was implemented and
benchmarked after the research.

Inputs collated here: the local [PyMuPDF/pdfminer.six comparison](2026-08-20-pymupdf-pdfminer-research.md), the [ChatGPT handoff brief](../../.chatgpt-handoff/mupdf-gap-brief.md), and the returned [ChatGPT result](../../.chatgpt-handoff/incoming/cgpt_mt19av7r_67adcc68.response.md). The handoff is corroborating research, not an acceptance gate.

## Identity note

There is no identifiable product named “Renu PDF-C” in the repository or in
the upstream PDF sources checked. In this conversation the phrase is almost
certainly a transcription of **MuPDF C engine**: MuPDF is the C core behind
PyMuPDF. The conclusion below is therefore about MuPDF, with that uncertainty
called out rather than silently treating “Renu” as a separate engine.

## What MuPDF actually optimizes

MuPDF’s public model is deliberately staged:

1. Interpret a page once into a reusable `DisplayList`.
2. Run that list through a text device to build a `TextPage`.
3. Derive plain text, words, JSON, search hits, or other outputs from the same
   representation.

The PyMuPDF documentation says the display list is the interpreted page and
can be reused for rendering, search, and extraction; a persistent `TextPage`
can serve all extraction formats. Reuse is reported as a 50–95% improvement
when the same page is otherwise rebuilt repeatedly. See [DisplayList/TextPage
reuse](https://pymupdf.readthedocs.io/en/latest/coop_low.html#working-together-displaylist-and-textpage)
and the [PyMuPDF reuse guidance](https://github.com/pymupdf/PyMuPDF/blob/main/README.md).

The C structured-text API makes the cost boundary explicit. A text page is a
block/line/span/character structure backed by a page pool, while the text
device gathers blocks and lines in source draw order. See the [structured-text
header](https://github.com/ArtifexSoftware/mupdf/blob/master/include/mupdf/fitz/structured-text.h)
and [text-device implementation](https://github.com/ArtifexSoftware/mupdf/blob/master/source/fitz/stext-device.c).

## Transferable lessons, ranked

| Priority | MuPDF lesson | Current Rust analogue | Recommendation |
|---|---|---|---|
| **P0** | Cheap output is a separate contract, not a full structured page with fields discarded later. | `extract_text_fast` still calls `Processor::process_stream`, including `Content::decode`, `content_text_layer_stats`, width/Unicode work, CTM transforms, visibility checks, and per-glyph `TextSegment` bookkeeping before retaining only `segment.content` ([`process_stream`](../../src/lib.rs#L3727-L3806), [`extract_text_fast`](../../src/lib.rs#L6901-L7000)). | Run one bounded `TextOnly` experiment for the fast override: retain PDF text operators, font decoding, Form XObject recursion, malformed-input handling, and resource limits; skip geometry, visibility/style metadata, and `TextSegment` allocation. This is the one change worth trying before closure, but gate it against lexical preservation and hidden-text fixtures. |
| **P1** | Interpret once, replay for multiple consumers. | Layout fallback may extract the whole document again with `laparams=None` after layout ([`processing.rs`](../../src/document/processing.rs#L2473-L2585)). | If fallback remains enabled in production, benchmark a page-level decoded/interpreted representation that can feed layout and no-layout output. Do not add a cache for one-pass fast calls: memory cost is not justified there. |
| **P1** | Keep per-character records compact and share style state. | `Glyph` owns a `String` character plus font/source strings and style metadata for every glyph; later grouping clones and scans those records ([`Glyph`/line grouping](../../src/lib.rs#L5090-L5570)). | If profiling still shows allocation pressure, intern font/source metadata per text object and store a scalar code point or string slice/range in glyphs. Defer `String` construction until line/segment emission. This mirrors MuPDF’s pooled page records and is higher risk than `TextOnly`. |
| **P2** | Make extraction options remove work, not merely hide output. | MuPDF exposes flags for images, accurate bboxes, ascenders, side bearings, style collection, vectors, structure, segmentation, and table hunting. | Keep fast mode text-only. For structured mode, make expensive geometry/style/table features explicit opt-ins; do not run them just because layout is requested. See [MuPDF structured-text options](https://mupdf.readthedocs.io/en/latest/reference/common/stext-options.html). |
| **P2** | Exclude images unless the caller asks for them. | The fast override already skips OCR; structured/OCR telemetry remains a separate path. | Preserve the current separation. If a future text-page cache is added, make “preserve images” an explicit bit and default it off. MuPDF reports 160 s → 77 s on its 2,700-page example when image extraction is disabled; that is upstream evidence, not a local speed claim. See [flags and performance](https://pymupdf.readthedocs.io/en/latest/app1.html#performance). |
| **P2** | Preserve unknown-code fallback knobs. | Older legal PDFs are sensitive to font/CMap behavior. | Keep CID/GID-style fallback and current glyph decoding safeguards in any text-only mode; never replace decode with byte splitting. MuPDF exposes explicit CID/GID fallback options in its [structured-text API](https://github.com/ArtifexSoftware/mupdf/blob/master/include/mupdf/fitz/structured-text.h). |

## Why the fast experiment is the right final move

The current fast lane is already materially quicker than structured extraction,
so the remaining gap is upstream of sentence splitting. MuPDF’s own speed table
shows plain `TEXT`, `BLOCKS`, and `WORDS` clustered near the baseline, while
character/layout-rich outputs cost substantially more; disabling image output
collapses much of that difference. See [Appendix 1](https://pymupdf.readthedocs.io/en/latest/app1.html#performance).

That points to a narrow experiment rather than another layout rewrite:

```text
PDF operators + font/CMap decode + recursion/limits
        ├── TextOnly: source-order text, cheap separators, sentence splitter
        └── Structured: current glyph geometry, layout, metadata, chunking
```

The text-only branch must retain `Tj`, `TJ`, text-state operators, Form XObject
recursion, Unicode fallback, and resource/depth limits. It may omit CTM/width
calculations and visibility heuristics only if the contract explicitly accepts
creator-order text and the regression check proves no lexical loss. A simple
acceptance gate is: all current corpus pages succeed; normalized lexical-token
multiset and order do not lose content; old Supreme Court fixtures retain their
quality/location checks; malformed and hidden-text fixtures remain covered.

Do **not** attempt a MuPDF-style display-list cache or a new public worker knob
unless profiling shows repeated consumers or thread oversubscription. Rayon
already parallelizes pages, and caching would increase memory for the common
one-pass call.

## Sources and limits

- [MuPDF Core](https://mupdf.com/core) identifies the C library as the fast,
  portable foundation for PDF extraction and conversion.
- [PyMuPDF DisplayList/TextPage](https://pymupdf.readthedocs.io/en/latest/coop_low.html)
  documents interpreted-page reuse and extraction from one `TextPage`.
- [PyMuPDF extraction appendix](https://pymupdf.readthedocs.io/en/latest/app1.html)
  documents output flags, image impact, and relative extraction costs.
- [MuPDF structured-text options](https://mupdf.readthedocs.io/en/latest/reference/common/stext-options.html)
  lists the expensive/optional features that can be disabled.
- [MuPDF C structured-text header](https://github.com/ArtifexSoftware/mupdf/blob/master/include/mupdf/fitz/structured-text.h)
  defines the pooled page model, text-device options, and Unicode fallback.

The upstream percentages and speed ratios are vendor measurements on their
corpora. They justify what to measure here; they are not claims about this
checkout.

## P0 experiment result

The recommended low-risk sink was implemented in `src/lib.rs`. The fast path
now uses `Processor::new_fast_text()` and appends already-decoded content to a
page string at the existing text flush points. The shared interpreter still
handles font/CMap decoding, text operators, Form recursion, visibility rules,
malformed input, and resource limits; only discarded `TextSegment` metadata
construction is bypassed. This deliberately stops short of a geometry-free
decoder, so the old custom-font/Supreme-Court behavior remains on the same
decoding path.

On the 21-document benchmark corpus (one warmup plus one measured run), the
final sink run measured **7,999.4 ms**, versus **12,463.6 ms** before the sink:
a **35.8% wall-time reduction** with 21/21 successful documents. A direct
sink-disabled comparison produced the same 3,569 chunks and the same
non-whitespace character stream across all documents (including **590,728
alphanumeric tokens in the same order**); output differed only in
whitespace/split punctuation spacing (3,277,696 versus 3,277,711 characters,
15 fewer, 0.00046%). The legal Supreme Court quality gate remained green.
This is directional single-run evidence, not a fixed throughput contract.

The ChatGPT handoff reached the same stop/go boundary: keep the existing
parser and decoding machinery, specialize the extraction sink, and only
investigate document-lifetime font/CMap caching if profiling shows repeated
construction. No such cache was added without that profile, and no MuPDF/`lopdf`
parser rewrite or display-list cache is justified for one-pass extraction.
