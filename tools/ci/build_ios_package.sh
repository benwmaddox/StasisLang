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
lipo -verify_arch arm64 "${executable}"
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
  printf 'architectures=%s\n' "$(lipo -archs "${executable}")"
  printf 'asset_manifest=%s\n' "${app}/stasis_game/assets/manifest.json"
  printf 'provenance=%s\n' "${app}/stasis_game/stasis_provenance.json"
  printf 'stasis_sources=0\n'
} > "${build_root}/evidence.txt"

cat "${build_root}/evidence.txt"
