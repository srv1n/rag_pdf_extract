#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

echo "== PDF extraction verification =="
echo "1. Founder regression gate"
cargo run --example founder_regression

echo
echo "2. Fixed founder/legal regression pack"
cargo run --release --example regression_pack -- \
  --manifest eval/regressions/founder_legal_pack.json \
  --out-dir eval/runs/regression/current

echo
echo "3. Broader archetype regression pack"
cargo run --release --example regression_pack -- \
  --manifest eval/regressions/archetype_benchmark_01.json \
  --out-dir eval/runs/archetype_benchmark/current

echo
echo "4. Vendored parity check"
python3 scripts/sync_vendored_pdf_extract.py --mode check

echo
echo "Verification complete."
