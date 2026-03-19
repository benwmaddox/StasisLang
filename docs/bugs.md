# Bugs

Allowed states: `READY`, `IN PROGRESS`, `DONE`, `NEEDS INPUT FROM USER`

If you use a cross-repo inbox, it may maintain a generated `NED-INBOX` block under `## READY`. Treat those entries like normal bugs. Those synced items should include direct GitHub links for the source PR and individual review threads so the agent can reply when appropriate.

## READY

- None.

## IN PROGRESS

- None.

## DONE

- [P1][PR #247] Addressed review feedback for `Preserve PR branches during Night Shift runs`
  - Source: https://github.com/benwmaddox/StasisLang/pull/247
  - Completed: `2026-03-17`
  - Fixed `tools/nightshift.sh` so `NIGHTSHIFT_BRANCH_MODE=preserve` rejects detached HEAD runs before validation/agent launch.
  - Added `tools/ci/test_nightshift_preserve_mode.sh` and wired it into `tools/validate_repo.sh`.
  - Reply on GitHub: https://github.com/benwmaddox/StasisLang/pull/247#discussion_r2946496401
- [P2][Issue #250] Closed stale completion-tracking item for Rust compilation review tasks
  - Source: https://github.com/benwmaddox/StasisLang/issues/250
  - Completed: `2026-03-18`
  - Confirmed from GitHub and `docs/reviews/rust-compilation-task-list-2026-03-10.md` that Tasks 1 through 5 were already completed.
  - Removed the stale open-item line from `docs/bugs.md` so repo tracking now matches the completed task list.

## NEEDS INPUT FROM USER

- None.
