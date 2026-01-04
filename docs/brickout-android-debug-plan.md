# Brickout Revenge - Android Debug Build Plan (Windows host -> Android phone)

This plan aims to produce a runnable **debug** build of Brickout Revenge on an Android phone, with a tight "edit -> rebuild -> install -> run" loop, and a simple way to push assets/data to the device for testing.

Design reference:
- `docs/brickout-revenge-brainstorm.md` captures the current game concept, layout goals (4x5 grid), and economy direction.

Scope: this is a developer workflow for local iteration, not a polished Play Store pipeline.

## Goals

- Build and install an Android `debug` APK that runs `samples/brickout_revenge/brickout_revenge.stasis` on device.
- Use the existing Stasis runtime C library on Android (SDL_Renderer-only backend).
- Provide scripts to:
  - build the Stasis game IR for Android
  - build the APK (Gradle wrapper)
  - install/update on device via `adb`
  - push `samples/brickout_revenge/` assets/data onto the device for runtime loading
- Keep everything deterministic and inspectable: no hidden downloads at runtime, no magic IDE steps.

## Non-goals (for this task)

- Hot-swap between ticks on Android (desktop runner feature).
- Full release signing / Play Store packaging.
- Broad device coverage (start with arm64).
- WASM browser target (not implemented in the compiler today).

## Assumptions and constraints

- Android builds use the SDL_Renderer backend (no OpenGL 2.1 + GLEW on Android).
- The Stasis compiler currently lowers to LLVM IR; for Android we will:
  - emit LLVM IR for the game from the Windows host
  - compile/link it into an Android `libmain.so` via the Android NDK toolchain
- Brickout Revenge loads SVG assets via relative paths like `samples/brickout_revenge/assets/...`.
  - The Android host must set the working directory to a known writable location (external app files dir),
    and assets must be pushed there preserving relative paths.

## Prerequisites (Windows dev machine)

Required:

- .NET 9 SDK (for `stasis.bat`)
- Android SDK + NDK (for Gradle + native build)
  - `ANDROID_SDK_ROOT` (or `ANDROID_HOME`) points to the SDK
  - `ANDROID_NDK_HOME` (or `ANDROID_NDK_ROOT`) points to the NDK
- Java JDK 17+ (for Gradle)
- `adb` in `PATH` (usually via Android platform-tools)

Recommended:

- vcpkg (to supply native SDL2 for Android builds)
  - `VCPKG_ROOT` should point to your vcpkg install (convention: `C:\vcpkg`)

## Deliverables

### 1) Android host app (Gradle)

- Add an `android/brickout-revenge/` Android app project that:
  - uses an SDL-based Activity (packaged in-repo)
  - loads `libSDL2.so` and `libmain.so`
  - calls into `SDL_main()` in `libmain.so`

### 2) Native library: `libmain.so`

`libmain.so` contains:

- A small Android/SDL entrypoint (implements `SDL_main`)
- The Stasis runtime (`runtime/stasis_graphics.c`, SDL-only build)
- The compiled Brickout Revenge module (from emitted LLVM IR compiled by the NDK toolchain)

### 3) Build + install scripts

Provide Windows-friendly scripts:

- `android/build_brickout_android_debug.ps1`
  - emits game LLVM IR for an Android target triple
  - builds the Android APK via Gradle wrapper
- `android/install_brickout_android_debug.ps1`
  - installs the APK via `adb install -r`
  - pushes `samples/brickout_revenge/` to the app external files directory

## Implementation steps (concrete)

### Step 0: Create branch and baseline

- Branch off `main`: `feat/brickout-android-debug`
- Keep the work self-contained (Android app and scripts in `android/`).

### Step 1: Make LLVM IR targetable for Android

Problem: the LLVM module builder currently pins the module target triple to the host.

Fix:

- Add an optional target triple to lowering (`LowerOptions.TargetTriple`).
- Thread it through to `LlvmModuleBuilder`, and set `module.Target` accordingly when provided.
- Expose a CLI flag (e.g. `--llvm-target <triple>`) so scripts can request:
  - `aarch64-linux-android21` (arm64, API 21) as the module triple.

Acceptance:

- `.\stasis.bat run samples\brickout_revenge\brickout_revenge.stasis --emit-ir --graphics --backend llvm --llvm-target aarch64-linux-android21 > out.ll`
  produces valid IR with the requested triple.

### Step 2: Add an Android app project (SDL Activity + Gradle wrapper)

- Create `android/brickout-revenge/` with:
  - Gradle wrapper (`gradlew`, `gradlew.bat`, `gradle/wrapper/...`)
  - `app/` module with an SDL-based Activity
  - `CMakeLists.txt` for `libmain.so`

Notes:

- Keep the Java glue minimal and pinned (committed in-repo).
- Prefer arm64 only initially: `abiFilters "arm64-v8a"`.

Acceptance:

- `android/brickout-revenge/gradlew.bat assembleDebug` succeeds on a machine with SDK/NDK.

### Step 3: Native build wiring (CMake)

- In `CMakeLists.txt`, build `libmain.so` from:
  - a small `stasis_android_main.c` implementing `SDL_main`
  - Stasis runtime sources (SDL-only)
  - a generated object file compiled from the Brickout `.ll`

Implementation details:

- Generate `brickout_revenge.ll` into a stable repo path (e.g. `android/out/brickout_revenge.ll`).
- Use `add_custom_command` to compile the `.ll` into `brickout_revenge.o` with the NDK clang.
- Link `brickout_revenge.o` into `libmain.so`.

Acceptance:

- `libmain.so` exports `SDL_main`.
- Running on device opens a window and renders frames.

### Step 4: Provide asset/data push workflow

Path conventions:

- On Android, use `SDL_AndroidGetExternalStoragePath()` as the base directory.
- The host sets `cwd` to that directory.
- Push `samples/brickout_revenge/` to `<external>/samples/brickout_revenge/...` so the existing relative paths work.

Acceptance:

- Sprites load on-device (no "file not found" logs).

### Step 5: Scripts for build + install

Add PowerShell scripts:

- `android/build_brickout_android_debug.ps1`
  - verifies required env vars
  - emits IR with `--llvm-target aarch64-linux-android21`
  - invokes Gradle wrapper to build the APK
- `android/install_brickout_android_debug.ps1`
  - finds the produced `app-debug.apk`
  - installs via `adb install -r`
  - pushes `samples/brickout_revenge/` to the external files directory
  - optionally launches the app via `adb shell am start`

Acceptance:

- A single command builds and installs on a connected device.

## Validation checklist

- `build.bat` still succeeds on Windows (unchanged baseline).
- `test.bat` still passes (C# tests + end-to-end tests).
- Android build: `android/brickout-revenge/gradlew.bat assembleDebug` succeeds.
- Android install: app launches and shows Brickout Revenge.

## Risks / unknowns

- SDL Java glue version compatibility with the SDL2 native build supplied by vcpkg.
- Runtime file IO assumptions (current working directory, path separators).
- Device-specific permissions/scoped storage behavior on newer Android versions.

If these become blockers, the fallback is to package assets inside the APK (Android assets) and teach the runtime to load via `AAssetManager` (bigger change; defer unless needed).
