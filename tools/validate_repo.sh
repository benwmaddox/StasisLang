#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -f "$HOME/.cargo/env" ]]; then
  source "$HOME/.cargo/env"
fi

python3 tools/ci/check_stasis_src_layout.py
python3 tools/ci/check_sdl3_migration.py
python3 tools/ci/check_jit_generation_contract.py
python3 tools/ci/check_unsafe_boundaries.py
python3 -m unittest tools.ci.test_jit_generation_contract
python3 -m unittest tools.ci.test_unsafe_boundaries
python3 -m unittest tools.ci.test_stasis_ai_efficiency_matrix
python3 -m unittest tools.ci.test_release_provenance
python3 -m unittest tools.ci.test_sdl3_migration
python3 -m unittest tools.ci.test_verify_render_parity
python3 tools/ci/verify_render_parity.py

set +e
ignored_tests="$(rg -n -U --pcre2 '#\s*\[\s*(?:ignore\b|cfg_attr\s*\([^\]]*\bignore\b)' apps crates mobile tests -g '*.rs' 2>&1)"
ignored_status=$?
set -e
if [[ $ignored_status -eq 0 ]]; then
  printf 'Rust tests must run by default; move external smoke checks to examples:\n%s\n' "$ignored_tests" >&2
  exit 1
elif [[ $ignored_status -ne 1 ]]; then
  printf 'Failed to audit Rust ignore attributes:\n%s\n' "$ignored_tests" >&2
  exit "$ignored_status"
fi

cargo test --workspace --all-targets -- --test-threads=1
