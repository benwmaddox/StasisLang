#!/usr/bin/env bash
set -euo pipefail
if [[ "$#" -ne 1 || -z "${STASIS_SIGN_ORDER_LOG:-}" ]]; then
  echo "usage: STASIS_SIGN_ORDER_LOG=<path> macos_ad_hoc_sign.sh <artifact>" >&2
  exit 2
fi
printf '%s\n' "$1" >> "${STASIS_SIGN_ORDER_LOG}"
exec /usr/bin/codesign --force --sign - "$1"
