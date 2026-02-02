#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"

file="${1:-samples/hotstate_tick_watch.stasis}"
module="${2:-hot}"

cd "${repo_root}"

if [[ ! -f "${file}" ]]; then
  echo "error: file not found: ${file}" 1>&2
  exit 2
fi

tmp_dir="$(mktemp -d)"
log="${tmp_dir}/watch.log"
backup="${tmp_dir}/orig.stasis"
cp "${file}" "${backup}"

cleanup() {
  set +e
  if [[ -n "${watch_pid:-}" ]]; then
    kill "${watch_pid}" >/dev/null 2>&1 || true
    wait "${watch_pid}" >/dev/null 2>&1 || true
  fi
  cp "${backup}" "${file}" >/dev/null 2>&1 || true
  rm -rf "${tmp_dir}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

export STASIS_ASSET_ROOT="${repo_root}"
: "${STASIS_CRANELIFT_JIT_RUNNER:=0}"
: "${STASIS_JIT_WATCHDOG_MS:=15000}"

./stasis.sh run "${file}" --watch --backend cranelift --module "${module}" --fps 60 >"${log}" 2>&1 &
watch_pid="$!"

deadline=$((SECONDS + 180))
while [[ "${SECONDS}" -lt "${deadline}" ]]; do
  if grep -q "HOTSWAP(ms):" "${log}"; then break; fi
  if ! kill -0 "${watch_pid}" >/dev/null 2>&1; then
    echo "error: watch process exited before initial compile. log tail:" 1>&2
    tail -n 80 "${log}" 1>&2 || true
    exit 1
  fi
  sleep 0.1
done

if ! grep -q "HOTSWAP(ms):" "${log}"; then
  echo "error: timed out waiting for initial compile. log tail:" 1>&2
  tail -n 80 "${log}" 1>&2 || true
  exit 1
fi

initial_count="$(grep -c "HOTSWAP(ms):" "${log}" || true)"

printf "\n// measure_hotswap_latency %s\n" "$(date +%s%N)" >> "${file}"

deadline=$((SECONDS + 60))
while [[ "${SECONDS}" -lt "${deadline}" ]]; do
  count="$(grep -c "HOTSWAP(ms):" "${log}" || true)"
  if (( count > initial_count )); then break; fi
  if ! kill -0 "${watch_pid}" >/dev/null 2>&1; then
    echo "error: watch process exited before reporting HOTSWAP. log tail:" 1>&2
    tail -n 120 "${log}" 1>&2 || true
    exit 1
  fi
  sleep 0.1
done

line="$(grep "HOTSWAP(ms):" "${log}" | tail -n 1 || true)"
if [[ -z "${line}" ]]; then
  echo "error: timed out waiting for HOTSWAP(ms). log tail:" 1>&2
  tail -n 120 "${log}" 1>&2 || true
  exit 1
fi

echo "${line}"
