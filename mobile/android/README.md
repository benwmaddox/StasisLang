# Stasis Android Workshop Shell

This is the first checked-in Android app shell for the `android` branch.

Current scope:

- Builds one Android app module.
- Targets `arm64-v8a` only.
- Loads a tiny native C library through JNI.
- Bundles a small Stasis-style workshop project under Android assets.
- Shows a native Android symbol browser grouped by Main, Structs, Systems, and Root.
- Seeds bundled `.stasis` files into app-private storage on first launch.
- Lets a selected symbol display and edit its source from the app-private `.stasis` file.
- Saves selected symbol edits back to the app-private `.stasis` file and reports `FastReload` versus `ResetRequired` expectations.
- Resizes and scrolls the editor when the Android keyboard opens so the active source remains visible.
- Keeps fixed trailing scroll space under the editor as a fallback for phones where IME resize is inconsistent.

It does not yet link the Stasis runtime or compile a Stasis game on-device. That remains the next mobile runtime bridge slice.

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
Stasis Workshop
Stasis Android native smoke loaded - 8 files - 17 symbols
Main
Structs
Systems
Root
Apply
Reset
No pending edit
```
