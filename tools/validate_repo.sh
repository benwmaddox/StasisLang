#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -f "$HOME/.cargo/env" ]]; then
  source "$HOME/.cargo/env"
fi

python3 tools/ci/check_stasis_src_layout.py
python3 -m unittest tools.ci.test_stasis_ai_efficiency_matrix
python3 -m unittest tools.ci.test_release_provenance
python3 -m unittest tools.ci.test_verify_render_parity
python3 tools/ci/verify_render_parity.py
cargo test --workspace --all-targets -- --test-threads=1
