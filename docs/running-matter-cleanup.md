# PDF running matter cleanup

`src/document/running_matter.rs` removes page furniture before continuation,
hyphenation, title-block merging and chunking. No reporter or corpus names are
matched; every rule is geometric or repetition-based.

## Running heads and feet

A visual line may contain several PDF font runs; repetition uses their x-ordered
combined text. Two rules remove a line:

- The existing detector: a short line in the outer 10% bands that repeats on most
  pages, or matches a page-number, date or copyright pattern.
- Recurring edge lines: a line in the outer 20% bands whose text recurs on at least
  three pages at roughly the same height (within two font sizes, because scanned
  pages drift). This catches alternating recto/verso heads and per-section heads,
  which each sit on only part of the pages, and reporter heads that sit about 14%
  down the page. Lines compare with only a page number at either end masked and all
  bracket kinds treated alike, so OCR noise such as `(1988]` beside `[1988]` still
  matches while numbered section heads ("Section 1 Overview") and footnote citations
  that differ only in their numbers stay distinct. A masked number must be the page
number: across the occurrences it moves with the page by one constant offset, so
"1 Overview" on pages 2, 5 and 9 stays. A recurring
  line counts only inside the unbroken stack of recurring lines nearest the page
  edge; a lookalike below a non-recurring line stays as body text.

A head that never repeats inside one document (a two-page judgment) is kept.

## Margin line-letter rails

A single uppercase letter outside the substantive text column by more than 0.3 font
sizes is a margin candidate. The column comes from the most common line end and the
leftmost recurring line start, ignoring lines that open with a lone capital, so a
glued rail letter cannot widen it. Removal requires four or more different letters on
the same side of a page, at least one page of the document showing five or more, in alphabetical order down the page and aligned within 1.5
font sizes, and each removed letter must sit at the same height (within two font
sizes) on at least three pages. A printed rail
repeats its A..H positions page after page; an A/B/C outline does not. A rail letter
the layout glued to the end of a body line (`... and, A`) or to its start
(`C the cardholder`) is trimmed from that line by the side its rail sits on, so a
right-rail line that opens with the article `A` keeps it. The retained prefix, body
and trailing whitespace keep their own output spans, and the segment's source range
excludes the letter. Isolated initials, outline labels
and letters inside the body column remain. A rail in a judgment shorter than three
pages is kept.

## Footnotes

On each page the body size is the font size carrying the most characters. Smaller
type below the last body line that opens with a superscript number, carries text,
whose opening mark sits on its first line just left of that text, and which is cited
by the same superscript number in the body is a footnote block. Only a mark with note
text beside it labels a footnote, so a small page number in the foot neither moves nor
claims a body superscript. A lone small page
number or a numbered caption fails these tests and stays. Footnote segments leave the page flow and follow the last page, so a sentence
that crosses a page break is not cut by the footnotes at the foot of the page.
Superscript reference marks in the body whose number matches a footnote on the same
page are dropped. Footnote segments keep their own page locations, and chunk page
ranges use the lowest and highest page of their segments, so a chunk that holds
relocated footnotes still reports every page it covers.

## Limits

A lettered outline of five or more items that repeats at the same heights on three
pages is read as a rail. These heuristics cannot repair arbitrary OCR, letters merged into the middle of an
OCR line, or a glyph whose layout box is wrong. Ambiguous text stays unchanged.
Original retained segments keep their page, source character ranges and located
text. There is no extra PDF parse, serialized span archive or second repeated-line
scan after merges. PDF_EXTRACT_SKIP_HEADER_FOOTER retains its existing debug bypass.
The backend must repin this revision and invalidate its PDF parse identity before
reprocessing existing receipts. An index-only rebuild cannot clean stored passages.

Tests in src/document/running_matter.rs cover split numbered headers, alternating
reporter heads, body lookalikes, positional and glued margin letters, footnote
relocation and a generated real PDF parsed through output_doc_new_schema with its
original page locations.
