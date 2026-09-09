# Located-cleaner offsets: audit and unrun validation

Date: 2026-09-09. Status: **draft; not validated for merge**.

## Baseline and available evidence

Base: `281679e886b3e3784ff9962e661dddc84a2a773e`. GitHub's PR #2 metadata and the `main` branch were checked before editing: main is PR #2's merge commit. Main was checked again before publishing the branch and had not moved.

Code/test commit: `b49a6b71f1962cda45ea7313b33105bdb2f5981d`, directly parented by that base. This report is a separate documentation commit. Record the final PR head SHA when running validation.

No tracked `AGENTS.md` was found in the recursive base tree; fetching the root path returned 404. These requested report paths also returned 404 on the base:

- `benchmarks/reports/pr2-2026-09-09/README.md`
- `benchmarks/reports/2026-09-09-structured-profile.md`

No report copies or PDF corpus were attached to this editing session. The request's account of PR #2's equivalent 21-PDF structured output without measurable end-to-end speedup, and the subsequent BPE/zstd/lopdf profiling, is context supplied by the requester, not a measurement reproduced here.

## Retained change

In `src/document/processing.rs`, `clean_located_text_for_indexing` previously called `out.chars().count()` immediately before each emitted character. Those calls repeatedly scan the growing output solely to obtain a Unicode-scalar offset. The patch maintains that offset incrementally.

The invariant is `output_chars == out.chars().count()`: both start at zero, every mutation of `out` appends exactly one Rust `char`, and the counter increments immediately after that append. Skipped controls, removed hyphens, and collapsed whitespace do not increment it. This is a character offset, **not a BPE token count**. No byte-count substitution is used.

Hyphenation, control filtering, the explicit Unicode-space set, whitespace handling, source lookup, synthetic parent references, span merging, and final span compaction retain their existing order and implementation. The location-backed cleaner is not replaced by the plain cleaner. The final BPE cap and all tokenizer calls remain unchanged.

This removes redundant output-prefix scans by inspection. It does not establish an end-to-end speedup or make the entire cleaner linear: source-span lookup and other work remain. Retention in a merge-ready optimization PR requires the measurements and regression results below.

The code commit changes only the counter and a test-module declaration in the existing source file (12 added lines, 3 removed), plus a new test file. Public APIs, dependencies, feature defaults, parser code, CI, and downstream dependency pins are untouched.

## Regression coverage authored, not executed

`src/document/processing_offset_tests.rs` retains the previous cleaner as a test reference, sharing only unchanged span helpers. Two tests compare the entire serialized `LocatedText` value, without normalization: text, span order, output offsets, PDF pages/ranges/boxes, synthetic kinds, and every parent reference.

Cases include empty and plain text, multibyte characters and combining marks, normalized and non-normalized Unicode whitespace, controls/CRLF, hyphenation, punctuation runs, longer input, and deterministic generated inputs. Each uses unmapped, coarse PDF-backed, and fragmented mixed PDF/synthetic mappings.

These tests are not a substitute for full extraction comparisons, independent exact token counting, or decoded ContentExt comparisons. They have not compiled or run in this environment.

## Candidate audit

| Candidate | Inspection and decision |
| --- | --- |
| Exact BPE reuse | The cap runs on the final cleaned paragraph; upstream counts can describe pre-cleaned text or approximate budgets. Exact-mode content-core construction can repeat tokenization, but safe reuse needs local provenance for the identical final text and tokenizer, including split/trim cases. Deferred rather than introducing count plumbing without execution or measurement. No cap removed; approximate metadata counts remain approximate. |
| Metadata serialization/compression | `create_content_ext_with_spans` already borrows its payload fields, serializes once with `serde_json::to_vec`, and compresses once with `zstd::bulk::compress(..., 3)`. No duplicate serialization/compression pass was identified. Span compaction is prepared even when detailed emission is disabled; avoiding that is an off-only candidate, not a fix for the reported spans-enabled compression cost. Deferred without measurements. Schema, codec, compression level, decoded metadata, and emission behavior are not changed. |
| Plain character/string cleaner | `clean_text_for_indexing` performs internal-hyphen cleaning, control removal, five explicit Unicode-space replacements, and ordered regex passes with owned-string conversions. Avoiding no-match allocations or combining compatible scans is plausible, but needs its own Unicode/order regression tests and separate fast-text measurements. Deferred. This path is distinct from the retained location-backed change. |
| Located cleaner offsets | Retained provisionally in this draft: a local invariant removes repeated scans without modifying transformations or mapping helpers. Full extraction performance and equivalence remain unrun. |
| Header/footer regex initialization | Normalization regexes are already lazy statics; detector construction still compiles fixed recognition patterns per instance. Deferred because worthwhile savings were not measured. |
| `span_map.rs` temporary copies | Slicing clones span/source data and location grouping builds temporary collections. Deferred: no evidence of worthwhile savings, and mapping/ordering behavior deserves independent tests. No span-map changes included. |
| lopdf parsing | Left unchanged. A parser rewrite is outside scope; the request's reported persistent parsing cost is not addressed by this patch. |

No speculative cross-chunk cache, compressor pool, new dependency, span disabling, or API change was introduced.

## Commands actually attempted

The local editing environment has no Cargo or rustc on PATH. GitHub connector reads and writes work, but direct Git access failed DNS resolution. These probes were attempted; no Rust compilation or test process started:

| Exact command | Observed result |
| --- | --- |
| `git ls-remote https://github.com/srv1n/rag_pdf_extract HEAD` | Exit 128; could not resolve host `github.com`. |
| `cargo --version` | Exit 127; `cargo: command not found`. |
| `rustc --version` | Exit 127; `rustc: command not found`. |
| `cargo test --locked --lib document::processing` | Exit 127; `cargo: command not found`; tests not run. |
| `cargo test --locked --release --lib document::processing` | Exit 127; `cargo: command not found`; tests not run. |
| `cargo test --locked` | Exit 127; `cargo: command not found`; tests not run. |
| `cargo fmt --all -- --check` | Exit 127; `cargo: command not found`; formatting not checked. |

The final code commit's GitHub diff was inspected: only the intended counter/test changes are present. This is source review, not runtime validation.

The existing `.github/workflows/rust.yml` still runs build, warning-budget, test, and semver jobs. Its `obi1kenobi/cargo-semver-checks-action@v2` step is unchanged. No CI success is asserted by this report; record actual CI results separately.

## Measured results

**None. No timing samples, medians, variability statistics, cold-start timings, or corpus-equivalence results were collected.**

| Required validation | Result |
| --- | --- |
| Structured extraction, 512-token cap, detailed spans disabled | Not run |
| Structured extraction, 512-token cap, detailed spans enabled | Not run |
| Fast-text extraction, separately | Not run; unchanged-path control, no fast-text speedup claimed |
| Repeated paired release timings and cold-start separation | Not run |
| Full ordered chunk, exact-token, ID, heading, location, and decoded-span comparison | Not run |
| New differential tests and existing regressions | Blocked before execution, as above |
| Formatting, warning budget, and semver validation | Not established in this session |

## Required paired validation before merge

Use separate clean base and candidate worktrees. Freeze one dependency resolution for both, record identical Cargo.lock SHA-256 values, and build with `--locked`. Record both full revision SHAs, rustc version/target, features, environment, RUSTFLAGS, and the same release configuration, including the repository's release debug setting. Build outside timing. Do not update dependency pins to obtain a result.

Use the same machine, CPU affinity, corpus bytes/order/checksums, and worker count for both revisions. A concrete initial configuration is `RAYON_NUM_THREADS=2` for both; this is a proposed setting, not a setting used for measurements here. Restore the original comparison corpus rather than labeling a different fixture collection as the requester's 21-PDF corpus.

Run structured extraction with `max_tokens=Some(512)`, cleaning enabled, and all other features/defaults held fixed. Make two separate paired cases, explicitly setting detailed output-span emission false and true. Keep the default token-count mode unchanged for the primary benchmark; exercise exact mode additionally as regression coverage. Benchmark fast-text separately as an unchanged-path control. Do not infer structured gains from plain-cleaner or fast-text microbenchmarks.

Collect at least eight paired repetitions in balanced base/patch and patch/base order, sequentially rather than competing on the same machine. Save every raw duration. Report per-PDF and total-corpus medians with IQR or MAD and min/max; do not pool the two span modes. Separate fresh-process startup (including tokenizer/regex initialization) from explicitly warmed in-process runs. State filesystem-cache treatment; a fresh process is not proof of a cold filesystem cache.

Outside timed regions, compare every chunk in sequence. Require identical text, stored token-count fields, IDs/hashes, source identity, headings, locations, repairs, and other deterministic schema fields. Independently tokenize the identical final chunk text using the same exact BPE/tokenizer version on both sides and verify count equality and the 512-token cap; approximate stored counts cannot establish cap compliance.

Decode every zstd ContentExt payload with the existing decoder and compare the complete JSON schema and values, including all citation spans, output/source ranges, bounding boxes, synthetic kinds, and parent references. Preserve array order, float values, and null-versus-absent distinctions. Do not discard detailed metadata to make comparisons pass. The only preidentified normalization candidate is `content_core.created_at`, which is generated from the clock; explicitly record its removal if used. Any other proposed normalization requires a separately identified nondeterministic source, not a blanket ignore rule.

Rerun the exact Cargo commands above on a working checkout and retain their complete results. Run the unchanged CI warning-budget and semver checks. This draft contains no new executable corpus benchmark/comparator; the above is an unexecuted validation protocol, not a claim that an existing script already performs every required comparison.
