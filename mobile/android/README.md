# Stasis Android Workshop Shell

This is the first checked-in Android app shell for the `android` branch.

Current scope:

- Builds one Android app module.
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
- Provides an explicit `Reset Project` control for restoring the bundled sample, and keeps dedicated `Compile` and `Run Tick` controls inside the editor overlay for manual review; the automatic loop and manual controls both use the native compile/run path, with `Run Tick` routed through the Rust JIT bridge when packaged. The probe reads project `.stasis` files, validates basic source structure, checks lifecycle roots, and writes `build/native_compile_manifest.txt` with project counts, per-function signature/body hashes, per-function compiled-stub artifacts under `build/functions`, a `build/runtime_state.txt` state artifact, and a reload classification (`InitialCompile`, `NoChange`, `FastReload`, or `ResetRequired`), then returns `CompilePlanned` or `CompileError` diagnostics.
- Resizes and scrolls the editor when the Android keyboard opens so the active source remains visible.
- Keeps fixed trailing scroll space under the editor as a fallback for phones where IME resize is inconsistent.

It packages the tested Rust/C ABI bridge from `crates/stasis_android_bridge`, runs the bundled Stasis game through the native compile/run path, and keeps compiler-owned Android compile plan/artifact rendering in `stasis_compiler::frontend::workshop`. The C/JNI scaffold remains as the Android host boundary and fallback layer while the bridge evolves toward the full workshop runtime.

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
gradle :app:assembleDebug
```

If your installed SDK is not 35, override it:

```powershell
gradle :app:assembleDebug -Pstasis.compileSdk=36 -Pstasis.targetSdk=36
```

Install to a connected device:

```powershell
.\build_debug.ps1 -Install
```

Expected app surface:

```text
tick=<avg ms> render=<avg ms> budget=<tick+render % of 60 fps frame>
[full-screen native preview]
[top-right menu button]
```

Open the top-right menu button to access the symbol tree, source editor, AI Patch Selected Symbol, Apply/Reset, Changes, Reset Project, and manual Compile/Run Tick controls.
