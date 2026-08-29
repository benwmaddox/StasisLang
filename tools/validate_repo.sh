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
python3 tools/ci/check_sdl3_migration.py
python3 tools/ci/check_deterministic_live_simulation_roadmap.py
python3 tools/ci/check_jit_generation_contract.py
python3 tools/ci/check_runtime_abi_contract.py
python3 tools/ci/check_unsafe_boundaries.py
python3 tools/ci/run_architecture_characterization.py --check
python3 -m unittest tools.ci.test_run_architecture_characterization
python3 tools/ci/run_architecture_characterization.py --run-fast
python3 -m unittest tools.ci.test_jit_generation_contract
python3 -m unittest tools.ci.test_deterministic_live_simulation_roadmap
python3 -m unittest tools.ci.test_runtime_abi_contract
python3 -m unittest tools.ci.test_cargo_cache
python3 -m unittest tools.ci.test_unsafe_boundaries
python3 -m unittest tools.ci.test_stasis_ai_efficiency_matrix
python3 -m unittest tools.ci.test_release_provenance
python3 -m unittest tools.ci.test_local_toolchain_install
python3 -m unittest tools.ci.test_sdl3_migration
python3 -m unittest tools.ci.test_windows_sign_runner
python3 -m unittest tools.ci.test_windows_signing_policy
python3 -m unittest tools.ci.test_verify_android_render_performance
python3 -m unittest tools.ci.test_verify_render_parity
python3 tools/ci/verify_render_parity.py
node --test runtime/web/tests/orientation_host_frame.test.mjs
node --test runtime/web/tests/viewport_fit.test.mjs
node --test runtime/web/tests/sys_memcpy_u8.test.mjs
node --test runtime/web/tests/sys_memcpy_typed.test.mjs
node --test runtime/web/tests/asset_paths.test.mjs
node --test runtime/web/tests/audio_suspended_queue.test.mjs

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

python3 tools/cargo_cache.py run -- cargo test --workspace --all-targets -- --test-threads=1
