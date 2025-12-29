#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if command -v apt-get >/dev/null 2>&1; then
  sudo apt-get update
  sudo apt-get install -y libsdl2-dev libglew-dev
fi

export STASIS_SUPPRESS_WARNINGS=1
export SDL_VIDEODRIVER=dummy
export SDL_AUDIODRIVER=dummy
export STASIS_USE_SDL=1

dotnet test -- RunConfiguration.MaxCpuCount=1

cmake -S "${script_dir}/runtime" -B "${script_dir}/runtime/build" -DCMAKE_BUILD_TYPE=Release
cmake --build "${script_dir}/runtime/build" --config Release

graphics_library_path="${script_dir}/runtime/build/bin/libstasis_graphics.so"
if [[ ! -f "${graphics_library_path}" ]]; then
  echo "error: graphics runtime not found at ${graphics_library_path}"
  exit 1
fi

"${script_dir}/stasis.sh" test samples --all --backend llvm --graphics --graphics-lib "${graphics_library_path}"
