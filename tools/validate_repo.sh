#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -f "$HOME/.cargo/env" ]]; then
  source "$HOME/.cargo/env"
fi

python3 tools/ci/check_stasis_src_layout.py
cmake -S runtime/tests -B target/mobile-runtime-tests -DCMAKE_BUILD_TYPE=Release
cmake --build target/mobile-runtime-tests --config Release
ctest --test-dir target/mobile-runtime-tests -C Release --output-on-failure
cargo test --workspace --all-targets -- --test-threads=1
