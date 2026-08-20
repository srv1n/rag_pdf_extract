# Wave review round 2 — PDF-T-0008..0016 rework (2026-08-04)

Two independent Opus reviewers verified every round-1 finding with file:line
evidence and re-ran builds/tests. Round-1 blockers B1-B5 are ALL genuinely
fixed, with tests that would fail against the old code. The rework also
introduced new defects, one of which both reviewers found independently.

## Verdicts

| Task | Round 1 | Round 2 | Why |
|---|---|---|---|
| PDF-T-0008 lopdf/RUSTSEC | rework | **accept** | Test now goes through extract_text_from_mem, asserts typed error; forced catalog deref makes it meaningful. |
| PDF-T-0012 page fallback | accept | **accept** | Untouched, still correct. |
| PDF-T-0015 repairs | rework | **accept** | Original-first parse + original-error preservation real, covered by tests that fail if reverted. |
| PDF-T-0009 classifier | rework | **rework** | Fast path destroyed (NR-1); classifier failure kills OCR (NR-2). |
| PDF-T-0010 budgets | rework | **rework** | M3 unfixed (error reports residual, correct branch dead); M4 double inflation default-on, unmeasured. |
| PDF-T-0011 harness | rework | **rework** | Recorded proof still self-comparison (NR-3); crash-on-adversarial scores as pass; nothing in Makefile/CI runs it. |
| PDF-T-0013 text quality | rework | **rework (small)** | Fold silently dropped mojibake/symbol triggers from document verdict (NR-5); Hebrew negative still missing. |
| PDF-T-0014 typed errors | rework | **rework (docs only)** | Code fine; MIGRATION.md still documents 9-arg parse_pdf (now 10), omits Copy loss, new fields, return-type changes; downstream_compat not extended. |
| PDF-T-0016 encryption | rework | **rework** | Newtype + 4 outcomes solid, but serde round-trip destroys the password (NR-4); redaction test still vacuous; success case only is_ok(). |

## What round 1 demanded and round 2 confirmed fixed

- B1/B2 original-first parsing, original error preserved with
  repair_attempted=true (lib.rs:7520,7556; tests/rework_core.rs:164-209).
- B3 depth overrun skips stream, document text survives (9-deep fixture test).
- B4 garble detection on decoded text; real Identity-H/CJK fixture classifies
  clean; accented WinAnsi negative.
- B5 classifier failure no longer aborts extraction (catch_unwind fallback) —
  but see NR-2 for what the fallback does instead.
- M-a Scanned uses /Width x /Height pixels; DocumentType::Scanned asserted.
- M-b/M-c Form XObject recursion (depth cap + cycle set) and inline images.
- M-d text_quality.rs deleted; one threshold set in quality.rs.
- M1/M2 budget counts inflated bytes across Flate/LZW/ASCII85 chains; bomb
  test genuinely inflates 64KiB against a 128-byte budget.
- M5 max_objects counts xref entries incl. /ObjStm (max with objects.len()).
- M6 version 0.9.0; ExtractionOptions derives restored + redacting Debug.
- M8 PdfPassword newtype with redacting Debug/Display.
- M10 RUSTSEC via crate entry point. M11 owner-only fixture + content
  assertion; UnsupportedHandler/InvalidDictionary asserted.

## New findings (round 2)

### Blocker

NR-1. src/classifier.rs:112,267-283 — classify_pdf extracts the ENTIRE
  document via output_doc to get decoded text, then discards all but the
  sampled pages. Measured (release): 65-page doc classify(8)=1.45s vs full
  extract 2.19s; 134-page doc classify(8)=1.77s vs extract 1.34s — sampling
  8 pages costs more than extracting everything. The 10-50ms attribution
  fast path is gone. Fix: extract only the sampled page set.

### Major

NR-2. processing.rs:1139-1141 + lib.rs:4812 — found INDEPENDENTLY by both
  reviewers. When classification fails (None fallback) or a page is absent
  from the assessment map, ocr_route collapses to false and image XObjects
  are never OCR'd, with an OCR handler installed. Pre-rework: handler
  present => process. Fail-closed disguised as graceful degradation — a
  scanned PDF whose classification fails yields a silent empty result.
  Fix: map_or(true, ...) — fail open when assessment unavailable.

NR-3. Recorded "strict paired harness: 3 passed" proof compares
  target/debug/examples/extract_markdown against ITSELF (candidate differs
  only by an unused trailing arg, defeating the string-equality
  distinct-command guard; all deltas exactly 0). Second consecutive
  self-referential proof for PDF-T-0011. Also: adversarial documents that
  die with empty stderr (SIGSEGV, returncode<0) score as PASSED
  (paired_regression.py:424-427); an ordinary text fixture is labeled
  adversarial, relaxing its failure rules; unused normalized_output_sha256
  would catch self-comparison if compared.

NR-4. lib.rs:6995-7010 — PdfPassword Serialize writes literal "[redacted]",
  Deserialize accepts it: config -> JSON -> config silently turns the
  password into "[redacted]" and later fails IncorrectPassword with no clue.
  api_compat_rework.rs:22 locks the bug in. Use #[serde(skip)] + docs.

NR-5. quality.rs:362-364 — folding looks_like_decode_garbled into
  assess_text_quality dropped the mojibake_char_ratio>0.03 and
  non-ASCII-symbol triggers: documents formerly flagged Degraded now report
  clean (confidence 0.70 > 0.50 gate). No test covers the status transition.

NR-6. examples/_tmp_timing.rs (untracked leftover) calls old 1-arg
  extract_text_from_mem — plain `cargo test` FAILS TO BUILD (E0061). The
  recorded "cargo test: 123 passed" cannot have been produced with this
  file present. Delete it; re-run proof.

NR-7. lib.rs:7343-7352 — decompression budget error reports remaining
  budget, not the configured limit; the branch reporting the correct limit
  is unreachable. Tests assert kind only, so it ships green.

### Minor (carry-forward + new)

- M4 double inflation still default-on and unmeasured; output_doc_new_schema
  validates budgets twice.
- M9 error-redaction test still vacuous (error built with no password near
  it); wrong-password test asserts variant, not message content.
- estimate_page_count + two more full-file scans run on every successful
  load (three scans before the parser sees the bytes).
- PdfPassword has public Deref<Target=str>, contradicting its own doc
  comment about a private accessor.
- processing.rs:1166-1168 ResourceLimit re-raise is dead code; reads as live
  protection.
- Depth default 8 still in three places; NotAPdf still string-matches
  lopdf's Display; flate2+miniz_oxide both present, undocumented choice.
- Classifier: multi-page paragraphs counted once per spanned page (inflates
  per-page char counts); BI and ID both matched (inline image can double-
  count); substitution-cipher heuristic still English-only over Latin text
  (Vietnamese/Turkish false-positive shape, latin_alpha>=64 guard limits
  blast radius); Hebrew negative missing; page-index base undocumented;
  sampling spread and 999/1000 vector boundary untested; old
  tests/classifier.rs tautologies untouched.
- --self-test prints a scary false "FAIL" line from a subtest (exit 0).
- OCR path now runs two full extraction passes (assessment + real), ~2x,
  undocumented (bounded; no recursion risk).

## Suggested closing order

1. Delete examples/_tmp_timing.rs; re-run and re-record proof (NR-6).
2. NR-2 fail-open OCR gate (one-line) + a test with classifier forced to fail.
3. NR-1 extract only sampled pages (plumbing, biggest real work).
4. NR-4 #[serde(skip)] password (+ fix the locking test); real redaction test.
5. NR-5 restore mojibake/symbol triggers with a status-transition test.
6. NR-3 honest paired run: different revisions, sha256 comparison, negative
   exit codes fatal, fix adversarial labeling, add a Makefile target.
7. NR-7 report configured limit.
8. PDF-T-0014: rewrite MIGRATION.md against the actual surface.
