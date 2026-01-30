#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
need_restore=0
assets_path="${SCRIPT_DIR}/Stasis.Cli/obj/project.assets.json"
if [[ ! -f "${assets_path}" ]]; then
  need_restore=1
else
  llvmsharp_version="$(
    grep -o 'PackageReference Include="LLVMSharp" Version="[^"]*"' "${SCRIPT_DIR}/Stasis.Compiler/Stasis.Compiler.csproj" \
      | sed 's/.*Version="\\([^"]*\\)".*/\\1/' \
      | head -n 1
  )"
  nuget_root="${NUGET_PACKAGES:-$HOME/.nuget/packages}"
  if [[ -n "${llvmsharp_version}" && ! -d "${nuget_root}/llvmsharp/${llvmsharp_version}" ]]; then
    need_restore=1
  fi
fi

if [[ "${need_restore}" == "1" ]]; then
  dotnet restore "${SCRIPT_DIR}/Stasis.sln"
fi

dotnet run --no-restore --project "${SCRIPT_DIR}/Stasis.Cli/Stasis.Cli.csproj" -- "$@"
