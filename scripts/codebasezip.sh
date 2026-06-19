#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

ARTIFACT_DIR="${ARTIFACT_DIR:-artifacts}"
STAMP="$(date +%Y%m%d-%H%M%S)"
ZIP_PATH="$ARTIFACT_DIR/codebase-$STAMP.zip"
LATEST_PATH="$ARTIFACT_DIR/codebase-latest.zip"

if ! command -v zip >/dev/null 2>&1; then
  echo "error: zip is required but was not found on PATH" >&2
  exit 1
fi

TMP_LIST="$(mktemp)"
cleanup() {
  rm -f "$TMP_LIST"
}
trap cleanup EXIT

emit_if_exists() {
  local path
  for path in "$@"; do
    if [[ -e "$path" ]]; then
      printf '%s\n' "$path"
    fi
  done
}

emit_tree() {
  local dir="$1"
  shift

  if [[ -d "$dir" ]]; then
    find "$dir" -type f "$@"
  fi
}

emit_top_level_code() {
  local path

  shopt -s nullglob
  for path in *.rs *.sh *.py; do
    if [[ -f "$path" ]]; then
      printf '%s\n' "$path"
    fi
  done
  shopt -u nullglob
}

{
  emit_if_exists \
    Cargo.toml \
    Cargo.lock \
    README.md \
    EVALUATION.md \
    INTEGRATION.md \
    rust-toolchain.toml \
    rust-toolchain \
    rustfmt.toml \
    clippy.toml \
    build.rs \
    Makefile

  emit_tree src
  emit_tree examples -name '*.rs'
  emit_tree tests \
    \( -name '*.rs' -o -name '*.link' -o -name '*.md' \)
  emit_tree docs -name '*.md'
  emit_tree scripts \
    \( -name '*.sh' -o -name '*.py' \)
  emit_tree tools -name '*.sh'
  emit_if_exists \
    eval/README.md \
    eval/REGRESSION_PACK.md \
    eval/compare.py \
    eval/eval.py \
    eval/generate_refs.py
  emit_tree eval/manifests -name '*.json'
  emit_tree eval/fixtures \( -name '*.json' -o -name '*.pdf' \)
  emit_tree eval/regressions -name '*.json'
  emit_tree eval/runs/stage_benchmark_current \( -name 'stage_benchmark.json' -o -name 'stage_benchmark.md' \)
  emit_tree eval/tools -name '*.py'

  emit_top_level_code
} \
  | sed 's#^\./##' \
  | grep -Ev '(^|/)(\.DS_Store|__pycache__|target|artifacts|models|pdfminer)(/|$)' \
  | awk '$0 ~ /^eval\/runs\/stage_benchmark_current\/stage_benchmark\.(json|md)$/ { print; next } $0 !~ /^eval\/(corpus|results|runs)\//' \
  | grep -Ev '^tests/docs_cache/' \
  | awk '!(/\.(zip|html|pyc)$/ || (/\.pdf$/ && $0 !~ /^eval\/fixtures\//))' \
  | sort -u > "$TMP_LIST"

if [[ ! -s "$TMP_LIST" ]]; then
  echo "error: no codebase files matched archive rules" >&2
  exit 1
fi

mkdir -p "$ARTIFACT_DIR"
rm -f "$ZIP_PATH" "$LATEST_PATH"

zip -q -@ "$ZIP_PATH" < "$TMP_LIST"
cp "$ZIP_PATH" "$LATEST_PATH"

echo "wrote $ZIP_PATH"
echo "wrote $LATEST_PATH"
echo "included $(wc -l < "$TMP_LIST" | tr -d ' ') files"
