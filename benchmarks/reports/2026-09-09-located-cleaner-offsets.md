# Located-cleaner Unicode-scalar offsets: validation

Date: 2026-09-09. Reviewed source head: `a81118985a04090c97be6b34060c315d0f5b3581`.
Base: `281679e886b3e3784ff9962e661dddc84a2a773e`.

## Verdict

The scalar counter is correct on the tested contracts and eliminates the expected
quadratic prefix scans. It has **no measurable end-to-end corpus improvement**:
the spans-off median is 1.00% slower and the spans-on median is 0.10% faster,
both within observed variability. Keep the optimization for its local complexity
reduction; do not claim a corpus speedup.

## Change audit

`clean_located_text_for_indexing` has exactly three output mutations. Each
`out.push` appends one Unicode scalar, and `output_chars` increments immediately
after it. Skipped controls, deleted hyphens, and collapsed whitespace do not
increment the counter. No byte offsets, BPE accounting, span operations,
token-cap logic, public APIs, dependencies, features, or parser behavior changed.

The added differential tests retain the previous rescanning implementation and
compare whole `LocatedText` serializations. They cover empty/plain text, multibyte
and combining Unicode, whitespace/control handling, hyphenation, unmapped/coarse/
fragmented locations, and 128 deterministic generated inputs.

## Correctness evidence

- Local: `cargo test --locked` — **PASS**, 145 passed / 4 ignored.
- CI at the reviewed SHA: `build` — **PASS** (build, warning budget, test).
- Structured comparison: **PASS** for 21/21 SHA-256-verified corpus PDFs in both
  `emit_output_spans=false` and `true` modes, with `max_tokens=512`, layout,
  `all_texts=true`, cleaning, and default approximate metadata mode.
- Each mode produced 2,519 ordered chunks. Complete normalized objects were
  byte-identical between base and head: content core text/order/counts/IDs/
  headings/status/hash/source, repairs, all decoded `ContentExt` JSON, locations,
  boxes, fragments, output spans, synthetic kinds, and parent references.
- The sole normalization was `ContentCore.created_at`, generated from the clock.
  No text trimming, reordering, float rounding, or metadata omission occurred.
- An independent gpt-4o `encode_ordinary` recount found every compared chunk at
  or below 512 tokens in both modes.

The full comparison exports were intentionally temporary (717 MiB combined).
Their exact equality result and configuration are recorded here; the raw timing
artifacts are retained under `benchmarks/reports/pr3-2026-09-09/measurements/`.

## Measurement method

Machine: Apple M1 Pro / macOS arm64, 8 logical CPUs; Rust/Cargo 1.97.1.
Both clean isolated worktrees used the same generated temporary lockfile
(`SHA-256 1d58a08d80d2f3b9710d59ba1f7d5b95d61997c34f56ee275666b74ad19e29a9`),
identical `Cargo.toml`, `--release --locked`, release debug settings, corpus,
options, and `RAYON_NUM_THREADS=1`. No dependency pin changed.

Build time and output comparison were outside timing. Runs were sequential and
balanced: spans-off `base, head, head, base, base, head, head, base, base, head,
base, head`; spans-on used the inverse order. The first process after release
build is shown separately; filesystem caches were not flushed, so it is not a
cold-cache claim. The following five samples per revision are the cache-warm
series. Raw per-document and batch reports are durable in the measurements path.

| mode | fresh base / head batch ms | warm n | base median batch ms (IQR) | head median batch ms (IQR) | head delta |
| --- | ---: | ---: | ---: | ---: | ---: |
| detailed spans off | 29091.159 / 28086.032 | 5 | 28420.741 (272.252) | 28704.452 (399.242) | +1.00% |
| detailed spans on | 44048.173 / 41260.172 | 5 | 33584.248 (83.323) | 33549.929 (317.867) | -0.10% |

The corresponding warm medians for summed document extraction time are
28,333.436 ms base vs 28,610.536 ms head (off, +0.98%), and 33,528.538 ms
base vs 33,494.154 ms head (on, -0.10%). The first-process values are one
sample each and are not interpreted as a performance result.

## Focused cleaner measurement

This is a separate, source-level diagnostic—not end-to-end extraction. On a
60,000-scalar `é中🦀` input with no source spans, seven alternating pairs gave:

- previous rescanning cleaner: `[223.790, 224.874, 225.248, 225.285, 225.619, 227.360, 245.520]` ms; median **225.285 ms**;
- scalar-counter cleaner: `[2.159, 2.182, 2.186, 2.188, 2.195, 2.215, 2.296]` ms; median **2.188 ms**.

That is about 103× on this deliberately prefix-scan-heavy input. It validates
the eliminated work; parsing, final BPE enforcement, and spans-enabled zstd
compression still dominate the corpus workload.

## CI semver status

`semver` remains a known upstream-baseline failure, not a PR regression. The
same main/base SHA fails the same four categories against published `0.9.0`:
added `OutputError` variants, removed legacy variants/functions, and changed
public function arities. The PR does not modify those APIs or the semver workflow.
The current PR head is mergeable but draft and `UNSTABLE` only because that
inherited check remains red.

## Merge recommendation

**Merge after the normal reviewer acceptance.** Correctness is established;
the semver failure is pre-existing. Treat performance as neutral: this is a
safe local algorithmic cleanup, not a demonstrated corpus acceleration.
