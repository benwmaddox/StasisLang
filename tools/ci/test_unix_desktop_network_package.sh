#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
export CMAKE_BUILD_PARALLEL_LEVEL="${CMAKE_BUILD_PARALLEL_LEVEL:-2}"
python3 tools/cargo_cache.py run -- cargo build -p stasis --bin stasis
target_dir="$(python3 tools/cargo_cache.py run -- cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
cli="$target_dir/debug/stasis"
workspace="$(mktemp -d "$PWD/target/desktop-network-package.XXXXXX")"
"$cli" new network_smoke --dir "$workspace/project"
python3 - "$workspace/project" <<'PYTHON'
import json
from pathlib import Path
import sys
root = Path(sys.argv[1])
manifest = root / "stasis.json"
if not manifest.exists():
    raise SystemExit("generated project manifest missing")
data = json.loads(manifest.read_text())
data["capabilities"] = {"network": True}
data["web"] = {"entry": "src/main.stasis"}
manifest.write_text(json.dumps(data) + "\n")
(root / "src/main.stasis").write_text(
    "function main(): i32 { return 0; }\n"
    "function tick(): i32 { return 0; }\n"
    "function render(): i32 { return 0; }\n")
PYTHON
"$cli" --workspace "$workspace/project" package --target desktop --development-build --out dist/network
package="$workspace/project/dist/network"
python3 - "$package" <<'PYTHON'
import argparse
from pathlib import Path
import sys
from tools.verify_package_provenance import verify_network_guest_bundles
root = Path(sys.argv[1])
assert (root / "network_guest.bundle").is_file()
verify_network_guest_bundles(argparse.ArgumentParser(), root)
PYTHON
case "$(uname -s)" in
  Linux)
    python3 tools/ci/test_linux_desktop_network_package.py \
      --executable "$package/network_smoke" --result "$workspace/result.json"
    ;;
  Darwin)
    test -x "$package/network_smoke.app/Contents/MacOS/network_smoke"
    /usr/libexec/PlistBuddy -c 'Print :NSLocalNetworkUsageDescription' \
      "$package/network_smoke.app/Contents/Info.plist"
    python3 tools/ci/test_linux_desktop_network_package.py \
      --executable "$package/network_smoke.app/Contents/MacOS/network_smoke" \
      --result "$workspace/result.json"
    ;;
  *) echo "unsupported native desktop package target" >&2; exit 1 ;;
esac
