#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

setup_repo() {
  local repo_dir="$1"
  mkdir -p "$repo_dir/tools" "$repo_dir/fakebin"
  cp "$ROOT/tools/nightshift.sh" "$repo_dir/tools/nightshift.sh"
  cat > "$repo_dir/tools/validate_repo.sh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
  cat > "$repo_dir/fakebin/codex" <<'EOF'
#!/usr/bin/env bash
echo codex-invoked
exit 99
EOF
  chmod +x \
    "$repo_dir/tools/nightshift.sh" \
    "$repo_dir/tools/validate_repo.sh" \
    "$repo_dir/fakebin/codex"
  git -C "$repo_dir" init -q
  git -C "$repo_dir" config user.name test
  git -C "$repo_dir" config user.email test@example.com
  echo hi > "$repo_dir/readme.txt"
  git -C "$repo_dir" add readme.txt
  git -C "$repo_dir" commit -q -m init
}

run_nightshift() {
  local repo_dir="$1"
  local output_file="$2"
  shift 2
  (
    cd "$repo_dir"
    env -u NIGHTSHIFT_EXPECT_BRANCH PATH="$repo_dir/fakebin:$PATH" "$@" ./tools/nightshift.sh
  ) >"$output_file" 2>&1
}

detached_repo="$TMP_DIR/detached"
setup_repo "$detached_repo"
git -C "$detached_repo" checkout --detach >/dev/null 2>&1
detached_output="$TMP_DIR/detached.out"
set +e
run_nightshift "$detached_repo" "$detached_output" NIGHTSHIFT_BRANCH_MODE=preserve
detached_status=$?
set -e
if [[ "$detached_status" -ne 4 ]]; then
  echo "expected detached preserve run to exit 4, got $detached_status"
  cat "$detached_output"
  exit 1
fi
if ! grep -q "preserve mode requires an attached branch; HEAD is detached" "$detached_output"; then
  echo "expected detached preserve run to explain detached HEAD rejection"
  cat "$detached_output"
  exit 1
fi
if grep -q "codex-invoked" "$detached_output"; then
  echo "detached preserve run should fail before agent launch"
  cat "$detached_output"
  exit 1
fi

reattach_repo="$TMP_DIR/reattach"
setup_repo "$reattach_repo"
expected_branch="feature/test-preserve"
git -C "$reattach_repo" branch -m "$expected_branch"
git -C "$reattach_repo" checkout --detach >/dev/null 2>&1
reattach_output="$TMP_DIR/reattach.out"
set +e
run_nightshift \
  "$reattach_repo" \
  "$reattach_output" \
  NIGHTSHIFT_BRANCH_MODE=preserve \
  NIGHTSHIFT_EXPECT_BRANCH="$expected_branch"
reattach_status=$?
set -e
if [[ "$reattach_status" -ne 99 ]]; then
  echo "expected detached preserve run with matching expected branch to reach codex stub, got $reattach_status"
  cat "$reattach_output"
  exit 1
fi
if ! grep -q "Switched to branch '$expected_branch'" "$reattach_output"; then
  echo "expected preserve run to reattach to the expected branch"
  cat "$reattach_output"
  exit 1
fi
if ! grep -q "== Branch mode: preserve current branch $expected_branch ==" "$reattach_output"; then
  echo "expected preserve run to report the reattached branch"
  cat "$reattach_output"
  exit 1
fi

echo "nightshift script checks passed"
