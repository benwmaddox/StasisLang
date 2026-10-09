# Night Shift Report

## 2026-10-09 - Maddox #638 supervised live authority

Added an opt-in Windows supervisor mode that launches the existing Stasis CLI as
the live JIT authority and bridges bounded control, response, readiness, peer,
and capture channels without a hidden child process. The authority stages and
verifies a fresh network guest bundle, reserves its host before application
startup, and publishes readiness only after the live workspace and frame setup
are ready. Fixed-schema requests accept normal host input, pause/resume, one-tick
scheduling, captures, and quit; current viewport checks reject invalid input and
oversized captures before acknowledgement or native image scheduling. Requests,
pipe writes, capture evidence, peer receipts, and shared shutdown cleanup have
explicit bounds. Packaged supervision remains on its existing mode.

Added a separate real-game TTT fixture using four byte-identical modules pinned
to Maddox and Friends commit `cb43e41a2c0a9c1f1123365885f731a35184a351` with
hashes recorded in `tests/fixtures/network_supervision/live_ttt/provenance.json`.
The host uses ordinary pointer frames and the peer uses the authenticated native
`NetworkClient` protocol. The independent peer oracle verified the win, rematch,
draw, and catalog return across 20 exact snapshots and 43 numeric receipts.
Four retained 640x360 PNGs were inspected; they show the win, cleared rematch,
full draw, and catalog. Six additional process cases passed: unread supervisor
stdout, malformed JSONL, stdin EOF, idle timeout, peer exit, and authority exit.
Each failed nonzero within its bound and both recorded child processes were gone.

Validation through the frozen Windows/MSVC runtime launcher passed:

- `cargo test -p stasis_runner supervised_live::tests -- --test-threads=1` (5/5).
- `cargo test -p stasis supervised_ -- --test-threads=1` (5/5).
- `cargo test -p stasis_network --features supervision-cli --lib supervision:: -- --test-threads=1 --nocapture` (7/7).
- `cargo test -p stasis_network --test supervision -- --test-threads=1` (3/3).
- `cargo test -p stasis_network --test native_client -- --test-threads=1` (6/6).
- `cargo test -p stasis_dynload --features network --lib -- --test-threads=1` (88/88).

Fresh `stasis`, `stasis-network-supervise`, and `supervision_live_ttt_peer`
binaries were built with the same local validation identity. The end-to-end
`run_acceptance.ps1` passed the exact state, receipt, capture-hash/dimension,
aggregate-evidence-size, and bounded-child-cleanup oracles. `cargo fmt --all`
and `git diff --check` passed. The legacy packaged regression
`tools/ci/test_network_supervision.ps1` also passed with fresh CLI and matching
graphics runtime/runner artifacts (`NETWORK_SUPERVISION_ACCEPTANCE_OK`). The
cold Windows CI job now derives the commit identity and builds matching native
artifacts before running that source-checkout test.

Visual evidence: inspected the four retained PNGs from the successful live run;
the application rendered a red host win, an empty rematch board, the complete
red/blue draw, and the catalog. Also inspected
`D:/code/.automation-evidence/nightly-20261009/task638/live-acceptance/evidence/ttt-checkpoint-sequence.mp4`,
a 223-frame checkpoint sequence assembled from those four actual captures. It
marks win, rematch, draw, and catalog; it is not a continuous input recording.
The SDL dummy display means the PNGs are captured renderer output, not a visible
desktop window recording.

Theory gained: a scheduled step is only an acknowledgement that one tick was
queued; the peer's next authoritative snapshot is the evidence that host input
actually reached the game. Windows JSONL clients commonly use CRLF, so the
supervisor strips exactly the terminal CR while preserving embedded CR for the
strict DTO parser to reject.

## 2026-10-03

- Added a dispatch-only iOS Generics simulator workflow with fresh matching host runtime/compiler builds, ARM64 runner checks, fail-closed package provenance/evidence validation, and partial artifact upload.
- Updated the mobile packaging guide and added focused workflow policy coverage; the new lane remains outside PR and nightly required checks and does not claim physical-device qualification.
- Verification: 107 focused workflow/helper/Android seam, placement, network-policy, and action-version tests passed; action-version and SDL migration checks passed; YAML parse and git diff check passed. Hosted iOS dispatch remains pending infrastructure merge and registration on the default branch.
- Visual evidence: not applicable; no user-visible behavior changed.
- Theory gained: package linking is not simulator runtime evidence; the generated app must run the selected generics oracle on an ARM64 simulator, and every saved result must carry the dispatched source commit.

- Documented the public `SpriteRunWriter` frame contract and a compilable Rig2D-to-sprite attachment recipe with an explicit destination-local pivot.
- Added Rig2D to the knowledge index and linked its example from the graphics and assets guide.
- Verification: fresh current-branch CLI fixture `format --check` and `check`; `cargo fmt -- --check`; `generated_knowledge_examples_compile_and_test`; `git diff --check`.
- Visual evidence: not applicable.

### Maddox #786 integer bitwise operators

Implemented integer `&`, `^`, `|`, `~`, `<<`, and `>>` across frontend validation, HIR/data-flow, and JIT/AOT/Wasm lowering. Literal-only trees inherit an exact expected integer lane recursively; validation rejects non-integer operands, mismatched typed lanes, and out-of-range contextual literals. Shift counts are masked by the left lane width, signed `i32 >>` is arithmetic, unsigned `>>` is logical, and narrow results are normalized. Generic type scanning now keeps parenthesized shift operators inside values such as `Bits<(1 << 3)>`, while parsing the following declaration separately. Docs, VS Code operator scopes, and LSP diagnostics cover the same surface. A shared `.stasis` oracle probes runtime parameters across `i32`, `u8`, `u16`, and `u32`; the existing generics sample runs the same oracle without changing its 507 digest.

Validation so far: `python tools/cargo_cache.py run -- cargo test -p stasis_compiler -j1 --test bitwise_operators_jit_aot_wasm -- --test-threads=1` passed 4/4, including JIT, linked/signed Windows native AOT execution, Node no-import Wasm, the public shifted-generic JIT fixture, operand/range diagnostics, and unsupported compounds. `python tools/cargo_cache.py run -- cargo test -p stasis_lsp -j1 -- --test-threads=1` passed 36/36, including diagnostic clearing after a full-document change. VS Code `npm test` passed 16/16; Rust formatting and `git diff --check` passed. The full serial compiler library suite passed 920/920 under the initialized Windows linker. Fresh exact-feature-SHA packaged sample and Android/iOS runtime gates remain pending.

Visual evidence: not applicable (language semantics and editor token scopes; runtime behavior has executable parity oracles).

Theory gained: runtime-parameter callees keep expected operator cases dynamic, so each backend is checked against independent golden values rather than a frontend-folded result. Narrow-lane correctness also needs chained expressions that expose whether an intermediate result was normalized before the next shift.

Good: one runtime-parameter oracle feeds JIT, linked AOT, Wasm, and the sample, with independent golden results, contextual-lane failures, and generic value-application coverage.
Bad: the initial compound-assignment fixture required the wrong diagnostic phase; the public generic test also exposed a scanner bug where shift `>` inside parentheses was treated as a generic closer and swallowed the next global declaration.
Adjustment: assert the unsupported-assignment cause independent of phase, track nested parentheses while scanning generic values, and document the required parentheses around shift expressions in generic applications. Initialize the supported MSVC environment so native AOT links and executes.

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
- Verification: ProgramSnapshot 14/14, state layout 5/5, JIT 178/178, compiler library 399/400 runnable plus 1 ignored with the Application Control-blocked case passing on isolated retry, Workshop 32/32, app compiler backend 84/84 plus 6 ignored benchmarks, Android bridge 48/48 plus 1 ignored, repository Python checks 29/29, workspace and release checks, Windows launch acceptance, Android JIT/AOT render acceptance, formatting, and diff checks. The 1,000-function selective-update benchmark improved from 202 ms to 27.560 ms p95 while emitting 2/1001 functions.
- Theory gained: one accepted semantic snapshot can safely outlive failed candidates because target artifacts are attached metadata, not an alternate source of program truth. Compiler-produced data-flow summaries are already immutable at index completion, so sharing their owner into snapshots preserves completeness while avoiding semantic-vector copies; an adjacent prediction is that other large immutable semantic tables can use the same ownership boundary.
- Good: moving the existing analysis cache behind the snapshot retained one-pass lowering data while the same compiler-owned records replaced app and Workshop reparsing; CI profiling then exposed and removed an unnecessary full data-flow copy.
- Bad: initial transaction boundaries missed prepared JIT delivery and AOT buffer rollback, and initial snapshot copies regressed selective compilation above its stop condition; staged candidates also failed to propagate custom analysis roots.
- Adjustment: every future snapshot consumer must test publication, post-mutation rejection, retained diagnostics, custom-root staged compilation, complete accepted semantic/artifact preservation, and the existing performance stop conditions.

## 2026-08-07

- Maddox #190: added one Git-common-directory-derived Cargo target for Codex/automation, child-scoped `CARGO_INCREMENTAL=0`, worktree/profile/incremental measurement, dry-run-first confined cleanup, and explicit CI/validation routing. The measured baseline was 46.72 GiB across 13 registered worktree targets; two concurrent Cargo checks safely shared the new target.
- Verification: cache policy 9/9; concurrent `stasis_assets` and `stasis_dynload` checks; toolchain CLI 21/21; JIT 17/17; Android bridge 51/51; portable policy/render tests 47/47; Windows launch 1/1; full workspace/all-target suite passed with one pre-existing ignored external-root test when the installed MSVC linker was explicit; Rust formatting and diff checks passed. Repository validation still reports two unchanged `origin/main` policy findings before Cargo: an unsafe boundary in `crates/stasis_ai/src/lib.rs` and the ignored external-root language-service test.
- Theory gained: build-cache identity belongs to the repository/toolchain/profile combination rather than a task branch; deriving the cache from Git's common directory preserves that identity across arbitrary linked-worktree locations. This predicts future automation entrypoints can share artifacts safely if they cross the same explicit wrapper boundary and retain Cargo's own locking.
- Good: real size measurement, two-worktree resolution, concurrent builds, and destructive-path tests made cache ownership and safety observable.
- Bad: the first implementation documented the wrapper but left the canonical validation script's Cargo phase outside it; cold validation also exposed hidden network and linker prerequisites.
- Adjustment: whenever automation policy changes an execution boundary, test every canonical entrypoint for that boundary and run one cold plus one concurrent representative build before publication.

- Added one SDL3-backed bounded PCM16 WAV mixer shared by desktop and mobile, with overlapping
  voice handles and Brickout-compatible music/effect helpers for loop, pause, stop, volume, and pan.
- Replaced the desktop JIT/AOT lifecycle-query stubs with optional calls into the same graphics
  runtime, and added release/mobile source inventory coverage so the mixer ships in generated
  archives and the iOS shell.
- Verification: focused JIT/AOT audio compiler tests; seven C runtime contract tests; mobile package
  and release-provenance tests; full pinned SDL runtime build; Windows AOT sample build and launch
  through SDL's dummy audio backend.
- Theory gained: audio samples and cursors are nondeterministic host resources while Stasis owns
  only opaque handles and deterministic decisions to play or adjust them. The successful AOT sample
  proves that this ownership boundary survives compilation, packaging, dynamic loading, decoding,
  and callback startup; an adjacent prediction is that a future streaming decoder can replace the
  asset source behind the same voice mixer without changing game state layouts.
- Good: the isolated executable test exposed three dev-tree blind spots—escaping imports, an invalid
  manifest shape, and a lifecycle stub—before the API reached a game.
- Bad: the first runtime-only contract passed even though the AOT host bridge still returned a fake
  unavailable result.
- Adjustment: every new runtime export family must include one packaged AOT executable that crosses
  the dynamic boundary and asserts real host behavior, not only compiler resolution and C-local tests.

## 2026-09-05 - Task 515: desktop game screenshots

- Replaced scheduled-only capture acknowledgement with bounded, cancelable PNG completion evidence and runtime identity checks. Wired active-task attachment, PNG preview, content hashes, upload/analysis state, and fail-closed provider image routing.
- Verification: 61 AI library tests, 18 desktop editor tests, five runtime capture tests, and a fresh SDL runtime integration capture; formatting and diff checks passed. Baseline `tools/validate_repo.sh` stops at existing unsafe-boundary findings in `stasis_network`; no unrelated policy files were changed.
- Visual evidence: `target/task515-evidence/live-game.png` was inspected and shows the expected 320x180 red rectangle on a dark background. `capture.json` records matching PNG dimensions, byte length, SHA-256, and runtime identity with the game paused at tick 0. The native runtime and SDL dependencies were freshly built from pinned sources.
- Theory gained: presentation continues while deterministic gameplay is paused, so a screenshot can complete without a gameplay step. The observed tick-0 PNG predicts that future paused-state inspection can share this presentation boundary without changing simulation state.
- Good: task/request snapshots and content hashes prevent late completion from marking newly attached images uploaded.
- Bad: the initial runtime fixture omitted the required `on_code_swap` entrypoint and failed before capture.
- Adjustment: live runtime fixtures must include all required lifecycle entrypoints before testing a new host interaction.

## 2026-09-05 - Task 516: generated images and native focus (desktop gate pending)

- Added task-scoped OpenAI/OpenRouter image production, bounded in-memory PNG previews, provider/routing/fallback/cost attribution, explicit review, and create-only asset import. Fixed one-shot editor focus requests, wrapped notices, scrolling, and splitter dragging. Added an optional native SDL focus capability without changing frame ABI layouts.
- Verification: 66 AI library tests and 26 desktop-filtered CLI tests passed. The native dynload focus test and live paused-focus/capture integration passed against a freshly built SDL runtime. Runtime ABI (797 comparisons), host contract (953 comparisons), 45 contract tests, Rust formatting, and diff checks passed. The full shell entrypoint cannot start with this Windows shell's missing dirname/python3; the direct unsafe audit still reports only the two pre-existing stasis_network files.
- Visual evidence: inspected `target/task516-evidence/runtime-focus.png`, the expected 320x180 red rectangle on a dark background after focus; `runtime-capture.json` records the focus response and unchanged paused tick. Desktop PNG/MP4 acceptance remains blocked: computer-use access to Stasis was denied, and automatic approval review rejected a retry because the app was not approved. No workaround was attempted after that rejection; validation processes were closed. No paid image provider was invoked.
- Theory gained: approval must be validated on a candidate task state before publishing image bytes; create-only publication failure then preserves approval for retry. The paused-focus integration also confirms that native window requests can share the tick boundary without advancing simulation.
- Good: review caught approval-after-write ordering before release, and regression tests now prove that pending/rejected imports leave assets untouched.
- Bad: desktop interaction evidence could not be completed because app-specific computer-use approval was unavailable.
- Adjustment: resume the native window sequence documented in `docs/desktop_editor_images.md` once Stasis computer-use access is approved; inspect both PNG and MP4 before marking the desktop gate complete.

## 2026-09-09 - Task 516: attribution refinement and authorized desktop retry

- Preserved retained image/import/focus implementation. Added pre-request attribution-capacity checks and explicit unknown labels for malformed reported identity, retaining generated bytes. Three regression tests cover pre-request rejection, the exact boundary, and accepted session attribution.
- Validation: 69 AI tests, 26 desktop tests, eight focused image tests, fresh SDL runtime capture integration, formatting, diff checks, and ABI/host contracts passed. The full validation script reaches existing unsafe-boundary failures in unchanged stasis_network files.
- Visual evidence: inspected `target/task515-evidence/live-game.png`, proving the expected red rectangle after a paused focus request. Desktop PNG/MP4 and interaction acceptance remain pending: connector recovery returned `Computer Use was not approved to use stasis` despite task authorization. No paid calls. Task-owned processes stopped.
- Current-main integration remains pending; the older editor must be ported into main's persistence/attachment/timeline architecture without overwriting it. No Git or Maddox state/publication operation occurred.
- Theory gained: provider metadata must fit session attribution before network access; malformed reported identity must stay explicitly unknown while artifact bytes remain reviewable.
## 2026-09-09 - Task 338: post-SVGO game SVG evaluation

- Preserved the recovered 300/1500-path LIVE masters and provenance. Added reproducible precision-2 baselines, isolated optimization experiments, structural audits and a native probe using the existing ThorVG bridge. No production optimization or renderer change: other passes provide no gain, increase compressed size, or alter rendered coverage.
- Verification: fresh MSVC/ThorVG Release build; 2/2 CTest and 4/4 Python tests; 420 full-image candidate/target/background comparisons; three identical native renders per unique SVG/target; independent regeneration reproduced all hashes and raw/gzip/Brotli sizes; diff checks passed. Commands and measurements are in `tools/svg_evaluation/README.md` and `evidence/`.
- Visual evidence: inspected `tools/svg_evaluation/evidence/review.png` for candidate comparisons and `tools/svg_evaluation/evidence/screen/baseline-1080x2400.png` for the full screen/contain behavior. Enlarged and background gates compare all pixels automatically.
- Theory gained: unchanged decimal precision does not guarantee unchanged raster coverage after another path normalization; the enlarged-size failures predict that future numeric passes need the same complete render matrix.
- Good: hashing recovered sources removed the previous missing-corpus blocker without rerunning vectorization.
- Bad: MSBuild initially misreported missing compilers because the inherited environment contained both Path and PATH.
- Adjustment: the standalone build helper normalizes Windows environment keys before invoking CMake/MSBuild.

### Task 338 render acceptance review repair

- Enforced the reviewed per-candidate, target, and background outcome matrix. Unexpected passes, failures, missing/extra comparisons, and inconsistent aggregate results now fail validation, including under `python -O`. Timing noise and nonzero delta magnitudes remain diagnostic.
- Preserved both branches' report entries while resolving the prepared merge conflict. Validation: eight Python tests, fresh native build and two CTest tests, and the complete 420-comparison render matrix.
- Visual evidence: not applicable; this repair changes validation and documentation only, preserving the existing SVG and visual evidence.

## 2026-09-06 - Task 312: safe external URL host action

- Added `open_external_url(string): i32` with a 2048-byte UTF-8 bound, strict HTTP(S) validation, one-attempt input authority, and invalid/ignored/accepted results. Desktop SDL, Android release/Workshop, iOS, and web adapters use platform browser APIs; headless/recording execution cannot launch a browser. JIT swap hooks suppress the action before dispatch.
- Preserved the existing string-handle ABI, AOT runtime export, wasm import, and release-package dynamic text metadata. The Stasis pointer fixture requests one Maddox Labs link on a down edge and does not repeat while held. No consumer UI was added in this task.
- Verification: all seven focused compiler/dynload/Android bridge tests pass, including actual linked AOT execution and JIT edge/swap assertions. The 118 web runtime tests, web metadata and mobile-shell package tests, 44 enabled dynload tests, two fresh native C executables, and 38 Python host/runtime ABI tests pass. Full validation stops at existing `stasis_network` unsafe-boundary findings; the broader compiler run also encounters missing optional signing certificates and a shared-cache access denial. Task builds use the required Cargo wrapper with an explicit worktree-local target because the shared cache is outside the allowed worktree.
- Visual evidence: browser/device media was not captured. There is no new in-canvas UI; real popup behavior remains a validation limit. The local web package harness rejected an unverified CLI build fingerprint and the Playwright wrapper did not become available. Android Java compilation could not resolve Android Gradle Plugin 8.7.3 from configured repositories. Android/iOS browser dispatch requires device validation; Xcode was unavailable on this Windows host.
- Theory gained: guest input values are data, not proof of a user gesture. The native/Workshop/web tests show that host-owned authority must be transferred to one tick/frame and consumed once, while swap execution suppresses dispatch. Adjacent privileged host actions should reuse that explicit authority boundary rather than infer permission from simulated input.
- Good: the edge fixture and host injection exercise real guest compilation without launching external applications.
- Bad: an initial AOT check skipped execution because the optional signer lacked a certificate; initial Workshop authority also incorrectly relied on simulated touch state.
- Adjustment: verify executable checks actually run, preserve required-signing policy, and supply trusted platform activation separately from guest-visible input.
## 2026-09-06 - Task 354: native guest transport ABI

- Added an optional Windows/Android native client using the existing browser WebSocket protocol, bounded mailbox, private pairing/resume identity, reconnect backoff, and shell background lifecycle. Client-only packages link transport support without host bundles or listeners.
- Verification: 33 network tests, seven network package tests, focused package/runtime configuration tests, Windows static-link probe, Android x86_64 static-link and AOT bridge probes, and real Chrome guest acceptance passed. Release provenance, ABI contracts, formatting, and diff checks passed.
- Validation limit: `tools/validate_repo.sh` reaches its existing ignored-test audit and fails on unchanged timing tests in `stasis_dynload/src/lib.rs` and `stasis_compiler/src/backend/program_snapshot.rs`; the workspace-wide Cargo phase therefore did not run. The optional Android Workshop source check also fails an unchanged `allowAiImageGeneration.setChecked(false)` assertion. No packaged graphical client or multi-device LAN run is claimed.
- Visual evidence: inspected `target/network-browser-acceptance/browser.png` and all five one-second samples of `browser.mp4`; they show browser join, snapshot, bidirectional command, and resumed-session stages without credentials. Native probe results are in `target/native-client-windows/result.txt` and `target/android-network-client/result.txt`.
- Theory gained: a connection generation owns socket work and queued messages. Lifecycle and slow-frame tests show obsolete work is discarded; another provisioning surface can reuse this boundary without adding sockets or credentials to deterministic state.
- Good: independent review exposed lifecycle races before the final platform probes.
- Bad: the first bridge include incorrectly treated the separately packaged network header as a runtime-local file.
- Adjustment: validate optional external header provenance alongside ABI and package capability tests.

### Task 354 review follow-up

- Gate the Windows CMake mode helper with its caller's platform condition. Protect Android join provisioning with an app-namespaced signature permission on the explicit NetworkJoin alias; direct launcher intents discard join extras.
- Replace blocking TCP establishment with a single cancellable nonblocking attempt, and replace 2 ms idle socket polling with a 25 ms command-interruptible wait. Regression coverage includes shutdown, background, generation cancellation, and component policy rejection cases.
- Validation: the exact architecture characterization fast lane passed on Windows; a non-Windows warnings-as-errors probe passed after reproducing the CI failure. All 35 network tests, package generation, seven Java admission policy scenarios, fresh Windows static-link probes, Android target compilation, ABI audits, formatting, and diff checks passed.
- Theory gained: the public launcher and private provisioning route need distinct component permissions even when they share an activity; lifecycle cancellation must also cover TCP establishment before a WebSocket exists.
- Good: a non-Windows compile probe reproduced the CI warning without suppressing warnings.
- Bad: the previous shutdown tests started after TCP establishment and missed dropped SYNs.
- Adjustment: test each transport phase's cancellation authority, including an indefinitely pending TCP readiness probe.
- Visual evidence: not applicable; this follow-up changes transport and component admission, with no graphical UI change.

- Android admission evidence: API 35 fixture using the exact production policy and provisioning method admitted the same-signer alias caller, denied the differently signed caller with SecurityException before dispatch, and rejected direct MainActivity provisioning in onCreate and onNewIntent. Inspected target/android_admission_evidence/evidence.txt; this is a minimal admission fixture, not the packaged SDL game.

### Task 424: focused desktop conversation layout

Removed the editor's fixed task/game placeholder split in favor of a narrow
resizable sidebar and one active conversation, matching the revised separate
native game window direction. Anchored reply/actions below the scroll area
and moved the verified screenshot preview into the active thread. Added a
layout regression test for composer visibility with a long thread at 900x600
and 1440x900. No provider, source-edit, or runtime contracts changed.

Validation (2026-09-08 retry): a fresh worktree-local Cargo build passed all
32 desktop_editor tests through tools/cargo_cache.py. The retained layout
test exposed clipped composer actions at 900x600; reserving 180 pixels for
the composer fixed it. The test now skips egui's invisible initial sizing
pass and verifies both subsequent frames at 900x600 and 1440x900. rustfmt
and git diff --check passed. No task-local test processes remained.

Git Bash with its utility PATH restored and a python3-to-python shell
function started tools/validate_repo.sh. Runtime ABI (797 comparisons),
host contracts (953 comparisons), compiler characterization, failed-publication
rollback, browser storage/network, and shared protocol fixtures passed.
The entrypoint then stopped at vscode.protocol because TypeScript dependencies
were absent. This is not a complete repository validation pass.

A fresh cargo run of the editor stopped before window creation: the CLI has
no verified build fingerprint. The installed-toolchain identity gate remains
intact. Read-only GitHub verification found prerequisite PR #697 OPEN with
mergedAt null; remaining child implementations are not integrated here.
Visual evidence: unavailable; fresh runtime PNG/MP4 capture and visual review
remain required. This bounded layout change does not complete parent acceptance:
image generation/import, bounded-context presentation, diff rendering, routing
policy acceptance, and full runtime workflow evidence still require follow-up.

Theory gained: a bottom-anchored composer still needs enough minimum height
for wrapped actions; the 900x600 regression demonstrated this. Additional
actions or a narrower sidebar allocation should rerun the clipping assertions.

Latest worker recheck (2026-09-08): preserved the retained layout changes.
`git diff --check` and file-scoped `rustfmt --check --edition 2021` passed.
A fresh worktree-local `python tools/cargo_cache.py run -- cargo test -p stasis
desktop_editor -- --test-threads=1` failed before tests: Application Control
blocked the glutin_egl_sys build script (OS error 4551). The repository signing
helper, run with ExecutionPolicy Bypass, found no matching signing certificate.
This attempt does not reproduce the earlier recorded passing tests. No
worktree-target processes remained after the failed build.
An authenticated `gh pr view 697 --repo benwmaddox/StasisLang --json
state,mergedAt,headRefName` still returned OPEN and mergedAt null. The local
GenerateImage and ImportImage intents still have no execution adapter; they
remain queued by flush_intents. Integrating the assigned child implementations
is still required before parent acceptance. No branches, commits, PRs, or task
state were modified. Visual evidence: unavailable in this recheck; fresh
desktop capture acceptance remains incomplete.

Current task-424 revalidation (2026-09-08): preserved the retained layout
implementation and rebuilt dependencies in target/task424-validation through
`python tools/cargo_cache.py run -- cargo test -p stasis --bin stasis desktop_editor -- --test-threads=1`.
All 36 selected tests passed, including long-thread composer visibility at
900x600 and 1440x900, task isolation, cancellation, screenshots, and hash-checked
apply/test receipts. Optional signing reported no matching certificate, but
the test executable ran successfully; the historical OS error 4551 did not recur.
File-scoped rustfmt and git diff --check passed.
Read-only authenticated GitHub checks show #697 still OPEN/unmerged and #699
MERGED. Local GenerateImage/ImportImage intents still fall through to the
pending queue without an adapter. Parent acceptance therefore remains incomplete
pending its assigned child integration; no Git or Maddox mutations were made.
Visual evidence: unavailable in this revalidation; headless layout assertions
are not native desktop PNG/MP4 workflow evidence.

Task-424 recheck (2026-09-10): preserved the retained layout implementation.
Fresh worktree-local validation through `python tools/cargo_cache.py run --
cargo test -p stasis --bin stasis desktop_editor -- --test-threads=1` passed
all 36 tests. Optional signing found no certificate, but execution succeeded.
File-scoped rustfmt and git diff --check passed. Live read-only GitHub evidence
confirms PR #722 merged at 2026-09-10T12:11:33Z; earlier open-PR dependency
claims above are historical. The supplied current task record identifies #516
as the only unfinished child. This checkout still queues GenerateImage and
ImportImage without execution adapters; the GitHub publication search found
no corresponding child PR. Its retained implementation is outside the supplied
worktree, so parent integration and generation/import acceptance remain pending.
Visual evidence: unavailable in this recheck; headless layout tests do not
replace the required reviewed native desktop generation/import workflow.

Task-424 recheck (2026-09-12): preserved the retained layout implementation.
Fresh worktree-local dependencies and desktop validation through
`python tools/cargo_cache.py run -- cargo test -p stasis --bin stasis
 desktop_editor -- --test-threads=1` passed all 36 tests, including composer
visibility at 900x600 and 1440x900. The initial exact short-name filter selected
zero tests and was corrected; it is not counted as validation. Optional signing
found no certificate, but the fresh executable ran. File-scoped rustfmt and
`git diff --check` passed.
Live read-only GitHub inspection now finds child PR #762 OPEN, mergedAt null,
with no checks reported. Its description explicitly records unfinished
current-main session-persistence integration and missing desktop PNG/MP4 review.
GitHub main matches local origin/main at fff47841e66bf86767db6a0bbf756208ed56881e;
that version explicitly reports image generation/import as unavailable.
Thus checking current main does not provide an integrated fallback. Parent
acceptance awaits the assigned child implementation and integrated evidence.
No branches, commits, pushes, PR mutations, or Maddox operations were performed.
Visual evidence: unavailable in this recheck; headless assertions do not prove
the required native desktop generation/import workflow.

### 2026-09-08 - Task 529: packaged Web AudioStream bindings

- Restore the eight public streaming imports and five AudioVoice imports, including the `stasis_jit_audio_play` failure reported by Gambit Guard. Stream pushes own copied stereo PCM, report bounded partial acceptance, and reject unavailable devices. Existing effect voice behavior is shared by the public aliases.
- Validation: all 119 Web runtime tests, all 15 Web packaging tests, 31 provenance/installer tests, native C11 ring tests, 797 runtime ABI comparisons, 953 host/runtime comparisons, Cargo formatting, and diff checks passed. The repository-wide Bash entrypoint could not start in this Windows shell (`dirname` and `python3` missing); the applicable checks were run directly.
- Chrome 152 acceptance: `build/audio-browser/receipt.json` binds seven rendered-PCM captures covering gesture, volume, mute, swap/reopen, lifecycle resume, device recovery, and reload. Stereo ratio error was zero, peaks were 0.5/0.25/0 as expected, and the signal was 480 Hz. The optimized package also passed startup and reload in `build/audio-browser-release/receipt.json`, retaining all thirteen imports.
- Native JIT evidence: `build/audio-native/receipt.json` records 96,000 stereo frames at 48 kHz, 479.72 Hz measured frequency, RMS 0.4731, and stereo ratio RMS error below 3e-8, with compiler/runtime/fixture/capture SHA-256 values.
- Provenance: local packages truthfully identify development/local_release and dirty source; these are not official release artifacts. SDL 3.4.10 and SDL_image 3.4.4 archives matched the repository's pinned SHA-256 before fresh native builds. Worker publication must ship the matching compiler/runtime/stdlib through the normal release workflow, then consumers can repin with official checksum provenance. No consumer snapshot was patched.
- Follow-ups: platform conformance can reuse the fixture and receipts; Android physical listening is separate. Marble Run and Gambit Guard still need their normal rebuild/deployment acceptance after the official release.
- Visual evidence: not applicable to this audio-only repair; rendered PCM and receipts provide the behavioral evidence. No physical listening is claimed.
- Theory gained: public extern names must survive both Wasm reachability and JavaScript feature pruning. Real optimized-package instantiation and PCM capture demonstrate this boundary; adjacent public audio APIs should extend the same import fixture rather than add consumer aliases.
- Good: real browser PCM capture caught the complete import-to-output path, including optimized packaging.
- Bad: inherited runtime and signing environment settings initially selected an unrelated binary or unavailable optional signer.
- Adjustment: validate with explicit matching local compiler/runtime paths and distinguish development evidence from official release provenance.
- Standalone native AOT: fresh desktop development packaging succeeded with optional signing unconfigured. `build/audio-aot/receipt.json` records 192,512 output frames, 190,656 nonzero frames, peak 0.5, zero stereo ratio error, and 479.98 Hz through the SDL disk driver; executable SHA-256 is `04345638e6907eaed7d4b15e81cdc583adb74d696acd22a93a4ea6eeabb0d0d4`. The bounded four-second probe is preserved in `build/probe_audio_aot.py`.

## 2026-09-08 - Task 312 recovery

- Reconciled the existing URL action with available main, preserving Android permissions and both URL/network lifecycle hooks. Updated the source-closure assertion to accept the added platform service source.
- Fresh CLI/runtime artifacts, SHA-256 values, focused validation, and baseline limits are recorded in `docs/task312_recovery_evidence.md`. The headless consumer probe returned 312 as expected. Publication remains worker-owned.
- Visual evidence: not applicable; no consumer UI was added and no device/browser media is claimed.
- Theory gained: independent host capabilities require additive lifecycle cleanup; the merged hooks preserve one-shot URL authority alongside network suspension.
- Good: fresh builds exposed the stale source adjacency assertion.
- Bad: optional signing initially skipped AOT execution.
- Adjustment: verify that executable checks actually execute, and check source sets rather than ordering.

### Task 529 merge repair

- Preserved both AudioStream and task 312 recovery reports when resolving the prepared main merge.
- Updated the audio test event mock to retain all listeners, preserving audio visibility checks alongside the new external URL listener. All 124 Web runtime tests pass.
- Visual evidence: not applicable; this repair changes report text and test infrastructure only.
- Theory gained: independent browser features share event types; test dispatch must retain every listener just as the DOM does.

### Task 529 runtime-selection review repair

- Require the selected native runtime to be the compiler's canonical sibling, then verify both binary paths and SHA-256 values through editor-info before recording. Reject changed binaries before publishing a receipt.
- Added six runtime-selection regressions to PR CI and repository validation. All 26 combined provenance and selection tests passed; diff checks passed.
- The retained target/debug compiler lacks a verified build fingerprint. The live probe correctly rejected it before creating an evidence directory; no new native audio capture is claimed for this review repair.
- Visual evidence: not applicable; this change validates evidence provenance, not graphical behavior.
- Theory gained: environment overrides cannot prove runtime selection when bundle siblings take precedence; evidence must verify the loader-selected pair.

### Task 532 receiver-owned named-struct arrays

- Route indexed receiver field reads and writes through the existing typed collection helpers using the receiver's storage handle. Fixed-capacity bounds traps remain ahead of access; unsupported whole-element and non-scalar operations fail deterministically.
- Cover Bone[24] scalar lanes, dynamic boundary reads/writes, nested calls, two independent owners, data-flow summaries, state layout, capacity invalidation, and hot-swap preservation/rejection. Wasm validates each owner candidate's scalar lane layout.
- Focused validation: 7 native seam/state tests and 29 receiver-related compiler tests passed, including linked native AOT and Node/Wasm execution. Fresh stasis_dynload artifacts were built. Formatting and source-layout checks passed.
- Downstream integration: temporarily tested the four rig2d seam/source files from b44dd6c0 in this worktree. Both rig2d_jit_aot_seam tests passed, including linked AOT result parity and all 11 Stasis cases; temporary source files were removed.
- Visual evidence: not applicable; this change has no graphical behavior.
- Theory gained: a receiver carries a stable owner path hash; appending the collection field selects the same flattened scalar lanes as concrete global access. The two-owner tests support this mapping, and capacity-only edits must re-emit unchanged accessors because their bounds are compiled constants.
- Good: shared typed helpers preserve scalar behavior without another parser path.
- Bad: inherited optional signer configuration failed despite unsigned local execution being supported; overlapping validation also exposed a Windows DLL lock.
- Adjustment: run validation serially with only applicable optional tool settings, and require actual linked executable results.
- Repository validation coverage passed in bounded runs: tools/validate_repo.sh preflight and all application targets, then `cargo test -p stasis_compiler --all-targets` (including 618 unit tests), then `cargo test --workspace --exclude stasis --exclude stasis_compiler --all-targets`, each Cargo invocation through tools/cargo_cache.py. Final Rust formatting, Stasis source-layout, and diff checks passed; no lingering test processes remained.
- Validation setup: fresh source-fingerprinted CLI/SDL runtime, MSVC environment and explicit linker, absolute worktree Cargo target, and checksum-verified local SDL archives for release builds. Optional unavailable signing settings were isolated in the test process. The real Git index retained an older unformatted probe; the compiler formatter gate passed against an isolated candidate index/object directory containing the corrected working copy. The worker's real index was preserved.

### Task 532 compiler benchmark review repair

- Restored the reporting-only compiler microbenchmark's original ignored attribute. Added a narrow validator exception for that named benchmark and documented its explicit `--ignored` invocation; ordinary correctness tests remain mandatory.
- Validation: exact default selection reports one ignored test; explicit `--ignored` execution passes. Repository ignore audit and rejection probes, shell syntax, Rust formatting, and diff checks pass.
- Visual evidence: not applicable; test scheduling only.
- Theory gained: correctness gates and timing reports have different execution contracts; the default skip and explicit successful run verify that separation.
- Good: the benchmark remains available without ordinary CI cost. Bad: the blanket ignore audit conflicted with its intended opt-in status. Adjustment: keep the exception restricted to this named reporting benchmark.

### PR 774 approved-model acceptance review

- Validate each task turn's transport `resolved_model` against the workspace allowlist and report every distinct observed model separately from the configured selection. Approved fallback models pass; missing evidence or an unapproved later turn fails acceptance.
- Validation: 8 focused harness tests, all 150 Windows desktop editor tests, and 67 ABI/host/roadmap/cache-policy tests passed. Rust formatting and diff checks passed. The repository script stopped on an existing evidence-file permissions error; its ABI check passed with a fresh report path (801 comparisons). Host execution resolved the desktop suite's evidence-file permissions failure. Local tests used the supported unsigned configuration because the optional development certificate was unavailable.
- The credentialed OpenRouter live run remains unverified because no credential was configured.
- Visual evidence: not applicable to this report-validation fix; no new graphical behavior.
- Theory gained: configured model selection is request intent; task-scoped transport usage records describe execution across fallback and repair turns. Mixed-model and unapproved-later-turn tests verify this distinction.

## 2026-09-23 - #639 Web physical text sampling

- Fixed Web text sampling for nonuniform and >8x backing transforms; fractional Canvas rounding now preserves the logical baseline through the atlas mapping. Added focused static/dynamic/fallback cache and restoration tests plus a reproducible real-browser capture harness.
- Validation: all 210 Web tests, Chrome host-renderer acceptance, Node syntax and diff checks passed. The required repository baseline is blocked by a pre-existing unsafe-boundary violation in `crates/stasis_compiler/tests/sprite_run_writer_public_seam.rs`.
- Visual evidence: inspected `docs/evidence/task639-web-text/before.png` and `after.png`; text is sharper with identical logical bounds/baselines and no visible clipping for the captured strings. This is real Canvas2D/WebGL2 with a synthetic guest ABI and SwiftShader, not packaged-game or physical-device qualification.
- Cross-platform #639 remains Active. `docs/validation/text_physical_raster_639.md` records native/Workshop cap and rounding gaps and unavailable Apple/physical-phone qualification. #547 retains consumer migration and desktop lifecycle acceptance.
- Theory gained: text sampling must cover both actual backing axes independently of the bounded scalar sprite tier; unchanged text scale can safely reuse its resource when only the sprite tier changes.


## 2026-09-23 - Restore baseline before Maddox #701

- Moved the Windows sprite writer seam's DLL registry bootstrap and raw storage reads into an owning, audited `stasis_dynload::AotProbeSession`. Typed descriptors come from compiler `StateLayout`; snapshots copy registered lanes while both DLLs remain loaded. Occupied registries are rejected and session storage is cleared before unloading.
- Kept unsafe compiler-test exemptions forbidden. Added coverage for occupied state preservation, foreign-thread literal mutations, teardown/reopening, malformed descriptors and mismatched snapshot types; retained linked JIT/AOT packet assertions.
- Restored the local installer's missing cJSON and replay-consumer source entries to match release provenance and actual runtime build inputs.
- Kept generated Windows replay bridges freestanding under `/X`: a bounded byte-copy helper handles signed-integer and floating-point snapshot bit copies without `<string.h>`. Snapshot forward declarations now carry the same export attribute as their later definitions. Mixed-lane and integer-only bridge tests compile the emitted C, and the no-default-library-directive check remains enforced.
- Restored recording argument validation before installed-runtime provenance preflight; valid requests still pass through the unchanged provenance gate. The existing pinned-vendor regression covers invalid bounds without runtime startup.
- Settled the desktop input fixture with one complete initial host frame before logical-coordinate injection, preserving the first guest tick and exact coordinate assertions.
- Aligned the manifest asset fixture with the documented sibling-first runtime contract and kept its configured-path precedence check; direct graphics and JIT asset calls now use the same selected DLL.
- Restored the generated mobile integration harness source list with the replay-consumer and cJSON compilation units already used by the canonical CMake mobile targets.
- Validation: focused sprite JIT/AOT/session, installer, freestanding bridge, recording, desktop input/assets, and generated mobile checks pass; all 334 application library and 481 CLI unit tests pass. Final formatting and diff checks pass. The exact repository gate reached its 900-second bound in Windows launch tests and did not reach remaining compiler workspace targets. A bounded single-test rerun identified ignored `samples/windows_launch_smoke/dist` artifacts copied by the fixture as the launch failure: packaging correctly rejected an already-existing output directory. No artifact cleanup, additional repair, push, or PR was performed; full validation remains unpassed.
- Visual evidence: not applicable; these changes repair native test boundaries and build inputs.
- Theory gained: a separately loaded runtime DLL has a separate storage registry. Retaining both DLLs and clearing that registry before unloading avoids exporting borrowed host-registry lifetimes into it. The linked seam and session lifecycle assertions exercise that mapping; additional exported storage lanes should extend the same typed descriptor path.
- Good: independent review found and closed a literal-table ownership race before publication.
- Bad: the original unsafe audit hid further installer and freestanding-bridge baseline failures; shared Cargo artifacts also retained a deleted worktree path.
- Adjustment: verify the full canonical-checkout gate with fresh package build scripts, and keep freestanding generated C independent of SDK headers.

### Follow-up: source-only Windows launch fixture

- Reused the existing copied-fixture cleanup for `build`, `dist`, `target`, and `.stasis_cache` in the failed launch matrix. The original sample artifacts remain untouched. The exact launch regression now passes (171 seconds).
- The separate bounded remaining-workspace run passed compiler unit/integration coverage and all four sprite seam tests, then stopped at the existing native-window focus seam: the graphics runtime rejected its focus request. Eleven later dynload tests failed from the poisoned shared mutex (41 passed, 12 failed). No further source repairs were attempted. The complete repository gate remains unpassed; its prior aggregate run exceeded 900 seconds.
- Visual evidence: not applicable; fixture isolation change, with existing pixel assertions exercised by the launch test.
- Theory gained: copying a developer sample must exclude generated output before testing package creation; the package command correctly refuses pre-existing output.
- Good: the exact failed launch test now completes successfully. Bad: local generated artifacts obscured the source-only fixture assumption. Adjustment: reuse source-only fixture setup and disclose independent baseline limits.


## 2026-09-23 - #640 Web PNG physical sampling (partial)

Prepare sprite pixels from both framebuffer axes, retain actual scales above
8x, and keep sheet crops in logical source coordinates. Separate sprite sampling
invalidation preserves sufficient cached resources across smaller-axis changes.
213 Web tests pass; real Chrome before/after, 1x/2x/fractional and pixel-identical
context restoration passed. Full repo baseline stops at the unrelated missing
CLI build fingerprint in `desktop_hot_swap_generation_seam`.

Visual evidence: inspected `docs/evidence/task640-web-png/{before,after,one,two,fractional,restored}.png`;
fine detail improves, crops remain stable, and restoration matches. Native,
Workshop, Apple and physical-device acceptance remains incomplete. See
`docs/validation/png_physical_raster_640.md` for the backend audit and limits.

Theory gained: physical sampling dimensions and logical source crop dimensions
are separate invariants; unchanged destination geometry alone cannot prove crop
parity. Visual review caught the distinction and the sheet test now enforces it.


## 2026-09-23 - Maddox #689 declared packaged host exports

Added parser-owned `@host_export(name)` with ABI-v1 scalar signatures, retained
reachability, stable `stasis_host_v1_*` symbols, typed manifest provenance and C
headers. JIT, Web, Windows packaging and shared mobile/monolithic bindings use
the same declaration. Android and browser adapters initialize host state after
main and before the first tick/frame. Existing lifecycle/layout versions remain
unchanged. Live swaps reject removal or signature changes of accepted exports.

Tests first observed missing JIT symbols, missing native manifest metadata and
missing browser startup invocation. Real JIT and Wasm execution now observe the
setter; generated native AOT objects execute through the mobile C runtime and
export the stable symbol in the Windows PE table. Android bridge ordering and
ABI rejection/rollback checks pass. Runtime and host contract audits retain
810 and 975 comparisons. Focused commands are recorded in the PR.

Baseline `tools/validate_repo.sh` reached workspace integration tests then
stopped because the CLI lacked the installed runtime's build fingerprint.
Rebuilding/signing the native runtime and rebuilding the CLI with matching
explicit provenance restored the exact failing desktop hot-swap test (4/4).
The full aggregate was not rerun; final validation uses focused bounded gates.
No Android device/emulator run was performed for this scalar ABI change.

Visual evidence: not applicable.

Theory gained: a host export is a source-owned reachability and ABI contract,
while FnId/object names remain compiler implementation details. The generated
mobile executable observes a setter between main and tick, and Web/JIT observe
the same scalar lanes. An adjacent host capability setter can use the same
annotation and adapters without a game-specific compiler path.

Good: shared typed metadata drives native headers/wrappers and backend export
selection; executable tests check the boundary rather than only string output.
Bad: the baseline initially paired a freshly built CLI with mismatched installed
runtime provenance, and two test-only metadata constructors required updating.
Adjustment: establish the matching signed CLI/runtime pair before aggregate
gates and compile metadata-owning library tests early in schema changes.


## 2026-09-23 - Maddox #654 release callback reachability

Packaged AOT and release Web compilation use an explicit release reachability
policy. Development JIT/live AOT retain the zero-argument void swap callback.
Resolved FunctionIds preserve ordinary same-name calls and shared dependencies;
release snapshots, active objects, imports, string literals and asset staging
exclude the reload-only closure. Policy participates in snapshot/cache identity.
Desktop/mobile implicit swap aliases and mobile manifest/header entries are gone.
Graphics construction roots and platform resource restoration are unchanged.

Validation and platform limits are recorded in `docs/release_swap_validation.md`.
Visual evidence: inspected local `build/654-desktop-startup.png` and
`build/654-android/android_resource_restore/e/stable-frame.png`; sprites and
text render after standalone startup and Android resource lifecycle actions.
Android IT-020 also checks initial, resumed and recreated resource pixels.

Theory gained: packaging must use the same policy for compilation and asset
preflight/staging. A release compiler alone cannot remove a reload-only asset
when the no-manifest staging fallback copies the complete asset directory.
The paired final Web packages and all three mobile object targets demonstrate
that snapshot-owned asset roots remove it while retaining the shared asset.
An adjacent package target should select the policy before snapshot construction.

Good: final Wasm interfaces, AOT objects, staged assets and real hosts validate
the policy across the compiler/package boundary.
Bad: installed and sibling build DLLs initially overrode matching runtime paths;
an empty inferred asset set also initially published an unnecessary identity file.
Adjustment: stage a fresh matching CLI/runtime pair and test both inferred assets
and assetless packages before interpreting package-size or host-smoke results.


## 2026-09-23 - Maddox #654 PR CI timeout follow-up

PR #833 job 107427103256 exhausted the integration step's 15-minute budget
while Web package assertions were still passing. Compilation took 2m40s;
Web started 6m26s into the step after the earlier integrations. Web packaging
now runs in its own 15-minute step in the same required job, reusing the Cargo
build. The broad target loop excludes it so each suite still runs once.
No timeout was increased and no test was disabled.

Validation: existing Cargo/placement policy tests (19) and action-version policy
passed. `python tools/cargo_cache.py run -- cargo test -p stasis --test
web_package -- --test-threads=1` passed all 17 tests locally in 46.74s;
`git diff --check` passed. Hosted Linux timing remains for CI verification.
Visual evidence: not applicable (workflow-only follow-up).
Theory gained: the timeout budget must match the owning suite, including compile
cost; passing suites can exceed a combined step budget without a failed assertion.
Good: retained the same required job and shared compilation.
Bad: the previous combined budget hid the Web suite's independent runtime cost.
Adjustment: preserve separate bounded steps and guard single ownership in the
existing CI placement tests.

## 2026-09-24 - Maddox #639 native physical text sampling

Completed the shared native and Android Workshop portions after the Web repair.
Text now samples from the larger actual viewport axis without the sprite 8x cap,
while logical widths, bearings, baselines, and quads remain unchanged through an
exact inverse transform. Native text invalidation is independent of sprite
density. Workshop cache identity includes exact scale and font source identity;
physical RGBA bytes, entry count, GL texture limits, eviction, replacement, and
surface-loss deletion are explicitly accounted for.

Fresh Visual Studio Release native contracts passed 2/2. Fresh ARM64 and x86_64
JNI bridges were built, their provenance verified, and the Workshop debug unit
suite passed. Visual evidence: no new native or Android capture was produced;
the Windows host cannot qualify Apple, and no emulator or physical-phone pass
was run. Web Chrome visual evidence remains in
`docs/evidence/task639-web-text/`.

Theory gained: layout preservation requires the physical raster extent and its
inverse mapping to be one contract; a nominal scale alone is insufficient after
integer allocation. Resource identity must include every input that can change
those physical pixels.

Good: lifecycle, fractional-axis, >8x, GL-limit, and cache-ownership behavior is
covered at the implementation seams. Bad: this host cannot provide Apple or
physical-device evidence, and a fresh native screenshot was not captured.
Adjustment: run the same focused contracts before platform-lane visual capture,
then qualify Apple and phone font backends without weakening the shared source
guarantees.

## 2026-09-24 - Agent symbol CLI workflow

Generated game guidance now starts with a compact discovery/edit/validation loop,
uses read-sourced v2 selectors, and explains v1 batches for dependent additions.
The CLI and offline references agree on discovery scope, hash guards, dry runs,
supported text-edit exceptions, test policy, and receipt recovery.

Validation: a fresh Cargo CLI/test build and the signed test harness passed all
7 semantic-symbol tests and 2 generated-project documentation tests. All 5 JSON
request examples were also applied in fresh temporary projects with a real
`tick() == 2` regression test. Release-provenance tests (29), Android shell
guidance checks, Rust formatting, and diff checks passed.

The Windows Cargo build stages an optional graphics DLL without embedding a
matching CLI fingerprint. The first Cargo test run therefore rejected runtime
startup in two headless tests. Running the freshly built harness in supported
source-development layout (temporarily unstaging that DLL after the build)
passed; the original staged DLL was restored and its SHA-256 verified.

Visual evidence: not applicable (documentation and executable-example coverage).

## 2026-10-01 - Maddox #775 SIMD validator repair

The bounded ARM64 SIMD characterization now runs in the default compiler test
suite instead of being hidden behind `#[ignore]`. Its existing default remains
one compile repetition across 12 cases, with five-tick JIT correctness checks,
JIT/AOT CLIF capture, and AOT object inspection. The explicit measurement tool
still raises repetitions, retains artifacts, and adds NDK disassembly.

Validation: the focused compiler test passed 12 characterization cases in 1.08
seconds; the exact ignored-test policy audit passed; the Python wrapper compiled;
and a fresh one-repetition measurement produced
`D:\code\.automation-evidence\nightly-20261001\775\simd-smoke\analysis.json`.
Visual evidence: not applicable (compiler validation policy only).

Theory gained: compiler-owned semantic and code-generation characterization can
run as a bounded correctness test; only external NDK analysis and repeated
sampling belong in the explicit wrapper. A future characterization can reuse
this split without suppressing its correctness oracle.

Good: the repair removed the policy mismatch without changing SIMD semantics or
expanding the ignore allowlist. Bad: the measurement test had mixed required
correctness with opt-in repetition, which hid both from normal validation.
Adjustment: keep the one-repetition correctness core default-on and add sampling
only in the artifact-producing wrapper.

## 2026-10-01 - Maddox #773 allocation-free numeric text

Added checked decimal formatting into caller-owned `ascii[N]` storage for the
full `i32` range and every finite binary32 value at fixed precision `0..6`.
Replace, append, and left-pad operations preflight capacity including the NUL,
return the resulting length or `-1`, and leave the complete header and payload
unchanged on rejection. Fixed formatting uses exact binary32 quantization,
halfway-away-from-zero rounding, and unsigned output for rounded negative zero.
The implementation has one fixed local digit array and no host formatter,
allocator, runtime ABI, or `i64` surface.

At the source freeze, the independent `BigUint` JIT corpus passed across every
normal exponent with selected significands, signs, and precisions, plus numeric
boundaries, tie neighbors, powers of ten, and deterministic raw-bit samples.
Literal byte and NUL self-checks also passed in JIT, a linked native AOT
executable, and executable Wasm. Native objects had no formatting or allocator
imports, and the Wasm module had no imports. The checked-in Stasis behavior
suite and 115 Android receipt/source-contract tests passed. Fresh exact-head
API 35 x86_64 execution, arm64 package/link verification, and the complete
bounded repository gate are recorded externally after the reviewed freeze
commit because pre-freeze packaging correctly refuses an unverified build
identity.

Visual evidence: pending at the source freeze. The formatter itself is a byte
contract; the exact-head API 35 run will capture the sample's green receipt
frame alongside its literal 98-case device log. No desktop visual change is
expected.

Theory gained: exact decimal output does not require a host formatting ABI when
the binary32 mantissa and exponent are reduced into a bounded decimal digit
vector. Backend parity is strongest when each executable compares literal
bytes and mobile reads the registered typed storage rather than inferring
behavior from compilation.

Good: one pure-stdlib implementation serves every backend, while independent
oracles cover arithmetic, transactionality, capacity, and ownership. Bad:
compile-only mobile evidence and a prefix-only log regex could hide byte
differences or trailing corruption. Adjustment: execute literal checks on each
backend, validate the typed Android header/NUL/full log line, and keep arm64
link evidence distinct from x86_64 execution evidence.

## 2026-10-01 - Maddox #787 stable LSP worker admission

Replaced the deprecated atomic conditional update in LSP live-request worker
admission with a private weak compare-exchange loop. The existing 64-worker cap,
successful `AcqRel` publication, `Acquire` observations, backpressure response,
and both `fetch_sub(AcqRel)` release paths remain unchanged.

Validation at source freeze: the unchanged source reproduced the Rust 1.99.0
warnings-denied deprecation in `cargo build -p stasis` while stable Rust 1.92.0
passed the same command. After the repair, that exact build passed under both
toolchains. Four boundary/contention admission tests passed, including one
128-thread race with exactly 64 successful reservations, the existing real
backpressure-path test passed, and all 35 `stasis_lsp` library tests passed
serially under both Rust versions. The pre-edit repository validator exposed a
stale native-runtime sibling; after building and verifying one matching source,
release, and fingerprint tuple, its exact failed display-metrics target passed
1/1. Final commit-head artifact identity and bounded full target coverage remained
pending at this source-freeze checkpoint.

Visual evidence: not applicable (private LSP concurrency and compiler-version
compatibility only).

Theory gained: a small explicit compare-exchange loop can preserve the exact
linearization point and ordering of a convenience atomic update while spanning
stable compiler versions on both sides of an API rename.

Good: boundary, overfull, real-path, and concurrent-cap behavior now have direct
oracles. Bad: the first broad baseline loaded a stale native DLL, obscuring an
otherwise unrelated gate. Adjustment: verify one source/release/fingerprint
tuple before native integration tests and keep compatibility checks explicit for
both the existing local and current stable toolchains.

## 2026-10-01 - Maddox #789 cross-thread launch-console minimization

Changed the visible console root-owner request in runtime/stasis_graphics.c from SW_MINIMIZE to SW_FORCEMINIMIZE. The console frontend is owned by a different UI thread; the forced show state is the Win32 operation intended for minimizing a window owned by another thread. The one-time guard, STASIS_CONSOLE_START_MINIMIZED opt-out, visible-window condition, ConPTY iconify fallback, and SDL game-window path are unchanged. No ABI, test, CMake, workflow, signing, or global desktop configuration changed.

Validation at the source-freeze checkpoint: a fresh build of accepted baseline 62bc5dc45f26abaedbfc52a6c1a3deecaaf7bd6c reproduced the default-mode stage-4 console failure. The direct console contract failed at stage 4; the configured pair passed sprite reservation and failed the console contract (1/2). After the one-call change, the direct four-mode contract passed. An unisolated local configured run also recorded a hidden stage-7 SW_RESTORE failure when a separate Windows Terminal root-owner stayed iconic through the existing one-second wait; that receipt is retained and does not establish a general product cause. A subsequent controlled configured run passed 2/2 under the independently reviewed owned-console isolation guard, which acted only on two proven test-owned frontend UUIDs and left no owned frontends or processes. This local isolation result is bounded evidence, not a claim about general Windows Terminal behavior.

The render-parity play on the dirty candidate exited 0, and the portable second-frame verifier passed with frame 2 lifecycle evidence, three sprites, SDL backend, and trace 1523793427. The inspected 640x360 capture at [frame.png](D:/code/.automation-evidence/nightly-20261001/789/dirty-candidate/parity/frame.png) shows the full test scene: checked sprites, overlapping circles, central bar, both diagonal lines, text bands, and guide marks without cropping or blank output. This capture demonstrates that the game still renders; the existing four-mode HWND-state contract is the evidence for console minimization.

Dirty-candidate provenance: source HEAD 62bc5dc45f26abaedbfc52a6c1a3deecaaf7bd6c; release candidate-789-dirty-62bc5dc45f26; fingerprint 451d5fd171815e48aa3476d2c5fe42e7b6f82d1305ce8d031782b80bd1f98971. Only runtime/stasis_graphics.c was dirty, SHA-256 A2F1C2DC6AF4A5998DEC9CA160DF30F809BE7988CCB16C7ABE2000C68DB15E83. The source and effective runtime hashes matched at A065AF6CF11662501E3FFC49FEA8BB3CF471AF7C5D0CDA15B74054180F218899; direct editor-info reported the same source, release, and fingerprint. The runner manifest contained both expected per-monitor DPI tags. See the [dirty-candidate parity receipt](D:/code/.automation-evidence/nightly-20261001/789/dirty-candidate/parity/dirty-candidate-parity-summary.md) and [post-implementation six-persona review](D:/code/.automation-evidence/nightly-20261001/789/postimplementation-review.md).

The clean-baseline monolithic tools/validate_repo.sh attempt timed out with exit 124 after 1052.97 seconds because its wrapper exceeded the limit while draining after timeout; it is incomplete and receives no full-validation credit. Its pre-Cargo source/policy checks and Cargo metadata succeeded, identifying 68 target entries: 66 selectable and two custom-build targets. Final validation is pending on the immutable commit: use the exact pre-Cargo commands and complete metadata-mapped coverage through 20 bounded serial shards, then the planned platform and hosted gates. No literal whole-shell validator pass is claimed. The hosted Windows console/bootstrap gate must pass twice on fresh executions.

Visual evidence: inspected the fresh parity frame described above. It is intentionally limited to the game rendering; it cannot prove the console state transition.

Theory gained: the selected console root-owner belongs to an external frontend thread, so the ordinary minimize request was not reliable for this cross-thread target. The force-minimize request preserves the synchronous startup behavior while leaving the separate ConPTY request path intact.

Good: the production patch changes one show-state constant and keeps the existing behavioral oracle unchanged. Bad: the accepted baseline reproduced the visible-console failure, and the unisolated local restore attempt exposed a separate frontend interaction; broad final validation and hosted repetitions are still pending. Adjustment: retain the one-time, opt-out, hidden, and ConPTY paths without retries or timeout changes; complete fresh exact-commit serial coverage and both hosted repetitions before treating the repair as accepted.

## 2026-10-06 - Maddox #625 isolate Windows presentation test instrumentation

Recovered the focused three-file fix from the recovery-only stash without applying or dropping the stash. The nightly Windows lane builds a separate poison-instrumented DLL and routes only its two poison-dependent recovery tests to it. Ordinary desktop and mobile consumers retain the ordinary runtime. The workflow audits separate paths, compile flags, five test-only exports, distinct hashes, and matching release/fingerprint compatibility. Public ABI v4 and production runtime behavior are unchanged by this follow-up.

Refreshed PR #909 against main while preserving main's six qualification/receipt files, genuine historical stale vendor fixture, strict vendor identity checks, and coherent v2 schemas. Presentation qualification remains intact. The explicitly rejected guest-owned redraw proposal #528 remains draft.

Validation: fresh ordinary configure/build took 33.78/44.83 seconds; ordinary CTest passed in 6.38 seconds. Poison configure/build passed in 3.07/3.04 seconds using the freshly built dependency tree. The exact Windows DesktopSdl suite passed all six targets in 162.11 seconds, including all four recovery tests and 48 consistent hot-swap frames. Python fixtures passed 84 tests, runtime ABI passed 961 comparisons, and Web tests passed 234/234. Cargo formatting, three workflow actionlint checks, PowerShell parsing, and plain diff checks passed. All 2,971 frozen tracked inputs and both DLL hashes remained unchanged after validation; no test processes remain.

Limits: the initial broad repository validator failed with a stale/mismatched runtime and does not receive a pass. The first focused suite failed on an ignored CLI-sibling DLL from nightly 331; its bytes were preserved outside the checkout before staging the fresh matching ordinary runtime and rerunning the complete suite successfully. A scratch CMake long-path attempt and direct Python import-path attempt also failed before their corrected executions. This is focused Windows/Python/Web evidence, not a new complete repository validator or iOS simulator pass. Actual iOS simulator evidence and an official post-fix SDK release remain task #625 completion gates.

Visual evidence: inspected D:/code/.automation-evidence/stash625-integration/windows/present-only-physical-poison.png (red logical canvas within fully initialized black physical margins) and it-009-recovered-valid-frame.png (valid scene recovered after malformed submissions). These stills do not establish motion or lifecycle timing.

Theory gained: test instrumentation changes binary identity while preserving source compatibility identity. Distinct paths, compile flags, exports, and hashes distinguish variants; a matching release/fingerprint lets both exercise the same loader contract. A CLI-sibling runtime takes priority over an environment path, so fresh native validation must provision the matching sibling as well.

## 2026-10-07 - Maddox #832 scalar math and short f64 text

Added the shared f32/f64 scalar math API, with signed-zero, NaN, infinity,
power-domain, and rounding behavior, plus its current tolerance bounds. Decimal
float literals default to f32, so callers use explicitly typed f64 locals when
they need wider overloads. Integer powers use a bounded 32-step repeated-square
path; floating powers with negative bases require integral exponents. The
sample's added f32/f64 math and short-text assertions run before the unchanged
13-value f32 raw-bit digest `-1430176193`.

Added `ascii_write_f64_short`, `ascii_from_f64_short`, and
`ascii_append_f64_short`, including precision `0..6`, magnitude-based fixed or
scientific notation, carry behavior, lowercase specials, unsigned signed-zero
output, and full-capacity transactional rejection. It stages digits as local
integers, then writes to the destination after capacity preflight, preserving
the caller-owned buffer contract.

Focused validation: `python tools/cargo_cache.py run -- cargo test -p stasis --test stasis_behavior_suite -- --test-threads=1 --nocapture` passed 22 math and 7 numeric-text assertions in 48.645 seconds. The recorded receipt is `D:\code\.automation-evidence\nightly-20261007\math832\focused-final\run-format-refactor\result.json`. The source snapshot reviewed here had `src/stdlib/math.stasis` SHA-256 `801FA3722E0E91815095795F60914393A45F06E9B132B7910CC71EC6887B0327` and `src/stdlib/stdlib.stasis` SHA-256 `09FCBBFD89EECA84BF50E9320DBE7276992E085C4CC947C05097BD91C8C203A8`.

This is bounded focused evidence. No final exact-source JIT/native AOT/Wasm/Android/iOS parity matrix, complete repository validator, hosted gate, or nightly publication pass is claimed here. The separate numerical-review artifacts at `D:\code\.automation-evidence\nightly-20261007\math832\numerical-review\review_final.md` and `review_rounding.md` are review evidence, not backend executions. Final identity-bound backend, full-gate, and hosted receipts will be recorded with the reviewed PR and Maddox ledger when they are available.

The desktop presentation prerequisite was also rechecked: its recovery seam waits for matching physical, drawable, native, and readback state across three continuous observations spanning 500 ms, with a five-second bound; existing pixel and counter assertions remain. The exact four-test target and five fresh whole-target processes passed. This prerequisite result does not establish math parity.

Visual evidence: inspected `D:\code\.automation-evidence\nightly-20261007\math832\post-fix-five-full-recovery-fresh\run-5\artifacts\seam-tests\present-only-physical-poison.png`. It shows the red logical rectangle with black physical margins and no magenta contamination for the desktop presentation prerequisite; it is not a visual test of scalar math.

Theory gained: the base argument selects the power result lane, while explicit local types make f64 intent visible before overload resolution. Relative error alone can misstate subnormal behavior, so the shared oracle pairs it with an absolute minimum-subnormal allowance and a separate zero/subnormal classification check.

Good: one shared Stasis oracle covers both precision lanes and exact ASCII output before the retained digest, while focused evidence has a concrete bounded receipt. Bad: focused host evidence and numerical review do not yet prove the final artifact on every backend. Adjustment: finish the source/vendor freeze, then record exact-source backend receipts separately and keep the focused result clearly bounded.

The packaged mobile check exposed a separate runtime parity gap in indexed text
views. An unchanged Android sample returned 21; a temporary diagnostic narrowed
the failure to comparison with the literal `1.00e-300`. The destination had
length 9, a NUL terminator, and first byte 49, while the literal's first indexed
byte read as zero. Desktop indexed loads already resolve unbound literal hashes;
the mobile loader omitted that lookup. The repair gives registered integer,
byte, and UTF-16 arrays priority, including out-of-range reads, and resolves
literal bytes only for an unbound field-zero view. It adds no ABI export, layout,
capacity, or format version.

The regression first failed through both the native runtime contract and a
generated Stasis executable linked to the real mobile runtime. The generated
fixture checks valid ASCII and UTF-8 indices; native contract checks cover
unsigned bytes, field selection, bounds, and registered-array collisions.
Language-level bounds traps remain separate from the native accessor's zero
result. Temporary diagnostic instrumentation is removed from the production
sample. Final mobile, merge, and subsequent official-nightly receipts remain
external acceptance gates recorded with PR #917 and the task ledger.

Theory gained: a text-view argument carries a hash that can name either an
array or a literal. Runtime array registration must win even when an index is
outside that array; only an unbound view may resolve the literal table.

Good: controlled original-versus-diagnostic Android runs localized the failure
before changing math kernels. Bad: host parity checks did not expose the mobile
literal lookup omission. Adjustment: retain a generated executable regression
for indexed literal arguments and require packaged mobile acceptance before
merging this slice.

## 2026-10-07 - Maddox #833 browser ad lifecycle and portal profiles

Added typed ad-task and portal-lifecycle modules, native/JIT/AOT/mobile unavailable
stubs, Web request/poll/release handling, four portal profiles, deterministic
upload ZIPs, and a no-SDK default. Browser requests remain asynchronous: the
frame pump and guest polling continue while the guest blocks simulation. Rewards
require provider-specific proof and are consumed once. The direct Web package
selects one portal SDK; native desktop/mobile LAN guest bundles remain plain Web.

Validation before the final user-facing message tweak: the full Web Node suite
passed 279/279 and the focused ad lifecycle suite passed 34/34. After the tweak,
the focused audio/runtime suite passed 20/20. Native unavailable
coverage passed 1/1 each for the dynload shim, JIT execution, linked Windows AOT
executable, and generated mobile AOT runtime seam. The fresh Windows CLI build
was `082f3ff3a66daa5062253ad6cd56f276eb2f0187e631d07d583802e2ab320b63`;
fresh `none` and all four portal development packages completed with exit 0 and
left the installed CLI unchanged. The packaged Wasm hash was
`dd5e37a5ef52e5bef119323d131cec89e0a3e0f421dc2bd171bfa4ed620982fa`.
The friendly adapter-import failure message was then checked with a new CLI
build (`fc1903518e03d51d2367ba1d2692d9d8441bf9f4a5964b5f1decd65dbcf1eb0a`), a
fresh GameMonetize package, and a new Chrome 404 run.

Real Chrome 155 CDP acceptance used deterministic SDK fakes intercepted at the
selected provider's actual script request; no SDK globals were preloaded. The
`none` build requested no SDK and returned `Unavailable` with no handle. All
four selected profiles made exactly one selected SDK request. CrazyGames and
Poki reported start/terminal callbacks and lifecycle boundaries; GameMonetize
reward requests were explicitly unavailable; GameDistribution's mouse and
390x844 Chrome touch-emulation paths both issued `showAd` inside a trusted
pointer-up while user activation was active. The GD browser run proved that a
promise without `SDK_REWARDED_WATCH_COMPLETE` grants zero, while proof followed
by a rejected promise remains consumable exactly once. Audio acceptance used a
real `AudioContext`: it resumed from the trusted gesture, suspended on actual
ad start, resumed on completion, and preserved an independently suspended
context. Requesting/playing frames held the simulation counter steady while
poll/render counters advanced; completion and no-fill resumed without catch-up
or replaying a held key. The optional CDP hidden/visible transition was skipped;
visibility lifecycle behavior has Node coverage, but this run makes no claim of
real-browser visibility-transition acceptance.

Separate real-browser failure runs returned to a playable state with
`mainResult=0`, guest `Unavailable`, no open handle, no gameplay block, and
continuing simulation/rendering when (a) the selected GameMonetize SDK script
failed to load and (b) the actual dynamic `ad_lifecycle.js` request returned
HTTP 404. Both paths executed no SDK break. The primary-source notes and product
docs state the user-selected GD policy: the global reward event belongs to the
latest issued rewarded call until its owner is released/reset/disposed/timed
out or a newer rewarded call is issued. The event has no documented ID, order,
or delay bound, so a late completion from an older call can be attributed to a
newer one; no vendor correlation guarantee is claimed. GameMonetize rewarded
remains unsupported.

The complete `tools/validate_repo.sh` attempt at the recorded baseline reached
its 900-second process-tree limit and is incomplete, not a pass. Bounded
remaining coverage shards passed, but exact-final-source validation still must
run after the focused commit. Actions was confirmed disabled, so no hosted
workflow was started. No live portal account, ad fill, submission approval,
publication, or supported release was tested; the nightly dependency is still
blocked by #832. The unreleased branch therefore does not establish downstream
#834 readiness.

Visual evidence: PNGs and actual timestamped Chrome screencast MP4s were captured
for the full matrix under
`D:\code\.automation-evidence\nightly-20261007\task833\browser-acceptance`.
Captured stage frames include `none-noaudio-mouse-JX1dhZ/frame-02.png`,
`crazygames-audio-mouse-K1aRpa/frame-02.png`, `frame-03.png`, and `frame-06.png`,
`gamemonetize-noaudio-mouse-ckoMeE/frame-02.png`, `frame-03.png`, and
`frame-06.png`, `gamedistribution-noaudio-mouse-cgtifK/frame-02.png`,
`frame-04.png`, and `frame-08.png`, the corresponding touch stages in
`gamedistribution-noaudio-touch-UhtQdP`, `poki-noaudio-mouse-84bLaz/frame-02.png`,
`frame-03.png`, and `frame-06.png`, SDK-failure `frame-02.png` in
`gamemonetize-noaudio-mouse-m5pfwB`, and module-failure `frame-02.png` in both
`gamemonetize-noaudio-mouse-7P1xsY` and the friendly-message recheck
`gamemonetize-noaudio-mouse-oJlSxV`. Each run directory contains `browser.mp4`,
encoded from real screencast timestamps; durations range from 0.65 to 17.31
seconds. Root directly inspected representative none, CrazyGames, GameDistribution
desktop/touch, SDK-failure, and pre-fix module-failure frames/MP4 samples; the
browser owner inspected the refreshed friendly-message frame. The remaining
captured media is available but is not claimed as individually reviewed. SDK
fakes prove local host behavior only, not live vendor ad delivery.

Theory gained: request acceptance, physical ad start, provider resume, and reward
proof are independent observations. A host can keep the browser frame pump
running while Stasis blocks only simulation, and a reward token should be
consumed only after explicit provider evidence. Untagged global callbacks with
no correlation contract set a real cross-request attribution limit that local
generation counters cannot remove.

Good: focused cross-backend contracts, four fresh packages, real browser
pointer/touch/audio paths, failure recovery, and visible motion evidence are
recorded. Bad: the full repository validator, hosted checks, portal approval,
and a published release are still outstanding. Adjustment: refresh artifact
identity from the focused commit, run final bounded validation, and keep #834
blocked until the supported release and payload verification exist.

## 2026-10-08 - Workshop render budgets (#836)

Set the configured Android Workshop render ceilings to P50 4.0 ms and P95
14.0 ms in the reusable device seam workflow and its contract assertions.
Nightly acceptance reuses that workflow. The measured baseline remains
historical evidence; sampling, retries, comparison and other budgets stay intact.
Validation: the task-specified Android emulator, PR seam placement and Android
release-shell Python contract suites; git diff --check.
Visual evidence: not applicable (configuration only).
Theory gained: the reusable device workflow owns the CI budget; documented
baseline measurements do not define the user-selected acceptance limits.

## 2026-10-08 - Maddox #815 standalone desktop adapter contract

Added a real CLI regression for an adapter-enabled v3 desktop project whose
guest exposes only `main()`. Against the frozen pre-fix source, the exact test
showed `package --target desktop --development-build` returning success and
publishing `dist/standalone_adapter_probe-desktop` while silently ignoring the
adapter. With the guard, the named test passes by asserting the exact
engine-callback diagnostic, no published adapter output, and success for the
adapter-free standalone baseline.

The implementation guard uses the existing engine-mode predicate: an
adapter-enabled desktop AOT build requires both a zero-argument `tick()` and
`render`. It runs before choosing the engine or standalone packaging branch.
Web and mobile packaging enter their target-specific flows before this desktop
AOT path. No visual runtime behavior changes.

Visual evidence: not applicable; this slice rejects an invalid desktop package
before publication and does not alter rendered output.

Theory gained: desktop adapter registration is meaningful only when the guest
provides the engine callbacks the adapter pumps. A successful standalone
package with a declared adapter is therefore a contract violation, while an
adapter-free standalone package must remain publishable.

Good: the real source-built CLI now rejects the incompatible package and keeps
the plain-standalone path working.
Bad: the first Windows adapter acceptance run exposed that its test harness
copied the stamped CLI without its required sibling graphics runtime, before
the first package could be generated.
Adjustment: stage the freshly built matching runtime beside the isolated CLI;
the two-generation lifecycle acceptance is pending on the frozen source.

## 2026-10-08 - Maddox #784 bounded two-bone IK

Added `Rig2D.solve_two_bone()` for a direct upper/lower chain. The lower bone's
local translation supplies the upper-link vector; its local X axis supplies the
virtual lower link. The solver validates the cached chain and inputs, projects
targets to the reachable annulus, applies the upper local-angle limit, recomputes
the elbow from its preserved offset, then aims and constrains the lower angle.
Invalid input leaves the rig unchanged. The Stasis behavior suite now covers
direction/radius/bend sweeps, nested and rotated parents, non-axis offsets,
reach boundaries, equal-link root targets, seam/wide arcs, post-clamp aiming,
and atomic rejection. The same deterministic sweep runs as a JIT, linked-AOT,
and Node-Wasm oracle.

The RED seam first failed because the public method did not exist. The solver
snapshots only the scalar FK and local fields it needs, then defers its two
local-angle writes until validation and calculations pass.

Validation: signed-required `python tools/cargo_cache.py run -- cargo test
--package stasis_compiler --test rig2d_jit_aot_seam -- --test-threads=1`; 3/3
Rust tests passed, including all 21 Stasis behavior cases and JIT/linked-AOT/
Node-Wasm oracle parity. The existing MSVC 14.44 linker and Rust sysroot
`gcc-ld` were added to PATH only for the test process. Documentation and
formatting checks remain part of final review.

Visual evidence: not applicable; this is renderer-independent pose math with no
user-visible graphics change.

Theory gained: a child's parent-relative offset is the geometric upper link,
while its frame angle controls a virtual distal X axis. The `(3, 4)` offset sweep
confirmed that compensating for the offset bearing preserves endpoint reach
under rotated roots and nested parents. This predicts that any translated or
rotated parent hierarchy should behave identically when the caller runs FK
before and after the helper.

Good: analytic reach, bend, and limit behavior now stays inside the existing
stdlib FK representation, and one oracle executes in all three backends. Tiny
positive links remain supported while a zero upper-link offset is rejected
atomically; both joints are checked across the limit sweep.
Bad: one limit-sweep call failed frontend resolution when target arithmetic and
trig calls were passed inline, although the same call resolves with equivalent
local arguments. The first AOT attempt also lacked an inherited linker PATH.
Adjustment: snapshot only the needed scalar fields and defer writes; assign
target coordinates to local `f32` values before that call without changing
compiler resolution behavior; and add the installed MSVC and Rust LLD
directories only to the bounded test process.

## 2026-10-08 - Maddox #815 desktop package provenance receipt

Fixed the official macOS package verification failure from release run
37804747943. The current CLI emits a `native_adapter` key in the desktop
package receipt, including `null` when no adapter is configured; the verifier
previously accepted only the two-key legacy receipt. It now accepts the exact
legacy or current root shape, preserves legacy receipts only when the packaged
manifest has no adapter, and verifies configured adapter ABI, normalized
source, digest syntax, and system links against the packaged manifest. The
manifest's system-link field remains optional; current non-null receipts
require the producer's system-link object, including `{}` when no links are
configured. Source normalization follows the validated desktop target's path
dialect and the Rust producer's serializer. The verifier does not dereference
the metadata source or claim to recompute its digest because the native C
source is not present in the final package.

Validation: the current-shape RED is preserved at
`D:\code\.automation-evidence\nightly-20261008-resumed\task815\verifier-repair\red-current-null.log`;
the focused desktop receipt tests pass 2/2, the complete release provenance
module passes 36 tests, and the exact PR-CI Python step passes 60 tests. The
full `tools/validate_repo.sh` gate and a fresh official cross-platform nightly
remain pending; the prior release failure is not treated as a release pass.

Visual evidence: not applicable; this changes release metadata verification,
not rendered output.

Theory gained: the receipt's adapter source is serialized metadata, and
backslashes in a valid POSIX filename can become slash-looking text after the
producer's normalization. The producer also always emits the system-link
object, while its empty platform lists and the manifest's default field are
omitted. Validate path semantics on the manifest value, then compare the
serialized receipt value exactly instead of interpreting it as a second
filesystem path.

Good: current and legacy receipts are both checked against the packaged
manifest, while ABI, hash, required system-link object, link enums, duplicates,
and capacity remain fail-closed.
Bad: the existing verifier rejected every current receipt before checking its
contents; the first path attempt also rejected valid POSIX backslash metadata.
Adjustment: validate the new root field and metadata contract explicitly,
require a validated target for configured adapters, and keep source digests
syntax-only when the packaged source is unavailable.

## 2026-10-08 - MaddoxTasks #268 browser playground

Implemented bounded virtual-project compilation in the browser compiler, an editor
with shared-lexer highlighting and compiler-backed completion, production Web-host
preview, transactional same-layout state-preserving swaps, safe PNG/SVG import, and
static ZIP export. Compiler analysis has isolated buffers, UTF-16 ranges, qualified
type identity, incomplete-draft recovery, and no retained symbols from deleted files.
The original ABI v1 fixed-sample request remains compatible. StaticWasm owns the
StasisLang.com route and its existing Cloudflare Git deployment integration.

Validation: compiler library 19/19; focused editor/runtime Node 12/12; existing Web
runtime 283/283; fresh wasm32 release core; fixed-sample Chrome smoke; standalone
Chrome acceptance (109 frames); site-wrapper acceptance (111 frames). Browser
acceptance covers completion keyboard insertion, canonical state/hook execution,
all rollback cases, overlapping sessions, PNG/SVG rendering, independent exported
WebGL2 execution, and zero application-server requests after readiness. Formatting,
syntax, action-version policy, and diff checks pass. Independent review findings
for session publication, qualified types, enum variants, and keywords were fixed.

The local full validation gate exposed an installer staging omission (fixed by
PR #922), an audio-stripping regression (fixed and exact owning test passes), and
stale native test artifacts. Fresh private native artifacts passed the display
seam; the hot-swap seam requires a matched CLI/runtime fingerprint. The final
matched native gate remains a separate pre-push check, not a claimed pass here.

Visual evidence: inspected `D:/code/.automation-evidence/task268/site-reviewed-editor/`
`syntax-highlighting-autocomplete.png`, `uploaded-asset-game.png`, and
`exported-asset-game.png`; decoded and inspected a frame from
`playground-acceptance.mp4`. They show the embedded editor, signature suggestions,
the running production renderer, and both imported images in the exported game.

Theory gained: canonical state migration and host resource lifetime are separate;
same-layout swaps preserve user bytes but must not rerun resource initialization.
Good: candidates publish only after restore and an effect-free hook succeed.
Bad: replacing a mounted iframe resets its browsing context, and matching bare
type names loses imported-module identity.
Adjustment: retain the winning mounted frame, use request tokens for publication,
and resolve completion types by their defining file as well as their name.

The full workspace gate exposed two Web packaging regressions in the playground
host: its import allowlist retained stripped input names, and its collection-view
ABI variable no longer matched the immutable boot contract. Input names now carry
the existing import-stripping markers, and compatible swaps retain the boot-time
collection ABI. The owning Web package tests remain the acceptance oracle; they
are rerun before push rather than weakening their expectations.

Theory gained: optional host imports must share the packager's feature boundaries,
while a compatible state swap cannot change the collection ABI. Good: packaging
tests caught both regressions. Bad: browser-only validation missed stripped
production packages. Adjustment: include the owning package suite in this slice.
Visual evidence: the final staged-site editor and image captures under
`D:/code/.automation-evidence/task268/site-final-git-build-passed/` were inspected;
the browser receipt records successful completion, swaps, assets, export, and zero
HTTP application requests after readiness.

## 2026-10-09: direct UTF-8 integer conversion and Pong receiver APIs (#838)

Pong formats each score directly into one bounded UTF-8 scratch buffer with
`buffer.from_i32(value)`. ASCII and UTF-8 share the concise method name through
normal typed overloads; their buffer representations remain distinct for call
arguments, typed lets, assignments, and returns. Explicit `from_ascii` copies
ASCII into UTF-8. Scalar numeric conversion statements retain their existing
fallback behavior. Legacy function-form aliases remain available.

The direct formatter uses negative-domain arithmetic for `i32::MIN`, checks
capacity before writes, and synchronizes the terminator and byte/character
lengths. Receiver normalization now uses the semantic walker before graph and
effect analysis so imported methods on global/nested buffers remain reachable.
Generated overload names already include receiver/parameter types; no ranking
rule or text-specific naming mechanism was added. No host ABI changed.

Validation: exact Linux core Cargo gate passed 1,246 tests across 44 suites,
plus the focused diagnostic-order regression. Final numeric integration passed
9/9 on Linux and Windows host, including linked native AOT and executable Wasm;
the fresh CLI passed 8 numeric-text tests. Node playground/publication tests
passed 24/24. ABI and host-runtime contract audits passed 964 and 1,150
comparisons; 69 tooling/policy tests passed. Fresh Wasm compiler bundle SHA-256:
`0b526b4fee3115dfb175e0bd5e69c3ada3dec092320ea9f7b13accfefc68420f`.
Full Chrome acceptance passed scoring, restart, hot swap, rollback, asset import,
ZIP export, and exported-game boot, with zero failures or post-readiness requests.
All six independent review personas returned GREEN. The user requested
consolidation into existing Pong PR #926 rather than a stacked PR.

Visual evidence: inspected starter, game-over, restart, and exported Pong PNGs
under `D:/code/.automation-evidence/nightly-20261009/task838-browser/`, plus decoded
frames from `pong-multistep-export.mp4` (H.264, 1440x960, 211 frames, 10.55 seconds).
They show font scores, the CPU's five-point win/restart message, restarted play,
and the exported game's sprites and scores.

Theory gained: source overload selection and generated symbol identity are
separate stages. Distinct text-family compatibility selects the right overload;
normalizing receiver calls before graph analysis preserves their executable
bodies. Adjacent receiver APIs should reuse that path rather than infer identity
from method spelling.
Good: executable backend tests caught both reachability and text-layout mistakes.
Bad: an initially broad Wasm argument guard rejected existing numeric widening.
Adjustment: validate the specific representation boundary before encoding and
retain the established contextual encoder for numeric conversions.
