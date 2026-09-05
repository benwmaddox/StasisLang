# Cargo Cache Policy

## Why this exists

Cargo normally creates a separate `target` directory in every linked worktree. That is useful for an interactive developer switching between incompatible branches, but automation worktrees are short-lived and rarely reuse their incremental state. Repeating the full dependency graph in every worktree can exhaust the build drive.

The policy separates those cases:

- Human interactive `cargo ...` commands are unchanged. They keep Cargo's normal per-worktree target and incremental behavior.
- Codex and other automation use `python tools/cargo_cache.py run -- cargo ...`.
- The wrapper derives `build/codex-cargo-target` from Git's common directory, so every linked worktree of the same repository resolves the same cache even when the worktrees live outside the main checkout.
- The wrapper sets `CARGO_INCREMENTAL=0` only in the Cargo child environment. An explicit `CARGO_TARGET_DIR` remains authoritative for tests or platform builds that require isolation.
- The noninteractive `tools/validate_repo.sh` gate routes its Cargo phase through the same wrapper, preventing mandatory validation in each worktree from rebuilding a private target.
- The repository pre-commit hook uses the wrapper for its compiler formatter check and formatter fallback for the same reason.
- CI sets `CARGO_INCREMENTAL=0` because hosted runners are ephemeral and do not reuse local incremental state.

Cargo's target locking makes concurrent commands safe. They may wait when they need the same artifact, but they must not corrupt the shared cache. Do not run cleanup while Cargo processes are active.

## Measured baseline

Measurement on 2026-08-07 used every registered Stasis worktree and reported 46.72 GiB across 13 existing `target` directories:

| Worktree or group | Target size | Dominant profile |
| --- | ---: | --- |
| Main checkout | 16.32 GiB | debug: 14.87 GiB |
| `formatter-blank-lines` | 8.90 GiB | debug: 8.82 GiB |
| `task-189-sdl3` | 5.08 GiB | debug: 5.08 GiB |
| `codex-local-declaration-pr` | 3.92 GiB | two validation profiles: 3.82 GiB |
| `release-asset-closure` | 3.41 GiB | debug: 3.41 GiB |
| `atomic-editor-toolchain` | 2.89 GiB | debug/release: 2.70 GiB |
| Seven remaining targets | 6.20 GiB | mostly debug |

The shared cache is expected to hold one union of the active debug, release, and cross-target artifacts instead of one union per worktree. The current measured union suggests an ordinary operating range below 25 GiB. This is an observation threshold, not a destructive hard cap: when the shared cache exceeds it, measure the profile breakdown and clean deliberately.

## Commands

Run a Cargo command under the automation policy:

```text
python tools/cargo_cache.py run -- cargo test -p stasis_compiler --lib
```

Measure registered worktrees, profiles, incremental bytes, and the shared cache:

```text
python tools/cargo_cache.py measure
python tools/cargo_cache.py measure --json
```

Preview removal of one stale worktree's entire Cargo target:

```text
python tools/cargo_cache.py clean --worktree "C:\src\StasisLang\.worktrees\old-task"
```

Remove only that target's incremental directories after reviewing the exact paths:

```text
python tools/cargo_cache.py clean --worktree "C:\src\StasisLang\.worktrees\old-task" --incremental-only --apply
```

Preview or remove the shared automation cache:

```text
python tools/cargo_cache.py clean --shared
python tools/cargo_cache.py clean --shared --apply
```

Cleanup accepts only a registered worktree belonging to the current Git common directory. It resolves the exact target, rejects path and symlink escapes, prints every selected path, and does nothing unless `--apply` is present. It never deletes source or a worktree.

## Ownership and recovery

- `target` in a normal worktree belongs to that worktree's developer or task.
- `build/codex-cargo-target` belongs to repository automation and is ignored by Git.
- A failed or interrupted build may leave reusable artifacts. Measure before cleaning; prefer `--incremental-only` when dependency artifacts are still useful.
- If the shared cache is corrupt, stop all Cargo processes, preview `clean --shared`, apply it, and rerun the wrapped command. Cargo will reconstruct the cache from `Cargo.lock`.
- Never delete another repository's target through this tool. Common-directory validation intentionally rejects it.
