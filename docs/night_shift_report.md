# Night Shift Report

## 2026-03-27

- Completed issue #263 by replacing the fixed OpenGL sprite atlas model with pageable atlas textures and reusable free-rect allocation in [runtime/stasis_graphics.c](/home/ben/StasisLang/runtime/stasis_graphics.c).
- Removed the fixed compile-time sprite table ceiling by growing sprite handles at runtime and honoring `STASIS_GFX_MAX_SPRITES`, while atlas page sizing now respects runtime GL limits and the atlas env var overrides.
- Added source-level regression checks in [apps/stasis/src/lib.rs](/home/ben/StasisLang/apps/stasis/src/lib.rs) for the two issue-critical guardrails: clamped sprite table growth and clearing reused atlas padding before mipmap regeneration.
- Documented the run outcome here after rebasing onto the existing remote `ned/issue-263` branch instead of overwriting earlier issue work.
- Verification: `tools/validate_repo.sh`
- Good: fetching and rebasing onto the already-populated bot branch preserved the earlier issue implementation and avoided another non-fast-forward failure.
- Bad: the first local pass started from `main` and only later discovered the remote issue branch already contained overlapping atlas work, which forced a rebase and report correction.
- Adjustment: when rerunning work on a reused Ned branch, fetch the remote issue branch before coding so the starting point matches the real handoff state.

## 2026-03-21

- Started the real Android AOT prerequisite slice for issue #254 instead of landing template-only scaffolding.
- Enabled Cranelift arm64 support in the workspace, added an explicit `AotTarget` config path, and taught the AOT backend to emit `aarch64-linux-android` ELF objects when that target is selected.
- Added Android bridge export coverage in `apps/stasis` so the runtime bridge now emits the fixed Android entry ABI symbols (`stasis_init`, `stasis_tick`, `stasis_render`, `stasis_on_input`) on the Android target path.
- Verification: `cargo test -p stasis_compiler aot_process_emits_android_arm64_elf_objects_when_target_is_configured -- --nocapture`, `cargo test -p stasis engine_bundle_runtime_bridge_source_includes_android_entry_exports -- --nocapture`, `tools/validate_repo.sh`
- Good: this slice stayed narrow but still proved the two core prerequisites in code and tests instead of only adding config plumbing.
- Bad: the first Android target compile failed because the workspace had only host-arch Cranelift enabled, so the target-selection code alone was not enough.
- Adjustment: whenever a new backend target is introduced, add one object-format test immediately so missing Cranelift feature flags surface before higher-level packaging work starts.

## 2026-03-19

- Refreshed PR #252 again after `main` advanced, merging the current branch tip into `chore/night-shift-workflow` and resolving the only conflict in `docs/night_shift_report.md`.
- Kept the reviewed `docs/night_shift_loop.md` branch-ownership wording intact, so the PR still preserves the runner-prepared branch instead of creating or switching branches locally.
- Verification: `tools/validate_repo.sh`
- Good: the follow-up refresh stayed isolated to report history, so the reviewed workflow change itself did not need to move again.
- Bad: GitHub still showed the PR as conflicting after the earlier refresh because `main` advanced again almost immediately.
- Adjustment: before closing a conflict-resolution pass, compare the live PR base SHA with the current remote `main` SHA so a second refresh is not missed.

## 2026-03-19

- Refreshed PR #252 by resolving the remaining merge conflicts against `main` without restoring the deleted repo-local Night Shift wrapper.
- Kept the branch-ownership wording in `docs/night_shift_loop.md` and aligned the related process docs so the PR branch now reflects the review fix on top of current `main`.
- Verification: `tools/validate_repo.sh`
- Good: the unresolved merge was confined to the same Night Shift process files already under review, so the refresh stayed narrow.
- Bad: the PR had already fixed the review comment, but the dirty merge state obscured that and kept the branch from moving forward.
- Adjustment: when a review thread is already resolved but the PR still shows `DIRTY`, check mergeability before assuming more content changes are needed.

## 2026-03-18

- Verified issue #250 against GitHub and the repo task list, and found the remaining work was repo-tracking cleanup rather than compiler code changes.
- Removed the stale open item from `docs/bugs.md` and recorded issue #250 as done so local workflow docs match the completed Rust compilation review task list.
- Verification: `tools/validate_repo.sh`
- Good: the issue scope was easy to resolve once the GitHub issue text and the repo task list were checked side by side.
- Bad: the repo still had an open tracking line for work that the task list already marked complete, which forced a second pass just to reconcile status.
- Adjustment: when a review task list item is completed, clear the matching `docs/bugs.md` or inbox-tracking entry in the same change so issue state does not drift from repo state.

## 2026-03-17

- Addressed PR #247 review feedback on preserve-mode branch safety in `tools/nightshift.sh`.
- `NIGHTSHIFT_BRANCH_MODE=preserve` now fails fast when `HEAD` is detached instead of continuing with an unattached commit path.
- Added `tools/ci/test_nightshift_preserve_mode.sh` and promoted it into the standard validation gate via `tools/validate_repo.sh`.
- Verification: `tools/ci/test_nightshift_preserve_mode.sh`, `tools/validate_repo.sh`
- Good: the launcher bug was easy to isolate once the shell regression ran inside a temporary git repo instead of the main workspace.
- Bad: inherited `NIGHTSHIFT_EXPECT_BRANCH` environment in the local shell initially masked the detached-HEAD path and made the first failing test less precise.
- Adjustment: clear inherited Night Shift environment variables in script-level regressions so each harness asserts one branch-management behavior at a time.

## 2026-03-16

- Prepared StasisLang for Night Shift style repo-local runs.
- Added process docs, validation script, and launcher script aligned with the existing Cargo and CI workflow.
- Verification: `tools/validate_repo.sh`
- Needs input from user: decide whether inbox-synced review feedback should map to `docs/bugs.md` only or also back-reference explicit checklist sections when both apply.
- Completed review Task 4 from `docs/reviews/rust-compilation-task-list-2026-03-10.md` by replacing AOT extern prefix heuristics with an explicit runtime export contract in `crates/stasis_compiler/src/backend/runtime_exports.rs`.
- Added regression coverage so fake `gfx_*` externs no longer resolve unless their symbol is explicitly exported, while existing runtime-shim and explicit-symbol extern cases still pass.
- Verification: `cargo test -p stasis_compiler aot_process_rejects_fake_runtime_prefix_extern_without_export_contract_entry -- --nocapture`, `cargo test -p stasis_compiler aot_process_accepts_known_runtime_shim_families -- --nocapture`, `cargo test -p stasis_compiler aot_process_prefers_known_runtime_extern_symbol_over_source_alias -- --nocapture`, `cargo test -p stasis_compiler aot_runtime_export_contract_requires_exact_symbol_matches -- --nocapture`, `tools/validate_repo.sh`
- Good: the failing case was easy to isolate because extern candidate resolution already sat behind one shared helper.
- Bad: the runtime export surface was implicit across `stasis_dynload` and compiler tests, so enumerating the real contract required source spelunking.
- Adjustment: keep runtime-callable export symbols in one compiler-owned table and add focused tests whenever a new export family is introduced.
- Completed review Task 5 from `docs/reviews/rust-compilation-task-list-2026-03-10.md` by adding `parity_corpus_covers_shared_lowering_shapes` in `crates/stasis_compiler/src/backend/aot.rs`.
- The new corpus covers extern calls, globals/collection access, control flow, struct-view field access, and string literal handling; it also captures AOT CLIF text and checks stable shape markers from the shared lowering path.
- Verification: `cargo test -p stasis_compiler parity_corpus_covers_shared_lowering_shapes -- --nocapture`, `cargo test -p stasis_compiler aot_engine_bundle_manifest_includes_string_literals -- --nocapture`, `cargo test -p stasis_compiler aot_process_prefers_known_runtime_extern_symbol_over_source_alias -- --nocapture`, `tools/validate_repo.sh`
- Good: writing the parity cases as one corpus made it easy to tighten IR-shape assertions after the first run exposed where helper calls actually appear.
- Bad: some CLIF expectations that looked obvious at first were wrong because collection and struct-view access lower through shared helper calls rather than inline load/store ops.
- Adjustment: keep future parity CLIF checks at the shared-lowering seam that is actually stable, and use behavior assertions for the rest instead of overfitting to incidental instruction placement.

## 2026-03-17

- Addressed PR #247 review feedback in `tools/nightshift.sh` by rejecting detached-HEAD preserve runs unless `NIGHTSHIFT_EXPECT_BRANCH` names a local branch whose tip matches `HEAD`, in which case the script now reattaches before continuing.
- Added `tools/test_nightshift.sh` and wired it into `tools/validate_repo.sh` so detached preserve-mode rejection and explicit reattach behavior stay covered.
- Verification: `tools/test_nightshift.sh`, `tools/validate_repo.sh`
- Good: the branch-mode logic sits in one small shell block, so the safety fix stayed narrow and easy to regression-test.
- Bad: the first implementation pass landed on the wrong local branch because the synced PR bug and the starting checkout did not match.
- Adjustment: when `docs/bugs.md` points to a specific PR, confirm the local checkout matches that PR head before editing and fast-forward it before the first validation run.

## 2026-03-18

- Updated the Night Shift process to treat GitHub issues, PR comments, and PR reviews as the only source of work selection.
- Repo-local docs now serve only as context and validation guidance; they no longer act as a competing task queue for Night Shift runs.
- Added a runner guard that stops when no selected GitHub item was provided, and kept the runner self-snapshot logic so editing `tools/nightshift.sh` during a run does not break the live process.
- Verification: `tools/test_nightshift.sh`
- Good: removing the split between GitHub-selected work and repo-local queue files makes the automation easier to reason about.
- Bad: standalone repo-local Night Shift runs are now intentionally narrower and require the inbox handoff to provide a selected item.
- Adjustment: keep repo-local docs focused on how to change and validate the repo, and keep work selection in GitHub.
- Removed the stale preparation step in `docs/night_shift_loop.md` that still told the executor to sync the default branch and create a fresh `nightshift/...` branch for issue-driven work.
- The loop contract now tells the executor to preserve the branch prepared by the central Ned inbox runner and to stop on branch/check-out mismatches instead of mutating local branch state.
- Verification: `tools/validate_repo.sh`
- Good: the review comment pointed to one concrete contract mismatch, so the fix stayed narrow and easy to verify.
- Bad: the loop doc still had one leftover instruction from the older repo-local runner model even after the ownership note moved branch setup to the inbox runner.
- Adjustment: when workflow ownership moves across systems, re-read the procedural checklist line by line and delete stale executor steps in the same change.

## 2026-03-24

- Completed issue #258 by adding a lookup CLI to `apps/stasis/src/main.rs` with `s|search`, `sig|signature`, and `def|definition` support plus `--entry` import-closure scope and `--file` single-file scope.
- Added parser-backed struct definition ranges in `crates/stasis_compiler/src/frontend/parser.rs` so definition lookups can print exact struct bodies without a second ad-hoc parser.
- Verification: `cargo test -p stasis parse_lookup -- --nocapture`, `cargo test -p stasis run_lookup_command -- --nocapture`, `cargo test -p stasis_compiler parses_top_level_struct_definition_ranges -- --nocapture`, `cargo run -p stasis -- sig tick --file samples/brickout_revenge/brickout_revenge.stasis`, `tools/validate_repo.sh`
- Good: keeping the lookup output parser-backed made the new command small and let the whole-directory search ignore invalid fixture files cleanly.
- Bad: the compiler parser exposed function ranges but not struct ranges, so exact struct-definition output needed a small parser extension before the CLI work could stay clean.
- Adjustment: when a new tooling feature needs source excerpts, expose precise ranges from the shared parser first instead of duplicating extraction logic in the CLI.

## 2026-07-18

- Completed Maddox #121 by adding deterministic TalkBack/keyboard traversal, visible focus treatment, keyboard and accessibility-action Paint editing, contrast-audited colors, configuration-retained Paint state, and compact/medium/expanded layouts that respond to font scale.
- Verification: `python tools/ci/check_android_shell.py`, `python tools/ci/check_stasis_src_layout.py`, `gradle :app:testWorkshopDebugUnitTest --no-daemon --max-workers=1`, `gradle :app:lintWorkshopDebug --no-daemon --max-workers=1`, `gradle :app:assembleWorkshopDebug --no-daemon --max-workers=1`, `git diff --check`.
- Good: pure layout/contrast policies made adaptive and accessibility decisions fast to test without an emulator.
- Bad: the first broad Cargo validation exhausted the host drive and hit a Windows PDB linker limit even though this slice changes only Android Java/resources.
- Adjustment: keep Android UI slices on the bounded Android/JVM/APK gates first, then attempt the full Cargo gate only with verified workspace capacity.

## 2026-08-01

- Maddox #180: introduced immutable compiler-owned `ProgramSnapshot` shared by JIT, AOT, runner-facing metadata, and app packaging. The snapshot is the canonical state-layout digest owner; target code pointers/object paths are non-semantic artifact mappings.
- Verification: ProgramSnapshot 12/12, state layout 5/5, JIT 178/178, AOT 46/47 runnable plus 1 ignored with the Application Control-blocked parity case passing on isolated retry, Workshop 32/32, app compiler backend 84/84 plus 6 ignored benchmarks, Android bridge 48/48 plus 1 ignored, repository Python checks 29/29, workspace check, Windows launch acceptance, formatting, and diff checks.
- Theory gained: one accepted semantic snapshot can safely outlive failed candidates because target artifacts are attached metadata, not an alternate source of program truth. The matching multi-file JIT/AOT execution and rollback tests support this; an adjacent prediction is that new tooling views can consume snapshot/source-item records without adding another source scanner.
- Good: moving the existing analysis cache behind the snapshot retained one-pass lowering data while the same compiler-owned records replaced app and Workshop reparsing.
- Bad: transaction boundaries were initially incomplete around prepared JIT delivery and AOT object buffers, so rejected candidates could lose precise diagnostics or invite expensive rollback cloning.
- Adjustment: every future snapshot consumer must test successful publication, post-mutation rejection, retained diagnostics, and preservation of the complete previously accepted semantic and artifact state.
