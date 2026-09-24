#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "iOS package validation requires macOS with Xcode" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workspace="${1:-${repo_root}/samples/mobile_storage_link}"
workspace="$(cd "${workspace}" && pwd)"
package_output="${2:-dist/ios-ci}"
build_root="${STASIS_IOS_BUILD_ROOT:-${repo_root}/target/ios-package-link}"
framework_root="${build_root}/frameworks"
download_root="${build_root}/downloads"
derived_data="${build_root}/derived-data"
simulator_derived_data="${build_root}/simulator-derived-data"
simulator_udid=""

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
python tools/cargo_cache.py run -- cargo run -p stasis -- \
  --workspace "${workspace}" \
  package-mobile \
  --target ios-arm64 \
  --out "${package_output}" \
  --development-build
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
otool -L "${executable}" | tee "${build_root}/linked-libraries.txt"
grep -Fq '@rpath/SDL3.framework/SDL3' "${build_root}/linked-libraries.txt"
grep -Fq '@rpath/SDL3_image.framework/SDL3_image' "${build_root}/linked-libraries.txt"
stasis_source=""
while IFS= read -r candidate; do
  stasis_source="${candidate}"
  break
done < <(find "${app}" -type f -name '*.stasis' -print)
if [[ -n "${stasis_source}" ]]; then
  echo "generated iOS app contains Stasis source" >&2
  exit 1
fi

{
  xcodebuild -version
  printf 'app=%s\n' "${app}"
  printf 'architectures=%s\n' "$(lipo "${executable}" -archs)"
  printf 'supported_orientations=%s\n' "${orientations}"
  printf 'asset_manifest=%s\n' "${app}/stasis_game/assets/manifest.json"
  printf 'provenance=%s\n' "${app}/stasis_game/stasis_provenance.json"
  printf 'stasis_sources=0\n'
} > "${build_root}/evidence.txt"

cat "${build_root}/evidence.txt"

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
