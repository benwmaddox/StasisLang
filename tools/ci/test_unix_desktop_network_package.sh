#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
export CMAKE_BUILD_PARALLEL_LEVEL="${CMAKE_BUILD_PARALLEL_LEVEL:-2}"
python3 tools/cargo_cache.py run -- cargo build -p stasis --bin stasis
target_dir="$(python3 tools/cargo_cache.py run -- cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
cli="$target_dir/debug/stasis"
workspace="$(mktemp -d "$PWD/target/desktop-network-package.XXXXXX")"
case "$(uname -s)" in
  Darwin)
    export STASIS_AOT_SIGN_TOOL="$PWD/tools/ci/macos_ad_hoc_sign.sh"
    export STASIS_REQUIRE_SIGNED_EXECUTION=1
    export STASIS_SIGN_ORDER_LOG="$workspace/sign-order.log"
    : > "$STASIS_SIGN_ORDER_LOG"
    ;;
esac
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
data["libraries"] = {
    "selections": {"stasis.network": {"features": ["host"]}}
}
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
    executable="$package/network_smoke.app/Contents/MacOS/network_smoke"
    dylib="$package/network_smoke.app/Contents/Frameworks/libstasis_network.dylib"
    app="$package/network_smoke.app"
    header="$package/network_smoke.host_exports.h"
    test -f "$dylib"
    test -f "$header"
    unexpected_macos_artifact="$(find "$app/Contents/MacOS" -type f ! -path "$executable" -print -quit)"
    if [[ -n "$unexpected_macos_artifact" ]]; then
      echo "non-Mach-O developer artifact was staged beside the executable: $unexpected_macos_artifact" >&2
      exit 1
    fi
    otool -L "$executable" | grep -q 'libstasis_network.dylib'
    otool -D "$dylib" | grep -Fxq '@rpath/libstasis_network.dylib'
    otool -l "$executable" |
      grep -A2 LC_RPATH | grep -Fq '@executable_path/../Frameworks'
    codesign --verify --strict "$executable"
    codesign --verify --strict "$dylib"
    codesign --verify --strict "$app"
    python3 - "$STASIS_SIGN_ORDER_LOG" "$executable" "$dylib" "$app" <<'PYTHON'
from pathlib import Path
import sys
order = Path(sys.argv[1]).read_text().splitlines()
executable_suffix = Path(*Path(sys.argv[2]).parts[-4:])
dylib_suffix = Path(*Path(sys.argv[3]).parts[-4:])
app_suffix = Path(Path(sys.argv[4]).name)

def has_suffix(value, suffix):
    path_parts = Path(value).parts
    suffix_parts = suffix.parts
    return path_parts[-len(suffix_parts):] == suffix_parts

if not any(has_suffix(path, executable_suffix) for path in order[:-1]):
    raise SystemExit(f"production signer did not sign the Mach-O executable: {order}")
if any(Path(path).suffix == ".h" for path in order):
    raise SystemExit(f"production signer received a generated header: {order}")
if (
    len(order) < 2
    or not has_suffix(order[-2], dylib_suffix)
    or not has_suffix(order[-1], app_suffix)
):
    raise SystemExit(f"production nested signing order differs: {order}")
PYTHON
    /usr/libexec/PlistBuddy -c 'Print :NSLocalNetworkUsageDescription' \
      "$package/network_smoke.app/Contents/Info.plist"
    python3 tools/ci/test_linux_desktop_network_package.py \
      --executable "$executable" \
      --result "$workspace/result.json"
    ;;
  *) echo "unsupported native desktop package target" >&2; exit 1 ;;
esac
