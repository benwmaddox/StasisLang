#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

pushd "${script_dir}/tools/cranelift-aot" >/dev/null
cargo build -p stasis-cranelift-aot --release
popd >/dev/null

cmake -S "${script_dir}/runtime" -B "${script_dir}/runtime/build" -DCMAKE_BUILD_TYPE=Release
cmake --build "${script_dir}/runtime/build" --config Release

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
