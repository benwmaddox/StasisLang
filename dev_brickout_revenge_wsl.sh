#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${script_dir}"

# Dev loop for Brickout Revenge (WSL/Linux).
# Usage: ./dev_brickout_revenge_wsl.sh [extra stasis args...]
#
# Notes:
# - For best hot-swap latency, run from the WSL filesystem (e.g. ~/src/StasisLang),
#   not from /mnt/c or /mnt/f.
# - Set STASIS_CRANELIFT_JIT_RUNNER=1 to use the diskless Cranelift JIT runner.

if [[ "${script_dir}" == /mnt/* ]]; then
  echo "warning: running from ${script_dir} (drvfs). For best watch/hot-swap latency, clone the repo into the WSL filesystem." 1>&2
fi

export STASIS_ASSET_ROOT="${script_dir}"
export STASIS_USE_SDL=1

: "${STASIS_CRANELIFT_JIT_RUNNER:=0}"
: "${STASIS_JIT_WATCHDOG_MS:=15000}"

exec ./stasis.sh run "samples/brickout_revenge/brickout_revenge.stasis" \
  --watch --backend cranelift --graphics --module brick --fps 60 \
  "$@"

