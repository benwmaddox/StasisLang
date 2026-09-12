#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
python3 tools/cargo_cache.py run -- cargo build -p stasis_network --release
target_dir="$(python3 tools/cargo_cache.py run -- cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
probe_dir=target/desktop-network-link
mkdir -p "$probe_dir"
case "$(uname -s)" in
  Linux) libraries=(-ldl -lpthread -lm) ;;
  Darwin) libraries=(-framework Security -framework CoreFoundation -lresolv) ;;
  *) echo "unsupported native desktop link target" >&2; exit 1 ;;
esac
for name in stasis_network_link_test stasis_network_client_link_test; do
  cc -std=c11 -D_POSIX_C_SOURCE=200809L -Wall -Wextra -Werror \
    -I crates/stasis_network/include "runtime/tests/${name}.c" \
    "$target_dir/release/libstasis_network.a" "${libraries[@]}" -o "$probe_dir/$name"
  python3 - "$probe_dir/$name" <<'PYTHON'
import subprocess
import sys
subprocess.run([sys.argv[1]], check=True, timeout=60)
PYTHON
done
