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
CLANG="$(command -v clang || true)"

if [ -z "$LLI" ] && [ -z "$CLANG" ]; then
  echo "error: neither lli nor clang found on PATH" >&2
  exit 1
fi

TMP="$(mktemp --suffix .ll)"
cleanup() { rm -f "$TMP"; }
trap cleanup EXIT

if [ "$CMD" = "run" ]; then
  dotnet run --project "$PROJ" -- "$FILE" "${EXTRA[@]}" > "$TMP"
  if [ -n "$LLI" ]; then
    "$LLI" "$TMP"
  else
    TMPEXE="$(mktemp --suffix .out)"
    clang "$TMP" -o "$TMPEXE"
    "$TMPEXE"
    rm -f "$TMPEXE"
  fi
elif [ "$CMD" = "test" ]; then
  dotnet run --project "$PROJ" -- "$FILE" --with-tests "${EXTRA[@]}" > "$TMP"
  if [ -n "$LLI" ]; then
    "$LLI" -entry-function=run_tests "$TMP"
  else
    TMPEXE="$(mktemp --suffix .out)"
    clang "$TMP" -o "$TMPEXE" -Wl,-e,run_tests
    "$TMPEXE"
    rm -f "$TMPEXE"
  fi
else
  echo "Unknown command: $CMD" >&2
  exit 1
fi
