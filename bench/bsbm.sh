#!/usr/bin/env bash
# Berlin SPARQL Benchmark: oxilite (bundled SQLite, system SQLite through dlopen, D1 through the
# Miniflare sidecar) against Oxigraph (RocksDB), with the official BSBM tools.
#
#   bench/bsbm.sh [products] [parallelism] [runs]
#
# Environment: OXIGRAPH (oxigraph CLI, default `oxigraph`), OXILITE_SQLITE_LIBRARY (system
# SQLite), OXILITE_D1_URL (sidecar; D1 is skipped when unset), ENGINES (default: all).
# Results: bench/results/*.xml and load.*.json; `cargo run -p oxilite-bench --bin bsbm-report`
# turns them into bench/results/summary.json and the README table.
set -eu
PRODUCTS=${1:-1000}      # about 350 triples per product
PARALLELISM=${2:-4}
RUNS=${3:-200}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/bench/results"
OXIGRAPH=${OXIGRAPH:-oxigraph}
LIB=${OXILITE_SQLITE_LIBRARY:-$([ "$(uname)" = Darwin ] && echo /usr/lib/libsqlite3.dylib || echo libsqlite3.so.0)}
ENGINES=${ENGINES:-"oxilite oxilite-dylib oxilite-d1 oxigraph"}
WORK=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$WORK"' EXIT

now() { python3 -c 'import time; print(time.time())'; }
mkdir -p "$OUT"
cargo build --release -p oxilite-cli --manifest-path "$ROOT/Cargo.toml"
OXILITE="$ROOT/target/release/oxilite"
cd "$ROOT/bench/bsbm-tools"
./generate -fc -pc "$PRODUCTS" -s nt -fn "$WORK/data" -dir "$WORK/td_data" > /dev/null
TRIPLES=$(wc -l < "$WORK/data.nt" | tr -d ' ')
echo "BSBM $PRODUCTS products, $TRIPLES triples"

BI_RUNS=${BI_RUNS:-$(( RUNS / 20 > 2 ? RUNS / 20 : 2 ))}
run_mixes() { # engine url
  for mix in explore businessIntelligence; do
    if [ "$mix" = explore ]; then runs=$RUNS; timeout=60000; else runs=$BI_RUNS; timeout=300000; fi
    ./testdriver -mt "$PARALLELISM" -runs "$runs" -w $((runs / 10 + 1)) -t "$timeout" -idir "$WORK/td_data" \
      -ucf "usecases/$mix/sparql.txt" -o "$OUT/bsbm.$mix.$1.$PRODUCTS.xml" "$2" > "$WORK/$1.$mix.log" 2>&1 \
      || echo "  $mix mix failed on $1 (see $WORK/$1.$mix.log)"
  done
}

record_load() { # engine seconds bytes
  printf '{"engine": "%s", "products": %s, "triples": %s, "load_seconds": %s, "size_bytes": %s}\n' \
    "$1" "$PRODUCTS" "$TRIPLES" "$2" "$3" > "$OUT/load.$1.$PRODUCTS.json"
}

port=7890
for engine in $ENGINES; do
  port=$((port + 1))
  url="http://127.0.0.1:$port/query"
  case $engine in
    oxilite|oxilite-dylib)
      db="$WORK/$engine.sqlite"
      extra=(); [ "$engine" = oxilite-dylib ] && extra=(--library "$LIB")
      start=$(now)
      "$OXILITE" load -l "$db" ${extra[@]+"${extra[@]}"} -f "$WORK/data.nt"
      secs=$(python3 -c "print(round($(now) - $start, 3))")
      size=$(cat "$db"* | wc -c | tr -d ' ')
      "$OXILITE" serve -l "$db" ${extra[@]+"${extra[@]}"} -b "127.0.0.1:$port" --threads "$PARALLELISM" 2> "$WORK/$engine.serve.log" &
      ;;
    oxilite-d1)
      [ -n "${OXILITE_D1_URL:-}" ] || { echo "skipping D1 (OXILITE_D1_URL unset)"; continue; }
      curl -s -X POST "$OXILITE_D1_URL/reset" > /dev/null
      start=$(now)
      "$OXILITE" load --d1-sidecar "$OXILITE_D1_URL" -f "$WORK/data.nt"
      secs=$(python3 -c "print(round($(now) - $start, 3))")
      size=0
      "$OXILITE" serve --d1-sidecar "$OXILITE_D1_URL" -b "127.0.0.1:$port" --threads 1 2> "$WORK/$engine.serve.log" &
      ;;
    oxigraph)
      command -v "$OXIGRAPH" > /dev/null || { echo "skipping Oxigraph ($OXIGRAPH not found)"; continue; }
      db="$WORK/oxigraph"
      start=$(now)
      "$OXIGRAPH" load --location "$db" --file "$WORK/data.nt" > /dev/null 2>&1
      secs=$(python3 -c "print(round($(now) - $start, 3))")
      size=$(du -sk "$db" | cut -f1); size=$((size * 1024))
      "$OXIGRAPH" serve --location "$db" --bind "127.0.0.1:$port" 2> "$WORK/$engine.serve.log" &
      ;;
  esac
  pid=$!
  sleep 2
  echo "$engine: loaded in ${secs}s, $size bytes; running the mixes"
  record_load "$engine" "$secs" "$size"
  run_mixes "$engine" "$url"
  kill "$pid"; wait "$pid" 2>/dev/null || true
done
echo "results in $OUT"
