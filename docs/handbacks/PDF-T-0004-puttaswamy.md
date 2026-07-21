# PDF-T-0004 diagnosis: Puttaswamy typed-slice materialization

Owning layer: backend materialization, not `rag_pdf_extract`.

Failing corpus object: `7. Puttaswamy v UoI.pdf`.

## Reproduction outside the server

With the task's fetched 21-document corpus:

```text
cargo run --release --example diagnose_bbox -- \
  "/tmp/rag_pdf_extract-corpus-20260721/7. Puttaswamy v UoI.pdf"
```

Observed parser output:

```text
chunks=754
content_chars=766148
content_bytes=772065
output_text_len=766148
output_spans=257420
invalid_bboxes=0
invalid_output_spans=0
negative_x=0
negative_y=0
```

The parser completes deterministically. Every emitted output span is a
non-empty range within its chunk text, and every emitted chunk bbox is finite,
nonnegative in x/y, and positive in width/height. The same parser command
completes for all 21 corpus objects.

## Ownership and handback

The reported error is raised after PDF extraction, in the backend's
`store_chunks_with_recipe` path, when it calls
`IndexingPipelineStore::materialize_text_slices_for_revision_with_identity`.
The backend wraps the underlying error as `Failed to materialize typed
canonical text slices`, but the recorded live-drive artifact preserves only
that outer context; it does not preserve the inner SQL/contract error.

Therefore there is no parser-owned input invariant to repair here. Backend
should replay this one object through that materializer with the inner error
chain enabled and inspect the first failing row against these invariants:

- `normalized_text` is valid UTF-8 and `length_chars` matches the backend's
  chosen offset unit;
- `start_offset <= end_offset` and the offsets match the stored normalized
  chunk text;
- canonical PDF locations decode to at least one page bbox with finite,
  nonnegative x/y and positive width/height;
- source-location, passage, and text-slice foreign keys reference rows in the
  same materialization transaction.

No parser fix is made for PDF-T-0004. The parser regression suite and the
21-document extraction run remain green; the backend needs to return the
inner materializer error (and fix that layer's violated invariant).
