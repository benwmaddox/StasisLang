#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-codex}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG_DIR="$ROOT/logs/nightshift"
TS="$(date +"%Y-%m-%d_%H%M%S")"
LOG_FILE="$LOG_DIR/run_$TS.log"
BRANCH="nightshift/$TS"
EXECUTOR_FILE="$ROOT/.nightshift-executor"
BRANCH_MODE="${NIGHTSHIFT_BRANCH_MODE:-fresh}"
EXPECTED_BRANCH="${NIGHTSHIFT_EXPECT_BRANCH:-}"

mkdir -p "$LOG_DIR"
exec > >(tee -a "$LOG_FILE") 2>&1

if [[ -f "$HOME/.cargo/env" ]]; then
  source "$HOME/.cargo/env"
fi

if [[ -f "$EXECUTOR_FILE" ]]; then
  MODE="$(tr -d '[:space:]' < "$EXECUTOR_FILE")"
fi

echo "== Night Shift starting at $TS =="
echo "Repo: $ROOT"
cd "$ROOT"

if git diff --name-only --cached | grep -E '(^|/)(\.env|.*secret|.*credential)' >/dev/null 2>&1; then
  echo "ERROR: staged changes may include secrets. Aborting."
  exit 2
fi

echo "== Git status =="
git status --short || true

if [[ "$BRANCH_MODE" == "preserve" ]]; then
  CURRENT_BRANCH="$(git branch --show-current)"
  if [[ -z "$CURRENT_BRANCH" ]]; then
    echo "ERROR: preserve branch mode requires a checked out branch; current HEAD is detached."
    exit 4
  fi
  if [[ -n "$EXPECTED_BRANCH" && "$CURRENT_BRANCH" != "$EXPECTED_BRANCH" ]]; then
    echo "ERROR: expected to run on branch '$EXPECTED_BRANCH' but current branch is '$CURRENT_BRANCH'."
    exit 4
  fi
  echo "== Branch mode: preserve current branch $CURRENT_BRANCH =="
else
  if git rev-parse --verify "$BRANCH" >/dev/null 2>&1; then
    git switch "$BRANCH"
  else
    git switch -c "$BRANCH"
  fi
fi

echo "== Baseline validation =="
"$ROOT/tools/validate_repo.sh"

echo "== Launch agent =="
case "$MODE" in
  claude)
    claude -p "@docs/night_shift_loop.md Follow the Night Shift loop now." --output-format text --dangerously-skip-permissions
    ;;
  codex)
    codex exec --dangerously-bypass-approvals-and-sandbox "Read docs/night_shift_loop.md and execute the Night Shift loop."
    ;;
  *)
    echo "ERROR: unknown mode: $MODE"
    exit 3
    ;;
esac

echo "== Night Shift finished =="
git --no-pager log --oneline -20 || true
