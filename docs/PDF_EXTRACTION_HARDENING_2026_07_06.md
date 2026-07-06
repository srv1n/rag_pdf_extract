# PDF Extraction Hardening - 2026-07-06

## Root Cause

The Tomaso Bruno and Devas v Antrix repro PDFs did not hang in PDF loading,
xref recovery, font decoding, OCR, or raw text extraction. Both completed via
`extract_text`, and both completed through layout extraction when chunking was
effectively disabled with a very high token cap.

The pathological path was layout-mode chunk hard-capping with
`LAParams { all_texts: true, .. }` and a small `max_tokens` budget. Sampling a
stuck debug run showed the main thread under:

```text
parse_pdf
  output_doc_with_ocr_telemetry
    extend_with_capped_output
      split_text_hard_capped
        split_at_token_boundary
          tiktoken_rs::CoreBPE::encode_ordinary
```

`split_text_hard_capped` re-tokenized the full remaining tail before every
chunk and `split_at_token_boundary` binary-searched across the full tail. For
long layout-produced legal text this becomes effectively quadratic in the
number of chunks. The fix changes the split search to bounded exponential
prefix probing plus a local binary search, and carries the unprocessed tail as a
borrowed slice instead of copying it on every iteration.

Operationally, the degenerating inputs were digital legal PDFs whose layout
pass produced unusually large clean text tails from dense fragment/line output,
not corrupted streams, broken xrefs, missing fonts, image-only pages, or OCR
fallback. With `all_texts=true` and `max_tokens=350`, Devas generated a long
continuous legal-text tail and Tomaso added noisy LibreOffice/symbol-heavy
layout output before normal judgment text, so each 350-token chunk caused the
old code to feed tens or hundreds of KB back through `tiktoken`. The other
corpus documents avoided the timeout because their layout output was naturally
broken into smaller outputs/tails, so the same quadratic behavior stayed below
the failure threshold. The production signature is high CPU with samples in
`split_text_hard_capped` / `split_at_token_boundary` /
`tiktoken_rs::CoreBPE::encode_ordinary`, no PDF decode/OCR error, and success
when using raw extraction or a very high token cap.

## Repro Fixtures

Permanent fixtures live under:

- `tests/fixtures/pdf_hangs/14. Tomaso Bruno v State of UP.pdf`
- `tests/fixtures/pdf_hangs/2. Devas v Antrix.pdf`

The full fixture tests are ignored by default because they parse real legal
PDFs:

```bash
cargo test --test pdf_hang_regression -- --ignored --nocapture
```
