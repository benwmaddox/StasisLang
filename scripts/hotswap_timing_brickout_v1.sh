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
: "${SDL_AUDIODRIVER:=dummy}"
: "${STASIS_HOTSWAP_DELAY_MS:=500}"

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

# wait for initial compile/runner start marker
initial_timeout_s=180
start_ts=$(date +%s)
while true; do
  if grep -q "HOTSWAP(ms):" "$out_log" 2>/dev/null; then
    break
  fi
  now=$(date +%s)
  if (( now - start_ts >= initial_timeout_s )); then
    echo "error: timed out waiting for initial HOTSWAP(ms) marker" >&2
    tail -n 50 "$out_log" >&2 || true
    exit 1
  fi
  sleep 0.05
done

swap_any_needle="HOTSWAP(ms):"
swap_ok_pattern='HOTSWAP\(ms\):.*load=[0-9]'
exit_needle="warning: runner exited with code"
prev_any_count=0
if [[ -f "$out_log" ]]; then
  prev_any_count=$(grep -c "$swap_any_needle" "$out_log" || true)
fi
prev_ok_count=0
if [[ -f "$out_log" ]]; then
  prev_ok_count=$(grep -E -c "$swap_ok_pattern" "$out_log" || true)
fi
prev_exit_count=0
if [[ -f "$err_log" ]]; then
  prev_exit_count=$(grep -c "$exit_needle" "$err_log" || true)
fi
max_retries="${MAX_RETRIES:-3}"

for i in $(seq 1 "$iterations"); do
  attempt=0
  while true; do
    attempt=$((attempt + 1))
    printf "\n// hotswap timing brickout_v1 aot %s.%s %s\n" "$i" "$attempt" "$(date +%s%N)" >> "$sample"
    sleep "$(awk "BEGIN {print $sleep_after_edit_ms/1000}")"

    start_ts=$(date +%s%N)
    while true; do
      if [[ -f "$out_log" ]]; then
        ok_count=$(grep -E -c "$swap_ok_pattern" "$out_log" || true)
        if (( ok_count > prev_ok_count )); then
          prev_ok_count=$ok_count
          break 2
        fi
      fi
      if [[ -f "$err_log" ]]; then
        exit_count=$(grep -c "$exit_needle" "$err_log" || true)
        if (( exit_count > prev_exit_count )); then
          prev_exit_count=$exit_count
          start_restart_ts=$(date +%s%N)
          while true; do
            if [[ -f "$out_log" ]]; then
              any_count=$(grep -c "$swap_any_needle" "$out_log" || true)
              if (( any_count > prev_any_count )); then
                prev_any_count=$any_count
                break
              fi
            fi
            now_ns=$(date +%s%N)
            elapsed_ms=$(( (now_ns - start_restart_ts) / 1000000 ))
            if (( elapsed_ms >= swap_timeout_ms )); then
              echo "error: timed out waiting for restart marker after runner exit (edit $i)" >&2
              tail -n 80 "$out_log" >&2 || true
              tail -n 80 "$err_log" >&2 || true
              exit 1
            fi
            sleep 0.05
          done
          if (( attempt >= max_retries )); then
            echo "error: exceeded retries after runner exits on edit $i" >&2
            tail -n 50 "$err_log" >&2 || true
            exit 1
          fi
          break
        fi
      fi
      now_ts=$(date +%s%N)
      elapsed_ms=$(( (now_ts - start_ts) / 1000000 ))
      if (( elapsed_ms >= swap_timeout_ms )); then
        echo "error: timed out waiting for HOTSWAP(ms) after edit $i (attempt $attempt)" >&2
        tail -n 50 "$out_log" >&2 || true
        exit 1
      fi
      sleep 0.05
    done
  done
done

loads=$(grep -E "$swap_ok_pattern" "$out_log" | sed -n 's/.*load=\\([-0-9][0-9.]*\\).*/\\1/p' | awk '$1 >= 0')

echo "load_ms: $(printf "%s" "$loads" | tr '\n' ' ')"

if [[ -z "$loads" ]]; then
  echo "summary: n/a"
  exit 0
fi

summary=$(printf "%s\n" "$loads" | awk 'NR==1{min=$1;max=$1;sum=$1;count=1;next}{if($1<min)min=$1;if($1>max)max=$1;sum+=$1;count++}END{printf "%.3f %.3f %.3f", min, max, sum/count}')
set -- $summary
min_ms=$(awk "BEGIN{printf \"%.3f\", $1}")
max_ms=$(awk "BEGIN{printf \"%.3f\", $2}")
avg_ms=$(awk "BEGIN{printf \"%.3f\", $3}")
echo "summary: min=${min_ms}ms avg=${avg_ms}ms max=${max_ms}ms"
