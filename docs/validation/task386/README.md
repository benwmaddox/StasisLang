# Task 386: downstream compatibility sweep

Date: 2026-09-10. **Historical sweep complete: all observed code failures are linked to existing defect tasks below.** No compiler or game source changes were made.

## Inventory and isolation

Discovery scanned `D:/code` using `rg --files --hidden -g stasis.json`, excluding Git internals, node_modules, and target outputs. It found 5,942 manifests, 11 canonical game identities, 154 duplicate repository checkouts, and 5,777 embedded/generated/experimental exclusions. `inventory.json` records every observed manifest disposition and canonical commit. This is a scan of the local code root, not every filesystem on the machine.

The nine expected games are included; Afterlight and ToddlerMatch are additional canonical games. StasisRiggingPrototype is excluded because its README identifies a standalone experiment and its Git repository has no HEAD or remote. Canonical selection prefers the shortest root matching the Git remote name. Remote URLs are normalized across HTTPS/SSH and .git suffixes.

Clean committed source snapshots were exported with `git archive` into `artifacts/task386/<game>` under the supplied worktree. No branches or Git worktrees were created. Dirty canonical contents were never copied or modified. Afterlight and RootbeerMaze3 had pre-existing tracked changes; their HEAD snapshots were tested. `checkout-audit.json` confirms unchanged tracked-status listings for all 11 canonical checkouts.

## Toolchain

Primary: **nightly-20260909-299**, source `d9898edeb0e025e87994c1302459688095120dcd`, Windows archive SHA-256 `4102fde1dac301b004e1ca7155a50946201cf116ee53561609fce633d669d3f3`, compiler/runtime fingerprint `016947e34ac0efb2e0d95ae294902630d52f2d8858f0129c10b94258c81fba98`.

Downloaded archive hashes match official GitHub release asset digests. `editor-info` verifies matching compiler and graphics-runtime identities. `git merge-base --is-ancestor` confirms merge `62caccdb89f82a9165f89e9d6f1b29ec208ef451` is present. Full metadata, comparisons, and hashes are in `release*.json`, `editor-info*.json`, and `toolchain-verification.json`. No compiler/runtime was installed inside a game project; shared task toolchains live alongside the snapshots.

Nightly 270 was tested first, but current games already depend on APIs introduced afterward. Its results remain at this directory root. The immediately prior published nightly 290 was used for source-check comparisons (`nightly290/`). The primary matrix uses 299 throughout; a successful prior check is not counted as a primary pass. Final isolated vendor snapshots were restored to 299 and inspected with `vendor status` (`vendor-status.json`).

## Primary matrix

Values are command exit codes. Native BannerfallTactics build includes the successful Ninja retry; its initial default-generator failure remains in the raw matrix. Runtime is a 120-tick headless run; BannerfallTactics additionally has a native frame-120 screenshot smoke.

| Project | Format | Check | Test | Native build | Runtime | Prior 290 check | Defect task |
|---|---:|---:|---:|---:|---:|---:|---|
| Afterlight | 0 | 1 | 1 | 1 | 1 | 1 | #549 |
| BannerfallTactics | 0 | 0 | 0 | 0 | 0 | not run | - |
| ChessTD | 1 | 1 | 1 | 1 | 1 | 0 | #480, #558 |
| Exterminator | 0 | 1 | 1 | 1 | 1 | 1 | #550, #559 |
| HamsterHavenTycoon | 0 | 1 | 1 | 1 | 1 | 0 | #551 |
| maddox-and-friends | 0 | 1 | 1 | 1 | 1 | 0 | #552 |
| maddox-marble-run | 0 | 1 | 1 | 1 | 1 | 0 | #553 |
| RobotTournaments | 0 | 1 | 1 | 1 | 1 | 1 | #554 |
| RootbeerMaze3 | 0 | 1 | 1 | 1 | 1 | 1 | #555 |
| SheepHerder | 0 | 1 | 1 | 1 | 1 | 0 | #556 |
| ToddlerMatch | 0 | 1 | 0 | 1 | 1 | 0 | #557 |

`nightly299/matrix.json` contains exact command arguments, working directories, exits, elapsed seconds, and raw log paths. Each command was bounded to 300 seconds; none reached that limit. The native Ninja retry was bounded to 600 seconds and the native executable smoke to 60 seconds. Afterlight's documented build script and Maddox & Friends' multi-suite wrapper were also run and failed at their existing source/test gate (`documented-commands.json`). Other specialized asset, Web, Android, and device validations were not run after their source prerequisites failed.

## Failure ownership and follow-up

Ten primary source checks fail. Six (ChessTD, HamsterHavenTycoon, maddox-and-friends, maddox-marble-run, SheepHerder, ToddlerMatch) pass on 290 and fail on 299: these are demonstrated source-compatibility regressions between those releases, not proven regressions introduced by merge #613. The graphics API diff replaces `SpriteSheet.handle: i32` with `sprite_ref: SpriteRef`; Maddox & Friends is rejected for array `.length` rather than `.max_length`. These migrations may be intentional; the evidence does not establish an unintended compiler implementation bug. Drafts are assigned to each affected game for migration/diagnosis rather than assuming one fix handles every project.

Afterlight calls removed audio functions; Exterminator calls private graphics identifiers; RobotTournaments calls the old input API; RootbeerMaze3 collides with the stdlib `asset_tasks` module alias. These also fail on 290 and are classified as game compatibility defects pending deeper diagnosis. Exterminator additionally lacks the native asset manifest on 270, independently of its newer graphics failure. ChessTD has a separate formatting gate failure. No stale test fixture has been proven.

`defect-requests.json` maps all 12 reproductions to existing Maddox task sequence numbers and stable issue IDs. `linked-defects.json` records their read-only state on 2026-09-11: #480 and #549 through #559. Eleven are children of #386; #480 is the existing ChessTD prerequisite. No task mutations were performed. Reproductions retain the exact failing project commit and first diagnostic; they do not claim standalone compiler minimization. Additional diagnostics may emerge after these gates are repaired.


## Native and visual evidence

Default Visual Studio CMake discovery failed to identify C/C++ compilers. The documented `CMAKE_GENERATOR=Ninja Multi-Config`, installed Ninja PATH, and two-worker fallback succeeded after allowing declared dependency downloads outside the network sandbox. This was an environment/tooling limitation resolved during the sweep, not an outstanding source defect. The first sandboxed download attempt was stopped with its process tree before retrying.

BannerfallTactics produced a fresh 2,673,432-byte AOT Windows executable, SHA-256 `087302acd0d0f333a90e883fb0a810bd6d21c63dc838725224e6181ccea52c6c`. `build-fallback.json` records the successful 59.078-second build; `native-smoke.json` records a 3.172-second native run exiting 0. Its build summary names the correct `main` entry and game source functions; the asset package records its manifest hash.

Visual evidence: inspected `nightly299/bannerfall-native.png`; it shows the native rendered tactics board, units, round/turn HUD, and controls at frame 120. This establishes startup rendering only, not input-driven gameplay, animation quality, or full visual approval. No visual evidence is available for the ten games that failed source checks.

## Confidence and remaining work

Compatibility confidence is limited: one of eleven games passes the primary source/native/startup lanes. Six failures appear after nightly 290, so this sweep cannot attribute them to #613. The linked tasks own repair and further diagnosis. Their completion states do not change this historical matrix; validating newer game commits is a separate rerun. Web/mobile packaging, physical devices, sustained gameplay, network play, and hot-swap behavior were not exercised. This task did not introduce or repair product behavior.

Theory gained: a post-merge toolchain ancestry check alone does not establish current game compatibility. Observed passes on 290 and failures on 299 show that later public API changes matter; another game using raw SpriteSheet handles is likely to need the same API migration.

## Retained-work verification (2026-09-11)

Verified the 12 existing task identities through the released MaddoxTasks read-only `agent issues` command. Reinspected the retained PNG. The ignored `artifacts/task386` snapshots/toolchains are no longer present; commands and outputs remain in this report, so no historical executable was rerun. Canonical checkout preservation is supported by the original `checkout-audit.json`, not a claim that unrelated checkouts have remained unchanged since September 10.

The original process audit recorded zero Stasis processes at sweep completion. The resume audit found PID 14488 at D:/code/StasisLang/bin/stasis.exe, outside this supplied worktree; it was preserved as an unrelated process. No process was launched by this resume pass. A machine-wide zero-process claim is therefore not made for September 11.
