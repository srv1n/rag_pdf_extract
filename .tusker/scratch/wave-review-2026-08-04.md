# Wave review — PDF-T-0008..0016 (2026-08-04)

Two independent Opus reviewers, disjoint scopes, both re-ran the test claims.
Claims verified: cargo check clean; cargo test 116 passed / 2 ignored; lopdf
0.42.0 resolved (Cargo.lock is gitignored — the Cargo.toml constraint
`>=0.42, <0.43` is the real protection and is correct).

Overall: the shape of every feature is right and the code is readable, but the
test suite systematically asserts the easy half of each contract. Five
blockers ship green today precisely because no test covers the property they
break.

## Verdicts

| Task | Verdict | One-line reason |
|---|---|---|
| PDF-T-0008 lopdf upgrade | rework | RUSTSEC test calls lopdf directly, never a pdf_extract entry point — A2 unproven for the crate. One-line fix. |
| PDF-T-0009 classifier | rework | Scanned verdict unreachable for US-Letter/inline-image scans; garble path false-positives on all CID/accented PDFs; classifier sees less content than extraction. |
| PDF-T-0010 budgets | rework | Decompression budget bypassable via filter chains; its only test never inflates a byte; max_objects measures a lazy map; recursion budget aborts whole documents. |
| PDF-T-0011 harness | rework | No adversarial fixtures in the corpus; recorded 21/21 proof compared a stub against itself, never invoking the extractor; default mode fails improvements. |
| PDF-T-0012 page fallback | **accept** | Single-pass, bounded, /Pages excluded, real test. Nits only. |
| PDF-T-0013 text quality | rework | Parallel quality module the contract explicitly forbade, with thresholds diverging from quality.rs; script gating bypassed in its only production consumer. |
| PDF-T-0014 typed errors | rework | Enum sound, but semver obligation unmet: no version bump, MIGRATION.md stale, four derived traits silently dropped from ExtractionOptions, compat test covers none of it. |
| PDF-T-0015 repairs | rework | Two independent A4 violations: original bytes never attempted before repair; failure path returns post-repair error, not the original. |
| PDF-T-0016 encryption | rework | Redaction real in-crate, but no password newtype (leak surface is downstream), owner-only case untested, 2 of 4 outcomes unasserted, vacuous error-redaction test. |

## Blockers

B1. src/lib.rs:7213-7238 — repairs applied eagerly; original bytes never
    tried. A PDF with leading garbage but absolute xref offsets that parses
    fine at HEAD gets its bytes shifted by the repair and fails
    InvalidStructure with no fallback. Direct regression + PDF-T-0015 A4
    violation.

B2. src/lib.rs:7266-7278 — repair retry returns the second error, not the
    original. Encrypted file + spurious EOF repair => caller sees
    InvalidStructure instead of Encrypted{PasswordRequired}. Real cause
    swallowed.

B3. src/lib.rs depth guard + src/document/processing.rs:1141-1144 — recursion
    limit converted from graceful per-stream skip (HEAD warns and continues)
    to whole-document abort. A 9-deep form-XObject PDF goes from "99% of text"
    to zero output. Reverses the attribution work of 25b112c/34ce18b.

B4. src/classifier.rs:230,251 — garble detector fed raw undecoded PDF string
    operands (String::from_utf8_lossy on font-encoded bytes). Every
    Identity-H/CJK PDF and most accented WinAnsi PDFs self-report garbled =>
    needs_ocr on all pages, misclassified Mixed. Defeats PDF-T-0013's
    multilingual gating entirely (gating never sees decoded text).

B5. src/document/processing.rs:996-998 (+2 more sites) — classifier decode
    failure aborts extraction with ? where extraction itself tolerates the
    same malformed stream (lib.rs:3725 skips and continues). OCR-enabled parse
    of a PDF with one bad content stream now errors instead of returning
    partial text.

## Majors (condensed)

Core lib:
- M1 Decompression budget bypass: non-flate filter chains and decoder errors
  fall back to counting compressed size; bomb passes budget, lopdf inflates it
  anyway later. M2: the "bomb" test is 64 invalid bytes — never inflates.
- M3 Budget error reports remaining, not configured limit; outer check dead.
- M4 Budget scan re-inflates every stream on every successful parse (~2x CPU
  on happy path); no before/after bench though make bench-corpus exists.
- M5 max_objects counts lopdf's lazily-populated object map — near no-op for
  /ObjStm PDFs.
- M6/M7 Semver: ExtractionOptions lost Copy/PartialEq/Eq/Serialize/Deserialize,
  OutputError variants removed, return types changed; version still 0.8.0,
  MIGRATION.md stale; downstream_compat.rs would not have caught any of it.
- M8/M9 Password is a bare pub Option<String> (contract required a redacting
  newtype); the error-redaction test constructs an error with no password in
  it — vacuous.
- M10 RUSTSEC test proves the dependency, not the crate (fixture itself is
  correct and would SIGABRT 0.38).
- M11 Owner-only decryption untested; UnsupportedHandler/InvalidDictionary
  outcomes never asserted; success case checks is_ok() only.

Classifier/quality:
- M-a image_area uses MediaBox points, not pixels: US-Letter (484,704 pt²)
  is under the 500K threshold => Scanned unreachable; missing MediaBox
  defaults to exactly the threshold => always "full-page scan".
- M-b No Form XObject recursion in classifier (extraction recurses):
  templated text PDFs classified NoText and routed to OCR.
- M-c Inline images (BI..ID..EI) uncounted: fax-style scans never classify
  Scanned.
- M-d text_quality.rs is an 837-line parallel of quality.rs with different
  thresholds (0.52 vs 0.45) — the contradictory-verdict defect class the epic
  exists to eliminate, and explicitly forbidden by the contract.
- M-e paired_regression.py without --reference-dir fails improvements
  (|candidate-baseline| vs threshold 0); rev labels are free text; nothing
  detects candidate==baseline — the recorded corpus proof ran
  `printf stable-output` on both sides.
- M-f No adversarial fixtures anywhere in benchmarks/corpus (21 SCOTUS PDFs
  only); --adversarial-dir supported but never supplied.

## Minors worth fixing during rework

- Vacuous tests: tests/classifier.rs asserts tautologies; text_quality tests
  import a private copy via #[path] so they pass even if the module is
  unwired; no Scanned/NoText reason test; sampling spread untested.
- estimate_page_count runs on every successful load (result discarded);
  bytes.to_vec() full-file copy before checking repairs apply; streaming
  Document::load replaced with read-whole-file (2-3x peak memory).
- Repair detection logic duplicated in two places with subtly different
  conditions; depth default 8 hardcoded in three places.
- OutputError has no source(); lopdf cause stringified; boxed_error turns I/O
  failures into Parse. NotAPdf decided by string-matching lopdf's Display
  ("invalid file header") — ironic in the typed-errors task.
- flate2 added as a new dependency while miniz_oxide (its backend) is already
  present.
- 1-based page indexing (consistent, but contradicts the contract's 0-based
  mandate and PageAssessment.page documents no base).
- Behavior change no contract authorized: processing.rs:1114-1116 now gates
  ALL image-XObject processing on the classifier's ocr_route; at HEAD
  ocr_handler presence was sufficient.
- Hebrew negative fixture missing (contract intent named it); multilingual
  negatives are one-line strings, not fixtures.
- render_benchmark_comparison.py hardcodes 2026-07-21 filenames — dead on
  second use.

## Cheapest unblocking paths

1. B1/B2: parse original bytes first; only on failure apply repairs; on
   repaired failure return the ORIGINAL error with repair_attempted=true.
   Add the A4 test the contract already specifies.
2. B3/B5: restore skip-and-continue for depth and classifier decode failures;
   record the skip in the result (attribution, not abort).
3. B4 + M-b: feed the classifier the text the extractor already decodes
   (process_stream glyph path) instead of lossy re-derivation from raw
   operands — fixes CID false positives and the Form-XObject blind spot at
   once, and removes a duplicate content-stream decode.
4. M-d: fold text_quality.rs into quality.rs behind one set of thresholds.
5. M6: bump to 0.9.0, restore the derived traits (or justify), update
   MIGRATION.md, extend downstream_compat.rs to the surface that changed.
6. M1/M2: count inflated bytes for ALL filter chains (or reject unknown
   chains over a size floor); write a real bomb fixture that inflates.
