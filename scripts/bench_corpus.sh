#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
date_value=${BENCHMARK_DATE:-$(date -u +%F)}
label=${BENCHMARK_LABEL:-full-core}
stability_runs=${BENCHMARK_STABILITY_RUNS:-2}
output_json=${BENCHMARK_OUTPUT_JSON:-"$repo_dir/benchmarks/runs/$label-$date_value.json"}
output_md=${BENCHMARK_OUTPUT_MD:-"$repo_dir/benchmarks/runs/$label-$date_value.md"}
fetch_ms_file=$(mktemp "${TMPDIR:-/tmp}/rag-pdf-fetch-ms.XXXXXX")
trap 'rm -f "$fetch_ms_file"' EXIT HUP INT TERM

"$script_dir/fetch_benchmark_corpus.sh" --fetch-ms-file "$fetch_ms_file"
cargo run --release --example corpus_benchmark -- \
    --corpus-dir "$repo_dir/benchmarks/corpus" \
    --output "$output_json" \
    --markdown-output "$output_md" \
    --label "$label" \
    --date "$date_value" \
    --fetch-ms "$(/usr/bin/sed -n '1p' "$fetch_ms_file")" \
    --stability-runs "$stability_runs" \
    --stability-tolerance-pct "${BENCHMARK_STABILITY_TOLERANCE_PCT:-20}"
echo "benchmark report: $output_md"
