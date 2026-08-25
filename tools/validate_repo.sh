#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -f "$HOME/.cargo/env" ]]; then
  source "$HOME/.cargo/env"
fi

python3 tools/ci/check_stasis_src_layout.py
if command -v cc >/dev/null 2>&1; then
  mkdir -p target/audio-ring-test
  cc -std=c11 -Wall -Wextra -Werror -Iruntime \
    runtime/stasis_audio_ring.c runtime/tests/stasis_audio_ring_test.c \
    -o target/audio-ring-test/stasis_audio_ring_test
  target/audio-ring-test/stasis_audio_ring_test
fi
cargo test --workspace --all-targets -- --test-threads=1
