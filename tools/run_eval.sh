#!/usr/bin/env bash
set -euo pipefail

# Simple end-to-end runner:
# - Bootstraps a small benchmark (10–12 docs)
# - Builds the Rust extractor examples
# - Runs extractor on each available PDF with 2 configs (baseline and LA-heap)
# - Prints per-PDF heuristics to console and stores logs under benchmarks/runs/
#
# Env knobs:
#   DOC_LAYNET_PAGES   (default: 0)
#   PUBLAYNET_PAGES    (default: 10)
#   PMC_AUTO_COUNT     (default: 0)
#   PMCIDS             (default: empty)
#   MAX_TOKENS         (default: 500)
#
# Usage: tools/run_eval.sh

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT_DIR"

DOC_LAYNET_PAGES=${DOC_LAYNET_PAGES:-0}
PUBLAYNET_PAGES=${PUBLAYNET_PAGES:-10}
PMC_AUTO_COUNT=${PMC_AUTO_COUNT:-0}
PMCIDS=${PMCIDS:-}
MAX_TOKENS=${MAX_TOKENS:-500}

PY=python3

echo "[1/5] Ensuring Python deps present..."
"$PY" - <<'PY'
import importlib, sys
pkgs=[('requests',None),('tqdm',None),('yaml','pyyaml'),('lxml',None),('bs4','beautifulsoup4'),('huggingface_hub',None),('datasets',None),('PIL','pillow')]
missing=[]
for mod,pip in pkgs:
  try:
    importlib.import_module(mod)
  except Exception:
    missing.append(pip or mod)
if missing:
  print("Installing:", ' '.join(missing))
  import subprocess
  cmd=[sys.executable,'-m','pip','install','--disable-pip-version-check','--quiet']+missing
  subprocess.check_call(cmd)
print('OK')
PY

echo "[2/5] Bootstrapping dataset ..."
BOOT_CMD=("$PY" bootstrap_benchmark.py --out benchmarks --doclaynet "$DOC_LAYNET_PAGES" --publaynet "$PUBLAYNET_PAGES" --allow-noncommercial 1)
if [[ -n "${PMCIDS}" ]]; then BOOT_CMD+=(--pmcids "$PMCIDS"); fi
if [[ "$PMC_AUTO_COUNT" != "0" ]]; then BOOT_CMD+=(--pmc-auto "$PMC_AUTO_COUNT"); fi
echo "Running: ${BOOT_CMD[*]}"
"${BOOT_CMD[@]}"

echo "[3/5] Building Rust extractor examples ..."
cargo build --release --examples >/dev/null

RUN_DIR="benchmarks/runs/$(date +%Y%m%d_%H%M%S)"
mkdir -p "$RUN_DIR"

echo "[4/5] Collecting PDFs from manifest ..."
"$PY" - "$RUN_DIR" <<'PY'
import sys, yaml, os, json
outdir=sys.argv[1]
with open('benchmarks/dataset.yml','r',encoding='utf-8') as f:
  m=yaml.safe_load(f)
docs=m.get('docs',[])
rows=[]
for d in docs:
  pdf=d.get('pdf_path') or ''
  if pdf and os.path.exists(pdf):
    rows.append({'doc_id':d.get('doc_id'), 'pdf':pdf})
if not rows:
  print('No PDFs found; nothing to run.'); sys.exit(0)
with open(os.path.join(outdir,'pdf_list.json'),'w',encoding='utf-8') as f:
  json.dump(rows,f)
print(f"Found {len(rows)} PDFs to process")
PY

if [[ ! -f "$RUN_DIR/pdf_list.json" ]]; then
  echo "No PDFs in manifest; attempting to fetch tests/docs/*.pdf.link ..."
  mkdir -p benchmarks/pdfs
  while read -r linkfile; do
    url=$(cat "$linkfile" | head -n1)
    base=$(basename "$linkfile" .pdf.link)
    out="benchmarks/pdfs/${base}.pdf"
    echo "Downloading $url -> $out"
    curl -L --fail --silent --show-error "$url" -o "$out" || true
  done < <(ls tests/docs/*.pdf.link 2>/dev/null)
  # Build a minimal pdf_list.json from fetched files
  "$PY" - "$RUN_DIR" <<'PY'
import os, json, sys, glob
outdir=sys.argv[1]
pdfs=glob.glob('benchmarks/pdfs/*.pdf')
rows=[{'doc_id':os.path.splitext(os.path.basename(p))[0], 'pdf':p} for p in pdfs if os.path.getsize(p)>0]
if rows:
  with open(os.path.join(outdir,'pdf_list.json'),'w',encoding='utf-8') as f:
    json.dump(rows,f)
  print(f"Fetched {len(rows)} PDFs from tests/docs links")
else:
  print('Still no PDFs; aborting.')
  sys.exit(0)
PY
fi

echo "[5/5] Running extractor on PDFs (2 configs) ..."
CONF1="baseline"
CONF2="la_heap"
mkdir -p "$RUN_DIR/$CONF1" "$RUN_DIR/$CONF2"

# Create the small helper called by Python to avoid quoting issues
cat > tools/run_one_stub.sh <<'SH'
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
SH
chmod +x tools/run_one_stub.sh

# Iterate list
"$PY" - "$RUN_DIR" "$CONF1" "$CONF2" <<'PY'
import json, os, subprocess, sys
run_dir, c1, c2 = sys.argv[1:4]
with open(os.path.join(run_dir,'pdf_list.json'),'r',encoding='utf-8') as f:
  rows=json.load(f)
for r in rows:
  pdf=r['pdf']; docid=r['doc_id']
  # conf1 baseline
  env=os.environ.copy()
  subprocess.call(['bash','-lc',f"./tools/run_one_stub.sh '{c1}' '{pdf}' '{docid}'"], env=env)
  # conf2 la_heap
  env2=os.environ.copy(); env2['PDF_EXTRACT_LA_HEAP']='1'
  subprocess.call(['bash','-lc',f"./tools/run_one_stub.sh '{c2}' '{pdf}' '{docid}'"], env=env2)
PY

echo
echo "Run complete. Logs under: $RUN_DIR/{baseline,la_heap}/*.log"
echo "Example: tail -n +1 $RUN_DIR/baseline/*.log | sed -n '/^SUMMARY:/,/^\=\=\=\=\=\=\=\=\=/p'"

# Optional: Minimal HTML index
HTML="$RUN_DIR/index.html"
cat > "$HTML" <<HTML
<!doctype html><html><head><meta charset="utf-8"><title>Eval Run</title>
<style>body{font-family:sans-serif}pre{white-space:pre-wrap;border:1px solid #ddd;padding:8px}</style>
</head><body>
<h1>Eval Run: $(date)</h1>
<p>Configs: baseline and la_heap (PDF_EXTRACT_LA_HEAP=1)</p>
<h2>Files</h2>
<ul>
$(for f in "$RUN_DIR"/baseline/*.log; do b=$(basename "$f"); echo "<li>$b — <a href=baseline/$b>baseline</a> | <a href=la_heap/$b>la_heap</a></li>"; done)
</ul>
</body></html>
HTML

echo "HTML index: $HTML"
