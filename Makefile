.PHONY: codebasezip codebase-zip warning-budget production-broad-report bench-corpus bench-corpus-2core bench-corpus-ecores bench-corpus-combined paired-regression paired-regression-self-test

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

bench-corpus-combined:
	@python3 ./scripts/render_benchmark_comparison.py \
		--full benchmarks/runs/full-core-2026-07-21.json \
		--two-core benchmarks/runs/2-core-2026-07-21.json \
		--efficiency benchmarks/runs/efficiency-cores-2026-07-21.json \
		--date 2026-07-21 \
		--output benchmarks/reports/2026-07-21-constrained-comparison.md

paired-regression-self-test:
	@python3 scripts/paired_regression.py --self-test

paired-regression:
	@test -n "$(PDF_BASELINE_REV)" -a -n "$(PDF_BASELINE_CMD)" -a -n "$(PDF_CANDIDATE_REV)" -a -n "$(PDF_CANDIDATE_CMD)" || (echo "set PDF_BASELINE_REV, PDF_BASELINE_CMD, PDF_CANDIDATE_REV, and PDF_CANDIDATE_CMD" >&2; exit 2)
	@python3 scripts/paired_regression.py \
		--corpus-dir tests/fixtures/harness_corpus \
		--baseline-rev "$(PDF_BASELINE_REV)" \
		--baseline-cmd "$(PDF_BASELINE_CMD)" \
		--candidate-rev "$(PDF_CANDIDATE_REV)" \
		--candidate-cmd "$(PDF_CANDIDATE_CMD)" \
		--adversarial-dir tests/fixtures/adversarial \
		--require-adversarial \
		--require-document-bound-commands \
		--output target/paired-regression/report.json \
		--markdown-output target/paired-regression/report.md
