# Remaining optimization validation — 2026-09-09

Base: `9816e3fe02377e987334852f99e2ca3b770e0d70` (PR #3 merged). Candidate: `3dc86350f25e8cb7f54c51ad959ea63c9b2b5f01`. Release builds used the same generated, uncommitted `Cargo.lock` SHA-256 `822d7201fd4474e1a32afd6334b3da615a3bfc12e378e447c86d80e585dab89a`, `RAYON_NUM_THREADS=1`, and the verified 21-PDF / 1,925-page corpus at 512 tokens.

## Accepted: skip un-emitted detailed-span compaction

Full 21-PDF structured outputs were byte-identical after removing only `content_core.created_at`: spans off SHA-256 `6d32efe81ac5998851282beb62c392c7816b348b05445bcb47864cf94bbd20c0` (83 MiB per side); spans on `268d23bf70b5d13cd3b866b38f1d982d3ad9f6cead5b58545b0222b5a901ece2` (275 MiB per side). This includes content/order/counts/caps/IDs/headings/locations and decoded metadata/spans.

| mode | base median (ms) | candidate median (ms) | range (ms) | result |
|---|---:|---:|---:|---|
| spans off, 5 alternating runs | 27,761.4 | 27,659.8 | base 27,687–27,897; candidate 27,597–28,038 | 0.37% faster; below noise |
| spans on, 3 alternating runs | 33,608.0 | 33,496.6 | base 33,556–33,640; candidate 33,484–33,570 | control unchanged within noise |

The frozen serializer differential test passes with spans off and on. There is no measurable corpus gain; the retained guard is nevertheless a three-line behavior-preserving removal of default-mode work.

## Excluded: plain cleaner allocation package

The legacy differential sweep passed (C0/C1, Unicode spaces, 256 deterministic mixed strings), and output digests matched. Its 4 MiB × 20 focused results do not justify merging it:

| input | base median (ms) | candidate median (ms) | peak RSS |
|---|---:|---:|---:|
| already-clean | 192.3 | 260.0 | 55.8 → 27.7 MiB |
| replacement-heavy | 1,130.9 | 1,043.7 | 264.2 → 36.9 MiB |

It improves replacement-heavy work 7.7% and avoids retained intermediate buffers, but regresses already-clean work 35.2%. The existing 21-PDF `fast_text_benchmark` was also attempted for fast-text/no-layout extraction and was killed by the host with exit 137 before writing output; no corpus cleaner speed claim is made.

## Checks and limits

- `cargo test content_ext_compaction_tests --lib`: PASS (1 test).
- `cargo test plain_cleaner_allocation_tests --lib`: PASS (2 tests), on the rejected experiment only.
- `git diff --check`: PASS for accepted commit.
- Full-workspace `cargo fmt --check` is pre-existing red across unrelated files; accepted added test was individually rustfmt-checked.
- No dependencies, source guards, or PR #3 code were bypassed or overwritten.
