#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "iOS package validation requires macOS with Xcode" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workspace="${1:-${repo_root}/samples/generics_collections}"
workspace="$(cd "${workspace}" && pwd)"
package_output="${2:-dist/ios-ci}"
build_root="${STASIS_IOS_BUILD_ROOT:-${repo_root}/target/ios-package-link}"
framework_root="${build_root}/frameworks"
download_root="${build_root}/downloads"
derived_data="${build_root}/derived-data"
simulator_derived_data="${build_root}/simulator-derived-data"
simulator_udid=""
simulator_acceptance="${STASIS_IOS_SIMULATOR_ACCEPTANCE:-aspect-fit}"
simulator_output=""
simulator_package_created=0

package_mobile() {
  local target="$1"
  local output="$2"
  if [[ -n "${STASIS_CLI_EXECUTABLE:-}" ]]; then
    [[ -x "${STASIS_CLI_EXECUTABLE}" ]] || {
      echo "configured staged archive CLI is not executable: ${STASIS_CLI_EXECUTABLE}" >&2
      return 1
    }
    local development_flag=()
    if [[ "${target}" == "ios-simulator-arm64" ]]; then
      development_flag=(--development-build)
    fi
    "${STASIS_CLI_EXECUTABLE}" --workspace "${workspace}" package-mobile \
      --target "${target}" --out "${output}" "${development_flag[@]}"
  else
    python tools/cargo_cache.py run -- cargo run -p stasis -- \
      --workspace "${workspace}" \
      package-mobile \
      --target "${target}" \
      --out "${output}" \
      --development-build
  fi
}

if [[ "${package_output}" = /* || "${package_output}" = *..* ]]; then
  echo "package output must be a confined workspace-relative path" >&2
  exit 1
fi
if [[ -e "${workspace}/${package_output}" ]]; then
  echo "mobile package output already exists: ${workspace}/${package_output}" >&2
  exit 1
fi

mkdir -p "${framework_root}" "${download_root}"
active_mount=""
package_created=0
cleanup() {
  local status=$?
  if [[ -n "${simulator_udid}" ]]; then
    xcrun simctl shutdown "${simulator_udid}" >/dev/null 2>&1 || true
    xcrun simctl delete "${simulator_udid}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${active_mount}" ]]; then
    hdiutil detach "${active_mount}" >/dev/null 2>&1 || true
  fi
  if [[ ${status} -ne 0 && ${package_created} -eq 1 ]]; then
    rm -rf -- "${workspace:?}/${package_output:?}"
  fi
  if [[ ${status} -ne 0 && ${simulator_package_created} -eq 1 ]]; then
    rm -rf -- "${workspace:?}/${simulator_output:?}"
  fi
  trap - EXIT
  exit "${status}"
}
trap cleanup EXIT

install_xcframework() {
  local name="$1"
  local version="$2"
  local archive_name="$3"
  local digest="$4"
  local repository="$5"
  local archive="${download_root}/${archive_name}"
  local mount_point="${build_root}/mount-${name}"
  local framework

  curl --fail --location --retry 3 --output "${archive}" \
    "https://github.com/libsdl-org/${repository}/releases/download/release-${version}/${archive_name}"
  printf '%s  %s\n' "${digest}" "${archive}" | shasum -a 256 --check
  mkdir -p "${mount_point}"
  hdiutil attach "${archive}" -readonly -nobrowse -mountpoint "${mount_point}" >/dev/null
  active_mount="${mount_point}"
  framework=""
  while IFS= read -r candidate; do
    framework="${candidate}"
    break
  done < <(find "${mount_point}" -type d -name "${name}.xcframework" -print)
  if [[ -z "${framework}" ]]; then
    echo "${name}.xcframework was not present in ${archive}" >&2
    exit 1
  fi
  ditto "${framework}" "${framework_root}/${name}.xcframework"
  hdiutil detach "${mount_point}" >/dev/null
  active_mount=""
}

verify_ios_generics_symbols() {
  local engine_manifest="$1"
  local symbols="$2"
  python3 - "${engine_manifest}" "${symbols}" <<'PY'
import json
import re
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    manifest = json.load(source)
with open(sys.argv[2], encoding="utf-8") as source:
    symbols = source.read()

functions = manifest.get("functions")
if not isinstance(functions, list):
    raise SystemExit("engine manifest is missing its full function table")
by_name = {
    function.get("name"): function.get("symbol")
    for function in functions
    if isinstance(function, dict)
}
for name in ("main", "tick", "render"):
    symbol = by_name.get(name)
    if not isinstance(symbol, str) or not symbol:
        raise SystemExit(f"engine manifest is missing lifecycle symbol {name}")
    if re.search(rf"^[0-9a-f]+ T _{re.escape(symbol)}$", symbols, re.MULTILINE) is None:
        raise SystemExit(f"linked app is missing lifecycle AOT symbol {name}={symbol}")

required = (
    "stasis_replay_state_snapshot_restore",
    "stasis_replay_state_snapshot_size",
    "stasis_replay_state_snapshot_write",
    "stasis_state_scalar__generics_collections_digest_value",
    "stasis_state_scalar__web_bounds_probe_index",
    "stasis_state_array__gfx_cmd_i32",
    "stasis_state_array__gfx_cmd_f32",
    "stasis_state_array__gfx_cmd_u8",
)
for symbol in required:
    if re.search(rf"^[0-9a-f]+ [A-Z] _{re.escape(symbol)}$", symbols, re.MULTILINE) is None:
        raise SystemExit(f"linked app is missing required workload symbol {symbol}")

linked_aot_functions = re.findall(r"^[0-9a-f]+ T _aot_fn_[0-9]+$", symbols, re.MULTILINE)
if len(linked_aot_functions) < 16:
    raise SystemExit(
        f"linked app contains only {len(linked_aot_functions)} AOT functions; expected the full workload"
    )
PY
}

install_xcframework \
  SDL3 \
  3.4.10 \
  SDL3-3.4.10.dmg \
  36f78737dcd13a6e47ee066a6e460501a3de7fca678fe97fc3deab7d5ebc8b0f \
  SDL
install_xcframework \
  SDL3_image \
  3.4.4 \
  SDL3_image-3.4.4.dmg \
  7481d597f90be0d92546a0189008c14a1e6d7b86eaa56beace2ed9f631d85282 \
  SDL_image

cd "${repo_root}"
package_mobile ios-arm64 "${package_output}"
package_created=1

package_root="${workspace}/${package_output}"
project_root="${package_root}/ios"
mkdir -p "${build_root}"
xcodebuild -version | tee "${build_root}/xcode-version.txt"
set -o pipefail
xcodebuild \
  -project "${project_root}/StasisMobile.xcodeproj" \
  -scheme StasisMobile \
  -configuration Debug \
  -sdk iphoneos \
  -arch arm64 \
  -derivedDataPath "${derived_data}" \
  STASIS_SDL_FRAMEWORKS="${framework_root}" \
  CODE_SIGNING_ALLOWED=NO \
  CODE_SIGNING_REQUIRED=NO \
  build | tee "${build_root}/xcodebuild.log"

app="${derived_data}/Build/Products/Debug-iphoneos/StasisMobile.app"
executable="${app}/StasisMobile"
test -f "${executable}"
test -d "${app}/Frameworks/SDL3.framework"
test -d "${app}/Frameworks/SDL3_image.framework"
test -f "${app}/stasis_game/assets/manifest.json"
test -f "${app}/stasis_game/stasis_provenance.json"
orientations="$(python3 - "${app}/Info.plist" <<'PY'
import plistlib
import sys

with open(sys.argv[1], "rb") as source:
    orientations = plistlib.load(source)["UISupportedInterfaceOrientations"]
expected = ["UIInterfaceOrientationLandscapeLeft", "UIInterfaceOrientationLandscapeRight"]
if orientations != expected:
    raise SystemExit(f"unsupported iOS orientation contract: {orientations!r}")
print(",".join(orientations))
PY
)"
lipo "${executable}" -verify_arch arm64
vtool -show-build "${executable}" | tee "${build_root}/device-platform.txt"
grep -Fq 'platform IOS' "${build_root}/device-platform.txt"
otool -L "${executable}" | tee "${build_root}/linked-libraries.txt"
grep -Fq '@rpath/SDL3.framework/SDL3' "${build_root}/linked-libraries.txt"
grep -Fq '@rpath/SDL3_image.framework/SDL3_image' "${build_root}/linked-libraries.txt"
nm -gU "${executable}" | tee "${build_root}/device-symbols.txt"
verify_ios_generics_symbols \
  "${package_root}/aot/engine_bundle_manifest.json" \
  "${build_root}/device-symbols.txt"
stasis_source=""
while IFS= read -r candidate; do
  stasis_source="${candidate}"
  break
done < <(find "${app}" -type f -name '*.stasis' -print)
if [[ -n "${stasis_source}" ]]; then
  echo "generated iOS app contains Stasis source" >&2
  exit 1
fi
compiler_payload=""
while IFS= read -r candidate; do
  compiler_payload="${candidate}"
  break
done < <(find "${app}" -type f \( -name 'stasis' -o -name 'stasis.exe' -o -name 'stasis_compiler*' \) -print)
if [[ -n "${compiler_payload}" ]]; then
  echo "generated iOS app contains compiler payload ${compiler_payload}" >&2
  exit 1
fi
cmp "${package_root}/stasis_provenance.json" "${app}/stasis_game/stasis_provenance.json"
shasum -a 256 \
  "${workspace}/src/main.stasis" \
  "${package_root}/stasis_mobile_package.json" \
  "${package_root}/aot/mobile_aot_bundle_manifest.json" \
  "${app}/stasis_game/assets/manifest.json" \
  "${app}/stasis_game/stasis_provenance.json" \
  "${executable}" \
  "${app}/Frameworks/SDL3.framework/SDL3" \
  "${app}/Frameworks/SDL3_image.framework/SDL3_image" \
  | tee "${build_root}/device-hashes.txt"

{
  xcodebuild -version
  printf 'app=%s\n' "${app}"
  printf 'architectures=%s\n' "$(lipo "${executable}" -archs)"
  printf 'supported_orientations=%s\n' "${orientations}"
  printf 'asset_manifest=%s\n' "${app}/stasis_game/assets/manifest.json"
  printf 'provenance=%s\n' "${app}/stasis_game/stasis_provenance.json"
  printf 'stasis_sources=0\n'
  printf 'compiler_payloads=0\n'
  printf 'physical_device_qualified=false\n'
} > "${build_root}/evidence.txt"

cat "${build_root}/evidence.txt"

if [[ "${simulator_acceptance}" = "generics" ]]; then
  simulator_output="${package_output}-simulator"
  cd "${repo_root}"
  package_mobile ios-simulator-arm64 "${simulator_output}"
  simulator_package_created=1
  simulator_package="${workspace}/${simulator_output}"
  simulator_project="${simulator_package}/ios"
python3 - "${simulator_package}/stasis_mobile_package.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    manifest = json.load(source)
if manifest.get("target") != "ios-simulator-arm64":
    raise SystemExit(f"unexpected simulator package target: {manifest.get('target')!r}")
expected_development = True
if manifest.get("development_build") is not expected_development:
    raise SystemExit(
        f"simulator package development_build={manifest.get('development_build')!r}; "
        f"expected {expected_development!r} for the selected compiler"
    )
PY
  xcodebuild \
    -project "${simulator_project}/StasisMobile.xcodeproj" \
    -scheme StasisMobile \
    -configuration Debug \
    -sdk iphonesimulator \
    -arch arm64 \
    -derivedDataPath "${simulator_derived_data}" \
    STASIS_SDL_FRAMEWORKS="${framework_root}" \
    'GCC_PREPROCESSOR_DEFINITIONS=$(inherited) TVG_STATIC=1 NOMINMAX=1 STASIS_ENABLE_SEAM_TESTS=1' \
    CODE_SIGNING_ALLOWED=NO \
    CODE_SIGNING_REQUIRED=NO \
    build | tee "${build_root}/simulator-xcodebuild.log"

  simulator_app="${simulator_derived_data}/Build/Products/Debug-iphonesimulator/StasisMobile.app"
  simulator_executable="${simulator_app}/StasisMobile"
  test -f "${simulator_executable}"
  lipo "${simulator_executable}" -verify_arch arm64
  vtool -show-build "${simulator_executable}" | tee "${build_root}/simulator-platform.txt"
  grep -Fq 'platform IOSSIMULATOR' "${build_root}/simulator-platform.txt"
  otool -L "${simulator_executable}" | tee "${build_root}/simulator-linked-libraries.txt"
  grep -Fq '@rpath/SDL3.framework/SDL3' "${build_root}/simulator-linked-libraries.txt"
  grep -Fq '@rpath/SDL3_image.framework/SDL3_image' "${build_root}/simulator-linked-libraries.txt"
  nm -gU "${simulator_executable}" | tee "${build_root}/simulator-symbols.txt"
  verify_ios_generics_symbols \
    "${simulator_package}/aot/engine_bundle_manifest.json" \
    "${build_root}/simulator-symbols.txt"
  cmp "${simulator_package}/stasis_provenance.json" \
    "${simulator_app}/stasis_game/stasis_provenance.json"
  shasum -a 256 \
    "${workspace}/src/main.stasis" \
    "${simulator_package}/stasis_mobile_package.json" \
    "${simulator_package}/aot/mobile_aot_bundle_manifest.json" \
    "${simulator_app}/stasis_game/assets/manifest.json" \
    "${simulator_app}/stasis_game/stasis_provenance.json" \
    "${simulator_executable}" \
    "${simulator_app}/Frameworks/SDL3.framework/SDL3" \
    "${simulator_app}/Frameworks/SDL3_image.framework/SDL3_image" \
    | tee "${build_root}/simulator-hashes.txt"
  for framework in SDL3 SDL3_image; do
    codesign --force --sign - "${simulator_app}/Frameworks/${framework}.framework"
  done
  codesign --force --deep --sign - "${simulator_app}"

  runtime_id="$(python3 - <<'PY'
import json
import subprocess

payload = json.loads(subprocess.check_output(
    ["xcrun", "simctl", "list", "runtimes", "--json"], text=True))
candidates = [runtime for runtime in payload["runtimes"]
              if runtime.get("isAvailable") and runtime.get("platform") == "iOS"]
if not candidates:
    raise SystemExit("no available iOS simulator runtime")
candidates.sort(key=lambda runtime: tuple(
    int(part) for part in runtime.get("version", "0").split(".")), reverse=True)
print(candidates[0]["identifier"])
PY
)"
  device_type_id="$(python3 - <<'PY'
import json
import subprocess

payload = json.loads(subprocess.check_output(
    ["xcrun", "simctl", "list", "devicetypes", "--json"], text=True))
preferred = ["iPhone 16 Pro", "iPhone 15 Pro", "iPhone 14 Pro"]
by_name = {device["name"]: device["identifier"] for device in payload["devicetypes"]}
for name in preferred:
    if name in by_name:
        print(by_name[name])
        break
else:
    raise SystemExit("no supported iPhone simulator type")
PY
)"
  simulator_name="StasisGenerics-${GITHUB_RUN_ID:-local}-$$"
  simulator_udid="$(xcrun simctl create "${simulator_name}" "${device_type_id}" "${runtime_id}")"
  xcrun simctl boot "${simulator_udid}"
  xcrun simctl bootstatus "${simulator_udid}" -b
  xcrun simctl install "${simulator_udid}" "${simulator_app}"
  bundle_id="$(/usr/libexec/PlistBuddy -c 'Print:CFBundleIdentifier' "${simulator_app}/Info.plist")"
  data_container="$(xcrun simctl get_app_container "${simulator_udid}" "${bundle_id}" data)"
  result_receipt="${data_container}/Documents/stasis-ios-generics-result.json"
  launch_output="$(
    SIMCTL_CHILD_STASIS_ENABLE_TEST_INPUT=1 \
    SIMCTL_CHILD_STASIS_SEAM_TEST_ID=IOS-GENERICS \
      xcrun simctl launch --terminate-running-process "${simulator_udid}" "${bundle_id}"
  )"
  printf '%s\n' "${launch_output}" | tee "${build_root}/simulator-launch.txt"
  wait_for_receipt() {
    local path="$1"
    for _ in $(seq 1 160); do
      if [[ -s "${path}" ]]; then return 0; fi
      sleep 0.25
    done
    echo "timed out waiting for simulator receipt: ${path}" >&2
    return 1
  }
  wait_for_receipt "${result_receipt}"
  cp "${result_receipt}" "${build_root}/simulator-result.json"
  sleep 1
  xcrun simctl io "${simulator_udid}" screenshot "${build_root}/simulator-frame.png"
  for _ in $(seq 1 40); do
    xcrun simctl spawn "${simulator_udid}" log show --style compact --last 5m \
      --predicate 'process == "StasisMobile"' > "${build_root}/simulator.log"
    if grep -Eq 'Stasis provenance: .* renderer=gfx_cmd schema=7' \
        "${build_root}/simulator.log"; then
      break
    fi
    sleep 0.25
  done
  if ! grep -Eq 'Stasis provenance: .* renderer=gfx_cmd schema=7' \
      "${build_root}/simulator.log"; then
    echo "simulator unified log did not publish the package provenance marker" >&2
    exit 1
  fi
  xcrun simctl terminate "${simulator_udid}" "${bundle_id}"

  run_bounds_probe() {
    local label="$1"
    local index="$2"
    local marker="${build_root}/bounds-${label}.marker"
    local launch_output=""
    local launch_pid=""
    local crash_report=""
    local crash_artifact=""
    local crash_sha256=""
    local crash_exception=""
    local crash_signal=""
    rm -f -- "${result_receipt}"
    touch "${marker}"
    launch_output="$(
      SIMCTL_CHILD_STASIS_ENABLE_TEST_INPUT=1 \
      SIMCTL_CHILD_STASIS_SEAM_TEST_ID=IOS-GENERICS \
      SIMCTL_CHILD_STASIS_IOS_GENERICS_BOUNDS_INDEX="${index}" \
        xcrun simctl launch --terminate-running-process "${simulator_udid}" "${bundle_id}"
    )"
    printf '%s\n' "${launch_output}" | tee "${build_root}/bounds-${label}-launch.txt"
    launch_pid="$(printf '%s\n' "${launch_output}" \
      | sed -n 's/.*: \([0-9][0-9]*\)$/\1/p' | tail -n 1)"
    if [[ -z "${launch_pid}" ]]; then
      echo "bounds probe ${label} launch did not report a process id" >&2
      exit 1
    fi
    for _ in $(seq 1 120); do
      while IFS= read -r candidate; do
        if grep -Fq 'StasisMobile' "${candidate}" && \
            grep -Eq "\"pid\"[[:space:]]*:[[:space:]]*${launch_pid}([^0-9]|$)|Process:[[:space:]]+StasisMobile[[:space:]]+\\[${launch_pid}\\]" \
              "${candidate}"; then
          crash_report="${candidate}"
          break
        fi
      done < <(find "${HOME}/Library/Logs/DiagnosticReports" -type f \
        \( -name 'StasisMobile*.ips' -o -name 'StasisMobile*.crash' \) \
        -newer "${marker}" -print 2>/dev/null)
      if [[ -n "${crash_report}" ]]; then break; fi
      sleep 0.25
    done
    if [[ -z "${crash_report}" ]]; then
      echo "bounds probe ${label} did not produce a crash report" >&2
      exit 1
    fi
    crash_artifact="${build_root}/bounds-${label}-crash.${crash_report##*.}"
    cp "${crash_report}" "${crash_artifact}"
    crash_sha256="$(shasum -a 256 "${crash_artifact}" | awk '{print $1}')"
    if grep -Eq 'EXC_BREAKPOINT' "${crash_report}" && \
        grep -Eq 'SIGTRAP|Trace/BPT trap' "${crash_report}"; then
      crash_exception="EXC_BREAKPOINT"
      crash_signal="SIGTRAP"
    elif grep -Eq 'EXC_BAD_INSTRUCTION' "${crash_report}" && \
        grep -Eq 'SIGILL|Illegal instruction' "${crash_report}"; then
      crash_exception="EXC_BAD_INSTRUCTION"
      crash_signal="SIGILL"
    else
      echo "bounds probe ${label} did not terminate through a recognized fatal trap" >&2
      exit 1
    fi
    if [[ -e "${result_receipt}" ]]; then
      echo "bounds probe ${label} unexpectedly completed a frame" >&2
      exit 1
    fi
    python3 - "${build_root}/bounds-${label}.json" "${label}" "${index}" \
      "${launch_pid}" "${crash_exception}" "${crash_signal}" \
      "${crash_artifact}" "${crash_sha256}" <<'PY'
import json
import sys

with open(sys.argv[1], "w", encoding="utf-8") as output:
    json.dump({
        "schema": "stasis.ios.generics.bounds.v1",
        "label": sys.argv[2],
        "index": int(sys.argv[3]),
        "process": "StasisMobile",
        "pid": int(sys.argv[4]),
        "fatal": True,
        "signal": sys.argv[6],
        "exception": sys.argv[5],
        "crash_report": sys.argv[7],
        "crash_report_sha256": sys.argv[8],
    }, output, indent=2)
    output.write("\n")
PY
  }
  run_bounds_probe low -1
  run_bounds_probe high 2

  python3 "${repo_root}/tools/ci/verify_ios_generics.py" \
    --receipt "${build_root}/simulator-result.json" \
    --frame "${build_root}/simulator-frame.png" \
    --log "${build_root}/simulator.log" \
    --bounds-low "${build_root}/bounds-low.json" \
    --bounds-high "${build_root}/bounds-high.json" \
    --output "${build_root}/simulator-evidence.json"
  {
    printf 'simulator_name=%s\n' "${simulator_name}"
    printf 'simulator_udid=%s\n' "${simulator_udid}"
    printf 'runtime=%s\n' "${runtime_id}"
    printf 'device_type=%s\n' "${device_type_id}"
    printf 'bundle_id=%s\n' "${bundle_id}"
    printf 'app=%s\n' "${simulator_app}"
    printf 'architectures=%s\n' "$(lipo "${simulator_executable}" -archs)"
    printf 'digest=507\n'
    printf 'physical_device_qualified=false\n'
  } > "${build_root}/simulator-evidence.txt"
  cat "${build_root}/simulator-evidence.txt"
  exit 0
fi

simulator_package="${build_root}/simulator-package"
ditto "${package_root}" "${simulator_package}"
simulator_project="${simulator_package}/ios"
cp "${repo_root}/tools/ci/ios_aspect_fit_bindings.c" "${simulator_package}/aot/published_aot_bindings.c"
printf '%s\n' '/* Intentionally empty simulator replacement object. */' > "${build_root}/simulator_placeholder.c"
while IFS= read -r object; do
  xcrun --sdk iphonesimulator clang -target arm64-apple-ios15.0-simulator -c "${build_root}/simulator_placeholder.c" -o "${object}"
done < <(find "${simulator_package}/aot" -type f -name '*.o' -print)

xcodebuild -project "${simulator_project}/StasisMobile.xcodeproj" -scheme StasisMobile -configuration Debug -sdk iphonesimulator -derivedDataPath "${simulator_derived_data}" STASIS_SDL_FRAMEWORKS="${framework_root}" STASIS_SDL_PLATFORM=ios-arm64_x86_64-simulator SDKROOT=iphonesimulator SUPPORTED_PLATFORMS=iphonesimulator CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO build | tee "${build_root}/simulator-xcodebuild.log"

simulator_app="${simulator_derived_data}/Build/Products/Debug-iphonesimulator/StasisMobile.app"
simulator_executable="${simulator_app}/StasisMobile"
test -f "${simulator_executable}"
lipo "${simulator_executable}" -verify_arch arm64
for framework in SDL3 SDL3_image; do
  codesign --force --sign - "${simulator_app}/Frameworks/${framework}.framework"
done
codesign --force --deep --sign - "${simulator_app}"

runtime_id="$(python3 - <<'PY'
import json
import subprocess

payload = json.loads(subprocess.check_output(
    ["xcrun", "simctl", "list", "runtimes", "--json"], text=True))
candidates = [
    runtime for runtime in payload["runtimes"]
    if runtime.get("isAvailable") and runtime.get("platform") == "iOS"
]
if not candidates:
    raise SystemExit("no available iOS simulator runtime")
candidates.sort(
    key=lambda runtime: tuple(
        int(part) for part in runtime.get("version", "0").split(".")
    ),
    reverse=True,
)
print(candidates[0]["identifier"])
PY
)"
device_type_id="$(python3 - <<'PY'
import json
import subprocess

payload = json.loads(subprocess.check_output(
    ["xcrun", "simctl", "list", "devicetypes", "--json"], text=True))
preferred = ["iPhone 16 Pro", "iPhone 15 Pro", "iPhone 14 Pro"]
by_name = {device["name"]: device["identifier"] for device in payload["devicetypes"]}
for name in preferred:
    if name in by_name:
        print(by_name[name])
        break
else:
    raise SystemExit("no supported cutout iPhone simulator type")
PY
)"
simulator_name="StasisAspectFit-${GITHUB_RUN_ID:-local}-$$"
simulator_udid="$(xcrun simctl create "${simulator_name}" "${device_type_id}" "${runtime_id}")"
xcrun simctl boot "${simulator_udid}"
xcrun simctl bootstatus "${simulator_udid}" -b
xcrun simctl install "${simulator_udid}" "${simulator_app}"
bundle_id="$(/usr/libexec/PlistBuddy -c 'Print:CFBundleIdentifier' "${simulator_app}/Info.plist")"
data_container="$(xcrun simctl get_app_container "${simulator_udid}" "${bundle_id}" data)"
SIMCTL_CHILD_STASIS_ENABLE_TEST_INPUT=1 xcrun simctl launch --terminate-running-process "${simulator_udid}" "${bundle_id}" | tee "${build_root}/simulator-launch.txt"

wait_for_receipt() {
  local path="$1"
  for _ in $(seq 1 160); do
    if [[ -s "${path}" ]]; then
      return 0
    fi
    sleep 0.25
  done
  echo "timed out waiting for simulator receipt: ${path}" >&2
  return 1
}

actual_receipt="${data_container}/Documents/stasis-ios-aspect-fit-actual.json"
left_receipt="${data_container}/Documents/stasis-ios-aspect-fit-landscape-left.json"
pointer_receipt="${data_container}/Documents/stasis-ios-aspect-fit-pointer.json"
right_receipt="${data_container}/Documents/stasis-ios-aspect-fit-landscape-right.json"
wait_for_receipt "${actual_receipt}"
wait_for_receipt "${left_receipt}"
cp "${actual_receipt}" "${build_root}/simulator-actual.json"
cp "${left_receipt}" "${build_root}/simulator-landscape-left.json"
xcrun simctl io "${simulator_udid}" screenshot "${build_root}/simulator-landscape-left.png"
touch "${data_container}/Documents/stasis-ios-aspect-fit-left-captured"
wait_for_receipt "${pointer_receipt}"
wait_for_receipt "${right_receipt}"
cp "${pointer_receipt}" "${build_root}/simulator-pointer.json"
cp "${right_receipt}" "${build_root}/simulator-landscape-right.json"
xcrun simctl io "${simulator_udid}" screenshot "${build_root}/simulator-landscape-right.png"
xcrun simctl spawn "${simulator_udid}" log show --style compact --last 5m --predicate 'process == "StasisMobile"' > "${build_root}/simulator.log"

python3 "${repo_root}/tools/ci/verify_ios_aspect_fit.py" --actual "${build_root}/simulator-actual.json" --left "${build_root}/simulator-landscape-left.json" --pointer "${build_root}/simulator-pointer.json" --right "${build_root}/simulator-landscape-right.json" --left-screenshot "${build_root}/simulator-landscape-left.png" --right-screenshot "${build_root}/simulator-landscape-right.png" --output "${build_root}/simulator-evidence.json"

{
  printf 'simulator_name=%s\n' "${simulator_name}"
  printf 'simulator_udid=%s\n' "${simulator_udid}"
  printf 'runtime=%s\n' "${runtime_id}"
  printf 'device_type=%s\n' "${device_type_id}"
  printf 'bundle_id=%s\n' "${bundle_id}"
  printf 'app=%s\n' "${simulator_app}"
  printf 'architectures=%s\n' "$(lipo "${simulator_executable}" -archs)"
  printf 'logical=1600x720\n'
  printf 'physical_device_qualified=false\n'
} > "${build_root}/simulator-evidence.txt"
cat "${build_root}/simulator-evidence.txt"
