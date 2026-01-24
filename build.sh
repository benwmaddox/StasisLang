#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if command -v apt-get >/dev/null 2>&1; then
  if [[ "$(id -u)" -eq 0 ]]; then
    apt-get update
    apt-get install -y libsdl2-dev libsdl2-image-dev libglew-dev
  elif sudo -n true >/dev/null 2>&1; then
    sudo apt-get update
    sudo apt-get install -y libsdl2-dev libsdl2-image-dev libglew-dev
  else
    echo "warning: skipping apt-get install (no passwordless sudo). Install libsdl2-dev, libsdl2-image-dev, and libglew-dev manually if builds fail." 1>&2
  fi
fi

pushd "${script_dir}/tools/cranelift-aot" >/dev/null
cargo build -p stasis-cranelift-aot --release
popd >/dev/null

skip_runtime="${STASIS_SKIP_RUNTIME:-0}"
if [[ "${skip_runtime}" != "1" && -x "$(command -v pkg-config)" ]]; then
  missing_pkgs=()
  for pkg in sdl2 SDL2_image glew; do
    if ! pkg-config --exists "${pkg}"; then
      missing_pkgs+=("${pkg}")
    fi
  done
  if (( ${#missing_pkgs[@]} > 0 )); then
    echo "warning: skipping runtime build (missing pkg-config deps: ${missing_pkgs[*]})." 1>&2
    echo "         Install SDL2/SDL2_image/GLEW dev packages or set STASIS_SKIP_RUNTIME=0 after installing." 1>&2
    skip_runtime="1"
  fi
fi

stasis_graphics_sdl_only="${STASIS_GRAPHICS_SDL_ONLY:-0}"

if [[ "${skip_runtime}" == "1" ]]; then
  echo "note: STASIS_SKIP_RUNTIME=1; skipping runtime build." 1>&2
else
  cmake -S "${script_dir}/runtime" -B "${script_dir}/runtime/build" -DCMAKE_BUILD_TYPE=Release -DSTASIS_GRAPHICS_SDL_ONLY="${stasis_graphics_sdl_only}"
  cmake --build "${script_dir}/runtime/build" --config Release
fi

dotnet build "${script_dir}/Stasis.sln"

if [[ -d "${script_dir}/assets_src" ]]; then
  if find "${script_dir}/assets_src" -name "*.svg" -print -quit | grep -q .; then
    dotnet run --project "${script_dir}/Stasis.SvgValidator/Stasis.SvgValidator.csproj" -c Release -- --dir "${script_dir}/assets_src"
  fi
fi

runtime_id="linux-x64"
case "$(uname -s)" in
  Darwin)
    runtime_id="osx-x64"
    ;;
esac

aot_dir="${script_dir}/build/aot"
dotnet publish "${script_dir}/Stasis.Cli/Stasis.Cli.csproj" -c Release -r "${runtime_id}" -p:PublishAot=true -p:SelfContained=true -o "${aot_dir}"

lsp_dir="${script_dir}/vscode-stasis/server"
dotnet publish "${script_dir}/Stasis.LanguageServer/Stasis.LanguageServer.csproj" -c Release -r "${runtime_id}" -o "${lsp_dir}"
