#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
backend_root=${RZN_BACKEND_ROOT:-"$(CDPATH= cd -- "$repo_dir/../backend" && pwd)"}
env_file=${RZN_BACKEND_ENV:-"$backend_root/.secrets/e2e/local.env"}
corpus_dir=${BENCHMARK_CORPUS_DIR:-"$repo_dir/benchmarks/corpus"}
manifest=${BENCHMARK_MANIFEST:-"$repo_dir/benchmarks/corpus.sha256"}
fetch_ms_file=

usage() {
    echo "Usage: fetch_benchmark_corpus.sh [--fetch-ms-file PATH]" >&2
    exit 2
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --fetch-ms-file)
            [ "$#" -ge 2 ] || usage
            fetch_ms_file=$2
            shift 2
            ;;
        *)
            usage
            ;;
    esac
done

[ -f "$env_file" ] || {
    echo "credentials file not found: $env_file" >&2
    exit 1
}

get_env_value() {
    awk -F= -v wanted="$1" '
        $1 == wanted {
            sub(/^[^=]*=/, "")
            gsub(/^"|"$/, "")
            print
            exit
        }
    ' "$env_file"
}

access_key_id=$(get_env_value SOURCE_OBJECT_ACCESS_KEY_ID)
secret_access_key=$(get_env_value SOURCE_OBJECT_SECRET_ACCESS_KEY)
bucket=$(get_env_value SOURCE_OBJECT_BUCKET)
endpoint=$(get_env_value SOURCE_OBJECT_ENDPOINT)
region=$(get_env_value SOURCE_OBJECT_REGION)

[ -n "$access_key_id" ] && [ -n "$secret_access_key" ] && [ -n "$bucket" ] && [ -n "$endpoint" ] && [ -n "$region" ] || {
    echo "required SOURCE_OBJECT_* keys are missing from $env_file" >&2
    exit 1
}

export AWS_ACCESS_KEY_ID="$access_key_id"
export AWS_SECRET_ACCESS_KEY="$secret_access_key"
prefix=tenants/public/sources/70c9e920-4919-4904-b45f-37dd9b92487d/raw
names='1. Energy Watchdog v CERC.pdf
1.pdf
10. Parswanath Saha v Bandhana Modak.pdf
11. Roxann v Arun Sharma.pdf
12. CCI v Kerala Film Exhibitors Federation & Ors.pdf
13. Kumari Shrilekha Vidyarthi v State of UP.pdf
14. Tomaso Bruno v State of UP.pdf
15. MC Mehta v UoI.pdf
16. BWSSB v A Rajappa.pdf
17. Sunil B Naik v Geowave Commander.pdf
18. Mardia Chemicals v UoI.pdf
19. CBSE v Aditya Bandopadyay.pdf
2. Devas v Antrix.pdf
20 RC Cooper v UoI.pdf
3. Gayatri Balasamy v ISG.pdf
4. Har Narain v Mam Chand.pdf
5. Vodafone v UoI.pdf
6. Gujarat Bottling v Coca Cola.pdf
7. Puttaswamy v UoI.pdf
8. Vishaka v State of Rajasthan.pdf
9. Anita Hada v Godfather Travels.pdf'

mkdir -p "$corpus_dir" "$(dirname -- "$manifest")"
start_ns=$(python3 -c 'import time; print(time.monotonic_ns())')
needs_fetch=0
if [ -f "$manifest" ]; then
    if ! (cd "$corpus_dir" && shasum -a 256 -c "$manifest" >/dev/null 2>&1); then
        needs_fetch=1
    fi
else
    needs_fetch=1
fi

if [ "$needs_fetch" -eq 1 ]; then
    printf '%s\n' "$names" | while IFS= read -r name; do
        destination=$corpus_dir/$name
        aws --endpoint-url "$endpoint" --region "$region" s3 cp \
            "s3://$bucket/$prefix/$name" "$destination" --only-show-errors
    done
    manifest_tmp=$manifest.tmp.$$
    : > "$manifest_tmp"
    printf '%s\n' "$names" | while IFS= read -r name; do
        destination=$corpus_dir/$name
        hash=$(shasum -a 256 "$destination" | awk '{print $1}')
        printf '%s  %s\n' "$hash" "$name" >> "$manifest_tmp"
    done
    mv "$manifest_tmp" "$manifest"
fi

file_count=$(find "$corpus_dir" -maxdepth 1 -type f -name '*.pdf' | wc -l | tr -d ' ')
[ "$file_count" -eq 21 ] || {
    echo "expected 21 cached PDFs, found $file_count" >&2
    exit 1
}
(cd "$corpus_dir" && shasum -a 256 -c "$manifest" >/dev/null)
end_ns=$(python3 -c 'import time; print(time.monotonic_ns())')
fetch_ms=$(python3 -c 'import sys; print(f"{(int(sys.argv[1]) - int(sys.argv[2])) / 1000000:.3f}")' "$end_ns" "$start_ns")

if [ -n "$fetch_ms_file" ]; then
    mkdir -p "$(dirname -- "$fetch_ms_file")"
    printf '%s\n' "$fetch_ms" > "$fetch_ms_file"
fi
echo "cached 21 PDFs; fetch/cache ms: $fetch_ms"
