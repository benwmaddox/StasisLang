# Build Checklist

This checklist is the implementation plan and is aligned with:
- `docs/spec.md`
- `docs/live-compilation-prd.md`
- `docs/android_workshop_prd.md`

Status note:
- This repository's stable compiler is implemented in Rust (`cargo build`, `cargo test`).

Locked decisions:
- Entrypoint is `function main(): i32`.
- Reachability-DCE roots are `main`, `tick`, and `on_code_swap` (when present), plus host-exported required entry symbols.
- Initial host externs are `print_i32` and `print_string`.
- Function-form calls remain supported indefinitely (receiver-form still preferred).
- Runtime boundary is host-set-based and deny-by-default: Stasis can only access extern symbols exported by the selected host set.
- Call dispatch policy: debug/hot-swap mode keeps indirect dispatch (`FnId -> code_ptr`); release/AOT mode can lower direct call edges where compatibility gates allow.
- Backend modes are:
- Cranelift JIT for development/watch/hot-swap runtime
- Cranelift AOT for production builds
- Android workshop v1 is sideload-first and uses symbol-based editing over normal `.stasis` files.
- Android workshop Git v1 uses GitHub API commit/push/PR flow; full local git can come later.
- Android workshop preview rendering will be selected by least architectural friction, starting near the existing Stasis runtime unless Android-native preview integration is clearly better.

## Language Ownership Legend

`Rust` is the compiler and runtime implementation.

- `Rust`: compiler implementation (frontend + lowering + Cranelift), host app/runtime boundary, platform integration, process/watch plumbing, and host-set/phase enforcement.
- `.stasis`: user code, stdlib, and samples.

Boundary rule:
- Keep the compiler single-source-of-truth: do not reintroduce parallel compiler implementations that diverge in semantics.

## Execution Rules

1. Build in feature slices only. Each slice must be shippable.
2. Every slice requires tests in the same PR.
3. When compiler behavior can be tested in `.stasis`, prefer `.stasis` tests over Rust-host reimplementation tests.
4. Remove dead code and temporary paths before slice completion.
5. Update docs in the same PR when behavior changes.
6. Preserve deterministic tick-based behavior.
7. No ambient host API paths; each new host interaction must ship with explicit host-set contract docs/tests.
8. Test command budget is strict: no single test command should exceed 5 minutes (300 seconds); split/shard test runs if needed and treat overruns as stability issues.

## Tooling Note

Current canonical workspace commands:
- `cargo build`
- `cargo test`

Release AOT optimization:
- Default AOT opt level is `speed_and_size` (release-friendly).
- Override with `STASIS_AOT_OPT_LEVEL`: `none` | `speed` | `speed_and_size`.

Historical bootstrap/self-host notes below are archival only and do not describe an active compiler track.

## Slice Plan
### Android Workshop Track

#### AW0 - Product and Syntax Decisions
- Language: `docs`.
- Scope: Lock sideload-first distribution, eventual full Android workshop direction, flexible preview surface decision, GitHub API v1 Git workflow, and Stasis-style syntax examples.
- Deliverable: `docs/android_workshop_prd.md` is the canonical Android workshop product/editor requirements document.
- Tests: Documentation-only slice; verify the workspace still compiles before implementation slices land.
- Done gate: No Android workshop examples use Rust-style `fn`/reference syntax for Stasis source.
- Status: `completed`

#### AW1 - Symbol Tree and Source Span Index
- Language: `Rust`.
- Scope: Expose Android editor-facing symbol metadata for lifecycle functions, receiver-owned struct functions, root utilities, and system files.
- Deliverable: The compiler frontend can map editable symbols to `.stasis` files and source spans.
- Tests: Deterministic unit tests over the Android workshop example layout.
- Done gate: Symbol tree groups match `Main`, `Structs`, `Systems`, and `Root`.
- Status: `completed`

#### AW2 - AI Patch Contract and Source Replacement
- Language: `Rust`.
- Scope: Add serializable AI request/response contract structs and apply validated symbol edits back to `.stasis` source spans.
- Deliverable: `replace_function` edits can update selected symbols while preserving normal files on disk.
- Tests: Contract serialization and function-span replacement tests.
- Done gate: Patch contract prefers receiver-style owner metadata and rejects mismatched symbol/file targets.
- Status: `completed`

#### AW3 - Reload Classification for Android UX
- Language: `Rust`.
- Scope: Add Android-facing reload classifications for changed symbol batches: `FastReload` for function-body-only changes and `ResetRequired` for layout/signature changes.
- Deliverable: Compiler/editor can explain reload expectations before or after a patch.
- Tests: Function-body edit and struct-layout edit tests.
- Done gate: Classification reason strings identify changed layout/signature facts.
- Status: `completed`

#### AW4 - GitHub API Change Summary Model
- Language: `Rust`.
- Scope: Add symbol-first change summary DTOs for Android GitHub API commit/push/PR review.
- Deliverable: Changed symbols are summarized before changed files, with raw file diffs as advanced review data.
- Tests: Summary ordering and grouping tests.
- Done gate: `Player`-owned edits group under `Player`; file list contains affected `.stasis` files.
- Status: `completed`


#### AW5 - Project Source Loader
- Language: `Rust`.
- Scope: Load Android workshop projects from a project root plus entry `.stasis` file, recursively following project-local imports into normalized editor paths.
- Deliverable: Android editor APIs can build symbol trees, AI requests, reload classifications, and Git summaries from a complete entry-file import closure.
- Tests: Import-closure loading, normalized path ordering, and missing-import diagnostics.
- Done gate: Unused `.stasis` files outside the import closure are not loaded for symbol editing.
- Status: `completed`

#### AW6 - Project-Wide Symbol Edit Application
- Language: `Rust`.
- Scope: Apply approved AI/editor symbol edits across a loaded Android workshop project, including both `replace_function` and `replace_struct` edits.
- Deliverable: Android can update normal `.stasis` files by symbol without hand-editing file text, then classify reload expectations from the before/after project sources.
- Tests: Multi-file project edit application, struct replacement, function replacement, and wrong-target rejection.
- Done gate: Struct edits can force `ResetRequired` via the existing reload classifier.
- Status: `completed`

#### AW7 - Symbol Placement Planner
- Language: `Rust`.
- Scope: Apply Android workshop placement rules for new/moved symbols: structs, lifecycle functions, receiver-owned functions, struct constructors, root utilities, and system functions.
- Deliverable: Android can choose the correct `.stasis` file before creating a symbol or requesting an AI edit.
- Tests: Lifecycle/root placement, struct-owned receiver placement, struct-return constructor placement, system placement, and struct file placement.
- Done gate: Placement results match the Android Workshop PRD function placement rules.
- Status: `completed`

#### AW8 - Android Phone Smoke Shell
- Language: `Android Java + C + Gradle + docs`.
- Scope: Add the first checked-in Android app shell under `mobile/android` with an arm64-only native JNI smoke library.
- Deliverable: Developers with Android SDK/NDK/JDK/Gradle can build and install a debug app that loads native code and displays a status string on a phone.
- Tests: Structural Android shell verifier covering manifest, Gradle config, native CMake, JNI entrypoint, and arm64 ABI selection.
- Done gate: The shell is installable in principle without linking Stasis runtime or game code yet.
- Status: `completed`

#### AW9 - Android Bundled Workshop Surface
- Language: `Android Java + .stasis assets + docs`.
- Scope: Replace the smoke-only screen with a native Android workshop surface backed by bundled Stasis-style project files.
- Deliverable: The installed app can show Main, Structs, Systems, and Root symbol groups and display selected symbol source from normal `.stasis` files.
- Tests: Structural Android shell verifier covers bundled assets, Stasis syntax rules, symbol-browser code path, and smoke JNI retention; debug APK builds and installs on a paired phone.
- Done gate: A sideloaded app shows a real workshop project surface without hard-coding Rust-style examples.
- Status: `completed`

#### AW10 - Android In-Memory Symbol Edit Flow
- Language: `Android Java + docs`.
- Scope: Add phone-runnable selected-symbol editing controls before durable file persistence and compiler-backed hot reload.
- Deliverable: The Android app can edit the selected symbol source in an `EditText`, apply it in memory, reset the editor, and show `FastReload` versus `ResetRequired` expectations from symbol kind/signature checks.
- Tests: Structural Android shell verifier covers the editor controls and reload classification strings; debug APK builds and installs on a paired phone.
- Done gate: Symbol editing is available from the sideloaded app without exposing raw file editing as the primary workflow.
- Status: `completed`

#### AW11 - Android Source Editor Keyboard Handling
- Language: `Android Java + manifest + docs`.
- Scope: Keep the selected-symbol editor usable when the soft keyboard opens on a phone.
- Deliverable: The Android activity requests keyboard resize behavior and scrolls the focused source editor into view after IME focus.
- Tests: Structural Android shell verifier covers `adjustResize`, fill-viewport scrolling, editor focus handling, and smooth scroll targeting; debug APK builds and installs on a paired phone.
- Done gate: Tapping into the source editor no longer leaves the active text area hidden under the keyboard.
- Status: `completed`

#### AW12 - Android Source Editor Bottom Spacer
- Language: `Android Java + docs`.
- Scope: Add a direct fallback for phones where soft-keyboard resize does not keep the source editor visible.
- Deliverable: The Android scroll content includes fixed trailing space below the editor controls so users can scroll active source above the keyboard.
- Tests: Structural Android shell verifier covers the keyboard spacer and spacer height; debug APK builds and installs on a paired phone.
- Done gate: Tapping into the source editor has enough trailing scroll room to manually position text above the keyboard.
- Status: `completed`

#### AW13 - Android App-Private Stasis File Persistence
- Language: `Android Java + docs`.
- Scope: Move the phone editor from bundled-asset display to app-private `.stasis` files that survive process restarts.
- Deliverable: On first launch, bundled sample files are seeded into `getFilesDir()/workshop_project`; selected-symbol Apply replaces the symbol span in the matching `.stasis` file and writes it back to disk.
- Tests: Structural Android shell verifier covers app-private project root, first-launch seeding, text file read/write helpers, and selected-symbol persistence; debug APK builds.
- Done gate: Android edits are symbol-based while the backing project remains normal `.stasis` files on disk.
- Status: `completed`

#### AW14 - Android Native Compile Probe
- Language: `Android Java + C JNI + docs`.
- Scope: Establish the first native compile bridge from saved app-private `.stasis` files without linking the Rust compiler/runtime yet.
- Deliverable: After Apply saves a symbol edit, Java calls `nativeCompileProject(projectRoot)`; the native probe recursively scans readable `.stasis` files and returns deterministic `CompileNotLinked` diagnostics with file and byte counts.
- Tests: Structural Android shell verifier covers the Java native method, post-Apply invocation, JNI entrypoint, recursive `.stasis` scan helper, and `CompileNotLinked` result string; debug APK builds.
- Done gate: Android has a tested native compile-call surface that can be replaced with the real compiler bridge.
- Status: `completed`
#### AW15 - Android Native Stasis Compile Check
- Language: `Android C JNI + docs`.
- Scope: Upgrade the native compile bridge from file scanning to a deterministic Android-side source check pass.
- Deliverable: `nativeCompileProject` reads `.stasis` files, validates comments/strings/braces, counts functions/structs/globals, checks `main` and `tick` lifecycle roots, and returns `CompileChecked` or `CompileError` diagnostics.
- Tests: Structural Android shell verifier covers the native analysis helpers and success/error result strings; debug APK builds.
- Done gate: Android can execute a native compile-check step over saved project files without linking the full compiler yet.
- Status: `completed`
#### AW16 - Android Native Compile Manifest
- Language: `Android C JNI + docs`.
- Scope: Give the Android native compile path a concrete deterministic output artifact before real codegen is linked.
- Deliverable: Successful native compile checks write `build/native_compile_manifest.txt` under the app-private project root with status, project hash, declaration counts, byte counts, and lifecycle roots, then return `CompilePlanned` diagnostics to Java.
- Tests: Structural Android shell verifier covers manifest path, manifest writer, project hash output, and `CompilePlanned` result string; debug APK builds.
- Done gate: Android compile has a persistent native output artifact that later compiler/linker stages can consume or replace.
- Status: `completed`
#### AW17 - Android Function Compile Manifest Entries
- Language: `Android C JNI + docs`.
- Scope: Add function-level compile artifact metadata for later dirty-function and hot-reload decisions.
- Deliverable: The native compile manifest includes one `function=` entry per Stasis function with source path, signature text, signature hash, and body hash.
- Tests: Structural Android shell verifier covers function manifest writer, project recursion, signature hash, and body hash markers; debug APK builds.
- Done gate: Android compile output can distinguish signature changes from body-only changes at function granularity.
- Status: `completed`
#### AW18 - Android Function Compile Stub Artifacts
- Language: `Android C JNI + docs`.
- Scope: Give each function-level compile entry a concrete output artifact location before real machine code is emitted.
- Deliverable: Successful native compile planning writes `build/functions/<body_hash>.stub` files containing source path, signature, signature hash, and body hash, and links each function manifest entry to its stub artifact.
- Tests: Structural Android shell verifier covers artifact directory, stub writer, `CompiledStub` marker, and manifest artifact references; debug APK builds.
- Done gate: Android compile output has per-function artifact files keyed by body hash for later replacement with real compiled code.
- Status: `completed`
#### AW19 - Android Manual Compile Control
- Language: `Android Java + docs`.
- Scope: Make the on-device native compile path runnable without requiring a symbol edit.
- Deliverable: The Android editor controls include a `Compile` button that calls `nativeCompileProject(projectRoot)` and displays the returned compile diagnostics.
- Tests: Structural Android shell verifier covers the compile control and Java compile runner; debug APK builds.
- Done gate: Users can explicitly run the Android compile path against saved app-private `.stasis` files.
- Status: `completed`
#### AW20 - Android Native Reload Classification
- Language: `Android C JNI + docs`.
- Scope: Make native compile planning compare against the previous app-private manifest and report the expected reload class.
- Deliverable: Successful native compile planning reads the prior `build/native_compile_manifest.txt` when present, writes `reload=<classification>` into the new manifest, and returns `CompilePlanned: reload=...` diagnostics for `InitialCompile`, `NoChange`, `FastReload`, and `ResetRequired` paths.
- Tests: Structural Android shell verifier covers previous-manifest parsing, reload classifier strings, manifest reload output, and compile diagnostics; debug APK builds.
- Done gate: Android compile planning now exposes the stateful reload decision needed before replacing compiled stubs with real runtime code.
- Status: `completed`
#### AW21 - Android Runtime State Artifact
- Language: `Android C JNI + docs`.
- Scope: Give successful Android compile planning a concrete runtime state file and entrypoint table before the run UI is wired in.
- Deliverable: The native compile manifest now records `entrypoint=main`, `entrypoint=tick`, optional `entrypoint=on_code_swap`, and `runtime_state=build/runtime_state.txt`; the state artifact initializes on `InitialCompile`/`ResetRequired` and is preserved for `NoChange`/`FastReload`.
- Tests: Structural Android shell verifier covers the runtime-state path, entrypoint manifest lines, state-ready marker, compile result state path, and runtime-state writer; debug APK builds.
- Done gate: Android compile planning now produces the state artifact the next on-device run/preview control can consume.
- Status: `completed`
#### AW22 - Android Run Tick Control
- Language: `Android Java + C JNI + docs`.
- Scope: Add the first on-device run control that consumes Android compile artifacts.
- Deliverable: The Android UI now has a `Run Tick` button wired to `nativeRunTick(projectRoot)`; native code requires `build/runtime_state.txt`, increments `tick_count`, persists it, and returns `RunTick` or `RunError` diagnostics.
- Tests: Structural Android shell verifier covers the Java native binding, Run Tick button, native JNI entrypoint, runtime state tick readers/writers, and run diagnostics; debug APK builds.
- Done gate: A sideloaded Android app can compile the bundled project and then run a visible native tick loop placeholder against app-private state.
- Status: `completed`
#### AW23 - Android Preview Tick Surface
- Language: `Android Java + docs`.
- Scope: Add a first native Android game preview surface that responds to the Run Tick path.
- Deliverable: `MainActivity` now includes a custom `GamePreviewView` that draws a simple arcade scene and updates from parsed `RunTick: tick_count=...` diagnostics after the native runtime state advances.
- Tests: Structural Android shell verifier covers the preview view, Canvas drawing path, tick parser, and Run Tick preview update; debug APK builds.
- Done gate: A sideloaded Android app now has an editor, compile control, run-tick control, and visible game preview placeholder on one screen.
- Status: `completed`
#### AW24 - Compiler-Owned Android Compile Plan
- Language: `Rust compiler frontend + docs`.
- Scope: Stop Android compile planning from growing as a parallel C compiler path.
- Deliverable: `stasis_compiler::frontend::workshop` now exposes `build_android_workshop_compile_plan`, which maps `IncrementalCompilerHost` output back to Android workshop symbols, entrypoints, function hashes, artifact paths, and reload classifications using compiler-owned metadata plus workshop layout fingerprints.
- Tests: Focused Rust tests compile sample workshop projects through `IncrementalCompilerHost`, build Android compile plans, verify function metadata/artifact paths, and classify `FastReload` versus `ResetRequired`; Android shell verifier and debug APK build continue to pass.
- Done gate: The next JNI slice has a Rust compiler-owned contract to call instead of expanding the native C scaffold.
- Status: `completed`
#### AW25 - Compiler-Owned Android Artifact Rendering
- Language: `Rust compiler frontend + docs`.
- Scope: Move Android manifest, runtime-state, and function-stub artifact contents into the compiler-owned workshop contract.
- Deliverable: `stasis_compiler::frontend::workshop` now exposes `render_android_workshop_artifacts`, producing `build/native_compile_manifest.txt`, optional `build/runtime_state.txt`, and per-function `CompiledStub` artifact text from `AndroidWorkshopCompilePlan`.
- Tests: Focused Rust tests render artifacts from real `IncrementalCompilerHost` output and verify manifest entrypoints, reload strings, runtime-state reset/preserve behavior, and function stub content; Android shell verifier covers the compiler-owned artifact API; debug APK builds.
- Done gate: JNI can switch from generating Android compile artifacts in C to writing compiler-rendered artifact text.
- Status: `completed`
#### AW26 - Rust Android Compiler Bridge Crate
- Language: `Rust compiler bridge + docs`.
- Scope: Start replacing Android C compile planning with a Rust bridge that calls the existing compiler/workshop APIs.
- Deliverable: Added `crates/stasis_android_bridge` as a workspace crate with `rlib`/`cdylib` outputs, a safe `compile_android_workshop_project` API, and C ABI functions that load a workshop project, run `IncrementalCompilerHost`, build the compiler-owned Android compile plan, and write compiler-rendered manifest/runtime/function artifacts.
- Tests: `cargo test -p stasis_android_bridge` covers artifact writing, fast-reload runtime-state preservation, and the C ABI compile message; Android shell verifier covers workspace/crate wiring and bridge API references; debug APK builds.
- Done gate: Android now has a tested Rust bridge crate that reuses compiler structure and can replace the native C scaffold in the JNI layer.
- Status: `completed`
#### AW27 - Optional Rust Bridge JNI Loader
- Language: `Android C JNI + CMake + docs`.
- Scope: Start routing Android `nativeCompileProject` to the Rust compiler bridge without breaking the current C fallback build.
- Deliverable: The JNI compile path now attempts to `dlopen("libstasis_android_bridge.so")`, calls `stasis_android_bridge_compile_project(projectRoot, "src/main.stasis")` when available, frees bridge strings through `stasis_android_bridge_free_string`, and falls back to the existing C scaffold when the Rust library is not packaged yet; CMake links `dl` explicitly.
- Tests: Android shell verifier covers the optional bridge loader and `dl` linkage, `cargo test -p stasis_android_bridge` keeps the bridge API valid, and debug APK builds with the fallback path.
- Done gate: The Android native layer now has an explicit compiler-bridge handoff point; the remaining step is packaging the Rust Android `.so` into the APK.
- Status: `completed`
#### AW28 - Package Rust Bridge in Android Debug APK
- Language: `PowerShell Android build + Rust + docs`.
- Scope: Build and package the Rust compiler bridge `.so` into the sideloadable Android debug APK.
- Deliverable: Added `mobile/android/build_rust_bridge.ps1`, which locates the NDK, builds `stasis_android_bridge` for `aarch64-linux-android` with the NDK clang linker, copies `libstasis_android_bridge.so` into `app/src/main/jniLibs/arm64-v8a`, and is called by `build_debug.ps1` before Gradle assembly; generated `jniLibs` output is ignored by git.
- Tests: `rustup target add aarch64-linux-android` completed locally; `build_debug.ps1` builds/copies the Rust bridge and assembles the APK; APK zip inspection confirms both `lib/arm64-v8a/libstasis_android_bridge.so` and `lib/arm64-v8a/libstasis_mobile_smoke.so` are packaged; Android shell verifier covers the packaging helper.
- Done gate: A debug APK built from the repo now contains the Rust compiler bridge library that JNI attempts to load first.
- Status: `completed`
#### AW29 - Android Game-First Preview Surface
- Language: `Android Java + docs`.
- Scope: Make the Android app run the preview by default and move symbol editing into a top-right hamburger overlay.
- Deliverable: `MainActivity` now uses a full-screen native preview root, hides the editor panel until the menu button opens it, keeps the keyboard spacer inside the overlay, and starts an automatic compile/tick loop on launch while retaining manual Compile and Run Tick controls for review.
- Tests: Structural Android shell verifier covers the full-screen `FrameLayout`, hidden editor overlay, hamburger toggle, automatic tick loop, and compile-ready state; debug APK builds.
- Done gate: A sideloaded app opens into the running preview first, with code editing available as an overlay instead of the default screen.
- Status: `completed`
#### AW30 - Android System-Bar Safe Preview Insets
- Language: `Android Java + docs`.
- Scope: Keep the game-first preview and overlay controls out of Android system bars and display cutouts.
- Deliverable: `MainActivity` colors the system bars black and applies root padding from `WindowInsets`, including display-cutout safe insets when available, so the preview, status row, hamburger button, and editor overlay are laid out inside the usable screen area.
- Tests: Structural Android shell verifier covers system bar color setup, root inset listener installation, system-window inset reads, display-cutout safe inset reads, and root padding application; debug APK builds.
- Done gate: The sideloaded preview no longer places visible UI under the bottom navigation bar or camera notch.
- Status: `completed`
#### AW31 - Android 60 FPS Runtime Tick Cadence
- Language: `Android Java + docs`.
- Scope: Make the Android preview/run loop target the product-default 60 fps cadence instead of the earlier slow smoke-test interval.
- Deliverable: `MainActivity` now uses a named 16 ms default tick interval for the automatic compile/run loop while keeping the existing placeholder preview unchanged until real runtime rendering replaces it.
- Tests: Structural Android shell verifier covers the 16 ms tick interval constant and loop scheduling call; debug APK builds.
- Done gate: The sideloaded app drives runtime ticks at the intended default cadence while runtime integration proceeds.
- Status: `completed`
#### AW32 - Android Real JIT Tick Bridge
- Language: `Rust + Android C JNI + docs`.
- Scope: Move Android `Run Tick` beyond the C runtime-state counter by invoking real compiled Stasis lifecycle functions through the existing JIT compiler structure.
- Deliverable: `JitProcess` exposes no-arg void lifecycle invocation and global i32 reads; `stasis_android_bridge` keeps a thread-local runtime session, compiles app-private `.stasis` files with `JitProcess`, runs `main()` once, runs `tick()` each frame, writes `mode=JitExecuted` runtime state, and JNI routes `nativeRunTick` through `stasis_android_bridge_run_tick` before the C fallback.
- Tests: Rust bridge tests prove `main()`/`tick()` mutate Stasis global state through JIT and the C ABI returns `JitExecuted`; Android shell verifier covers the bridge/JNI symbols; debug APK builds.
- Done gate: The sideloaded Android app's tick path executes real Stasis code when the Rust bridge is packaged.
- Mobile input note: Android example games must be playable without a hardware keyboard; sample game input should be touch-friendly before phone testing.
- Status: `completed`
#### AW33 - Android Native Stasis Test Runner
- Language: `Rust + Android Java + C JNI + docs`.
- Scope: Run real `.test.stasis` tests against the app-private workshop project through the packaged Rust compiler bridge.
- Deliverable: `nativeRunTests(projectRoot)` invokes `stasis_android_bridge_run_tests`, discovers and executes test declarations through the JIT, and reports passed/failed counts to the Android UI and AI tool contract. The editor provides a manual `Run Tests` control.
- Tests: `android_bridge_runs_bundled_stasis_tests`, the bounded desktop `stasis test --dir mobile/android/app/src/main/assets/workshop_sample/tests` command, and the Android shell verifier cover the bridge, valid Stasis test syntax, and UI control; debug APK builds and installs on a paired phone.
- Done gate: A sideloaded workshop can run bundled real Stasis tests without JSON scenario files, and test success requires at least one passing test with no failures.
- Status: `completed`
#### AW34 - Android Manual Raw-Diff Review
- Language: `Android Java + docs`.
- Scope: Complete the PRD's manual Git-review preparation flow by making raw changed-file diffs available alongside the existing symbol-first change summary.
- Deliverable: The editor's `Raw Diffs` control compares the app-private project to the bundled baseline and presents deterministic unified diff hunks for every changed `.stasis` file.
- Tests: Android shell verifier covers the review control and unified-diff formatter; debug APK compiles.
- Done gate: A manual editor user can inspect both changed symbols/files and the corresponding advanced raw file diffs without using an AI call or phone-hosted service.
- Status: `completed`
#### AW35 - Android Manual Stasis Test Editing
- Language: `Android Java + docs`.
- Scope: Make bundled and user-authored `.test.stasis` files first-class manual workshop sources instead of AI-only files.
- Deliverable: The app-private project seeds the bundled test fixture, includes project tests in the symbol tree, parses valid `test `name`(): bool` declarations into a `Tests` section, and marks saved test edits as requiring the native `Run Tests` validation. Change and raw-diff baselines read immutable bundled assets rather than current app-private files.
- Tests: Android shell verifier covers bundled test seeding, test-declaration parsing, test reload guidance, and immutable asset reads; Java sources compile.
- Done gate: A phone-only manual editor user can find, edit, review, and run real Stasis tests from the same app-private project used by the runtime.
- Status: `completed`
#### AW36 - Android Manual Stasis Test Creation
- Language: `Android Java + docs`.
- Scope: Let a phone-only manual workshop user add a behavior test without an AI request or external tooling.
- Deliverable: The editor's `New Test` control creates a uniquely named app-private `tests/manual_test_N.test.stasis` template, selects it in the Tests tree, and starts with `return false` so the user must implement and validate real behavior through `Run Tests`.
- Tests: Android shell verifier covers the control, creation path, selection helper, and intentional failing-template guidance; Java sources compile.
- Done gate: Users can create, edit, review, and run a real Stasis test entirely through manual Android workshop controls.
- Status: `completed`
#### AW37 - Android Manual Saved-Symbol Revert
- Language: `Android Java + docs`.
- Scope: Let manual workshop users safely undo a persisted change without resetting the whole app-private project.
- Deliverable: `Revert Saved` restores the selected bundled symbol source from immutable assets, refreshes the symbol tree/change review, and recompiles the project. User-created symbols report that a bundled revert is unavailable instead of deleting content implicitly.
- Tests: Android shell verifier covers the control, immutable-baseline restore path, and user-created-symbol guard; Java sources compile.
- Done gate: A manual user can distinguish discarding unsaved editor text from reverting a saved bundled symbol on disk.
- Status: `completed`
#### AW38 - Android Manual Test Deletion
- Language: `Android Java + docs`.
- Scope: Complete the manual test lifecycle without making reset-project the only way to discard a draft test.
- Deliverable: `Delete Test` removes the selected user-created test file and refreshes the symbol tree/review. Bundled tests are guarded from deletion and can instead be restored with `Revert Saved`.
- Tests: Android shell verifier covers the deletion control, user-created-test path, bundled-test guard, and completion status; Java sources compile.
- Done gate: A phone-only manual user can create, edit, run, review, revert, and discard draft Stasis tests safely.
- Status: `completed`
#### AW39 - Android Manual Root Helper Creation
- Language: `Android Java + docs`.
- Scope: Let manual workshop users add normal Stasis code according to the PRD's no-owner helper placement rule.
- Deliverable: `New Helper` creates a uniquely named `manual_helper_N` void function in `src/root.stasis`, compiles it transactionally, and selects it for editing. A compile failure restores the original root source.
- Tests: Android shell verifier covers the control, root-function template, and successful creation status; Java sources compile.
- Done gate: A manual user can add an ordinary root helper without an AI request while retaining compile safety.
- Status: `completed`
#### AW40 - Android Pull-Down Workspace Priorities
- Language: `Android Java + docs`.
- Scope: Reorder the game overlay around chat and command entry first, with API configuration collapsed, manual source/browser secondary, and compact background GitHub sync state.
- Deliverable: The pull-down workspace opens with `Chat and Commands`, a request field, `Run AI Change`, and visible progress pills. API key/model fields move into collapsed `AI Settings`, while manual editing, diagnostics, and review remain secondary controls.
- Tests: Structural UI verifier plus debug APK build; focused local tests for persisted settings and command state where applicable.
- Done gate: Opening the workshop makes commands immediately available without hiding source, settings, or sync review.
- Status: `completed`
#### AW41 - Android Voice Change Shortcut
- Language: `Android Java + platform voice integration + docs`.
- Scope: Add a top-game shortcut for a voice change request with explicit recording, cancel, and run states.
- Deliverable: The Workshop flavor now requests microphone permission on demand, captures a platform speech-recognizer transcript from a top-game `Voice` shortcut, previews it with explicit `Cancel` and `Run` controls, and routes a confirmed request through the same AI validation flow as typed commands.
- Tests: Structural UI verifier covers permission, recognizer, cancel/run, and start state; Java sources compile. Device microphone/transcription validation remains pending while the phone is unavailable.
- Done gate: A user can start, cancel, or run a voice change request without accidental code application.
- Status: `in progress (device validation deferred)`
#### AW42 - Android Manual Root Helper Deletion
- Language: `Android Java + docs`.
- Scope: Complete the lifecycle for manually created no-owner helpers without exposing destructive deletion for bundled source.
- Deliverable: `Delete Helper` removes a selected user-created function in `src/root.stasis`, recompiles transactionally, restores the original source on failure, and protects bundled helpers with the existing revert path.
- Tests: Android shell verifier covers the control, source guard, bundled-helper protection, and success status; Java sources compile.
- Done gate: Manual users can create, edit, review, and safely discard draft root helpers without resetting the project.
- Status: `completed`
#### AW43 - Android GitHub Sync Configuration
- Language: `Android Java + docs`.
- Scope: Establish a background-sync configuration contract without claiming that a remote backup has completed before an authenticated API write exists.
- Deliverable: The command-first pull-down shows compact GitHub sync state and exposes collapsed settings for a GitHub token, `owner/repository`, and branch. Valid saved settings report `ready`; missing settings report `not configured`.
- Tests: Android shell verifier covers persisted sync settings, collapsed configuration, and truthful status states; Java sources compile.
- Done gate: A user can configure the repository target once without displacing chat/commands from the primary workflow.
- Status: `completed`
#### AW44 - Android Background GitHub Contents Sync
- Language: `Android Java + GitHub REST API + docs`.
- Scope: Use saved GitHub settings to sync changed app-private project files serially in the background, with compact progress/error state and no false success claim.
- Deliverable: A configured Workshop can now manually start serial Contents API uploads for changed app-private files, using the configured branch, Base64 content, and existing remote SHA when replacing a file. Local files are never modified by upload failure; compact status reports queued, progress, complete, or error. Automatic scheduling, deletion sync, and authenticated repository validation remain pending.
- Tests: Isolated request/response helpers, serial sync scheduling, conflict/error paths, structural verifier, and debug APK build; authenticated repository validation when configured.
- Done gate: A configured Workshop can back up changed Stasis sources to GitHub without making sync controls the foreground editor workflow.
- Status: `in progress (authenticated validation deferred)`
#### AW45 - Android GitHub Review and Pull Request Flow
- Scope: Create a review branch/PR from the configured project, with symbol-first and raw-diff review before submission.
- Deliverable: GitHub settings now expose an explicit review step that displays the symbol summary and raw file diffs and fingerprints that exact change set. Submission rejects missing or stale review, creates or reuses a deterministic Workshop branch from the configured base, uploads changed project files serially, and creates or finds the matching open pull request. Remote failures do not modify local project files.
- Tests: Android shell verifier covers the review controls, fingerprint gate, branch/ref API, serial upload path, and create-or-find PR API; Java sources compile. Authenticated repository/device validation remains deferred.
- Done gate: A configured workshop can create or update a GitHub PR without losing local edits.
- Status: `in progress (authenticated validation deferred)`
#### AW46 - Android Sync Reliability and Credential Protection
- Scope: Queue serial sync work, persist retry/error state, and move API keys/tokens from plain preferences to Android credential storage.
- Deliverable: GitHub and OpenAI secrets use an AES-GCM key held by Android Keystore; preferences retain only versioned ciphertext. Existing plaintext preferences migrate on first read and are removed only after encrypted storage commits successfully. A single executor serializes sync and PR work, persists queued/running/complete/error state plus the retryable operation type, recognizes work interrupted by process shutdown, and reconstructs retries from current local files. PR retry retains the reviewed-change fingerprint and still rejects stale local changes.
- Tests: Android shell verifier covers Keystore/AES-GCM storage, plaintext migration removal, masked editors, serial execution, persisted operation states, interrupted-state recovery, retry routing, and executor shutdown; Java sources compile and the debug APK builds. Device process-death/offline validation remains deferred.
- Done gate: Interrupted/offline sync never loses local source; secrets are not stored in plain text.
- Status: `in progress (device interruption validation deferred)`
#### AW47 - Android Project Import, Export, and Switching
- Scope: Support multiple normal Stasis projects, import/export archives, project metadata, and explicit project switching.
- Done gate: A user can open, back up, restore, and switch projects while retaining symbol editing, tests, and compile behavior.
- Status: `planned`
#### AW48 - Android Preview and Touch Gameplay Parity
- Scope: Replace placeholder preview assumptions with a renderer/runtime contract that displays real game output and supports touch-first sample gameplay.
- Done gate: A representative Stasis game renders and is playable on a phone using touch input through the same runtime used by the workshop.
- Status: `planned`
#### AW49 - Android Diagnostics and Change Recovery UX
- Scope: Provide structured compiler/test diagnostics, source locations, safe rollback/recovery history, and clear hot-reload/reset explanations.
- Done gate: A user can identify, navigate to, and recover from a failed edit without raw log hunting.
- Status: `planned`
#### AW50 - Android Published Build and Release Validation
- Scope: Validate the published/AOT flavor, signing/install workflow, runtime assets, and release performance/error reporting.
- Done gate: A signed sideloadable published build runs a representative game without developer workshop dependencies.
- Status: `planned`
#### AW51 - Android Device Acceptance Suite
- Scope: Add a repeatable device validation checklist/automation for editor, tests, voice, touch preview, sync, and lifecycle recovery.
- Done gate: Every user-facing workshop slice has an on-device proof or an explicitly recorded hardware/environment limitation.
- Status: `planned`
#### AW52 - Android Image Import and Asset Library
- Scope: Import images through the Android photo/document picker, copy them into project-relative asset storage, generate bounded previews, and expose an asset library with rename/delete/reference safety.
- Done gate: A user can import, preview, select, persist, export, and GitHub-sync a project image without exposing arbitrary device paths to Stasis or AI.
- Status: `planned`
#### AW53 - Android Mini Paint Editor
- Scope: Add a touch-first bitmap editor with brush, eraser, palette/color picker, undo/redo, clear, canvas/crop sizing, save-as, cancel, and bounded image dimensions.
- Done gate: A user can create or modify a simple game image, review it, cancel without mutation, or save it as a normal project asset.
- Status: `planned`
#### AW54 - Android Multimodal AI Attachments
- Scope: Attach imported/painted images to typed or voice AI requests using real image input blocks, with thumbnail review, remove controls, format conversion, size limits, and per-request cost visibility.
- Done gate: The AI receives the exact selected project image(s), while unselected assets and private device media are never sent.
- Status: `planned`
#### AW55 - Android Pixel Screenshot to AI
- Scope: Capture the actual preview framebuffer as a bounded image, retain the logical render snapshot as structured context, and let the user explicitly attach either or both to an AI request.
- Done gate: A request can include a visually accurate game screenshot plus runtime/render metadata, with explicit preview/remove/consent before upload.
- Status: `planned`
#### AW56 - AI-Generated Image Asset Review
- Scope: Accept AI-generated or AI-edited image outputs into a temporary review area with before/after preview, accept/reject, undo, project persistence, export, and GitHub sync.
- Done gate: AI image work cannot overwrite an accepted project asset without review and a recoverable prior version.
- Status: `planned`
#### AW57 - Android Audio Asset Workflow
- Scope: Import/record, preview, trim, normalize, rename, delete, reference-check, export, and GitHub-sync bounded sound/music assets; expose selected audio to multimodal-capable AI only with explicit consent.
- Done gate: A user can add and manage game audio without arbitrary device paths, silent transcoding surprises, or orphaned Stasis references.
- Status: `planned`
#### AW58 - Android Command History, Sessions, and AI Budget Controls
- Scope: Persist chat/command history per project, expose cancel/retry, retain tool/test outcomes, show token/cost estimates, and enforce configurable per-run/monthly spend limits.
- Progress: The Workshop persists the 20 most recent unique submitted requests per project behind `Recent Commands`. AI Settings now provides a default `$0.25` per-run cap and `$5.00` monthly cap, records each returned Terra call immediately, blocks unknown-priced models while limits are active, stops multi-turn agents before another paid call at either limit, and conservatively bounds each response with `max_output_tokens`. Outcome history, resume/retry, and cancellation remain.
- Done gate: Users can understand, resume, cancel, and audit AI work while preventing accidental budget overruns.
- Status: `in progress`
#### AW59 - Android Lifecycle, Autosave, and Background Work
- Scope: Define autosave points, process-death recovery, pause/resume behavior, foreground-service/notification rules for long work, battery/network constraints, and safe cancellation.
- Done gate: Rotation, backgrounding, process death, offline transitions, and app upgrades do not lose accepted project edits or falsely report work complete.
- Status: `planned`
#### AW60 - Android Onboarding, Templates, and First-Run Setup
- Scope: Provide a first-run path, sample/template selection, API/GitHub setup guidance, permission explanations, and a zero-AI manual tutorial.
- Done gate: A new user can create/open a project, run it, make a tested change, and understand optional AI/sync configuration without external documentation.
- Status: `planned`
#### AW61 - Android Accessibility and Adaptive Layout
- Scope: Add content descriptions, scalable text/touch targets, contrast/focus support, screen-reader/keyboard navigation, orientation handling, and phone/tablet/foldable layouts.
- Done gate: Core preview, command, editor, test, asset, and review workflows pass accessibility checks and remain usable across supported display sizes.
- Status: `planned`
#### AW62 - Android Privacy, Permissions, and Data Management
- Scope: Minimize permissions, disclose exactly what code/media is sent externally, provide attachment consent and credential revocation, and support project/cache/history/trace deletion.
- Done gate: Users can inspect and erase stored data and secrets, and no project/media leaves the device without an explicit configured action.
- Status: `planned`
#### AW63 - Android Project Format Versioning and Migration
- Scope: Version project/workshop metadata and migrate app-private projects, settings, manifests, assets, tests, and sync state across app/compiler upgrades with rollback-safe backups.
- Done gate: Upgrading the Workshop preserves existing projects or stops with a recoverable, actionable migration diagnostic.
- Status: `planned`
#### AW64 - Android Crash Recovery and Support Bundle
- Scope: Capture bounded local crash/compile/sync diagnostics, detect interrupted operations, offer recovery, and export a redacted support bundle without secrets or unapproved source/media.
- Done gate: A failure can be diagnosed and recovered from without exposing credentials or requiring raw Android log access.
- Status: `planned`

### Cross-Platform Sprite and Audio Track

#### AS0 - Versioned Asset Manifest and Stable Handles
- Scope: Define project-relative sprite/audio entries, content hashes, stable runtime handles, format metadata, dependency tracking, and missing/invalid diagnostics.
- Done gate: JIT/AOT/desktop/Android resolve the same manifest to the same asset identities without arbitrary filesystem access.
- Status: `planned`
#### AS1 - Sprite Decode, Texture Upload, and Lifetime
- Scope: Implement bounded PNG/SVG sprite decoding, GPU upload, handle ownership, release, fallback texture, and deterministic load errors across supported render backends.
- Done gate: A packaged sprite loads and renders identically on desktop and Android, and malformed/missing assets fail safely.
- Status: `planned`
#### AS2 - Sprite Batching and Hot Reload
- Scope: Complete command batching, ordering, transforms, alpha, clipping/atlas policy, resource-generation swaps, and failed-reload preservation.
- Done gate: A changed sprite becomes visible without restarting while the prior texture remains active if decode/upload fails.
- Status: `planned`
#### AS3 - Audio Decode, Mixer, and Playback API
- Scope: Add bounded sound/music decode, voices/streams, play/stop/pause, loop, volume/pan, mixing, asset handles, and deterministic audio-event submission.
- Done gate: Stasis code can play overlapping effects and streaming music through a real mixer rather than the current unavailable stub.
- Status: `planned`
#### AS4 - Desktop and Android Audio Backends
- Scope: Implement device initialization, callback/queue integration, focus/interruption handling, pause/resume, route changes, latency, underrun recovery, and clean shutdown.
- Done gate: The same audio sample plays on desktop and Android and recovers correctly from Android lifecycle/audio-focus events.
- Status: `planned`
#### AS5 - Asset Packaging and JIT/AOT Parity
- Scope: Package referenced assets for dev/JIT, production/AOT, Android Workshop, published APK, import/export, and GitHub sync with reachability and size diagnostics.
- Done gate: A representative game uses the same source/manifest in every execution mode with no missing runtime-only asset path.
- Status: `planned`
#### AS6 - Headless Asset and Event Tests
- Scope: Add deterministic manifest/decode/event/mixer tests, golden sprite output, audio buffer checks, corruption/limit cases, and host-set denial tests without requiring hardware.
- Done gate: CI proves asset semantics, hot-reload safety, and audio mixing deterministically; hardware checks are a separate acceptance layer.
- Status: `planned`
#### AS7 - Sprite and Audio End-to-End Sample Acceptance
- Scope: Upgrade a representative game (Brickout Revenge) to load real sprites and audio, hot reload assets, run in JIT/AOT, and pass desktop plus Android acceptance checks.
- Done gate: The sample is visibly rendered with sprites and audibly produces music/effects on supported devices in dev and published builds.
- Status: `planned`
### Current Snapshot (2026-03-02)
- Completed slices (baseline): `S0`, `S1`, `S2`, `S3`, `S4`, `S5`, `S6`, `S7`, `S8`, `S9`, `S11`.
- Partially complete/in progress: `S8b`, `S10`.
- Release direction (locked):
- Production/release backend is Cranelift AOT.
- AOT must run the same sample games as JIT (notably Brickout Revenge).
- Next language priority: add `f64` end-to-end (pause `u16`/`u32` narrow-int work for now).
- Explicit non-goals for current release approach:
- Optional plugin libraries.
- Anything that depends on a self-host `.stasis` compiler for the release pipeline.
- Self-host note:
- `S10b` and other self-host `.stasis` compiler work under `compiler/` is experimental and must not block the Rust compiler release pipeline.
- Decision update (2026-02-23): host-set sandbox architecture is now locked (`deny-by-default` host access via explicit extern symbols in selected host set).
- Host-set contract selection is profile-only (`--host-set-profile` + optional `--host-set-registry-file` mapping; env fallback: `STASIS_HOST_SET_PROFILE` / `STASIS_HOST_SET_REGISTRY_FILE`). Do not introduce legacy direct contract flags (`--host-set-id`, `--host-set-hash`).
- Compile and commit contracts include host-set metadata (`host_set_id`, `host_set_hash`), and the runtime validates it at commit time (missing/mismatch fails before hook/pointer swap).
- Planned host-set hardening slices (`S13`-`S16`) are scheduled after current AOT parity + `f64` work unless they directly unblock the release pipeline.
- Strategy pivot (2026-02-23): active compiler-slice direction is now symbol-level reachability pruning (function + struct metadata) with simple one-pass lowering to Cranelift; additional parser-shape fallback expansion is deprecated and should be removed when touched.
- Cleanup pivot progress (2026-02-23): detector-heavy simple-shape metadata extraction functions were removed from `compiler/simple_pass_compiler.stasis`; single-pass parser/fingerprint/layout coverage is now the active path.
- Reset update (2026-02-23): compiler implementation has been restarted as a straightforward single-pass pipeline in `compiler/simple_pass_compiler.stasis`; prior detector-metadata expansion work is superseded and should not be resumed.
Archived priority override (2026-02-13, historical):
- At that time, `S10b` self-host AOT CLI core was treated as the top priority.
- That is no longer the release priority; the stable release pipeline is Rust + AOT parity.
- Main integration gap: real backend compile path is now default, but emitted function patches are metadata-only (`FnId` mapping from hashes) and are not yet executing newly generated machine code through the pointer table.
- Shared global-layout arena lowering now emits one owning `global` declaration per compile unit and uses `global_import` for other lowered functions, enabling multi-object AOT linking without duplicate data symbol definitions.
- Simple-pass compiler now builds a root-based function reachability set in `.stasis` (`main`, `tick`, `on_code_swap`) and emits reachable functions only through host analysis harness output (current call-edge discovery uses direct identifier-call tokens; files with no roots keep all functions reachable to avoid helper-file drops in current per-file host analysis flow).
- Ownership enforcement: compiler semantics live in Rust; `.stasis` compiler work under `compiler/` is experimental and must not be treated as the stable pipeline.
- Rust semantic analyzer paths were removed from `crates/stasis_compiler`; ownership guard test now enforces zero reintroduction (`tests/compiler_logic_ownership_guard.rs`).
- Rust-native JIT now resolves typed global paths from top-level declarations (`struct`, `global name: Type`, and `global Name { ... }`) and emits typed runtime global loads/stores (`i32` and `f32`) without implicit `i32` fallback.
- Rust-native JIT now resolves top-level `const` identifiers (`i32`/`f32`/`bool`) and enum variant identifiers (`Enum.Variant`, including explicit enum discriminants) as immediate values during expression lowering.
- Brickout probe progress: fixed-size `foreach` and explicit indexed path expressions/assignments (`collection[index]`, `collection[index].field`) now lower in Rust-native JIT, including enum-typed field assignment/comparison in indexed paths.
- Rust-native JIT call dispatch now supports `i32`/`bool` return functions with uniform `f32` argument lanes (arity `1..8`) in addition to existing `i32` lanes.
- Statement parser/lowering now supports call-expression statements (`callee(...);`) in addition to assignment/conversion/flow-control statements.
- Rust-native JIT now supports:
- top-level extern declaration parsing (`extern function ...` and `@extern("...")`) and direct extern call emission
- string constants in top-level `const` declarations
- conversion statements targeting locals, global paths, and indexed paths
- mixed-ABI call lowering fallback through exact-signature indirect call emission
- local indexed collection load/store for fixed/view array bindings
- local `foreach` over fixed-size local array bindings
- `for` headers require `init`, `condition`, and `step` segments (no missing-segment forms such as `for (; cond; step)`).
- Reachability-gated emission now compiles roots (`main`, `tick`, `render`, `on_code_swap`) plus reachable callees; same-name overload siblings are included to preserve receiver overload behavior.
- Non-engine JIT contract assembly now consumes only emitted symbol code pointers (unemitted, unreachable functions are excluded from override patch emission).
- Host memory intrinsics used by stdlib/gfx paths are now real externs with Rust-native bindings:
- `sys_memcpy_u8/i32/f32`
- `sys_memmove_u8/i32/f32`
- Brickout probe status (Rust-native JIT): `samples/brickout_revenge/brickout_revenge_v1.stasis` compiles and commits successfully for `--ticks 1` with runtime launch + hook + swap commit success.
- Brickout probe status (AOT prod path): `samples/brickout_revenge/brickout_revenge_v1.stasis` now compiles and commits successfully for `--ticks 1` with runtime launch + hook + swap commit success.
- Brickout v1 now declares explicit `render()` entrypoint alongside `tick()`, and engine-mode JIT contract coverage now includes a Brickout-specific proof that `render` lowers to a non-zero code pointer (`jit_dev_brickout_v1_builds_engine_package_with_render_pointer`) while runtime smoke still compiles/commits in-process (`real_backend_smoke_compiles_and_commits_brickout_v1`).
- Watch-mode behavior now includes built-in dependency-graph filtering for `--watch-file`/inferred entry roots under `--watch-dir`: only changes in the current import-closure of the watched root trigger recompiles (`watch_directory_dependency_change_triggers_recompile`, `watch_directory_ignores_non_dependency_changes`).
- Runtime launcher now falls back to generic `--watch-file` launch for non-scenario fixtures (instead of hard failing unknown scenario mapping), so compiler-slice fixtures can be exercised through runtime launch path as well.
- Incremental semantic guard for mutating `from_*` conversions now uses block-aware statement splitting and annotation-aware function scanning, removing false-positive rejection on valid `if/else` conversion statements while preserving expression misuse diagnostics.
- Incremental JIT emit/apply is now transactional for failure paths: dirty flags are cleared only after successful full emit, and JIT artifact/module updates are staged then committed atomically so failed emits preserve prior executable dispatch state for retry.
- Rust-native JIT local assignments now treat collection/view handles as reference values with `=` support and explicit rejection of compound assignment operators, preserving simple alias/rebind semantics without numeric-handle arithmetic.
- Typed `let` annotations in Rust-native JIT now parse full type text (including `Type[]`/array forms) instead of identifier-only tokens, so view-typed local bindings lower correctly without falling back to inferred-only declarations.
- Added regression coverage for typed ASCII view locals in Rust-native JIT (`jit_process_executes_typed_ascii_view_let_binding`) to lock `let view: ascii[] = ...` parsing/lowering behavior.
- Indexed struct-value copy assignment now lowers directly in Rust-native JIT for `arr[target] = arr[source]` by emitting deterministic field-wise copy across SoA field paths, with mismatch diagnostics + regression coverage (`jit_process_executes_indexed_struct_value_copy_assignment`, `jit_process_rejects_indexed_struct_copy_assignment_for_mismatched_layouts`).
- Added regression coverage to lock index-evaluation semantics for struct copy (`jit_process_evaluates_struct_copy_indices_once_each`): source/target index expressions are evaluated once per assignment, not once per copied field.
- Global struct-path value copy now lowers in Rust-native JIT for scalar-only layouts (`dst = src` on flattened struct roots), with explicit rejection when copied struct paths include collection/string handle fields (`jit_process_executes_global_struct_path_value_copy_assignment`, `jit_process_rejects_global_struct_copy_assignment_with_collection_fields`).
- Added regression coverage for nested global-block struct roots and operator gating on struct-path copy (`jit_process_executes_global_block_nested_struct_path_copy_assignment`, `jit_process_rejects_global_struct_path_copy_compound_assignment`).
- Struct-copy lowering now also covers cross-shape assignments between global struct roots and indexed struct elements (`target = source[i]`, `target[i] = source`) for scalar-only layouts, with deterministic runtime parity coverage (`jit_process_executes_struct_copy_from_indexed_to_global_path`, `jit_process_executes_struct_copy_from_global_to_indexed_path`).
- Added mismatch diagnostics coverage for both cross-shape struct-copy directions (`jit_process_rejects_struct_copy_from_indexed_to_global_on_layout_mismatch`, `jit_process_rejects_struct_copy_from_global_to_indexed_on_layout_mismatch`).
- Local struct-array parameter paths now use named-struct field-type metadata in emit/lowering, enabling deterministic `arr[idx].field` and local `foreach` struct-array parameter behavior with runtime parity coverage (`jit_process_executes_local_indexed_struct_array_parameter_field_access`, `jit_process_executes_foreach_over_local_struct_array_parameter`).
- Added explicit local struct-array view parameter coverage for indexed field read/write (`jit_process_executes_local_indexed_struct_array_view_parameter_field_access`) to lock `Type[]` parity with fixed-array parameter behavior in current JIT lowering.
- Indexed struct value-copy lowering now also covers local collection bindings (`arr[target] = arr[source]`) for both fixed and view params (`jit_process_executes_local_indexed_struct_value_copy_assignment`, `jit_process_executes_local_indexed_struct_value_copy_assignment_for_view_param`) with mismatch diagnostics (`jit_process_rejects_local_indexed_struct_copy_assignment_for_mismatched_layouts`).
- Local indexed struct element access without field suffix now fails explicitly in JIT lowering (`jit_process_rejects_local_indexed_struct_element_without_field_suffix`) to prevent accidental scalar-lane fallback behavior.
- Local indexed struct copy now has explicit operator gating coverage (`jit_process_rejects_local_indexed_struct_copy_compound_assignment`) so only `=` is accepted for struct value-copy assignment.

### S0 - Workspace Bootstrap
- Language:
- `Rust`
- Scope:
- Create real crate/app sources for `apps/stasis`, `crates/stasis_compiler`, `crates/stasis_jit`, `crates/stasis_runner`.
- Create/verify required `Cargo.toml` and `src/` roots for each workspace member referenced by root `Cargo.toml`.
- Deliverable:
- `cargo build` and `cargo test` pass with scaffold smoke tests.
- Tests:
- Workspace compile smoke.
- Done gate:
- Clean build/test on branch with no placeholder dead modules.
- Status: `completed`

### S1 - Minimal Front-End Parse
- Language:
- `Rust + .stasis`
- Rust: host invocation/test harness and in-process compiler host bindings.
- `.stasis` (experimental): self-hosting compiler prototypes under `compiler/`.
- Scope:
- Implement lexer/parser for minimum executable subset:
- `function`, `return`, integer/string literals, call expression, extern declaration.
- Deliverable:
- Parser accepts minimal program containing `main`.
- Tests:
- Parser fixtures for positive/negative cases (`tests/stasis/parser_valid_main.stasis`, `tests/stasis/parser_invalid_missing_semicolon.stasis`).
- Done gate:
- Parses minimal valid program and emits actionable diagnostics on failures.
- Status: `completed`

### S2 - Minimal Execution (`main(): i32`)
- Language:
- `Rust + .stasis`
- Rust: lowering bridge to Cranelift and execution harness.
- `.stasis`: compile pipeline decisions selecting/validating `main`.
- Scope:
- Wire parser output into minimal lowering and JIT execution (dev mode) for:
- `function main(): i32 { return <int>; }`
- Deliverable:
- Runner executes `main` and returns process status code.
- Tests:
- End-to-end test asserts returned status code (`tests/stasis/run_main_returns_7.stasis`).
- Done gate:
- Exit status path is stable and deterministic.
- Status: `completed`

### S3 - Console Externs
- Language:
- `Rust + .stasis`
- Rust: host extern ABI implementation (`print_i32`, `print_string`).
- `.stasis`: extern symbol declarations and compile-time binding checks.
- Scope:
- Add stable host extern ABI for:
- `print_i32(value: i32)` and `print_string(value: string)`.
- Ensure console path supports `string`, `ascii[]`, and `utf8[]` call sites for `print_string`.
- Treat these externs as symbols exported by the selected host set in runtime boundary contracts (full deny-by-default enforcement tracked in `S13+`).
- Deliverable:
- Stasis program can print deterministic output through host boundary.
- Tests:
- End-to-end golden stdout tests.
- Done gate:
- Output is deterministic and ABI contract is documented.
- Status: `completed`

### S4 - Core Statements and Expressions
- Language:
- `Rust + .stasis`
- Rust: expression lowering/eval codegen primitives.
- `.stasis`: semantic rules and compile pipeline ordering.
- Scope:
- Add `let`, assignment, infix arithmetic/comparison, block scopes, and `if`/`else if`/`else`.
- Deliverable:
- Small real programs beyond single return execute correctly.
- Tests:
- Semantic and codegen unit tests plus end-to-end fixtures.
- Added parser coverage fixtures:
- `tests/stasis/parser_s4_valid_control_flow.stasis`
- `tests/stasis/parser_s4_invalid_let_missing_init_or_type.stasis`
- Added runtime smoke fixtures that execute `compiler/simple_pass_compiler.stasis` parse counts and failure paths:
- `tests/stasis/run_parser_s4_counts.stasis`
- `tests/stasis/run_parser_invalid_let_missing_init_or_type.stasis`
- Done gate:
- Behavior matches `docs/spec.md` operator and assignment rules.
- Status: `completed`

### S5 - Call Model and Conversion Semantics
- Language:
- `Rust + .stasis`
- Rust: overload resolution engine and IR lowering support.
- `.stasis`: receiver-preference policy, conversion semantics checks, and diagnostics policy.
- Scope:
- Implement receiver-scoped resolution key `(function_name, parameter0_type)`.
- Keep function-form calls supported indefinitely.
- Implement conversion semantics:
- Mutating `from_*` operations.
- Pure `to_*` operations.
- Explicit enum conversion surface `enum_to_i32(value: EnumType): i32` (no implicit enum/int conversion).
- Seed-compiler compatibility path exists only for bring-up; steady-state path is self-hosted intrinsic implementation.
- Deliverable:
- `enemy.damage(5)` and `damage(enemy, 5)` both resolve correctly.
- Conversion semantics follow spec examples.
- Tests:
- Overload resolution tests, conversion tests, negative diagnostics.
- Current parser/execution coverage fixture:
- `tests/stasis/run_parser_s5_receiver_and_function_calls.stasis` (receiver-form and function-form call parsing baseline).
- Added semantic-level regression coverage in `crates/stasis_compiler`:
- receiver-scoped signature distinction test for overloads by parameter0 type
- conversion misuse diagnostic test for invalid `from_*` expression usage (`4001`)
- Done gate:
- Receiver-form preferred but both call forms behave consistently and deterministically.
- Status: `completed`

### S6 - Global Memory and Layout
- Language:
- `Rust + .stasis`
- Rust: layout computation primitives and stable hashing implementation.
- `.stasis`: layout-policy checks and rejection rules.
- Scope:
- Implement global declarations and deterministic layout metadata/hashing.
- Deliverable:
- Stable layout hash for unchanged declarations.
- Tests:
- Layout determinism tests across repeated compiles.
- Current runtime coverage fixtures:
- `tests/stasis/run_layout_hash_deterministic.stasis`
- `tests/stasis/run_layout_hash_changes_on_layout_update.stasis`
- `tests/stasis/run_layout_hash_file_db_change_detection.stasis`
- Done gate:
- Layout-affecting changes are detected reliably.
- Status: `completed`

### S7 - Incremental Compiler V1
- Language:
- `Rust + .stasis`
- Rust: in-memory file DB, cache storage, and invalidation substrate.
- `.stasis`: whole-file semantic pass orchestration and per-function codegen gating policy.
- Scope:
- Add in-memory file database, file-level invalidation, whole-file semantic check.
- Gate codegen per function using semantic hashes.
- Deliverable:
- Unchanged function bodies skip backend regeneration.
- Tests:
- Incremental cache hit/miss tests and file-level invalidation tests.
- Current runtime coverage fixture:
- `tests/stasis/run_incremental_file_db_counts.stasis` (exercises `compiler_upsert_file` parse + reuse counters).
- `tests/stasis/run_incremental_function_hash_metrics.stasis` (exercises per-function reused/changed/codegen hash gating counters).
- Done gate:
- Semantic pass always runs per changed file; backend work is correctly gated.
- Status: `completed`

### S8 - Function Pointer Table ABI
- Language:
- `Rust`
- Scope:
- Implement stable `FnId -> code_ptr` indirection and generation-based code regions.
- Current implementation:
- `crates/stasis_jit::FunctionPointerTable` now owns `FnId -> CodePtr` mapping, generation increments, and safe-window retirement bookkeeping.
- `apps/stasis` swap commit path now sources commit generation IDs from `FunctionPointerTable`.
- Deliverable:
- Runtime dispatch goes through pointer table only.
- Tests:
- ABI and indirect-call tests.
- Done gate:
- No direct raw-address calls from runtime callsites.
- Status: `completed`

### S8b - Cranelift AOT Production Path
- Language:
- `Rust`
- Scope:
- Add production AOT compilation path and artifact wiring using Cranelift AOT outputs.
- Current implementation:
- `DevHotSwapPipeline` now supports explicit `TargetMode` (`JitDev` or `AotProd`) dispatch.
- `apps/stasis` runner config/CLI can request `AotProd` compile requests (`--target-mode aot` / `--aot-prod`).
- `crates/stasis_compiler::backend::aot::AotProcess` produces native object artifacts in-process through the shared Cranelift lowering path.
- `apps/stasis::IncrementalCompilerBackend` now emits per-function AOT object artifacts for changed functions when `TargetMode::AotProd` compile requests are processed in the real backend path.
- Real backend now writes `last_patch_manifest.json` alongside emitted AOT object artifacts to persist request/artifact mapping for runtime handoff.
- `crates/stasis_jit` now provides optional object-bundle link support (`link_objects_to_dynamic_library`) and the real backend can emit linked bundle artifacts when `STASIS_AOT_LINK_ARTIFACTS=1` and linker tooling is available.
- `SwapCommitRequest` now carries explicit `target_mode` so runtime safe-point commit can apply mode-specific gating deterministically.
- Compile/commit contracts now carry optional `aot_linked_image_path`, and runtime commit gate rejects `AotProd` commits when linked-image metadata is missing or the declared linked image is missing at commit time.
- Runner events now include explicit `aot_linked_image_validation` success/failure records for commit-time artifact handoff diagnostics.
- Compile/commit contracts now also carry optional `aot_linked_image_size_bytes`; runtime commit gate validates expected linked-image size to catch artifact drift between compile and commit.
- Compile/commit contracts now also carry optional `aot_linked_image_sha256`; runtime commit gate validates linked-image content hash to catch artifact substitution/drift between compile and commit.
- Compile/commit contracts now also carry optional `aot_function_symbols`; runtime commit gate requires symbol mapping coverage for all patched `FnId`s in `AotProd`.
- Runtime now supports optional exported-symbol resolution for `AotProd` pointer-table overrides (`STASIS_AOT_RESOLVE_EXPORTS=1`), resolving code pointers from linked-image export tables when available.
- Runtime now supports optional in-process dynamic loader resolution for `AotProd` pointer-table overrides (`STASIS_AOT_USE_LOADER=1`), loading linked artifacts and resolving exported symbol addresses via OS loader APIs.
- AOT link step now forwards emitted function symbol exports to the linker (Windows `/EXPORT:` flags) so linked bundles can expose compiled symbol entrypoints when toolchain/linker supports export emission.
- In `AotProd`, runtime commit gate now requires complete linked-image metadata (`path + size + sha256`) and rejects incomplete handoff payloads.
- Linked-image SHA-256 computation/validation now uses buffered streaming I/O (chunked reads) in compile + commit paths to avoid full-file memory spikes on large artifacts.
- Runtime now caches successful linked-image validation tuples (`path + size + sha256 + probe mode`) to avoid redundant hash/format/probe work across unchanged commits.
- Runtime now applies `AotProd` pointer-table updates with explicit code-pointer overrides derived from linked-image symbol handoff metadata (`FnId -> symbol`), instead of generation-only placeholder pointer synthesis.
- Default `AotProd` pointer override behavior remains deterministic metadata-derived when export resolution mode is disabled, preserving stable bring-up behavior while export-resolution path matures.
- Runtime commit gate now performs safe linked-image format validation (`MZ/PE` on Windows, `ELF` on Linux, `Mach-O` magic on macOS) before allowing `AotProd` swap.
- Runtime now records successful linked-artifact activation (`AotLinkedImageActivated`) and tracks the active linked-image path in runner summary state for downstream runtime ownership.
- Optional runtime loadability probe support is now available (`RunnerConfig.aot_probe_loadability` / CLI `--aot-probe-load`) to attempt OS-level load/free validation of linked artifacts at commit time.
- Runtime now tracks AOT linked-image lifecycle (active + retired artifacts) through `AotArtifactRegistry` and emits retirement events (`AotLinkedImageRetired`) when pointer-table generations exit the safe-retire window.
- Runtime now tracks loaded AOT module lifecycle by pointer-table generation and unloads retired generations by dropping generation-bound loader handles after safe-retire.
- `AotArtifactRegistry` now bounds retained retirement history (`DEFAULT_MAX_RETIRED_IMAGES`) to avoid unbounded memory growth during long watch-mode sessions.
- AOT artifact lifecycle is now generation-bound: activation/retirement metadata is recorded against the committed pointer-table generation and surfaced through runner events/summary state.
- Runtime relaunch path now passes `--no-runtime-launch` to spawned child processes to prevent recursive process trees during watch-mode swap iteration.
- Simple `i32` return-body extraction now supports deterministic `if`/`else if`/`else` branch evaluation with branch-local `let`/assignment handling and fallthrough continuation to later top-level `return`.
- AOT simple-body lowering now preserves conditional return chains as expression-level select trees (`SimpleI32Condition` + `SimpleI32ReturnExpr::Select`) instead of only compile-time branch selection.
- Incremental compiler function metrics now include declared return type metadata, and AOT stub emission uses return-type-aware signatures (`void` functions emit `return` without value; `i32` functions keep value-return lowering).
- Deterministic simple-body condition evaluation now supports logical composition in `if` conditions (`&&`, `||`, `!`, and parenthesized grouping) in addition to comparison operators.
- Symbolic simple-body condition extraction now preserves logical condition trees (`And`/`Or`/`Not`) for `if` return-chain lowering, enabling AOT condition emission beyond comparison-only predicates.
- Top-level conditional return emission now lowers through explicit branch blocks (`brif` + branch-return blocks) in AOT stub CLIF output instead of select-only expression lowering.
- Deliverable:
- Production mode runs from AOT artifacts without requiring runtime JIT.
- Tests:
- AOT compile-and-run smoke tests for representative fixtures.
- Added backend regression test coverage for AOT helper failure diagnostics (`apps/stasis::compiler_backend::tests::aot_compile_reports_missing_helper_binary`).
- Added backend regression test coverage for AOT artifact manifest generation with deterministic fake helper success path (`apps/stasis::compiler_backend::tests::aot_compile_writes_manifest_with_artifacts_on_success`).
- Added backend + JIT regression coverage for optional AOT link step and linked-image manifest field (`apps/stasis::compiler_backend::tests::aot_compile_can_link_bundle_and_record_linked_image_in_manifest`, `crates/stasis_jit::tests::aot_linker_can_be_driven_by_configured_fake_linker`).
- Added runner regression coverage for AOT commit-time linked-image validation (`apps/stasis::tests::aot_commit_rejects_missing_linked_image_path`).
- Added runner regression coverage for `AotProd` metadata-presence gating (`apps/stasis::tests::aot_commit_rejects_missing_linked_image_metadata_path`).
- Added runner regression coverage for AOT linked-image size mismatch rejection (`apps/stasis::tests::aot_commit_rejects_linked_image_size_mismatch`).
- Added runner regression coverage for linked-image hash mismatch rejection (`apps/stasis::tests::aot_commit_rejects_linked_image_hash_mismatch`).
- Added runner regression coverage for missing linked-image size/hash metadata rejection (`apps/stasis::tests::aot_commit_rejects_missing_linked_image_size_metadata`, `apps/stasis::tests::aot_commit_rejects_missing_linked_image_hash_metadata`).
- Added deterministic SHA-256 utility coverage in both backend and runtime paths (`apps/stasis::compiler_backend::tests::compute_file_sha256_hex_matches_known_value`, `apps/stasis::tests::compute_file_sha256_hex_matches_known_value`).
- Added runtime validation-cache tuple semantics coverage (`apps/stasis::tests::aot_validation_cache_hits_only_on_exact_metadata_tuple`).
- Added contract/pipeline coverage for AOT function-symbol propagation (`crates/stasis_runner::swap::contracts`, `crates/stasis_runner::swap::pipeline::compile_hook_symbol_propagates_to_commit_request`).
- Added pointer-table override commit coverage (`crates/stasis_jit::tests::commit_patch_set_with_code_ptrs_applies_override_pointers`).
- Added runtime export-resolution coverage on Windows (`apps/stasis::tests::resolve_aot_symbol_export_code_ptr_finds_kernel32_export`, `apps/stasis::tests::resolve_aot_symbol_export_code_ptr_rejects_missing_export`).
- Added loader-resolution coverage on Windows (`apps/stasis::tests::build_aot_code_ptr_overrides_loader_mode_resolves_export_address`, `apps/stasis::tests::build_aot_code_ptr_overrides_loader_mode_rejects_missing_export`, `crates/stasis_dynload::tests::can_load_kernel32_and_resolve_export`).
- Added loader native-entry invocation coverage on Windows (`crates/stasis_dynload::tests::can_invoke_get_tick_count_export`).
- Added runtime commit-loop coverage for AOT loader + native hook execution path (`apps/stasis::tests::runner_aot_loader_native_hook_executes_and_reports_return_value`).
- Added runner regression coverage for invalid linked-image format rejection (`apps/stasis::tests::aot_commit_rejects_invalid_linked_image_format`) and dedicated validator unit coverage (`apps/stasis::aot_validation::tests::rejects_non_binary_payload`).
- Added positive runner regression coverage for `AotProd` commit acceptance when linked-image metadata is present and valid (`apps/stasis::tests::aot_commit_accepts_valid_pe_linked_image_metadata` on Windows).
- Added optional loadability-probe coverage on Windows (`apps/stasis::aot_validation::tests::loadability_probe_accepts_system_library`, `apps/stasis::tests::aot_commit_accepts_system_library_when_probe_enabled`).
- Added lifecycle coverage for AOT artifact retirement when generations retire (`apps/stasis::tests::aot_activation_retires_previous_image_after_generation_safe_window`, `apps/stasis::aot_artifacts::tests::*`).
- Added bounded-retirement-history coverage (`apps/stasis::aot_artifacts::tests::retired_history_is_bounded`).
- Added lifecycle coverage for generation-bound activation/retirement (`apps/stasis::tests::aot_commit_accepts_valid_pe_linked_image_metadata`, `apps/stasis::aot_artifacts::tests::activate_same_path_updates_generation_without_retiring`).
- Done gate:
- Production pipeline uses AOT artifacts with deterministic behavior.
- Status: `in_progress`
- Remaining:
- Priority update (2026-02-13):
- `S8b` hardening/parity work is explicitly lower priority than `S10b` self-host AOT CLI core.
- Do not schedule additional `R*` slices unless required to unblock `SH1`, `SH2`, or `SH3`.
- Slice R1: Add real branch/join block emission for runtime-dependent `if/else` in `AotProd` (no select-only fallback for supported bodies). (completed 2026-02-13)
- Slice R2: Add short-circuit boolean control-flow lowering (`&&`, `||`, `!`) in real emitted branch blocks. (completed 2026-02-13)
- Slice R3a: Lower direct no-arg `i32` return-call bodies (`return callee();`) in emitted AOT bodies when callee dispatch resolves uniquely from compiler metadata in the patch set. (completed 2026-02-13)
- Slice R3b1: Extend call-site lowering to direct `i32` return-call bodies with constant additive offsets (`return callee() +/- <int_literal>;`). (completed 2026-02-13)
- Slice R3b2a: Extend call-site lowering to simple no-arg two-call `i32` return bodies (`return lhs() +/- rhs();`) with resolver gating/diagnostics. (completed 2026-02-13)
- Slice R3b2b: Extend call-site lowering to argument-bearing `i32` call-return bodies. (completed 2026-02-13)
- Slice R3b2c1: Extend call-site lowering to nested one-arg call-return bodies (`return callee(arg_fn());`) with resolver gating/diagnostics. (completed 2026-02-13)
- Slice R3b2c2a: Extend call-site lowering to one-arg call-return additive-offset bodies (`return callee(<int_literal>) +/- <int_literal>;`, `return callee(arg_fn()) +/- <int_literal>;`). (completed 2026-02-13)
- Slice R3b2c2b: Extend call-site lowering to broader nested/mixed call-heavy `i32` return expressions and resolver sources.
- Slice R4a: Lower simple void host extern print calls (`print_i32(<int_literal>)`) in emitted AOT bodies and add deterministic side-effect verification fixtures. (completed 2026-02-13)
- Slice R4b1: Extend host-side-effecting extern-call lowering to simple call-argument print bodies (`print_i32(callee())`) with resolver gating and diagnostics. (completed 2026-02-13)
- Slice R4b2a: Extend host-side-effecting extern-call lowering to additive-offset call-argument print bodies (`print_i32(callee() +/- <int_literal>)`). (completed 2026-02-13)
- Slice R4b2b1: Extend host-side-effecting extern-call lowering to folded literal-expression print bodies (`print_i32(<int_literal> +/- <int_literal>)`). (completed 2026-02-13)
- Slice R4b2b2a: Extend host-side-effecting extern-call lowering to one-arg call print bodies (`print_i32(callee(<int_literal>))`). (completed 2026-02-13)
- Slice R4b2b2b1: Extend host-side-effecting extern-call lowering to nested one-arg call print bodies (`print_i32(callee(arg_fn()))`). (completed 2026-02-13)
- Slice R4b2b2b2a: Extend host-side-effecting extern-call lowering to one-arg literal additive-offset print bodies (`print_i32(callee(<int_literal>) +/- <int_literal>)`). (completed 2026-02-13)
- Slice R4b2b2b2a2: Extend host-side-effecting extern-call lowering to nested one-arg additive-offset print bodies (`print_i32(callee(arg_fn()) +/- <int_literal>)`). (completed 2026-02-13)
- Slice R4b2b2b2a3: Extend host-side-effecting extern-call lowering to two-call print bodies (`print_i32(lhs() +/- rhs())`) via `.stasis` shape detection routed through shared two-call metadata channels. (completed 2026-02-13)
- Slice R4b2b2b2b: Extend host-side-effecting extern-call lowering/parity to broader `print_i32` body shapes beyond literal/direct-call/additive-offset/one-arg-call/nested-one-arg-call/one-arg-additive forms.
- Slice R5: Lower runtime-dependent local mutation/update flows in emitted bodies (non-constant locals and assignment chains).
- Slice R6a: Add compatibility-gate coverage for unresolved direct-call dispatch in real lowered AOT bodies. (completed 2026-02-13)
- Slice R6a2: Add compatibility-gate coverage for unresolved two-call dispatch in real lowered AOT bodies. (completed 2026-02-13)
- Slice R6a3: Add compatibility-gate coverage for unresolved one-arg direct-call dispatch in real lowered AOT bodies. (completed 2026-02-13)
- Slice R6a4: Add compatibility-gate coverage for unresolved one-arg direct-call argument-target dispatch in real lowered AOT bodies. (completed 2026-02-13)
- Slice R6b1: Add compatibility-gate coverage for unresolved void print-call dispatch in real lowered AOT bodies. (completed 2026-02-13)
- Slice R6b2a: Add compatibility-gate coverage for `AotProd` commit-time missing function-symbol mapping metadata (patched `FnId` dispatch compatibility). (completed 2026-02-13)
- Slice R6b2b: Add compatibility-gate coverage for `AotProd` commit-time missing symbol entries for patched `FnId`s. (completed 2026-02-13)
- Slice R6b2c1: Add compatibility-gate coverage for `AotProd` loader-mode mapped-symbol export resolution failures at commit time. (completed 2026-02-13)
- Slice R6b2c2a: Add compatibility-gate coverage for `AotProd` commit-time duplicate symbol-mapping entries for the same patched `FnId`. (completed 2026-02-13)
- Slice R6b2c2b: Add compatibility-gate coverage for additional real lowered body signature/layout mismatch classes at commit time. (deferred post dev/jit + self-host)
- Slice R7a: Add rollback-path coverage for unresolved direct-call compile failure (compile failure must skip commit and preserve previous generation state). (completed 2026-02-13)
- Slice R7a2: Add rollback-path coverage for unresolved two-call compile failure (compile failure must skip commit and preserve previous generation state). (completed 2026-02-13)
- Slice R7a3: Add rollback-path coverage for unresolved one-arg direct-call compile failure (compile failure must skip commit and preserve previous generation state). (completed 2026-02-13)
- Slice R7a4: Add rollback-path coverage for unresolved one-arg direct-call argument-target compile failure (compile failure must skip commit and preserve previous generation state). (completed 2026-02-13)
- Slice R7b1: Add rollback-path coverage for unresolved void print-call compile failure (compile failure must skip commit and preserve previous generation state). (completed 2026-02-13)
- Slice R7b2a: Add rollback-path coverage for `AotProd` commit failure on missing function-symbol mapping metadata (no partial commit, previous generation preserved). (completed 2026-02-13)
- Slice R7b2b1: Add rollback-path coverage for `AotProd` commit failure on missing symbol entries for patched `FnId`s (no partial commit, previous generation preserved). (completed 2026-02-13)
- Slice R7b2b2a: Add rollback-path coverage for second-commit `AotProd` loader-mode mapped-symbol export resolution failures preserving previous active artifact/generation. (completed 2026-02-13)
- Slice R7b2b2b1: Add rollback-path coverage for second-commit `AotProd` missing linked-image path metadata preserving previous active artifact/generation. (completed 2026-02-13)
- Slice R7b2b2b2a: Add rollback-path coverage for second-commit `AotProd` duplicate-symbol-mapping commit failures preserving previous active artifact/generation. (completed 2026-02-13)
- Slice R7b2b2b2b1: Add rollback-path coverage for second-commit `AotProd` missing linked-image size metadata preserving previous active artifact/generation. (completed 2026-02-13)
- Slice R7b2b2b2b2a: Add rollback-path coverage for second-commit `AotProd` missing linked-image hash metadata preserving previous active artifact/generation. (completed 2026-02-13)
- Slice R7b2b2b2b2b: Add rollback-path coverage for additional real lowered body commit failure classes beyond unresolved direct-call/void-print-call compile rejection. (deferred post dev/jit + self-host)
- Slice R8: Add emitted-symbol runtime parity tests for runtime-dependent control-flow bodies in loader mode. (deferred post dev/jit + self-host)
- Slice R9a: Add emitted-symbol runtime parity coverage for direct no-arg `i32` call-return bodies in loader mode. (completed 2026-02-13)
- Slice R9b: Extend emitted-symbol runtime parity coverage to broader call-heavy bodies (arguments, nested calls, mixed control flow). (deferred post dev/jit + self-host)
- Slice R10: Add emitted-symbol runtime parity tests for local-mutation-heavy bodies in loader mode. (deferred post dev/jit + self-host)
- Slice R11: Add Brickout-oriented gameplay dispatch parity coverage using real emitted exported entrypoints in watch loop. (prioritize dev/jit path first; AOT/export parity deferred post dev/jit + self-host)
- Slice R12 (optional): Perform long-session watch-mode stability/perf pass and memory-growth checks after R1-R11.

### S9 - Two-Phase Swap Commit
- Language:
- `Rust + .stasis`
- Rust: commit transaction mechanism and thread-safe pointer swap.
- `.stasis`: swap eligibility policy inputs and diagnostics policy.
- Scope:
- Implement background compile patch generation and between-ticks commit.
- Implement typed boundary contracts for dev flow messages:
- `FileChangeEvent`, `CompileRequest`, `CompileResult`, `SwapCommitRequest`, `SwapCommitResult`.
- Current implementation:
- `DevHotSwapPipeline` now rejects compile/commit payloads with mismatched `contract_version` and surfaces explicit failure diagnostics/errors instead of partially proceeding.
- Deliverable:
- Atomic swap behavior: all-or-nothing commit.
- Tests:
- Swap success, swap rejection, and no-partial-commit tests.
- Boundary contract tests for message ordering and failure propagation.
- Done gate:
- On failure, old code/data remain active.
- Runtime/compiler ownership boundaries enforced in code paths.
- Status: `completed`

### S10 - `on_code_swap` Hook
- Language:
- `Rust + .stasis`
- Rust: hook invocation boundary and rollback/error propagation.
- `.stasis`: hook definition and rule enforcement semantics.
- Scope:
- Run optional `function on_code_swap(): void` before pointer swap.
- Current implementation:
- `CompileResult` now carries optional `hook_symbol`; pipeline forwards it into `SwapCommitRequest` (instead of hardcoded symbol) so hook execution is compiler-declared.
- `CompileResult`/`SwapCommitRequest` now also carry optional `hook_fn_id`, and real backend resolves/populates hook function identity for commit-time hook handling metadata.
- Incremental compiler host now reports `hook_symbol` from full tracked program state (not only changed-function emission), so `on_code_swap` remains commit-visible across non-hook edits in the same file.
- Real backend watch-mode regression now verifies hook runs across subsequent commits even when `on_code_swap` body is unchanged (`apps/stasis::tests::real_backend_runs_hook_on_subsequent_commits_when_hook_body_unchanged`).
- Contract regression coverage now verifies hook symbol + function-id propagation (`crates/stasis_runner::swap::contracts` and `crates/stasis_runner::swap::pipeline::compile_hook_symbol_propagates_to_commit_request`).
- Runner hook events now include optional `hook_fn_id` telemetry to keep commit-time hook outcomes tied to compiled function identity.
- Runtime commit gate now rejects hook execution when hook symbol metadata is present but `hook_fn_id` is absent, preventing ambiguous hook dispatch contracts.
- Runtime now resolves hook dispatch through pointer-table staged code-pointer preview (`FunctionPointerTable::preview_code_ptr_after_commit`) and rejects commit when hook dispatch cannot be resolved for `hook_fn_id`.
- Runner hook events now include optional `hook_code_ptr` telemetry so commit-time hook outcomes are tied to both function identity and staged dispatch target.
- Runtime now supports optional native hook-entry invocation for `AotProd` loader mode (`STASIS_AOT_EXECUTE_NATIVE_HOOK=1`), executing the staged hook address before swap and aborting commit on invocation failure.
- Native hook-entry invocation now uses a `void` ABI call shape aligned with `on_code_swap(): void` semantics to avoid mismatched return-signature invocation risk.
- Runner hook events now include optional `hook_return_value` telemetry for native-invocation mode.
- Deliverable:
- Explicit state adjustment point between ticks.
- Tests:
- Hook success/failure transactional tests.
- Added runner regression for hook metadata consistency rejection (`apps/stasis::tests::runner_rejects_hook_symbol_without_hook_fn_id_metadata`).
- Added runner regression for unresolved hook dispatch rejection (`apps/stasis::tests::runner_rejects_hook_when_pointer_table_has_no_dispatch_entry`).
- Added pointer-table unit coverage for staged hook dispatch preview (`crates/stasis_jit::tests::preview_code_ptr_after_commit_*`).
- Added native hook-invocation coverage on Windows (`apps/stasis::tests::maybe_execute_native_hook_invokes_loaded_export_for_aot_loader_mode`, `apps/stasis::tests::maybe_execute_native_hook_skips_when_native_execution_disabled`).
- Added runtime commit-loop native hook telemetry coverage in `AotProd` loader mode (`apps/stasis::tests::runner_aot_loader_native_hook_executes_and_reports_return_value`).
- Added transactional abort coverage for native hook invocation failure in `AotProd` loader mode (`apps/stasis::tests::runner_aot_loader_native_hook_failure_aborts_swap`).
- Added multi-commit transactional preservation coverage for `AotProd` loader mode (`apps/stasis::tests::runner_aot_loader_second_commit_failure_preserves_previous_active_artifact`) to verify a failed subsequent swap preserves the previously active linked artifact generation.
- Added real-backend emitted-artifact loader-mode commit coverage (`apps/stasis::tests::real_backend_emitted_aot_artifact_commits_via_loader_mode`) using deterministic emitted AOT artifact handoff plus runtime linked-image metadata validation/activation.
- Added compiler-backend emitted-symbol fidelity coverage for `AotProd` (`apps/stasis::compiler_backend::tests::aot_compile_emits_hook_fn_symbol_mapping_and_patch_coverage`) to verify symbol mapping covers all patched functions and includes `hook_fn_id`.
- Added linker export-argument propagation coverage (`crates/stasis_jit::tests::aot_linker_includes_configured_export_symbols`) and opportunistic real-toolchain export validation (`apps/stasis::compiler_backend::tests::aot_compile_with_real_linker_exports_emitted_symbols_when_available`).
- Added opportunistic runtime loader-resolution coverage for true emitted AOT symbols (`apps/stasis::tests::emitted_aot_symbols_resolve_via_loader_when_real_link_available`) to verify emitted-symbol `SwapCommitRequest` payloads resolve to non-zero loaded addresses when real toolchain output is available.
- AOT stub emission now uses parsed simple `i32` return-body semantics when available (local `let` bindings, local reassignment with `= += -= *= /= %=`, and arithmetic expression trees with precedence, parentheses, unary minus, and `+ - * / %`) with deterministic body-hash fallback for unsupported bodies, and opportunistic real-toolchain coverage verifies emitted symbol invocation reflects source-body changes across recompiles (`apps/stasis::compiler_backend::tests::aot_emitted_symbol_return_changes_when_body_changes_if_real_link_available`).
- Simple `i32` return-body extraction now also supports deterministic `if`/`else if`/`else` chains with branch-local `let`/assignment evaluation and fallthrough continuation to later top-level `return` (`crates/stasis_compiler::tests::simple_i32_return_expr_supports_else_if_and_fallthrough_return`, `crates/stasis_compiler::tests::simple_i32_return_expr_supports_branch_local_statements_before_return`).
- AOT stub CLIF emission now lowers conditional return trees through `icmp` + `select` when extracted simple-body conditions are available (`apps/stasis::compiler_backend::tests::aot_stub_uses_icmp_and_select_for_conditional_expression`), and compiler extraction coverage now includes nested return-chain select generation (`crates/stasis_compiler::tests::simple_i32_return_expr_builds_nested_select_for_else_if_return_chain`).
- Added opportunistic real-toolchain emitted-symbol execution coverage for conditional branch semantics in `AotProd` (`apps/stasis::compiler_backend::tests::aot_emitted_symbol_executes_if_else_select_semantics_if_real_link_available`), asserting expected true/false branch return values via loader invocation.
- Added opportunistic runtime loader-override invocation coverage for conditional branch semantics (`apps/stasis::tests::emitted_aot_loader_overrides_execute_if_else_semantics_when_real_link_available`), validating `build_aot_code_ptr_overrides` dispatch addresses execute expected true/false branch values across recompiles.
- Added AOT stub signature regression coverage for `void` return functions (`apps/stasis::compiler_backend::tests::aot_stub_uses_void_signature_for_void_return_type`) and compiler metadata coverage for parsed return types (`crates/stasis_compiler::tests::parse_records_functions_and_hashes`).
- Added compiler semantic regression coverage for logical condition operators in simple `if` extraction (`crates/stasis_compiler::tests::simple_i32_return_expr_supports_logical_condition_operators`).
- Added AOT condition-lowering coverage for logical predicates in select conditions (`apps/stasis::compiler_backend::tests::aot_stub_uses_logical_condition_ops_for_select_conditions`) validating CLIF logical condition op emission (`bor`, `bnot`) before `select`.
- Added AOT branch-block lowering coverage for top-level conditional returns (`apps/stasis::compiler_backend::tests::aot_stub_uses_branch_blocks_for_top_level_conditional_expression`) validating `brif` branch emission and removal of top-level `select` fallback for supported conditional bodies.
- Added explicit short-circuit branch-block coverage for logical `&&` conditions in top-level conditional lowering (`apps/stasis::compiler_backend::tests::aot_stub_uses_short_circuit_branching_for_and_conditions`), validating multi-block `brif` lowering without `band` fallback.
- Incremental compiler metadata now records direct no-arg `i32` return-call target hashes for simple function bodies (`return callee();`) and surfaces this through host compile metrics (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler`).
- AOT stub emission now lowers direct no-arg `i32` return-call bodies to real CLIF calls when callee dispatch resolves uniquely from the patch-set metadata (`apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_when_simple_return_call_target_is_resolved`, `apps/stasis::compiler_backend::tests::resolve_simple_i32_return_call_target_symbol_returns_unique_match`).
- Added opportunistic emitted-symbol runtime parity coverage for direct no-arg call-return lowering (`apps/stasis::compiler_backend::tests::aot_emitted_symbol_executes_direct_call_semantics_if_real_link_available`) to verify `main` dispatch matches `callee` export invocation in linked `AotProd` output.
- Added AOT compatibility-gate rejection coverage for unresolved direct-call dispatch (`apps/stasis::compiler_backend::tests::aot_compile_rejects_unresolved_direct_call_target`) so unresolved direct-call bodies fail compile instead of silently falling back.
- Added runner rollback-path coverage for unresolved direct-call compile rejection in real backend `AotProd` mode (`apps/stasis::tests::real_backend_aot_compile_failure_skips_commit_for_unresolved_direct_call_target`), asserting compile failure increments while swap commit remains skipped.
- Incremental compiler metadata now records direct `i32` return-call additive-offset bodies (`return callee() +/- <int_literal>;`) and surfaces signed offset deltas through host compile metrics (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_add_delta`).
- AOT stub emission now lowers direct `i32` return-call additive-offset bodies to emitted call + integer add/sub sequence (`apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_add_delta_when_metadata_is_resolved`).
- Incremental compiler metadata now records simple two-call `i32` return bodies (`return lhs() +/- rhs();`) via left/right callee id-hash and operation code capture (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_two_call_metadata`).
- AOT stub emission now lowers simple two-call `i32` return bodies to emitted left/right call + `iadd/isub` sequence, with compatibility-gate rejection when either callee dispatch cannot be resolved (`apps/stasis::compiler_backend::tests::aot_stub_uses_two_call_add_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::aot_stub_uses_two_call_sub_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::aot_compile_rejects_unresolved_two_call_target`).
- Added runner rollback-path coverage for unresolved two-call compile rejection in real backend `AotProd` mode (`apps/stasis::tests::real_backend_aot_compile_failure_skips_commit_for_unresolved_two_call_target`), asserting compile failure increments while swap commit remains skipped.
- Incremental compiler metadata now records simple one-arg `i32` return-call bodies (`return callee(<int_literal>);`) and surfaces callee id-hash + literal payload through host compile metrics (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_metadata`).
- AOT stub emission now lowers simple one-arg `i32` return-call bodies to emitted direct-call `external callee(i32) -> i32`, with compatibility-gate rejection when target dispatch cannot be resolved to a unique `i32 <- (i32)` signature (`apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_one_i32_arg_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::aot_compile_rejects_unresolved_one_arg_direct_call_target`).
- Added runner rollback-path coverage for unresolved one-arg direct-call compile rejection in real backend `AotProd` mode (`apps/stasis::tests::real_backend_aot_compile_failure_skips_commit_for_unresolved_one_arg_direct_call_target`), asserting compile failure increments while swap commit remains skipped.
- Incremental compiler metadata now records nested one-arg `i32` return-call bodies (`return callee(arg_fn());`) via outer callee id-hash + argument-call target id-hash capture (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_call_arg_metadata`).
- AOT stub emission now lowers nested one-arg `i32` return-call bodies to emitted `arg_fn()` call feeding `callee(i32)` call, with compatibility-gate rejection when the argument-call target dispatch is unresolved (`apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_one_call_arg_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::aot_compile_rejects_unresolved_one_arg_direct_call_arg_target`).
- Added runner rollback-path coverage for unresolved one-arg argument-target compile rejection in real backend `AotProd` mode (`apps/stasis::tests::real_backend_aot_compile_failure_skips_commit_for_unresolved_one_arg_direct_call_arg_target`), asserting compile failure increments while swap commit remains skipped.
- Incremental compiler metadata now records additive-offset variants for one-arg call-return bodies (`return callee(<int_literal>) +/- <int_literal>;`, `return callee(arg_fn()) +/- <int_literal>;`) through the shared direct-call add-delta metric (`crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_literal_add_delta`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_call_arg_add_delta`).
- AOT stub emission now lowers additive-offset one-arg call-return bodies to emitted callee-call + integer add/sub (and arg-call prelude where applicable) (`apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_one_i32_arg_and_add_delta_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_one_call_arg_and_add_delta_when_metadata_is_resolved`).
- Incremental compiler metadata now records simple void `print_i32(<int_literal>)` bodies and surfaces literal payloads through host compile metrics (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler`).
- AOT stub emission now lowers simple void `print_i32(<int_literal>)` bodies to explicit extern calls with deterministic CLIF coverage (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_for_simple_void_print_metadata`).
- Incremental compiler metadata now records simple void `print_i32(callee())` bodies via callee id-hash capture and surfaces this through host compile metrics (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_call_target_hash`).
- AOT stub emission now lowers simple void `print_i32(callee())` bodies to emitted direct call + host extern call sequence, with compatibility-gate rejection when callee dispatch cannot be resolved (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_direct_call_target_for_simple_void_metadata`, `apps/stasis::compiler_backend::tests::aot_compile_rejects_unresolved_void_print_call_target`).
- Incremental compiler metadata now records additive-offset simple void print-call bodies (`print_i32(callee() +/- <int_literal>)`) via void-print call add-delta metrics (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_call_target_add_delta`).
- AOT stub emission now lowers additive-offset simple void print-call bodies to emitted direct call + integer add/sub + host print call (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_direct_call_target_and_add_delta_when_metadata_is_resolved`).
- Incremental compiler metadata now folds simple literal-expression void print bodies (`print_i32(<int_literal> +/- <int_literal>)`) into literal print payloads (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_literal_add_expression`).
- Incremental compiler metadata now records one-arg simple void print-call bodies (`print_i32(callee(<int_literal>))`) via callee id-hash + argument literal capture (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_one_arg_call_target_and_literal`).
- AOT stub emission now lowers one-arg simple void print-call bodies to emitted `callee(i32)` call + host print call (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_direct_one_i32_arg_call_target_for_simple_void_metadata`).
- Incremental compiler metadata now records nested one-arg simple void print-call bodies (`print_i32(callee(arg_fn()))`) via callee id-hash + argument-call target id-hash capture (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_one_arg_call_target_with_arg_call`).
- AOT stub emission now lowers nested one-arg simple void print-call bodies to emitted `arg_fn()` call feeding `callee(i32)` then host print call (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_direct_one_call_arg_call_target_for_simple_void_metadata`).
- Incremental compiler metadata now records one-arg literal additive-offset simple void print-call bodies (`print_i32(callee(<int_literal>) +/- <int_literal>)`) via shared void-print call add-delta metrics plus one-arg call metadata (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_one_arg_call_target_literal_add_delta`).
- AOT stub emission now lowers one-arg literal additive-offset simple void print-call bodies to emitted `callee(i32)` call + integer add/sub + host print call (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_direct_one_i32_arg_call_target_and_add_delta_for_simple_void_metadata`).
- Incremental compiler metadata now also recognizes one-arg literal-expression simple void print-call bodies (`print_i32(callee(<i32_literal> (+|-|*|/|%) <i32_literal>))`, including additive-offset form), folds the inner expression into existing one-arg literal channels, preserves callee id-hash capture, and rejects divide/mod-by-zero folds (`crates/stasis_compiler::tests::compile_records_simple_void_print_i32_one_arg_call_target_with_literal_multiply_expression`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_one_arg_call_target_with_literal_divide_expression_add_delta`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_one_arg_call_target_with_literal_mod_expression_add_delta`, `crates/stasis_compiler::tests::compile_does_not_fold_simple_void_print_i32_one_arg_call_target_literal_divide_by_zero_expression`, `crates/stasis_compiler::tests::compile_does_not_fold_simple_void_print_i32_one_arg_call_target_literal_mod_by_zero_expression`).
- Token-shape matching for one-arg literal-expression call bodies is now deduplicated in `.stasis` helper functions (`parser_match_literal_binary_expression_at`, `parser_fold_literal_binary_expression_at`, `parser_match_return_callee_one_arg_literal_expression`, `parser_match_simple_void_print_i32_callee_one_arg_literal_expression`) and reused by both `i32` return-call and void print-call metadata detectors to reduce fragile offset-chain divergence while keeping parse/orchestration ownership in `compiler/simple_pass_compiler.stasis`.
- Incremental compiler metadata now records nested one-arg additive-offset simple void print-call bodies (`print_i32(callee(arg_fn()) +/- <int_literal>)`) via one-arg argument-target metadata + shared void-print call add-delta metrics (`compiler/simple_pass_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_one_arg_call_target_with_arg_call_add_delta`).
- AOT stub emission now lowers nested one-arg additive-offset simple void print-call bodies to emitted `arg_fn()` call feeding `callee(i32)` + integer add/sub + host print call (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_direct_one_call_arg_call_target_and_add_delta_for_simple_void_metadata`).
- Incremental compiler metadata now records simple void two-call print bodies (`print_i32(lhs() +/- rhs())`) in `.stasis` and routes them through existing shared two-call channels (`function_simple_i32_return_two_call_*`) to avoid new Rust-side compiler schema ownership (`compiler/simple_pass_compiler.stasis`).
- Added `.stasis` fixture coverage for void two-call print metadata routing (`tests/stasis/run_incremental_void_print_two_call_metrics.stasis`) and host-bridge regression coverage (`crates/stasis_compiler::tests::compile_records_simple_void_print_i32_two_call_metadata`).
- AOT stub emission now lowers void two-call print bodies to emitted left/right call + add/sub + host print call (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_two_call_targets_for_simple_void_metadata`).
- Added runner rollback-path coverage for unresolved void print-call compile rejection in real backend `AotProd` mode (`apps/stasis::tests::real_backend_aot_compile_failure_skips_commit_for_unresolved_void_print_call_target`), asserting compile failure increments while swap commit remains skipped.
- Added `AotProd` commit compatibility-gate coverage for missing function-symbol mapping metadata (`apps/stasis::tests::aot_commit_rejects_missing_function_symbol_mapping_metadata`).
- Added `AotProd` commit compatibility-gate coverage for missing symbol entries for patched `FnId`s (`apps/stasis::tests::aot_commit_rejects_missing_symbol_for_patched_function_id`).
- Added `AotProd` commit compatibility-gate coverage for loader-mode missing export resolution when symbol mapping exists (`apps/stasis::tests::aot_commit_rejects_loader_mode_when_symbol_export_is_missing`).
- Added `AotProd` commit compatibility-gate coverage for duplicate symbol-mapping entries targeting the same `FnId` (`apps/stasis::tests::aot_commit_rejects_duplicate_symbol_mapping_for_fn_id`).
- Added rollback-path coverage for second-commit `AotProd` missing-symbol-mapping failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_missing_symbol_mapping_preserves_previous_active_artifact`).
- Added rollback-path coverage for second-commit `AotProd` missing patched-`FnId` symbol failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_missing_symbol_for_patched_fn_preserves_previous_active_artifact`).
- Added rollback-path coverage for second-commit `AotProd` loader-mode missing export resolution failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_missing_loader_export_preserves_previous_active_artifact`).
- Added rollback-path coverage for second-commit `AotProd` missing linked-image path metadata failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_missing_linked_image_path_preserves_previous_active_artifact`).
- Added rollback-path coverage for second-commit `AotProd` duplicate-symbol-mapping commit failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_duplicate_symbol_mapping_preserves_previous_active_artifact`).
- Added rollback-path coverage for second-commit `AotProd` missing linked-image size metadata failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_missing_linked_image_size_metadata_preserves_previous_active_artifact`).
- Added rollback-path coverage for second-commit `AotProd` missing linked-image hash metadata failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_missing_linked_image_hash_metadata_preserves_previous_active_artifact`).
- Migration gate M0/H0 completed: host compile analysis now runs through `.stasis` harness output only; Rust-side analyzer/simple-expression parsing removed.
- Added `.stasis` semantic validation for invalid `from_*` expression usage (`4001`) in `compiler/simple_pass_compiler.stasis`.
- Done gate:
- Hook errors abort swap with clear diagnostics.
- Status: `in_progress`
- Remaining:
- Slice H1: Execute `on_code_swap` from real lowered hook bodies in `AotProd` loader mode (remove simple-extraction dependency for supported hook bodies).
- Slice H2: Add hook parity fixture for deterministic state mutation (`on_code_swap`) and verify match vs current JIT/dev behavior.
- Slice H3: Add hook parity fixture for branch-dependent state mutation (`on_code_swap` with runtime condition paths).
- Slice H4: Add hook parity fixture for hook-side intra-program call effects where supported by real lowered hook codegen.
- Slice H5: Add hook parity fixture for hook-side extern/host-call effects where supported by real lowered hook codegen.
- Slice H6: Add rollback coverage for real lowered hook execution call-failure/unresolved-dispatch modes with previous generation preservation checks.
- Slice H7: Add rollback coverage for explicit hook failure-signal modes with previous generation preservation checks.
- Slice H8: Add compatibility-gate rejection coverage specific to real lowered hook dispatch (hook signature/layout/body incompatibility combinations).

### S10b - Self-Hosted AOT CLI Core
- Language:
- `.stasis` (orchestration) + minimal `Rust` host extern bridge
- Scope:
- Introduce a `.stasis` compiler CLI core entry that owns compile orchestration policy:
- enumerate source files from project dir via host extern bridge
- load each `.stasis` source via host extern bridge
- stage sources into `compiler/simple_pass_compiler.stasis` file DB
- run compile/entry validation and emit diagnostics
- delegate AOT artifact emission through a single host bridge call for now
- Current implementation:
- Added `compiler/stasis_aot_cli_core.stasis` with `compiler_cli_compile_project(...)` orchestration and host-extern bridge declarations (`host_source_file_count`, `host_load_source_file`, and staged AOT bridge calls `host_emit_ir_from_compiler_state`, `host_run_cranelift_aot`, `host_link_executable_from_objects`, `host_write_aot_cli_summary`, `host_set_summary_file`).
- Added `compiler/stasis_aot_cli_entry.stasis` with argv-driven CLI entry orchestration (`compiler_cli_main_from_argv` + `main`) and host-extern CLI argument bridge declarations (`host_cli_arg_count`, `host_cli_arg_value`) to define stage1 executable argument contract in `.stasis`.
- Added `.stasis` parser fixture coverage for bridge surface shape (`tests/stasis/run_parser_self_host_aot_cli_bridge_counts.stasis`).
- Added runnable host bridge command path `stasis aot-cli --project-dir <dir> --out <exe>` that:
- enumerates `.stasis` files from directory
- compiles via `.stasis`-driven incremental analysis path
- links emitted Cranelift AOT objects to a runnable executable using resolved `main` entry symbol
- Host bridge implementation now runs through explicit staged functions in order:
- `host_emit_ir_from_compiler_state` (produces staged IR-bundle metadata)
- `host_run_cranelift_aot` (produces staged object-bundle metadata from emitted Cranelift AOT objects)
- `host_link_executable_from_objects` (links runnable executable with resolved `main` entry symbol)
- Added deterministic fake-toolchain coverage for executable linker path and CLI flow (`crates/stasis_jit::tests::aot_executable_linker_can_be_driven_by_configured_fake_linker`, `apps/stasis::compiler_backend::tests::self_host_aot_cli_links_runnable_executable_with_main_entry_symbol`).
- `stasis aot-cli` now supports writing a machine-readable summary artifact (`--summary-file <path>`) containing staged bundle paths, entry symbol, and object layout contract for stage parity checks.
- `stasis aot-cli` now supports optional `--entry-file <file.stasis>` host contract routing (`STASIS_AOT_ENTRY_FILE`) so self-host AOT can compile a selected program when a project directory contains multiple `main()` declarations; backend source discovery now resolves project-local import closure from that entry file instead of compiling every `.stasis` file under the directory.
- `stasis aot-cli` now supports optional `--quality-gate` (`STASIS_AOT_QUALITY_GATE=1`) which rejects outputs when the selected entry symbol is still fallback-stub lowered, preventing shipping non-playable placeholder executables as "quality" game builds.
- `.stasis` incremental parser compatibility was extended for Brickout `_v1` source forms used by self-host AOT CLI input: top-level `import`, `const`, `enum`, `struct`, and `global name: Type;` declarations, `function @inline ...` signatures, float literal tokenization compatibility (`123.45` treated as a contiguous numeric primary token), and comment-aware `for` header parsing (all `for` header segments are required; missing-segment forms are rejected).
- Verified self-host `.stasis` compile path advances through Brickout `_v1` parsing/incremental analysis and now fails at host linker invocation stage when linker tooling is unavailable (`link.exe`/`STASIS_AOT_LINKER`), indicating parser-side `_v1` compatibility blockers are removed for this slice.
- With `STASIS_AOT_LINKER` set to `lld-link.exe`, Brickout `_v1` self-host compile advances past linker discovery and currently fails on runtime-bridge object unresolved runtime symbols (`core::panicking::*`, `memset`) during final executable link, making runtime-bridge/object-link contract the active blocker.
- Runtime bridge executable-link fallback now retries with CLIF bridge object when rustc-emitted bridge object fails to link on Windows toolchains (e.g., `lld-link` unresolved runtime symbols), unblocking self-host AOT executable emission for Brickout `_v1`.
- Linker spawn failures now emit deterministic self-host guidance with `STASIS_AOT_LINKER` override instructions (including missing default `link.exe`/`cc` toolchain hints) and regression coverage in `crates/stasis_jit`.
- Self-host compiler source staging buffer contract was raised to `262144` bytes (`compiler_state`, `.stasis` AOT CLI core temp source buffer, host analysis harness buffer, runtime bridge source-load buffer) so current `compiler/simple_pass_compiler.stasis` size no longer hard-fails lexing during stage analysis.
- Added opt-in real-toolchain compiler-subset build smoke (`STASIS_RUN_REAL_SELF_HOST_COMPILER_SUBSET_BUILD_SMOKE=1`) that compiles the self-host compiler entry import-closure subset (entry/core/incremental/state/stdlib) via `--entry-file`, asserting 5-file staged contract build viability.
- Added opt-in real-toolchain stage1 executable parity probe (`STASIS_RUN_REAL_SELF_HOST_STAGE1_EXEC_PARITY_SMOKE=1`) that publishes argv/source/staged-bridge env contracts and executes compiled stage1 compiler subset binary for stage2 summary generation; probe now completes with exit `0` and stage2 summary parity via runtime-bridge CLI-entry host extern routing.
- AOT patch manifests now include fallback stub detail hints (`symbol`, `id_hash`, `sig_hash`, `body_hash`, `ordinal`) so stage1 executable parity failures can map exit-code body hashes back to concrete fallback symbols/functions deterministically during diagnostics.
- `compiler/stasis_aot_cli_entry.stasis` now lowers `compiler_cli_parse_from_argv` as a direct no-arg host extern call (`host_run_self_host_aot_cli_from_env`), and runtime bridge exports this symbol in both rustc and CLIF fallback objects so default stage1 executable path no longer depends on parse-bridge lowering.
- Temporary parse-bridge shim scaffolding and parse-bridge-specific quality/reject gates were removed; unlowerable entry functions now surface through normal fallback-stub manifest diagnostics and strict/quality fallback gates.
- Direct-call lowering now recognizes known host no-arg `i32` extern targets (`host_cli_arg_count`, `host_run_self_host_aot_cli_from_env`) when resolving call-target symbols, so this path no longer hard-fails unresolved-target gating for AOT stub emission.
- No-arg direct-call target resolution now enforces zero-parameter callee signature (`param_count == 0`) so signature-mismatched candidates (for example one-arg callees) are rejected deterministically instead of being lowered as no-arg calls.
- Simple-pass CLIF emission now attempts deterministic no-arg `i32` function evaluation (locals, assignment, `if` branches, and intra-file calls with up to four `i32` args) before hash fallback, enabling exact executable-path verification for conditional-addition `main` fixtures.
- Simple-pass parser now builds deterministic flattened global field layout metadata (nested struct fields included) and lowers direct nested global set/read shape (`State.first_enemy.hp = 7; return State.first_enemy.hp;`) to CLIF direct address + `store`/`load` operations (no runtime hash lookup) against a deterministic shared arena symbol (`sp_global_mem_layout_<layout_hash>`) in the current entry-main lowering path.
- Added real-backend JIT smoke for `for` accumulation fixture (`apps/stasis::tests::real_backend_smoke_compiles_and_commits_for_accumulation_main`) using `tests/stasis/run_main_for_accumulation_returns_6.stasis`.
- Added real-toolchain self-host AOT executable smoke for `for` accumulation main (`apps/stasis::compiler_backend::tests::self_host_aot_cli_runs_for_accumulation_main_if_real_toolchain_available`), verifying exit code `6` end-to-end.
- Self-host AOT CLI now links directly from in-process AOT object files and no longer depends on the staged runtime-bridge helper module.
- Added opt-in real-toolchain staged extern smoke (`STASIS_RUN_REAL_RUNTIME_BRIDGE_STAGED_EXTERN_SMOKE=1`) that links and executes a runtime-bridge driver executable, asserting live `host_emit_ir_from_compiler_state`/`host_run_cranelift_aot`/`host_link_executable_from_objects`/`host_write_aot_cli_summary` behavior via env-backed contracts.
- CLIF fallback runtime bridge now mirrors env-backed staged AOT extern behavior for `host_emit_ir_from_compiler_state`, `host_run_cranelift_aot`, `host_link_executable_from_objects`, and `host_write_aot_cli_summary` (using Win32 `GetEnvironmentVariableA`/`SetEnvironmentVariableA`/`CopyFileA`), with opt-in real-toolchain fallback coverage (`STASIS_RUN_REAL_RUNTIME_BRIDGE_CLIF_STAGED_EXTERN_SMOKE=1`).
- CLIF fallback runtime bridge now also uses env-backed CLI/source extern behavior for `host_cli_arg_count`, `host_cli_arg_value`, `host_source_file_count`, and `host_load_source_file` (indexed env-key selection with deterministic key tables), with opt-in real-toolchain fallback coverage (`STASIS_RUN_REAL_RUNTIME_BRIDGE_CLIF_ARG_SOURCE_SMOKE=1`).
- Added host regression coverage for entry-file CLI parsing and project-local import closure selection (`apps/stasis::tests::parse_aot_cli_contract_args_accepts_entry_file_flag`, `apps/stasis::compiler_backend::self_host_file_selection_tests::self_host_project_entry_selects_project_local_import_closure`).
- Strengthened ownership guard coverage for `.stasis` orchestration boundaries: `apps/stasis::tests::aot_cli_host_glue_stays_out_of_compile_orchestration` now also rejects staged bridge/incremental orchestration calls in host CLI glue, `apps/stasis::tests::aot_cli_orchestration_contract_is_declared_in_stasis_sources` validates `.stasis` contract ownership, and `crates/stasis_compiler::tests::self_host_cli_orchestration_contract_stays_in_stasis_files` enforces the same boundary from compiler crate tests.
- Added process-env serialization coverage for opt-in real-toolchain self-host/runtime-bridge smokes (`apps/stasis::compiler_backend::tests::*if_real_toolchain_available`) by taking the shared `stasis_process_env_lock`, reducing CI/load variance from concurrent `STASIS_*` env mutation.
- Incremental parser + AOT lowering now support one-arg first-parameter passthrough return-call wrappers (`return callee(param0);`, including additive-offset form) and resolve known host single-arg `i32` extern targets (`host_set_summary_file`, `host_source_file_count`) for this shape; coverage: `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_first_param_passthrough_metadata`, `apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_first_param_passthrough_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::resolve_simple_i32_return_one_arg_target_symbol_supports_known_host_single_arg_extern`, `apps/stasis::compiler_backend::tests::aot_compile_accepts_known_host_one_arg_passthrough_direct_call_target`.
- Incremental parser + AOT lowering now support first/second, first/second/third, and first/second/third/fourth parameter passthrough wrappers for `i32` return-call shapes (including additive-offset form) and resolve known host argument-bearing extern targets for these wrappers (`host_cli_arg_value`, `host_write_aot_cli_summary`, `host_load_source_file`); coverage: `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_first_second_param_passthrough_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_first_second_third_param_passthrough_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_first_second_third_fourth_param_passthrough_metadata`, `apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_first_second_param_passthrough_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_first_second_third_param_passthrough_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_first_second_third_fourth_param_passthrough_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::resolve_simple_i32_return_two_arg_passthrough_target_symbol_supports_known_host_extern`, `apps/stasis::compiler_backend::tests::resolve_simple_i32_return_three_arg_passthrough_target_symbol_supports_known_host_extern`, `apps/stasis::compiler_backend::tests::resolve_simple_i32_return_four_arg_passthrough_target_symbol_supports_known_host_extern`, `apps/stasis::compiler_backend::tests::aot_compile_accepts_known_host_two_arg_passthrough_direct_call_target`, `apps/stasis::compiler_backend::tests::aot_compile_accepts_known_host_three_arg_passthrough_direct_call_target`, `apps/stasis::compiler_backend::tests::aot_compile_accepts_known_host_four_arg_passthrough_direct_call_target`.
- Incremental parser + AOT lowering now also support mixed literal+passthrough wrapper shape for argument-bearing host externs (`return callee(<i32_literal>, param0);`, `return callee((<i32_literal>), param0);`, `return callee(<i32_literal> (+|-|*|/|%) <i32_literal>, param0);`, and `return callee((<i32_literal> (+|-|*|/|%) <i32_literal>), param0);`, including additive-offset form) and resolve known host target `host_cli_arg_value` for this pattern; coverage: `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_literal_first_second_param_passthrough_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_literal_first_second_param_passthrough_add_delta_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_parenthesized_literal_first_second_param_passthrough_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_parenthesized_literal_first_second_param_passthrough_add_delta_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_literal_expression_first_second_param_passthrough_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_literal_expression_first_second_param_passthrough_add_delta_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_parenthesized_literal_expression_first_second_param_passthrough_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_parenthesized_literal_expression_first_second_param_passthrough_add_delta_metadata`, `apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_literal_first_second_param_passthrough_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::resolve_simple_i32_return_two_arg_literal_first_second_param_passthrough_target_symbol_supports_known_host_extern`, `apps/stasis::compiler_backend::tests::aot_compile_accepts_known_host_two_arg_literal_first_second_param_passthrough_direct_call_target`, `apps/stasis::compiler_backend::tests::aot_compile_accepts_known_host_two_arg_parenthesized_literal_first_second_param_passthrough_direct_call_target`, `apps/stasis::compiler_backend::tests::aot_compile_accepts_known_host_two_arg_parenthesized_literal_first_second_param_passthrough_add_delta_direct_call_target`, `apps/stasis::compiler_backend::tests::aot_compile_accepts_known_host_two_arg_literal_expression_first_second_param_passthrough_direct_call_target`, `apps/stasis::compiler_backend::tests::aot_compile_accepts_known_host_two_arg_parenthesized_literal_expression_first_second_param_passthrough_direct_call_target`.
- Incremental parser now also recognizes non-wrapper one-arg literal-expression call shapes (`return callee(<i32_literal> +/- <i32_literal>);`, `return callee(<i32_literal> * <i32_literal>);`, `return callee(<i32_literal> / <i32_literal>);`, `return callee(<i32_literal> % <i32_literal>);` with nonzero divisor for foldable `/` and `%`), including additive-offset form, folds the inner argument expression into existing one-arg literal metadata, and lowers through existing direct-call path without fallback for resolved targets; coverage: `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_literal_expression_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_literal_expression_add_delta_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_literal_multiply_expression_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_literal_multiply_expression_add_delta_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_literal_divide_expression_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_literal_mod_expression_add_delta_metadata`, `crates/stasis_compiler::tests::compile_does_not_fold_simple_i32_return_call_one_arg_literal_divide_by_zero_expression`, `apps/stasis::compiler_backend::tests::aot_compile_accepts_one_arg_literal_expression_direct_call_target`.
- Incremental parser + AOT lowering now also support parenthesized non-wrapper one-arg literal-expression call shapes (`return callee((<i32_literal> (+|-|*|/|%) <i32_literal>));` with optional additive offset), folding the inner expression into existing one-arg literal metadata and preserving direct-call lowering without fallback for resolved targets; coverage: `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_parenthesized_literal_expression_metadata`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_parenthesized_literal_expression_add_delta_metadata`, `apps/stasis::compiler_backend::tests::aot_compile_accepts_one_arg_parenthesized_literal_expression_direct_call_target`.
- Deliverable:
- `.stasis`-owned core compile CLI orchestration exists and is test-covered independently of runtime watch loop.
- Tests:
- `.stasis` parser/incremental fixture coverage for host bridge extern surface and core function declarations.
- Done gate:
- Core compile orchestration policy remains in `.stasis`; Rust bridge contains no compile-policy branching.
- Status: `in_progress`
- Remaining:
- Immediate remaining (current focus):
- Simple-path rule: keep compilation one-pass by default (`parse/check/lower per function`) with explicit exceptions only for required pre-scan metadata and jump fixup resolution.
- Simple-path rule: no new parser-shape/fallback-detector expansions; if a path depends on detector branching, replace or delete it instead of extending it.
- DCE task: implement function reachability graph in `.stasis` and mark reachable symbols from roots `{main, tick, on_code_swap}` plus host-required exported entries.
- DCE progress note: root closure + direct-call reachability and reachable-only function emission are wired, host-required entry roots are injectable from Rust host harness into `.stasis`, and cross-file reachability now runs in `.stasis` using a hashed file-function index (bucketed slot chains) instead of repeated full scans.
- DCE task: implement struct reachability from reachable function signatures/body type references + reachable globals, and prune unreachable struct metadata before lowering.
- DCE progress note: layout reachability now marks reachable globals from reachable function bodies, marks struct closure from reachable function type annotations + reachable globals, and prunes unreachable struct/global layout metadata before flattened offset lowering; remaining work is host-required export root wiring and broader cross-file closure semantics.
- DCE task: gate Cranelift emission to reachable functions only (JIT + AOT paths) and remove unreachable symbol emission.
- DCE task: remove legacy `simple_*` detector metric channels from compiler state + host parsing once reachability contracts are wired end-to-end.
- Cleanup task: aggressively delete non-conforming detector/fallback metadata paths and obsolete tests as reachability-lowered paths replace them.
- Lowering task: enforce compact lowering-state invariants at statement/function boundaries (`value stack`, `block depth`, `pending jumps`) with deterministic diagnostics on violation.
- Lowering task: add only a tiny local post-emit cleanup pass (for example trivial dead-branch/terminator cleanup) before Cranelift handoff; no broad optimizer track.
- Tooling task: keep one optional diagnostic/instrumented mode on the same pipeline (extra invariant checks + trace markers), not a separate compile path.
- Keep full `apps/stasis` test suite stable under CI/load timing variance (watch/AOT failure-path regressions) while preserving deterministic assertions.
- Enforce the 5-minute (300-second) per-command test budget by running bounded targeted groups; if a command exceeds budget, treat it as a regression signal and split/optimize before continuing slices.
- Process enforcement task: keep narrow slice commits (avoid mixing reachability/DCE work with unrelated backend/runtime work in one commit).
- Process enforcement task: keep bounded targeted test groups plus explicit post-step lingering-process checks as required workflow.
- Dual-lane execution policy (effective now):
- Maintain two active lane lists: `Language/Compiler Completeness` and `Windows Runtime/UI Proving`.
- Alternate slices between lanes (`A1 -> B1 -> A2 -> B2 ...`) to continuously prove language work in runtime context.
- If blocked on one lane (tooling, external dependency, unresolved design gate), pause that lane, record blocker in commit/PR notes, and continue with the next slice from the other lane.
- Lane A (`Language/Compiler Completeness`) priority queue:
- `CS1` (finish fully in-process compile path), then `CS2`, `CS3`, `CS4`, `CS5`, `CS6`, `CS7`, `CS8`.
- `S10/S10b` completion and remaining `S13/S14/S15/S16` host-set and phase/budget policy work.
- Lane B (`Windows Runtime/UI Proving`) priority queue:
- Wire runtime consumption and validation of mode outputs (`JitEnginePackage` in dev, `AotEngineBundle` in prod) through commit/runtime path.
- Keep graphics/window/render-loop implementation in Rust host runtime code.
- In dev/runtime iteration, use in-process JIT compile outputs as the active execution path (`JitEnginePackage`) so edits stay fast.
- In production/release path, use AOT bundle outputs (`AotEngineBundle`) for packaged/runtime execution.
- Add deferred engine-overhead benchmark/test (`package/load/swap/render-loop`) and baseline it separately from compiler-only timings.
- Add deferred engine hot-update latency benchmark for warm edits: measure `watch change -> compile -> commit/swap -> first tick/render with new code` and track p50/p95 in dev JIT mode (target: keep typical warm updates under 100ms on Brickout-scale scenarios).
- Strengthen Windows executable/runtime parity smokes (JIT + AOT) on Brickout-oriented scenarios.
- Lane B progress note:
- Runtime commit path now supports JIT `FnId -> code_ptr` override application sourced from compile results when available (dev path can consume real JIT pointers instead of synthetic placeholder pointer generation).
- Added engine-overhead benchmark harness for package/load/swap/render-loop timing slices: `cargo run -p stasis --release --example engine_overhead_bench -- --mode both --samples 3 --ticks 240`.
- Initial smoke snapshot recorded (single-sample run) in `docs/rust_native_compiler_prd.md` for both JIT and AOT runtime-overhead timing breakdown.
- Runtime real-backend JIT smoke fixtures are now rust-native-compatible (`literal`, `binary literal`, `on_code_swap + literal`) so no-fallback dev mode stays deterministic while richer syntax slices are implemented.
- Compile-speed lock-in checklist (PRD v2, current top compiler priority):
- Scope anchor doc: `docs/compiler_prd_v2_compile_speed.md`.
- Target gates:
- Single-function incremental edit (1k function project): typical end-to-end compile path <= 5ms.
- Cold start (1k function project): <= 250ms.
- Cold start (5k function project): <= 1000ms.
- Deterministic output and deterministic invalidation behavior are required for all targets.
- Constraint gates:
- Keep `.stasis` as compiler logic owner; Rust remains host/runtime glue only.
- Keep compiler data flat and fixed-cap where practical (`ascii[n]`, fixed arrays, index-based adjacency); avoid new per-function heap allocations.
- Avoid new parser-shape fallback expansion; prefer direct parse/emit and delete non-conforming paths when touched.
- Windows policy hardening: keep executable signing in the default execution path for local `cargo run/test` on Windows via repo runner hook, with strict mode available (`STASIS_REQUIRE_SIGNED_EXECUTION=1`).
- Slice CS0: Add deterministic compile-time benchmark harness and baselines.
- Language: `Rust + .stasis` (measurement only; no policy ownership shift).
- Scope: add repeatable benchmark fixtures for 1k/5k function projects and emit p50/p95 timings for cold/incremental runs.
- Deliverable: benchmark command(s) and checked-in fixture generator with deterministic seed.
- Tests: benchmark smoke runs in CI-optional mode and validates output format/consistency.
- Done gate: baseline numbers are recorded in docs and used as acceptance checks for subsequent slices.
- Current progress:
- Added deterministic benchmark executable: `cargo run -p stasis_compiler --release --example compile_bench`.
- Benchmark executable now supports explicit mode selection: `--mode analysis` (in-process analysis only) and `--mode jit` (Cranelift machine-code generation via rust-native `JitProcess`).
- Added benchmark smoke/unit checks: `cargo test -p stasis_compiler --example compile_bench`.
- Baseline snapshot (2026-02-24, local machine, seed=1337, chunk_size=500, 1 sample each):
- 1k functions: cold p50/p95 `4390.080ms`, incremental p50/p95 `4280.470ms`.
- 5k functions: cold p50/p95 `7177.709ms`, incremental p50/p95 `4542.079ms` (completes within 5-minute budget).
- Benchmark hygiene note: incremental sample generation now forces a real body mutation per sample (no no-op edit timing).
- Host analysis now parallelizes per-file harness runs within a compile request (cold-start improvement; incremental unchanged).
- Status: `done`
- Slice CS1: Remove hot-path bootstrap harness process spawning from incremental compile path.
- Language: `Rust + .stasis`.
- Scope: replace per-file/project shell-out analysis with in-process compile-state invocation path.
- Deliverable: `compile_changed_files` no longer launches external bootstrap process during normal operation.
- Tests: existing incremental/reachability tests remain green; add explicit regression test asserting no external harness invocation on normal compile path.
- Done gate: single-function incremental compile path executes entirely in-process.
- Current progress:
- `crates/stasis_compiler::IncrementalCompilerHost::compile_changed_files` now analyzes changed sources fully in-process (threaded Rust parser/evaluator path), with no per-file external harness process launch in normal operation.
- Removed external harness-only tests and stale process-signing/override code paths from `crates/stasis_compiler/src/lib.rs`.
- Preserved in-memory reachability behavior and changed/newly-reachable emission behavior under the new in-process analysis path.
- `apps/stasis` runtime compile path now has an in-process engine-mode fast path: when `tick`+`render` entrypoints are present, backend compile bypasses legacy host analysis and compiles via rust-native JIT/AOT process contracts directly.
- JIT engine/non-engine compile paths in `apps/stasis` now keep a long-lived in-memory `JitProcess` per backend instance and apply changed-file upserts incrementally; full process rebuild only occurs on source removals.
- Non-engine `JitDev` compile path now runs rust-native JIT compilation only and emits explicit diagnostics on unsupported shapes; silent fallback to legacy host analysis was removed for this path.
- Rust-native JIT return-expression lowering now supports parameter identifiers and infix arithmetic (`+ - * / %`) in addition to literal-only returns, with in-memory two-arg execution coverage for verification.
- Rust-native JIT `i32` statement lowering now supports a simple block subset (`let` bindings and `if` branches with comparison conditions) with deterministic in-memory execution tests, while still keeping direct one-pass lowering.
- Rust-native JIT now supports direct `i32` call expressions with `0..2` arguments via in-process dispatch symbols (`callee()`, `callee(x)`, `callee(x,y)`) and validates these through in-memory execution tests.
- Rust-native JIT now supports receiver-form calls (`receiver.method(...)`) lowered as function-form calls (`method(receiver, ...)`) with compile-time signature-based target selection.
- Rust-native type interning now keeps user type names in signature metadata so overload resolution can distinguish same method names by receiver type.
- Rust-native JIT statement lowering now supports spec-defined compound assignment operators (`-=`, `*=`, `/=`, `%=`) in both regular statements and `for`-loop step clauses; non-spec loop keyword `while` remains rejected in this path.
- Rust-native JIT statement lowering now supports spec-defined `if/else if/else` chains (including deterministic branch execution verification via in-memory JIT tests) and keeps non-spec loop keywords rejected.
- Rust-native JIT condition lowering now supports spec-defined logical composition in control flow (`&&`, `||`, `!`, parenthesized grouping) for both `if` branches and `for` conditions, with deterministic in-memory verification.
- Rust-native JIT expression/statement lowering now supports dotted-path i32 global access through real runtime imports (`load/store`) so assignments/reads like `state.rng_state = ...; return state.rng_state;` compile in-process without external analysis fallback.
- Rust-native JIT now has initial non-i32 path support in the same pipeline: `f32`/`bool` signature lowering, float literals/arithmetic/comparisons, and conversion statements (`from_i32`, `from_f32`) for local bindings.
- Rust-native JIT parser now skips line comments (`// ...`) inside function bodies in the direct statement parser path.
- Rust-native JIT parser now also skips block comments (`/* ... */`) between statements in the direct statement parser path, with regression coverage in both in-memory JIT execution and AOT compile contract tests.
- Statement terminator + delimiter matching are now comment/string-aware in the direct parse path, preventing false statement splits on `;`/braces inside string literals or comments and allowing control-flow comments near delimiters (`if/*...*/(...)`, `for (...) /*...*/ { ... }`).
- Direct expression tokenization now skips both line and block comments inside expressions/conditions, enabling shapes like `let x = 1 /*a*/ + 2;` and `if (x /*lhs*/ == /*rhs*/ 3)` without parse failures.
- Condition and `for`-header top-level operator/segment scans now ignore comment payload, so operator-like tokens and `;` text inside comments do not affect parse boundaries (`/* || */`, `/* == */`, `/* ; */`).
- `for` control segments now accept call-expression init/step and `from_*` conversion init/step statements in the direct parser path (including global-path and indexed-path conversion targets), with deterministic in-memory JIT execution coverage and AOT compile-contract coverage for mixed call+conversion headers.
- `foreach` headers are `let`-only (`foreach (let value in items)`, `foreach (let value, i in items)`); non-`let` forms are rejected by the Rust-native compiler.
- Rust-native lowering now enforces no-shadowing local binding semantics across parameters, `let`, `for`-init `let`, and `foreach` item/index bindings with deterministic diagnostics.
- Rust-native parser now rejects `for` headers with any missing segment; `init`, `condition`, and `step` are all required (`for (; ...; ...)`, `for (...; ; ...)`, and `for (...; ...; )` fail deterministically).
- Direct parser/lowering now supports inferred `let` bindings (`let name = expr`) for local declarations, including inferred `for` init declarations (`for (let i = 0; ... )`) and inferred `f32` locals from float literals.
- Direct parser/lowering now enforces spec condition typing for expression-form conditions: `if (<expr>)` and `for (...; <expr>; ...)` require `<expr>` to be `bool`; numeric truthiness (`i32`/`f32`) is rejected with deterministic diagnostics.
- Brickout v1 input path now uses a per-tick Stasis snapshot model (`input_model`) refreshed once in `tick()` and consumed by gameplay/UI logic (`record_tap_pulses`, `handle_pointer_input_*`) instead of scattered direct input function reads.
- Added `.stasis` input-snapshot fixture coverage (`tests/stasis/rust_native_tick_input_snapshot.stasis`) with rust-native JIT in-memory execution test (`crates/stasis_compiler::backend::jit::tests::jit_process_executes_tick_from_stasis_fixture_with_input_snapshot`) proving test flow: seed snapshot in-language -> run `tick` -> verify result/state, with no real IO runtime dependency.
- Rust-native JIT now seeds fixed-collection `max_length` metadata into both header lanes and `.max_length` path slots for fixed arrays/`ascii[N]`/`utf8[N]`, and stdlib ASCII scan helpers now use explicit bounded loop limits (`ascii_scan_limit`) instead of literal-condition infinite loops.
- Added runtime regression coverage for fixed-array `max_length` initialization parity (header bytes + `.max_length` path): `crates/stasis_compiler::backend::jit::tests::jit_process_initializes_fixed_array_max_length_header_and_path`.
- Added imported-stdlib bounded-string regression coverage in rust-native JIT path: `crates/stasis_compiler::backend::jit::tests::jit_process_stdlib_ascii_copy_truncates_to_destination_capacity` and `crates/stasis_compiler::backend::jit::tests::jit_process_stdlib_ascii_recount_is_bounded_by_capacity`.
- UTF-8 stdlib copy path now clamps by header capacity (`utf8_max_length`) in addition to caller-provided bound, with rust-native JIT regression coverage: `crates/stasis_compiler::backend::jit::tests::jit_process_stdlib_utf8_from_ascii_clamps_to_header_capacity`.
- String length setters/getters in stdlib now clamp to header capacity (`ascii_set_len`/`length`, `utf8_set_byte_len`/`utf8_set_char_len`/`length_bytes`/`length_chars`), with rust-native JIT regression coverage: `crates/stasis_compiler::backend::jit::tests::jit_process_stdlib_ascii_set_len_clamps_to_max_length` and `crates/stasis_compiler::backend::jit::tests::jit_process_stdlib_utf8_set_len_ascii_clamps_to_max_length`.
- UTF-8 from-ASCII conversion now also bounds source reads by ASCII capacity (`ascii_scan_limit`) so unterminated source buffers cannot overrun reads, with rust-native JIT regression coverage: `crates/stasis_compiler::backend::jit::tests::jit_process_stdlib_utf8_from_ascii_respects_source_capacity_without_terminator`.
- Rust-native frontend parser now owns top-level `test` declaration discovery and rewrite (`parse_top_level_test_declarations`, `rewrite_top_level_test_declarations`), removing duplicate scanner logic from `apps/stasis`.
- Headless test command path keeps test-mode emit roots explicit from discovered declarations and supports recursive discovery for both `*.test.stasis` and `*.stasis` files containing top-level `test` declarations.
- Headless test discovery now uses a cheap prefilter (`test` + backtick) before full parse for non-`.test.stasis` files and skips `.git`/`target` directories during recursion to reduce discovery overhead in repo-root runs.
- Headless test discovery now also skips `.stasis_cache` directories (local generated cache files were inflating repo-root `.stasis` scan counts by ~3.3k files).
- CLI test path now supports watch mode: `stasis test --dir <path> --watch [--watch-settle-ms <ms>]` for repeated in-process reruns without process restart overhead.
- CLI watch test mode now defaults to `watch_settle_ms=0` (no debounce delay) unless explicitly provided.
- Watch-mode test reruns now persist per-file JIT compile state in-process and skip compile for unchanged source hashes (reuse existing compiled process/image for unchanged files).
- Watch-mode test reruns now also perform automatic per-file runtime rebind compile when switching between cached per-file JIT processes, preventing stalled reruns from cross-process runtime table state drift while preserving unchanged-file compile-skip for single-file sessions.
- Rust-native `JitProcess` now caches expensive compile-analysis metadata (call signatures, extern symbol bindings, constants, global path types, and `foreach` collection info) behind a loaded-file fingerprint key so unchanged stdlib/dependency metadata is reused across compile calls.
- Incremental JIT emit selection now treats cached artifacts as reusable only when both `function_id` and `body_hash` match, preventing stale artifact reuse after reindex function-id shifts (insert/remove/reorder) and forcing deterministic re-emit of affected reachable functions.
- Incremental JIT now forces reachable re-emit when compile-analysis semantics change (resolved extern signatures/symbol addresses, top-level constants, global path types, or `foreach` collection metadata), preserving correctness for edits that do not change function body hashes (for example imported constant updates).
- Import graph discovery now tolerates UTF-8 BOM-prefixed source files (`\u{feff}`) so top-level `import` directives remain detectable in watch/test flows for editor-generated BOM files.
- Added compiler dependency-invalidation regression coverage for fan-out and multi-level signature-change ripple closure in Rust-native index pass (`compiler::tests::signature_change_propagates_dirty_to_fan_out_dependents`, `compiler::tests::signature_change_propagates_dirty_through_multi_level_chain`).
- Added compiler regression coverage for signature-equivalent/no-op formatting edits (`compiler::tests::signature_equivalent_formatting_edit_does_not_dirty_or_emit`) to ensure no dirty-propagation or emit work when signature/body hashes are unchanged.
- Watch-session JIT cache reuse now probes loaded imported dependency files for on-disk drift and forces per-file recompile when dependencies change (without forcing recompile for unchanged test-root source), with regression coverage in `stasis_test_runner::tests::session_recompiles_when_imported_dependency_changes`.
- Test discovery now canonicalizes file paths before session caching/upsert so relative/mixed path inputs resolve consistently in watch/test JIT runs.
- JIT call-signature lookup map now uses `HashMap` in the hot call-resolution path (behavior unchanged; average-case lookup cost reduced versus ordered-map lookups).
- JIT import-graph loading now caches parsed top-level import lists by `(file path, source hash)` and reuses them across unchanged compiles; cache entries refresh deterministically when source hash/import sets change.
- Rust-native JIT unit tests are now process-serialized via a test-only global `JitProcess` guard to avoid cross-test global runtime table races; full `backend::jit::tests` module now runs stable in one pass.
- JIT shape tests now align with spec-enforced `for` header semantics (`init`, `condition`, and `step` all required); empty-segment shapes are asserted as deterministic compile errors.
- JIT runtime dispatch/code-pointer assembly now uses an internal `FunctionId -> artifact` index map (rebuilt after emit pass) so execution lookups and dispatch-table refresh avoid repeated linear artifact scans.
- Startup now performs stale `.stasis_cache` cleanup with a 7-day default TTL (`STASIS_CACHE_TTL_DAYS` override) so cache files are retained for short-term reuse but aged out automatically.
- Added shared host-boundary test helper module `src/runtime/input_testkit.stasis` and first Brickout `.test.stasis` fixture (`samples/brickout_revenge/brickout_revenge_v1_input_model.test.stasis`) so game tests set domain input/state without direct host-frame layout writes.
- Expanded Brickout `.test.stasis` coverage to include gameplay-side `record_tap_pulses()` assertions sourced from `input_testkit` snapshot input (`tests_discovered=2` in `samples/brickout_revenge` test dir).
- Expanded Brickout `.test.stasis` coverage further (`tests_discovered=4`) with explicit assertions for inactive-pointer slot clearing in `refresh_input_model()` and occupied-slot skip behavior in `record_tap_pulses()`.
- Added pure-core Brickout `.test.stasis` coverage file (`samples/brickout_revenge/brickout_revenge_v1_core.test.stasis`) for deterministic gameplay helpers (`brickout_can_buy`, `brickout_shop_anim_step`, `brickout_level_reset`) and level-sequencing behavior (`brickout_level_consume_spawns`).
- Added Brickout input-model clamp edge-case tests (negative/oversized raw pointer count via `input_testkit_set_pointer_count_raw`) and brought sample-dir headless coverage to `tests_discovered=11`.
- Status: `done`
- Slice CS2: Split compiler flow into explicit fast index pass and dirty-function emit pass.
- Language: `.stasis`.
- Scope:
- Index pass: parse signatures only, update symbol table, compute signature hashes, mark dirty set.
- Emit pass: parse/resolve/emit bodies only for dirty functions and invalidated dependents.
- Deliverable: unchanged files/functions skip body parse and skip CLIF emission.
- Tests: signature-only change, body-only change, unchanged-file no-op, and mixed-file edit cases.
- Done gate: dirty-function set is deterministic and minimal for covered scenarios.
- Current progress:
- Rust-native compiler flow runs explicit index then emit stages (`Compiler::index_pass`, `Compiler::emit_pass_with`) and emit work is restricted to dirty function ids only.
- Regression coverage now includes signature-only changes, body-only changes, unchanged-source no-op, and mixed-file body edit gating in `crates/stasis_compiler::compiler::tests::*`.
- Status: `done`
- Slice CS3: Implement O(1) function symbol lookup via open-addressed hash table in `.stasis`.
- Language: `.stasis`.
- Scope: replace linear function-name scans for call resolution and file-function lookup with open-addressed table operations.
- Deliverable: symbol table API with deterministic probing and collision handling.
- Tests: collision-heavy fixture, duplicate-name/across-file behavior, and lookup determinism.
- Done gate: no hot-path linear scans remain for function symbol resolution.
- Current progress:
- Rust-native compiler index path now uses a deterministic open-addressed symbol table (linear probing) for `name_hash -> FunctionId` resolution (`SymbolTable` in `crates/stasis_compiler/src/compiler.rs`) instead of generic map lookups in the hot path.
- Added regression coverage for collision-heavy probe behavior, duplicate-hash replacement semantics, duplicate-name across-file resolution, and repeated lookup determinism.
- Status: `done`
- Slice CS4: Implement first-class dependency invalidation graph (dependencies + dependents) with ripple propagation.
- Language: `.stasis`.
- Scope: store forward + reverse adjacency in flat arrays and propagate dirty state from signature/body changes to dependents.
- Deliverable: deterministic ripple invalidation without full-project rebuild for local edits.
- Tests: single-edge, fan-out, multi-level chain, and no-op signature-equal edits.
- Done gate: invalidation matches expected closure and touches only impacted functions.
- Current progress:
- Rust-native compiler index stage builds forward/dependent adjacency from dependency hashes each pass and stores edges in compact per-function vectors (`dependencies`, `dependents`).
- Signature-change ripple propagation is enforced via queue walk (`propagate_dirty_from_signature_changes`) with regression coverage for single-edge, fan-out, and multi-level chain closures.
- No-op signature-equivalent edits remain clean (`signature_equivalent_formatting_edit_does_not_dirty_or_emit`), and body-only fan-out edits keep dependents clean to preserve minimal invalidation (`body_change_keeps_fan_out_dependents_clean`).
- Status: `done`
- Slice CS5: Remove legacy `simple_*` detector metadata channels from compiler state and host contracts.
- Language: `.stasis + Rust`.
- Scope: delete obsolete detector/fallback metrics and rely on real parse/resolve/emit behavior for supported slices.
- Deliverable: reduced `FunctionMetric`/state surface and simpler host<->compiler contract.
- Tests: replace detector-centric tests with behavior/e2e compile-and-run checks.
- Done gate: no temporary fallback metadata contract remains in active compile path.
- Current progress:
- Removed stale detector-centric `compile_records_simple_*` host tests in `crates/stasis_compiler/src/lib.rs` that expected deprecated call-shape metadata.
- Kept semantic guard coverage by enforcing `from_*` conversion usage as statement-only (expression usage now returns semantic error `4001`).
- Status: `in_progress`
- Slice CS6: Add interned `TypeId` table and remove string-based type comparisons from hot parse/emit paths.
- Language: `.stasis`.
- Scope: introduce fixed-cap intern table (`TypeId`) and switch param/return/global/struct field typing to interned IDs.
- Deliverable: O(1) type identity checks in parse/emit path.
- Tests: type interning determinism, unknown-type diagnostics, and unchanged behavior on current fixtures.
- Done gate: hot path has no repeated string type comparisons for resolved types.
- Current progress:
- Rust-native frontend type interning now canonicalizes `Type[]` and `Type[N]` into structured interned `TypeId`s instead of raw type-name strings.
- Array storage layout metadata now models `max_length` header semantics (`Type[N]` carries header words + payload sizing metadata); view forms (`Type[]`) are represented as distinct interned types to support mixed-capacity call compatibility.
- `ascii`/`utf8` layout metadata now includes explicit header-word counts aligned with current runtime contract (`ascii`: `byte_length + max_length`; `utf8`: `byte_length + max_length + char_length`).
- `string` now canonicalizes as an alias of `utf8[]` (`string[N]` -> `utf8[N]`) in type interning.
- Type compatibility rules are now explicit in the interned type table (`argument -> parameter`): `Type[N]` is compatible with `Type[]` when element type matches; same for `ascii[N] -> ascii[]` and `utf8[N] -> utf8[]`, while cross-family string compatibility remains rejected by default.
- Rust-native JIT call overload matching now uses interned type-compatibility checks instead of raw `TypeId` equality only.
- Rust-native expression tokenization now accepts UTF-8 string literal codepoints (not ASCII-only), so non-ASCII literals compile in direct JIT parse/emit paths.
- Added targeted coverage for this shape in both paths: `crates/stasis_compiler::backend::jit::tests::jit_process_accepts_non_ascii_utf8_string_literal_argument` and `apps/stasis::compiler_backend::tests::aot_compile_accepts_utf8_literal_call_contract`.
- JIT top-level constant/global primitive typing now routes through interned `TypeId`/`TypeCategory` checks instead of raw `"i32"/"f32"/"bool"` and `"string"/"utf8[]"/"ascii[]"` string matching in hot-path analysis helpers (`crates/stasis_compiler/src/backend/jit.rs`).
- Added regression coverage for ASCII constant-identifier call flow on the interned path: `crates/stasis_compiler::backend::jit::tests::jit_process_executes_ascii_constant_identifier_argument`.
- Added explicit text-view helper APIs in `TypeTable` (`ensure_utf8_view_id`, `ensure_ascii_view_id`, `string_literal_type_id`) and switched JIT compile/string-literal lowering paths to use those interned helpers instead of direct `"string"`/`"ascii[]"` name resolution in hot-path code (`crates/stasis_compiler/src/frontend/types.rs`, `crates/stasis_compiler/src/backend/jit.rs`).
- Legacy `return_type` string has been removed from the active compiler metric contract; active AOT resolution/fallback checks use only numeric `return_type_code` (`RETURN_TYPE_CODE_*`) instead of raw string equality checks (`crates/stasis_compiler/src/lib.rs`, `apps/stasis/src/compiler_backend.rs`).
- Incremental compile metrics now also expose explicit `uses_stub_fallback` derivation, and AOT fallback manifest classification consumes that flag directly instead of recomputing fallback status from a bundle of `simple_*` channels (`crates/stasis_compiler/src/lib.rs`, `apps/stasis/src/compiler_backend.rs`).
- Legacy one-arg passthrough booleans (`simple_i32_*_passthrough`) have been removed from the active compiler metric contract; active AOT one-arg lowering/validation now consumes only canonical shape-code metadata (`simple_i32_one_arg_call_shape_code`) in host paths (`crates/stasis_compiler/src/lib.rs`, `apps/stasis/src/compiler_backend.rs`).
- Incremental compile metrics now also expose canonical one-arg call-shape codes (`SIMPLE_I32_ONE_ARG_CALL_SHAPE_*` via `simple_i32_one_arg_call_shape_code`), and active AOT one-arg target resolution/validation consumes this single numeric contract instead of branching over multiple legacy passthrough booleans (`crates/stasis_compiler/src/lib.rs`, `apps/stasis/src/compiler_backend.rs`).
- Rust fallback expression parsing now accepts identifier arguments in call expressions (e.g., `return callee(value);`) so one-arg passthrough shapes are recognized in the Rust incremental analysis path as well (`crates/stasis_compiler/src/lib.rs`, `crates/stasis_compiler::tests::compile_sets_one_arg_passthrough_shape_code_for_identifier_wrapper_shape`).
- Legacy `simple_void_print_is_one_arg` has been removed from the active compiler metric contract; void-print lowering/validation now uses only canonical shape-code metadata in active paths (`crates/stasis_compiler/src/lib.rs`, `apps/stasis/src/compiler_backend.rs`).
- Incremental compile metrics now also expose canonical void-print call-target shape codes (`SIMPLE_VOID_PRINT_CALL_TARGET_SHAPE_*` via `simple_void_print_call_target_shape_code`), and active AOT void-print target resolution/validation consumes this single numeric contract instead of ad-hoc branch conditions over raw fields (`crates/stasis_compiler/src/lib.rs`, `apps/stasis/src/compiler_backend.rs`).
- Added deterministic unit coverage in `crates/stasis_compiler/src/frontend/types.rs` for array/string interning and layout metadata.
- JIT fixed-array `max_length` header seeding now consumes interned type metadata (`TypeTable::fixed_collection_len`) instead of parsing type-name strings, removing string parsing from this hot path (`crates/stasis_compiler/src/backend/jit.rs`).
- Status: `in_progress`
- Slice CS7: Enforce direct CLIF emission from dirty-function body parse and remove non-conforming fallback branches.
- Language: `.stasis`.
- Scope: emit CLIF directly during dirty-function body parse with compact scratch reuse; keep only real supported paths.
- Deliverable: no stub/fallback emission in supported slices; unsupported constructs fail deterministically with diagnostics.
- Tests: per-slice CLIF assertions + JIT/AOT executable verification for representative branches.
- Done gate: each new compiler feature slice includes compile->JIT run and compile->AOT exe run verification.
- Status: `in_progress`
- Slice CS8: Lock acceptance gates and stop conditions.
- Language: `Rust + .stasis + docs`.
- Scope: wire benchmark thresholds, deterministic invalidation checks, and compile-path invariants into routine verification.
- Deliverable: documented pass/fail gates for cold/incremental targets and regression criteria.
- Tests: gated benchmark and invalidation suites.
- Current progress:
- Added `Perf CI` workflow (`.github/workflows/perf-ci.yml`) running `rust_native_jit_bench` for 1k functions with conservative stop conditions (cold p95 <= 1800ms, incremental p95 <= 35ms) to catch major compile-time regressions while PRD v2 targets are still being chased.
- Engine-overhead benchmark task (separate from compiler-only timing gates): added Perf CI gate (`engine-overhead-bench`, windows) running `cargo run -p stasis --release --example engine_overhead_bench -- --mode both --samples 3 --ticks 240` and hard-failing if `total_ms_p95` exceeds a conservative stop condition (300ms).
- Engine hot-update benchmark task (separate from compiler-only timing gates): added end-to-end watch/update benchmark (`apps/stasis/examples/engine_hot_update_bench.rs`) measuring `watch change -> jit.compile -> build_engine_package -> swap pointers -> first tick+render with new code`, plus Perf CI gate (`engine-hot-update-bench`, windows) hard-failing if `warm_update_total_ms_p95` exceeds a conservative stop condition (100ms).
- Outstanding: confirm CI environment baseline and set exact p50/p95 thresholds for PRD v2 hard-fail gating (stop conditions above are tripwires, not targets).
- Done gate: project can reject regressions automatically against PRD v2 targets.
- Status: `in_progress`
- Slice SH1: Wire minimal host bridge implementations for `S10b` externs in CLI path and execute `compiler_cli_compile_project`. (completed 2026-02-13; current host command path is `stasis aot-cli`)
- Slice SH2a: Replace monolithic `.stasis` host bridge declaration with staged AOT extern contract (`emit_ir`, `run_cranelift_aot`, `link_executable`) while preserving `.stasis` orchestration ownership. (completed 2026-02-13)
- Slice SH2b: Wire host bridge implementations for staged AOT extern calls and route CLI execution through them end-to-end. (completed 2026-02-13; current host `aot-cli` path executes staged bridge sequence)
- Slice SH2c: Add summary-bridge extern contract from `.stasis` (`host_write_aot_cli_summary`) and host sidecar summary emission parity (`<exe>.summary.json`) for staged contract introspection. (completed 2026-02-13)
- Slice SH3a: Add deterministic repeated-run self-host smoke on fixed source input (same staged bridge path, stable summary/output contract). (completed 2026-02-13)
- Slice SH3b1: Add stage1->stage2 metadata-contract smoke (independent stage1/stage2 builds on identical sources produce matching staged bundle contract: entry symbol + object layout metadata). (completed 2026-02-13)
- Slice SH3b2: Add true stage1->stage2 self-host smoke by executing stage1 compiled `.stasis` compiler binary to produce stage2, then verify diagnostics + function metadata summary parity.
- Slice SH3b2e: Add executable CLI contract bridge for self-host compiler main (`argv` extern surface + runtime host binding path) so compiled stage1 `.exe` can receive `aot-cli --project-dir --out --summary-file` arguments directly from process launch.
- Slice SH3b2e1: Define `.stasis` CLI-entry extern contract and parser/runtime fixture coverage for argument loading and compile invocation wiring. (completed 2026-02-13)
- Slice SH3b2e2: Wire host runtime extern implementations for CLI-entry contract in produced AOT executable path.
- Slice SH3b2e2a: Align host CLI parser path with `.stasis` argv contract semantics and summary override bridge (`--summary-file` routed via host summary path override) with unit coverage. (completed 2026-02-13)
- Slice SH3b2e2b: Bind runtime extern dispatch for compiled stage1 executable (`host_cli_arg_count`, `host_cli_arg_value`, `host_set_summary_file`) in produced AOT process path.
- Slice SH3b2e2b1: Link self-host runtime extern bridge object into produced AOT executable with explicit stub symbol coverage for CLI bridge externs and existing staged host externs (host/runtime glue only). (completed 2026-02-13)
- Slice SH3b2e2b1a: Publish process-backed CLI argument env contract (`STASIS_SELF_HOST_ARG_COUNT`, `STASIS_SELF_HOST_ARG_<n>`, `STASIS_AOT_SUMMARY_FILE`) from host `aot-cli` path with restore semantics for runtime bridge consumption. (completed 2026-02-13)
- Slice SH3b2e2b1b: Add host-boundary guard coverage in `apps/stasis` CLI path to keep host responsibilities limited to argv/env glue + self-host entry delegation (no direct compile orchestration in `main.rs`). (completed 2026-02-13)
- Slice SH3b2e2b2: Replace runtime bridge stubs with live process-backed extern implementations and validate stage1 executable argv-driven compile flow.
- Slice SH3b2e2b2a: Add `aot-cli --entry-file` contract path and project-local import-closure source selection so self-host AOT can target one program in multi-sample directories without multi-main rejection. (completed 2026-02-13)
- Slice SH3b2e2b2b: Add deterministic linker discovery/fallback guidance + coverage for self-host AOT CLI executable link step on Windows dev environments lacking `link.exe` on `PATH` (use `STASIS_AOT_LINKER` override contract and document expected toolchain paths). (completed 2026-02-23)
- Slice SH3b2e2b2a: Add process-backed runtime bridge function implementations (`host_cli_arg_count`, `host_cli_arg_value`, `host_set_summary_file` semantics) in host module with unit coverage, ready to bind into emitted runtime extern bridge. (completed 2026-02-13)
- Slice SH3b2e2b2b: Bind emitted runtime bridge extern symbols to process-backed implementations (replace CLIF stubs) and validate compiled stage1 executable argument-driven behavior. (in progress: rustc + CLIF fallback bridge externs are env-backed as of 2026-02-23; CLI-entry runtime host extern route (`host_run_self_host_aot_cli_from_env`) is wired and lowerable; remaining work is reachability-first pruning and deletion of detector/fallback-heavy lowering paths)
- Slice SH3b2e2b2b1: Emit Windows runtime bridge object via live `no_std` rustc object with process-backed extern semantics for CLI arg/env bridge, with explicit mode marker and fallback path to CLIF stubs. (completed 2026-02-13)
- Slice SH3b2e2b2b1a: Add opt-in real-toolchain runtime-bridge executable smoke (`STASIS_RUN_REAL_RUNTIME_BRIDGE_ARGC_SMOKE=1`) verifying compiled executable observes `STASIS_SELF_HOST_ARG_COUNT` via live `host_cli_arg_count` binding. (completed 2026-02-13)
- Slice SH3b2e2b2b1b: Extend live runtime bridge extern coverage for source staging env contract (`host_source_file_count`/`host_load_source_file`) and add opt-in executable smoke for `STASIS_SELF_HOST_SOURCE_FILE_COUNT` binding. (completed 2026-02-13)
- Slice SH3b2e2b2b2: Execute compiled stage1 executable through argv path and verify stage2 summary parity using live runtime bridge extern binding. (in progress: real-toolchain probe wired via `STASIS_RUN_REAL_SELF_HOST_STAGE1_EXEC_PARITY_SMOKE=1`; current path verifies stage2 summary parity through the runtime-entry host extern route; remaining work is reachability-root pruning and non-conforming fallback-path deletion)
- Slice SH3q1: Add consolidated `aot-cli` end-to-end quality harness (stage contract parity + invalid-program diagnostic coverage + per-run time-budget assertions), gated as opt-in (`STASIS_RUN_E2E_SELF_HOST_QUALITY=1`) until bootstrap harness/env interactions are fully isolated. (completed 2026-02-13)
- Slice SH3b2a: Add opt-in real-toolchain runnable-exe smoke hook for self-host AOT CLI output (`STASIS_RUN_REAL_AOT_EXE_SMOKE=1`) to exercise executable startup path while startup ABI hardening is in progress. (completed 2026-02-13)
- Slice SH3b2b: Add stage parity harness using `aot-cli --summary-file` outputs so stage1/stage2 compiler executions can be compared via stable machine-readable contract outputs once stage1 executable invocation is wired.
- Slice SH3b2b1: Add CLI summary-parity integration test for repeated `aot-cli --summary-file` runs on identical source, asserting stable `source_file_count`, `entry_symbol`, and `object_file_names` contract fields. (completed 2026-02-13)
- Slice SH3b2b2: Execute the same summary-parity harness through stage1 compiled self-host executable invocation once true stage1->stage2 execution is wired. (in progress: executable-path parity harness now runs stage1 subset binary with published argv/source/bridge env contracts and validates summary parity via runtime-entry host extern routing; remaining work is removing residual fallback-only behavior via reachability-first deletion of non-conforming lowering branches)
- Slice SH3b2d: Historical strict self-host stub-fallback gate. This was removed when the repo collapsed onto the direct Rust AOT path and deleted the helper-era fallback scaffolding. (retired 2026-03-10)
- Slice SH3b2c: Add signed self-host AOT artifact flow (post-link signing hook + signer/policy validation checklist) so Windows security policy allows stage1/stage2 executable invocation in default dev environments.
- Slice SH3b2c1: Add optional post-link signing hook (`STASIS_AOT_SIGN_TOOL`) in self-host AOT CLI bridge and deterministic fake-signer test coverage for signer invocation contract. (completed 2026-02-13)
- Slice SH3b2c2: Add signed-artifact policy validation smoke for stage1 executable invocation path on Windows dev environments with Application Control enabled.
- Slice SH3b2c2a: Add opt-in signed real-toolchain executable smoke hook (`STASIS_RUN_SIGNED_SELF_HOST_SMOKE=1` + `STASIS_AOT_SIGN_TOOL`) to validate signed self-host startup path when signing toolchain/certs are configured. (completed 2026-02-13)
- Slice SH3b2c2b: Run signed stage1->stage2 parity smoke in a cert-trusted environment and record policy outcome/requirements.
- Priority:
- This is the active front-of-line work. Complete `SH1` before any additional non-blocking `R*`/`H*` slices.

### S11 - Swap Indicator (Tick-Based)
- Language:
- `.stasis` (feature logic) + `Rust` (draw host bridge only)
- Scope:
- Integrate `DebugUI.swapFlashTicks` behavior in Stasis game code.
- Current implementation:
- `samples/brickout_revenge/brickout_revenge_v1.stasis` now defines `on_code_swap()` to arm `swap_flash_ticks` and renders/decrements indicator ticks in `draw_swap_indicator()`.
- Deliverable:
- Successful swaps trigger visible deterministic indicator.
- Tests:
- Tick countdown behavior tests and no-indicator-on-failure tests.
- Current runtime coverage fixture:
- `tests/stasis/run_swap_indicator_tick_behavior.stasis` (arms `on_code_swap`, decrements `swapFlashTicks` over 180 ticks, and verifies no indicator once expired).
- Done gate:
- Indicator follows tick policy and does not fire on failed swap.
- Status: `completed`

### S12 - Brickout Revenge End-to-End
- Language:
- `.stasis` gameplay/compiler script + `Rust` host runner/JIT integration
- Scope:
- Run `samples/brickout_revenge/brickout_revenge_v1.stasis` through incremental compiler and hot-swap loop.
- Validate intended vertical window proportion.
- Deliverable:
- Real sample runs in watch/compile/swap workflow.
- Tests:
- End-to-end scenario test with window config assertion.
- Current runtime coverage:
- `apps/stasis` scenario run path uses the real incremental backend and drives runtime launch in graphics mode for Brickout.
- Transitional note removed: the legacy external analysis harness path is no longer part of the active repo.
- Done gate:
- Brickout runs with correct proportion and swap loop remains stable.
- Real compile -> function patch -> commit path updates patch identity on source edit.
- Status: `completed`

### S13 - Host-Set Contract Surface (Sandbox Baseline)
- Language:
- `Rust + .stasis`
- Rust: contract transport fields and runtime validation hooks.
- `.stasis`: required host-set declaration extraction and diagnostics policy.
- Scope:
- Add host-set payloads to compile/commit contracts (`host_set_id`, `host_set_hash`).
- Keep phase classes in host-set contract metadata (`tick_safe`, `commit_only`, `effect_queued`).
- Reject unresolved/missing host-set requirements before swap.
- Deliverable:
- Pending patch always carries a host-set contract and runtime validates it before hook/pointer swap.
- Tests:
- Host-set hash determinism tests.
- Missing host-set mapping rejection tests.
- Host-set hash/phase mismatch rejection tests.
- Done gate:
- No swap can commit without host-set contract validation.
- Current progress:
- Compile contracts include optional host-set metadata fields (`host_set_id`, `host_set_hash`) on `CompileRequest`/`CompileResult`, and commit contracts include host-set metadata on `SwapCommitRequest`.
- `apps/stasis` resolves a host-set contract by profile and sets it on the swap pipeline; commit-time validation rejects missing/mismatched host-set metadata before hook/pointer swap.
- Outstanding questions / TODO:
- Define `.stasis` required-host declaration syntax and deterministic diagnostics (source of truth: explicit directive vs inference from `@extern`/`@link` usage).
- Status: `in_progress (contract transport + commit validation implemented; required-host extraction pending)`

### S14 - Host-Set Registry and Profile Selection
- Language:
- `Rust`
- Scope:
- Implement runtime host-set registry keyed by stable `host_set_id` + `host_set_hash`.
- Add explicit host-set selection by mode/profile (`dev`, `test`, `prod`).
- Remove/guard ambient host call paths that bypass host-set checks.
- Deliverable:
- Runtime extern dispatch is routed exclusively through selected host-set exports.
- Tests:
- Host-set registry lookup determinism tests.
- Profile host-set selection tests.
- Missing/unselected host-set dispatch rejection tests.
- Done gate:
- Host access is deny-by-default across runtime dispatch paths and requires selected host set.
- Current progress:
- `apps/stasis/src/host_set_registry.rs` implements deterministic profile-to-contract resolution with optional JSON registry file mapping.
- CLI/env surface exists: `--host-set-profile`, `--host-set-registry-file`, `STASIS_HOST_SET_PROFILE`, `STASIS_HOST_SET_REGISTRY_FILE` (precedence: CLI > env > inferred target mode).
- Outstanding questions / TODO:
- Profile inference mapping is implemented (`JitDev -> dev`, `AotProd -> prod`); decide whether `test` is a first-class mode and where it applies.
- Decide which runtime dispatch paths must become deny-by-default as part of S14 vs later S15/S16 enforcement.
- Status: `in_progress (registry + profile selection implemented; deny-by-default dispatch pending)`

### S15 - Phase-Gated Effects and Tick Determinism
- Language:
- `Rust + .stasis`
- Rust: deterministic effect queue + commit-boundary flush path.
- `.stasis`: phase-usage diagnostics and policy checks.
- Scope:
- Enforce host-set phase rules for extern invocation on tick and commit paths.
- Route nondeterministic host effects through queued requests or host-fed deterministic snapshots.
- Restrict `on_code_swap` extern calls to host-set commit-safe classes.
- Deliverable:
- Host-set extern calls preserve deterministic tick semantics.
- Tests:
- Tick-phase host-set policy violation rejection tests.
- Effect-queue ordering/replay determinism tests.
- `on_code_swap` phase-policy rejection + rollback tests.
- Done gate:
- Nondeterministic host effects cannot bypass queued/snapshotted paths.
- Status: `planned (post-S10b)`

### S16 - Host-Set Budgets and Failure Containment
- Language:
- `Rust`
- Scope:
- Add per-host-set/per-tick budgets (call count/time/bytes).
- Define deterministic policy for budget violations (reject patch or fail tick without partial commit).
- Emit host-set budget diagnostics/telemetry through runner events.
- Deliverable:
- Host-set call overuse is bounded and failure behavior is explicit.
- Tests:
- Budget-threshold enforcement tests.
- Budget-overrun rollback/preservation tests.
- Done gate:
- Host-set misuse cannot produce partial state mutation or silent nondeterminism.
- Status: `planned (post-S10b)`

## PR Sequence

1. PR-A: S0-S2
2. PR-B: S3-S5
3. PR-C: S6-S8
4. PR-D: S8b-S10
5. PR-E: S11-S12
6. PR-F: S13-S14
7. PR-G: S15-S16

Each PR must include:
- tests for that slice set
- docs updates for changed behavior
- removal of obsolete paths introduced during the slice

## Backlog

- Evaluate hard security sandbox options (separate process / OS sandbox / WASM runtime) for adversarial plugin or untrusted code scenarios.
