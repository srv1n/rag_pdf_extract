#!/usr/bin/env bash
set -euo pipefail

budget="${PDF_EXTRACT_WARNING_BUDGET:-64}"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

cargo check --all-targets --message-format=json >"$tmp"
warnings="$(
  python3 - "$tmp" <<'PY'
import json
import sys

count = 0
with open(sys.argv[1], "r", encoding="utf-8") as fh:
    for line in fh:
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        if item.get("reason") == "compiler-message" and item.get("message", {}).get("level") == "warning":
            count += 1
print(count)
PY
)"

if [ "$warnings" -gt "$budget" ]; then
  echo "warning budget exceeded: ${warnings} > ${budget}" >&2
  exit 1
fi

echo "warning budget ok: ${warnings} <= ${budget}"
