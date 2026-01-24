#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dotnet run --no-restore --project "${SCRIPT_DIR}/Stasis.Cli/Stasis.Cli.csproj" -- "$@"
