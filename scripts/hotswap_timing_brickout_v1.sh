#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

iterations="${ITERATIONS:-8}"
fps="${FPS:-60}"
sleep_after_edit_ms="${SLEEP_AFTER_EDIT_MS:-2000}"
swap_timeout_ms="${SWAP_TIMEOUT_MS:-30000}"

sample="$root/samples/brickout_revenge/brickout_revenge_v1.stasis"
if [[ ! -f "$sample" ]]; then
  echo "error: sample not found: $sample" >&2
  exit 1
fi

out_log="$root/build/hotswap_brickout_v1.out.log"
err_log="$root/build/hotswap_brickout_v1.err.log"

mkdir -p "$root/build"
rm -f "$out_log" "$err_log"

orig_tmp="$(mktemp)"
cp "$sample" "$orig_tmp"

export STASIS_DEV=1
export STASIS_USE_SDL=1
export STASIS_ASSET_ROOT="$root"
: "${STASIS_CRANELIFT_JIT_RUNNER:=0}"
: "${STASIS_DISABLE_AUDIO:=1}"

./stasis.sh run "$sample" --watch --backend cranelift --graphics --module brick --fps "$fps" >"$out_log" 2>"$err_log" &
watch_pid=$!

cleanup() {
  if kill -0 "$watch_pid" >/dev/null 2>&1; then
    kill "$watch_pid" >/dev/null 2>&1 || true
    wait "$watch_pid" >/dev/null 2>&1 || true
  fi
  if [[ -f "$orig_tmp" ]]; then
    cp "$orig_tmp" "$sample" || true
    rm -f "$orig_tmp" || true
  fi
}
trap cleanup EXIT

# wait for initial HOTRELOAD
initial_timeout_s=180
start_ts=$(date +%s)
while true; do
  if grep -q "HOTRELOAD phases(ms):" "$err_log" 2>/dev/null; then
    break
  fi
  now=$(date +%s)
  if (( now - start_ts >= initial_timeout_s )); then
    echo "error: timed out waiting for initial HOTRELOAD" >&2
    tail -n 50 "$err_log" >&2 || true
    exit 1
  fi
  sleep 0.05
done

load_needle="HOTSWAP load(us):"
prev_count=0
if [[ -f "$out_log" ]]; then
  prev_count=$(grep -c "$load_needle" "$out_log" || true)
fi

for i in $(seq 1 "$iterations"); do
  printf "\n// hotswap timing brickout_v1 aot %s %s\n" "$i" "$(date +%s%N)" >> "$sample"
  sleep "$(awk "BEGIN {print $sleep_after_edit_ms/1000}")"

  start_ts=$(date +%s%N)
  while true; do
    if [[ -f "$out_log" ]]; then
      count=$(grep -c "$load_needle" "$out_log" || true)
      if (( count > prev_count )); then
        prev_count=$count
        break
      fi
    fi
    now_ts=$(date +%s%N)
    elapsed_ms=$(( (now_ts - start_ts) / 1000000 ))
    if (( elapsed_ms >= swap_timeout_ms )); then
      echo "error: timed out waiting for HOTSWAP load after edit $i" >&2
      tail -n 50 "$out_log" >&2 || true
      exit 1
    fi
    sleep 0.05
  done
done

loads=$(grep "$load_needle" "$out_log" | sed -n 's/.*: \([-0-9][0-9]*\).*/\1/p' | awk '$1 >= 0')

echo "load_us: $(printf "%s" "$loads" | tr '\n' ' ')"

if [[ -z "$loads" ]]; then
  echo "summary: n/a"
  exit 0
fi

summary=$(printf "%s\n" "$loads" | awk 'NR==1{min=$1;max=$1;sum=$1;count=1;next}{if($1<min)min=$1;if($1>max)max=$1;sum+=$1;count++}END{printf "%d %d %.3f", min, max, sum/count}')
set -- $summary
min_ms=$(awk "BEGIN{printf \"%.3f\", $1/1000}")
max_ms=$(awk "BEGIN{printf \"%.3f\", $2/1000}")
avg_ms=$(awk "BEGIN{printf \"%.3f\", $3/1000}")
echo "summary: min=${min_ms}ms avg=${avg_ms}ms max=${max_ms}ms"
