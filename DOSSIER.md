# PDF Extract — dossier
Last updated: 2026-08-18 (update this line every edit)

## One paragraph
PDF Extract is a Rust library—not an end-user application—that turns PDF files into token-bounded text chunks for downstream search and retrieval systems. It can preserve headings, page and bounding-box locations, source-to-output spans, and extraction-quality metadata; optionally it can use layout analysis, Form XObject traversal, and OCR for PDFs whose text is not available through the basic path.

## Status
Active library build: the latest commit is dated 2026-08-09 and recent history contains extraction hardening, recovery, location, and benchmark work. RZN Backend and RZN Tauri use it locally; Sarav is its only user, and no externally deployed user surface was reported. The GitHub repository is public but has never been promoted; GitHub showed one star, zero forks, no releases, and no packages on 2026-08-18.

## What it does (features, user-facing)
- Extract text from a PDF path or in-memory bytes.
- Split extracted text into chunks capped by a caller-selected token limit.
- Preserve detected heading hierarchy and clean text for indexing.
- Return page, character-range, bounding-box, and source-span metadata for citations or highlighting.
- Enable layout-aware reading order, multi-column and table handling, and optional Form XObject text extraction.
- Classify PDFs and pages to explain when OCR is needed; optionally run OCRS/RTen OCR when built with the OCR feature and supplied models.
- Detect and suppress repeated headers, footers, hidden text, and duplicate Form content.
- Handle password-protected PDFs and return typed errors for encryption, malformed input, and resource limits.
- Report page count separately so callers can distinguish an empty document from failed/silent text extraction.
- Emit one structured extraction telemetry event per parse and expose the same quality/location summary as a Rust value.

## Who it's for
Primary maintainer/integrator: Sarav, through RZN Backend and RZN Tauri. The intended downstream experience is for non-technical people—such as a lawyer—to use desktop software that supplies their documents as context to LLMs without setting up developer-oriented PDF tooling. Today the library has no audience beyond Sarav's local products.

## Numbers that are true
- Historical 2026-07-21 M1 Pro full-core benchmark: 21/21 Supreme Court PDFs completed; 1,925 pages and 3,271,444 output characters; second run took 39,625.79 ms total, 20.58 ms/page weighted, and 19.76 ms/page median. Reproduce with `make bench-corpus`; corpus identity is pinned by `shasum -a 256 -c benchmarks/corpus.sha256`. This predates the 2026-08-06 and 2026-08-09 hardening commits.
- Same corpus/settings on two Rayon workers: 46,060.48 ms total and 23.93 ms/page weighted. Reproduce with `make bench-corpus-2core` after fetching the pinned corpus.
- Same corpus/settings under macOS background QoS: 356,524.55 ms total, 185.21 ms/page weighted, and 0.113x full-core throughput. This is a scheduling approximation, not hard efficiency-core pinning. Reproduce with `make bench-corpus-ecores` on the recorded Apple M1 Pro setup.
- Decompressed `ContentExt` is capped at 16 MiB; the default cumulative non-image decompressed-stream budget is 128 MiB, while decoded image working sets are checked independently. Reproduce with `rg 'CONTENT_EXT_DECOMPRESS_LIMIT_BYTES|max_decompressed_stream_bytes' src/lib.rs MIGRATION.md` and `cargo test --test decompression_limits`.
- GitHub stars: 1 as of 2026-08-18; reproduce from the public `srv1n/rag_pdf_extract` repository page. Sarav reports the star is his own. GitHub releases: 0 and packages: 0 on the same date, reproducible from that page.
- The fork has no GitHub releases or packages and no independently tracked installs/downloads. It is not a standalone commercial product, and no separate revenue is attributed to it. There is no external production volume or measured real-world loss rate because use is local to Sarav.
- A later repository evaluation run at git `ddc8d25` passed 2 of 4 files: the legal case passed, while one mixed fixture had an invalid PDF header and another exceeded the configured empty-section threshold at 10.9%. Reproduce with `python eval/eval.py --history` and inspect `eval/runs/latest.json`; WER, CER, heading F1, and content coverage are null, so this run does not substantiate the README's broad quality claims.
- A dirty-tree stage snapshot at git `de635f7` passed 9/9 product and raw-layout cases, used fallback on 1/9, emitted no over-cap chunks, and reported full span coverage for those fixtures. Reproduce with `make production-broad-report`; treat the checked-in result as a development snapshot, not release proof.

## Tech shape (short)
Rust 2018 library crate, MIT-licensed, built around `lopdf` 0.42, Rayon, serde/JSON, zstd, BLAKE3, and `tiktoken-rs`; optional OCR uses OCRS/RTen.
One pipeline loads and bounds a PDF, extracts glyphs and images, optionally analyzes layout/OCR, filters and restructures text, chunks it, then emits schema-v2 core content plus compressed location metadata.
Notable decisions: lopdf-only parser path; typed reason codes; original-first bounded repairs; stable source/range/ordinal chunk IDs separate from content hashes; optional detailed output spans; workload-specific `all_texts`; OCR disabled by default.

## Recent changes (rolling, newest first, keep last ~10)
- 2026-08-09: Bounded decoded image working sets to prevent compact images from inflating past the resource ceiling.
- 2026-08-09: Corrected image stream budget accounting and added a decompression report.
- 2026-08-06: Added typed extraction errors, encrypted-PDF handling, document classification, bounded repairs, and paired regression tooling.
- 2026-07-31: Added page-count APIs so empty extraction can be treated as a countable failure by callers.
- 2026-07-31: Recovered text silently lost through CMap width decoding and inline image-mask parsing defects.
- 2026-07-21: Added a pinned 21-document Supreme Court performance and stability benchmark.
- 2026-07-21: Normalized non-zero PDF page-box origins for location metadata.
- 2026-07-06: Prevented nested Form XObject font-resource decode collisions.
- 2026-07-06: Emitted compact, grouped chunk locations while retaining detailed source spans.
- 2026-07-06: Replaced effectively quadratic hard-cap chunk splitting on long legal text with bounded prefix probing.

## Deliberate exclusions
- No document-type policy or change-data-capture logic; those stay in the consuming application because editability and citation-stability policy are caller concerns.
- No selectable PDFium/backend abstraction; the experimental backend surface was removed to keep one coherent lopdf path and avoid a native runtime dependency.
- No Tesseract integration; `ocr-tesseract` is only a compatibility alias for OCRS/RTen.
- OCR is not in default builds, and Form XObject traversal is not enabled broadly; callers opt in only for workloads that prove they need them to avoid cost and hidden/template duplication.

## Open questions / embarrassments
- It began after recurring extraction failures on legal PDFs while Sarav was building RZN Tauri as a desktop LLM context layer. Structure-aware chunks were needed for BM25 and vector retrieval without asking non-technical users to assemble developer PDF tooling; no single triggering document or date was identified.
- Current use is limited to local RZN Backend and RZN Tauri builds, with Sarav as the only user. There is no external user feedback, production volume, or standalone commercial performance to report.
- Sarav is proud that geometric heading inference can provide useful document structure without OCR and can run locally. His estimates of “90% of the way,” higher speed/lower cost than OCR, and release-mode capacity are not yet backed by a named reproducible comparison.
- Package metadata still names upstream author Jeff Muizelaar and points documentation/repository links at `jrmuizel/pdf-extract`, while this checkout is `srv1n/rag_pdf_extract`; intended attribution and release ownership are unclear.
- README and integration docs show `pdf-extract = "0.9.0"`, but the repository contains no proof that this fork/version is published to crates.io.
- Evaluation artifacts cover legal, mixed, scientific, OCR, layout, and rendering cases, but coverage is uneven and several snapshots are dirty-tree or old-revision runs. The latest golden evaluation passed only 2/4 files and contains no WER, CER, heading F1, or content-coverage measurements; broad quality claims remain unsupported.
- `docs/ARCHITECTURE.md` claims a 5–10x tokenization speedup, about 0.1 ms per tokenization call, 5–10 MB peak memory per 100 pages, about 70% metadata compression, and quality improvement from layout analysis without a checked-in reproducible measurement for those claims.
- README says positioning is “precise” and layout analysis “improves quality”; code and fixtures test location invariants and regressions, but those broad quality claims are not established across a representative committed corpus.
- Code-visible limitations: heading heuristics can misclassify signature lines, title-block merging can remove intended line breaks, unusual spacing may defeat layout thresholds, `all_texts` can duplicate hidden/template text, and right-to-left text and equations are not established by committed coverage.
