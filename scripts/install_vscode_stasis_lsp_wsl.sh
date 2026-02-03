#!/usr/bin/env bash
set -euo pipefail

force=0
skip_install=0
configuration="Release"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --force|-f)
      force=1
      shift
      ;;
    --skip-install)
      skip_install=1
      shift
      ;;
    --configuration|-c)
      configuration="${2:-}"
      shift 2
      ;;
    *)
      echo "Usage: $0 [--force] [--skip-install] [--configuration <Debug|Release>]" >&2
      exit 2
      ;;
  esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
server_project="$repo_root/Stasis.LanguageServer/Stasis.LanguageServer.csproj"
extension_dir="$repo_root/vscode-stasis"
server_out="$extension_dir/server"
vsix_dir="$extension_dir/.vsix"
vsix_path="$vsix_dir/stasislang.stasis.vsix"

command -v dotnet >/dev/null || { echo "error: dotnet not found in PATH" >&2; exit 1; }
command -v npm >/dev/null || { echo "error: npm not found in PATH" >&2; exit 1; }
command -v npx >/dev/null || { echo "error: npx not found in PATH" >&2; exit 1; }
command -v code >/dev/null || { echo "error: code (VS Code CLI) not found in PATH" >&2; exit 1; }

mkdir -p "$server_out"
dotnet publish "$server_project" -c "$configuration" -o "$server_out" \
  -p:StasisIncludeLibLLVM=false \
  -p:SelfContained=false \
  -p:PublishSingleFile=false \
  -p:PublishReadyToRun=false \
  -p:UseAppHost=false

pushd "$extension_dir" >/dev/null
if [[ ! -d node_modules ]]; then
  npm install
fi

npm run build

mkdir -p "$vsix_dir"
npx @vscode/vsce package --out "$vsix_path"

if [[ $skip_install -eq 1 ]]; then
  echo "Built VSIX: $vsix_path"
  popd >/dev/null
  exit 0
fi

install_args=(--install-extension "$vsix_path")
if [[ $force -eq 1 ]]; then
  install_args+=(--force)
fi
code "${install_args[@]}"
echo "Installed VSIX: $vsix_path"

popd >/dev/null

