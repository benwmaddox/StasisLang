# Stasis Android Workshop Shell

This is the first checked-in Android app shell for the `android` branch.

Current scope:

- Builds one Android app module.
- Targets `arm64-v8a` only.
- Loads a tiny native C library through JNI.
- Shows a native status string on screen.

It does not yet link the Stasis runtime or a compiled Stasis game. That is the next mobile build slice.

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

Expected app text:

```text
Stasis Android native smoke loaded
```