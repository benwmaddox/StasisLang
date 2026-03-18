#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

mkdir -p "$TMP_DIR/repo/tools" "$TMP_DIR/bin"
cp "$ROOT/tools/nightshift.sh" "$TMP_DIR/repo/tools/nightshift.sh"
chmod +x "$TMP_DIR/repo/tools/nightshift.sh"

cat > "$TMP_DIR/repo/tools/validate_repo.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 0
EOF
chmod +x "$TMP_DIR/repo/tools/validate_repo.sh"

cat > "$TMP_DIR/bin/codex" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 0
EOF
chmod +x "$TMP_DIR/bin/codex"

cd "$TMP_DIR/repo"
git init >/dev/null 2>&1
git config user.name "Night Shift Test"
git config user.email "nightshift-test@example.com"
git checkout -b preserve-branch >/dev/null 2>&1
printf 'base\n' > README.md
git add README.md
git commit -m "test: init" >/dev/null 2>&1
git checkout --detach >/dev/null 2>&1

set +e
OUTPUT="$(
  env -u NIGHTSHIFT_EXPECT_BRANCH -u EXPECTED_BRANCH \
    PATH="$TMP_DIR/bin:$PATH" \
    NIGHTSHIFT_BRANCH_MODE=preserve \
    ./tools/nightshift.sh codex 2>&1
)"
STATUS=$?
set -e

if [[ "$STATUS" -eq 0 ]]; then
  printf 'expected preserve mode on detached HEAD to fail, but it succeeded\n' >&2
  printf '%s\n' "$OUTPUT" >&2
  exit 1
fi

if [[ "$OUTPUT" != *"current HEAD is detached"* ]]; then
  printf 'expected detached HEAD preserve-mode error, got:\n%s\n' "$OUTPUT" >&2
  exit 1
fi
