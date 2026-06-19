#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

echo "== Exact token-cap regression =="
echo "Running fixture-backed hard-cap test via the real parse_pdf entrypoint..."

cargo test emitted_chunks_respect_exact_token_cap_on_repo_fixtures -- --nocapture

echo
echo "Token-cap regression passed."
