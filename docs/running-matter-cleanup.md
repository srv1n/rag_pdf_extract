# PDF running matter cleanup

The layout pipeline removes repeated short visual lines in the outer 10% page
bands before continuation, hyphenation, title-block merging and chunking. A line
may contain several PDF font runs; repetition uses their x-ordered combined text
and the existing page-number normalization/detector. Body occurrences of the same
words remain intact. No reporter or corpus names are matched.

A single uppercase letter outside the substantive text column by more than two
font sizes is a margin candidate. Removal requires three different letters on
the same side of a page. Isolated initials and letters inside the body column
remain. This conservative layout heuristic cannot repair arbitrary OCR or
identify every margin layout; ambiguous text stays unchanged.

Original retained segments keep their page, source character ranges and located
text. There is no extra PDF parse, serialized span archive or second repeated-line
scan after merges. PDF_EXTRACT_SKIP_HEADER_FOOTER retains its existing debug bypass.
The backend must repin this revision and invalidate its PDF parse identity before
reprocessing existing receipts. An index-only rebuild cannot clean stored passages.

Tests in src/document/running_matter.rs cover split numbered headers, body
lookalikes, positional margin letters, and a generated real PDF parsed through
output_doc_new_schema with its original page locations.
