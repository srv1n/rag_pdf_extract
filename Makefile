.PHONY: codebasezip codebase-zip warning-budget production-broad-report bench-corpus bench-corpus-2core bench-corpus-ecores

codebasezip:
	@./scripts/codebasezip.sh

codebase-zip: codebasezip

warning-budget:
	@./scripts/check_warning_budget.sh

production-broad-report:
	@cargo run --example stage_benchmark -- --manifest eval/manifests/production_broad_corpus.json --out-dir eval/runs/production_broad_corpus


bench-corpus:
	@BENCHMARK_LABEL=full-core BENCHMARK_STABILITY_RUNS=2 ./scripts/bench_corpus.sh

bench-corpus-2core:
	@BENCHMARK_LABEL=2-core BENCHMARK_STABILITY_RUNS=1 RAYON_NUM_THREADS=2 ./scripts/bench_corpus.sh

bench-corpus-ecores:
	@BENCHMARK_LABEL=efficiency-cores BENCHMARK_STABILITY_RUNS=1 BENCH_SCHEDULER='taskpolicy -c background' taskpolicy -c background -- ./scripts/bench_corpus.sh
