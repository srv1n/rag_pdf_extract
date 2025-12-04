#!/usr/bin/env bash
set -euo pipefail
conf="$1"; pdf="$2"; docid="$3"
RUN_DIR="benchmarks/runs/"$(ls -1dt benchmarks/runs/* | head -n1 | xargs basename)
CONF_DIR="$RUN_DIR/$conf"
mkdir -p "$CONF_DIR"
log="$CONF_DIR/${docid}.log"
(
  echo "--> [$conf] $pdf"
  ./target/release/examples/extract "$pdf" "${MAX_TOKENS:-500}" --layout --la-boxes-flow 0.5
) >>"$log" 2>&1 || true
awk '/^SUMMARY:/{flag=1;next}/^=/{if(flag){exit}}flag' "$log" | sed 's/^/    /' >>"$log"
