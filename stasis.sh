#!/usr/bin/env bash
set -euo pipefail

if [ $# -lt 2 ]; then
  echo "Usage: stasis run <file> [extra cli args...]" >&2
  echo "       stasis test <file> [extra cli args...] (adds --with-tests automatically)" >&2
  exit 1
fi

CMD="$1"
FILE="$2"
shift 2
EXTRA=("$@")
PROJ="Stasis.Cli/Stasis.Cli.csproj"
LLI="$(command -v lli || true)"

if [ -z "$LLI" ]; then
  echo "error: lli not found on PATH" >&2
  exit 1
fi

TMP="$(mktemp --suffix .ll)"
cleanup() { rm -f "$TMP"; }
trap cleanup EXIT

if [ "$CMD" = "run" ]; then
  dotnet run --project "$PROJ" -- "$FILE" "${EXTRA[@]}" > "$TMP"
  "$LLI" "$TMP"
elif [ "$CMD" = "test" ]; then
  dotnet run --project "$PROJ" -- "$FILE" --with-tests "${EXTRA[@]}" > "$TMP"
  "$LLI" -entry-function=run_tests "$TMP"
else
  echo "Unknown command: $CMD" >&2
  exit 1
fi
