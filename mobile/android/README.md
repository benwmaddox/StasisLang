# Stasis Android Workshop Shell

This is the first checked-in Android app shell for the `android` branch.

Current scope:

- Builds one Android app module with `workshop` and `published` product flavors.
- Targets `arm64-v8a` only.
- Loads a tiny native C library through JNI.
- Bundles a small Stasis-style workshop project under Android assets.
- Opens the native Android symbol browser and source editor from a top-right hamburger overlay grouped by Main, Structs, Systems, and Root.
- Opens into a full-screen native Android preview surface by default and starts an automatic 60 fps compile/tick loop so the preview advances without pressing `Compile` and `Run Tick`.
- Keeps the preview, status text, and menu button inside Android system-bar and display-cutout safe insets, so they do not sit under the bottom navigation bar or camera notch.
- Seeds bundled `.stasis` files into app-private storage when missing and preserves edits across app launches.
- Lets a selected symbol display and edit its source from the app-private `.stasis` file.
- Saves selected symbol edits back to the app-private `.stasis` file, reparses the project so later edits use fresh symbol spans, and reports `FastReload` versus `ResetRequired` expectations.
- Provides a dev-first AI edit panel that stores an OpenAI API key/model in app-private preferences, sends the selected symbol context to the Responses API, applies supported `replace_function`/`replace_struct` JSON edits, and refreshes compile state.
- Provides an explicit `Reset Project` control for restoring the bundled sample. Manual `Apply` saves and compiles immediately; there is no separate manual compile button. The automatic loop and manual controls both use the native compile/run path, with runtime ticks routed through the Rust JIT bridge when packaged. The probe reads project `.stasis` files, validates basic source structure, checks lifecycle roots, and writes `build/native_compile_manifest.txt` with project counts, per-function signature/body hashes, per-function compiled-stub artifacts under `build/functions`, a `build/runtime_state.txt` state artifact, and a reload classification (`InitialCompile`, `NoChange`, `FastReload`, or `ResetRequired`), then returns `CompilePlanned` or `CompileError` diagnostics.
- Resizes and scrolls the editor when the Android keyboard opens so the active source remains visible.
- Keeps fixed trailing scroll space under the editor as a fallback for phones where IME resize is inconsistent.

It packages the tested Rust/C ABI bridge from `crates/stasis_android_bridge`, runs the bundled Stasis game through the native compile/run path, and keeps compiler-owned Android compile plan/artifact rendering in `stasis_compiler::frontend::workshop`. The C/JNI scaffold remains as the Android host boundary and fallback layer while the bridge evolves toward the full workshop runtime.

## Published Build Path

The `published` flavor is a parallel runtime-only Android target. It keeps the same native rendering/runtime surface but skips the workshop chrome entirely: no hamburger drawer, no symbol browser, no source editor, no AI controls, and no manual hot-swap UI. It uses a separate application id, `com.stasislang.workshop.published`, so it can be installed next to the developer workshop app.

Build the release-style APK with:

```powershell
.\build_published.ps1
```

Install a debug-signed published variant for device testing with:

```powershell
.\build_published.ps1 -Install
```

AOT status: `stasis_compiler` already has an AOT engine-bundle path and the published build script can run its AOT bundle contract check with `-ValidateAot`. The Android runtime still executes through the current native bridge/JIT path until the next bridge step links or packages the Android AOT engine image and calls it directly from the host.
## Build

Prerequisites:

- Android SDK
- Android NDK
- JDK 17+
- Gradle or Android Studio

From this directory:

```powershell
.\build_debug.ps1
```

Or call Gradle directly:

```powershell
gradle :app:assembleWorkshopDebug
```

If your installed SDK is not 35, override it:

```powershell
gradle :app:assembleWorkshopDebug -Pstasis.compileSdk=36 -Pstasis.targetSdk=36
```

Install to a connected device:

```powershell
.\build_debug.ps1 -Install
```

## Host AI Run Review

Validate the Android AI Responses payload locally before a live run. This does not call OpenAI or modify the bundled sample, and writes a timestamped trace under the ignored `artifacts/android_ai_runs/` directory:

```powershell
python ..\..\tools\android_ai_agent_host.py --preflight --prompt "verify Android AI payload"
```

For a live local run, the same host tool always writes a trace, including API failures, tool observations, test results, token usage, and cost estimates. After it finishes, provide the printed trace path for review:

```powershell
python ..\..\tools\android_ai_agent_host.py --reset-paddle-speed-feature --prompt "enemy paddle should have speed change"
```

Expected app surface:

```text
tick=<avg ms> render=<avg ms> budget=<tick+render % of 60 fps frame>
[full-screen native preview]
[top-right menu button]
```

Open the top-right menu button to access the AI prompt first, with manual symbols/source collapsed below it. Manual Apply/Reset, Changes, Reset Project, and Run Tick live with the selected manual symbol editor; Apply compiles immediately.

GitHub settings remain collapsed below the command workflow. After configuring a token, `owner/repository`, and base branch, `Sync GitHub Now` uploads changed project files directly to that branch. For reviewed work, use `Review GitHub Changes` first; it shows the symbol summary and raw diffs and records the exact reviewed change set. `Create / Update Pull Request` then creates or reuses the deterministic Workshop review branch, uploads those reviewed files, and creates or finds its open pull request. If local files change after review, submission stops until they are reviewed again.

OpenAI and GitHub secrets are encrypted with AES-GCM using a key held by Android Keystore. Preferences contain ciphertext rather than plaintext; installations upgrading from the earlier format migrate each legacy secret on first read and remove the plaintext only after encrypted storage succeeds.

GitHub uploads and pull-request operations share one serial background queue. The app persists whether an operation was queued, running, completed, failed, or interrupted. `Retry GitHub Operation` reconstructs failed work from current app-private files; pull-request retries also recheck the saved review fingerprint and stop if the files changed after review.

`Run AI Change` permits one active run. `Cancel AI` disconnects an active model request and prevents later model/tool turns. If cancellation arrives during an atomic source-write batch, that batch first reaches its existing compile/test/rollback boundary so source is never left partially applied. Any model call that already returned remains included in budget totals.

`Recent Commands` also shows bounded per-project AI outcomes, including cancellation/failure/rollback status, usage summaries when available, and the local trace path. `Retry Last AI` restores the latest recorded request and starts it again through the normal key, pricing, per-run, and monthly budget checks.

`Projects` is collapsed below the command workflow. The existing app-private `workshop_project` is adopted as `Bundled Workshop` without moving its files. `New Project From Sample` creates a separately identified, versioned app-private project; `Switch Project` changes the root used by symbols, compile, tests, AI history, and GitHub state and immediately recompiles it. Project creation/switching is blocked while AI or GitHub work is active or the source editor contains an unapplied edit. GitHub repository/branch targets and review/retry state are project-specific; the encrypted token is shared.

`Export Project Archive` opens Android's document-creation picker and writes a deterministic ZIP of the active project's normal files and versioned metadata. Generated `build/` output and temporary files are excluded. Export is limited to 512 files, 32 MiB per file, and 128 MiB total and does not require broad storage access.
