.PHONY: codebasezip codebase-zip warning-budget production-broad-report

codebasezip:
	@./scripts/codebasezip.sh

codebase-zip: codebasezip

warning-budget:
	@./scripts/check_warning_budget.sh

production-broad-report:
	@cargo run --example stage_benchmark -- --manifest eval/manifests/production_broad_corpus.json --out-dir eval/runs/production_broad_corpus
