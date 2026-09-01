# Lean iOS and Android Build Research

## Goal

Figure out the smallest practical path to ship StasisLang games on Android and iOS without dragging the desktop dev architecture onto mobile.

This note is intentionally biased toward low setup and low maintenance, not maximum flexibility.

## Bottom Line

The lean mobile plan is:

1. Use AOT only on mobile.
2. Do not support JIT, hot swap, or runtime dynamic code loading on mobile.
3. Use one shared mobile runtime path: SDL-only rendering/input/audio.
4. Package one compiled game into one native app shell.
5. Start with one ABI per platform:
   - Android: `arm64-v8a`
   - iOS: `arm64` device, optionally `arm64` simulator later
6. Avoid mobile-specific package managers beyond what each platform already expects.
7. Prefer checked-in platform wrappers over extra setup tools.

If we do that, Android and iOS become mostly packaging work around the existing AOT backend rather than a second runtime architecture.

The concrete v1 mobile packaging ABI is defined in `docs/mobile_packaging_abi.md`.

## What The Repo Already Has

Useful existing pieces:

- Production direction is already AOT, not JIT.
  - `docs/spec_implementation_status.md`
  - `README.md`
- The runtime already has an SDL-only mode for restricted platforms.
  - `runtime/CMakeLists.txt`
  - `runtime/README.md`
- The runtime already builds a static graphics library.
  - `runtime/CMakeLists.txt`
- There is already an Android runtime helper script.
  - `runtime/build_android.ps1`

Current blockers for mobile reuse:

- The desktop runner is built around loading compiled code as a shared library and rebinding symbols at runtime.
  - `runtime/stasis_runner.c`
- `stasis_dynload` is Windows-oriented and explicitly says dynamic loading is only supported on Windows.
  - `crates/stasis_dynload/src/lib.rs`
- The current desktop dev flow assumes `play` + JIT/hot swap.
  - `README.md`

Inference from the repo:

- Mobile should not try to reuse the current dev runner.
- Mobile should consume the AOT output directly and link it into an app.

## Platform Facts That Matter

### Android

Officially relevant requirements:

- Rust supports Android targets including `aarch64-linux-android`.
- NDK builds are expected to go through the Android CMake toolchain with settings like `CMAKE_TOOLCHAIN_FILE`, `ANDROID_ABI`, and `ANDROID_PLATFORM`.
- Android ABI support includes `arm64-v8a`.

What that means for Stasis:

- We should start with `aarch64-linux-android` only.
- We should not build multiple ABIs until the first pipeline is stable.
- We should link a native shared library for the app and keep the Java/Kotlin layer thin.

### iOS

Officially relevant requirements:

- Rust supports Apple iOS targets including `aarch64-apple-ios` and simulator targets.
- Building for iOS requires the Apple SDKs that come from Xcode.
- App Store Review Guideline 2.5.2 says apps should be self-contained and must not download, install, or execute code that changes features or functionality.

What that means for Stasis:

- Shipping JIT or hot-loaded user code is the wrong model for iOS.
- iOS should be AOT-only.
- The compiled game logic needs to be part of the shipped app, not something dynamically compiled or loaded later.

## Leanest Architecture That Fits Both Platforms

### 1. One shared mobile execution model

Use this model for both Android and iOS:

- Compile `.stasis` game code on the developer machine using the existing AOT path.
- Produce mobile-linkable objects or a static library instead of a desktop `.exe`/`.dll`-style output.
- Link those objects into a native app shell.
- Put game assets into the app bundle.
- Run the fixed game entrypoints from the app shell:
  - `main`
  - `tick`
  - `render`

This is much smaller than trying to port the dev runner.

### 2. One runtime backend on mobile

Use the SDL renderer on both Android and iOS.

Reason:

- The native runtime has one renderer implementation on every target.
- Reusing it on iOS avoids carrying a separate mobile renderer implementation.

Inference:

- The mobile shells link the same SDL renderer runtime as desktop.

### 3. No dynamic loading on mobile

Do not use:

- `stasis_runner` shared-library loading model
- `stasis_dynload`
- JIT
- hot swap
- plugin-style game loading

Instead:

- link the compiled game into the shipped app
- use a fixed native entry surface

This is required for iOS policy reasons and also keeps Android simpler.

## Recommended Minimal Setup

### Android setup

Keep Android setup to:

- Rust toolchain
- `rustup target add aarch64-linux-android`
- Android SDK
- Android NDK
- JDK
- CMake
- Ninja
- checked-in Gradle wrapper for the app shell

Avoid for mobile:

- vcpkg
- multiple ABIs
- custom Java/Kotlin layers beyond a minimal SDL activity wrapper
- building the Stasis CLI on the device

Recommendation:

- Keep desktop vcpkg usage if useful.
- Do not make vcpkg part of the Android path.
- Vendor SDL for the Android app shell instead of introducing another mobile package-management layer.

Why:

- `runtime/build_android.ps1` proves Android was already being approached through NDK + CMake.
- But vcpkg is extra setup surface for mobile and is not the leanest long-term mobile path.

### iOS setup

Keep iOS setup to:

- macOS
- Xcode
- Rust toolchain
- `rustup target add aarch64-apple-ios`
- optionally `rustup target add aarch64-apple-ios-sim`
- a checked-in Xcode project/workspace for the app shell

Avoid for mobile:

- on-device compilation
- JIT
- runtime code loading
- extra cross-platform iOS packaging tools unless they replace real Xcode work cleanly

Recommendation:

- Use Xcode directly.
- Keep the iOS shell thin and native.
- Bundle the precompiled game and assets into the app.

## Smallest Repo Changes Needed

### P0: make AOT output mobile-linkable

Needed:

- A supported AOT output mode that emits mobile-linkable object files or a static library, not only desktop-oriented final images.
- A stable exported entry ABI for mobile app shells.
- Generated entrypoint, link, and asset metadata matching `docs/mobile_packaging_abi.md`.

Target outcome:

- Android and iOS both consume the same compiled game core.
- Platform shells do not guess exported symbols or asset locations.

### P1: add a shared mobile runtime mode

Needed:

- Link the canonical SDL renderer as the mobile runtime path.
- Ensure the runtime can be linked into mobile app targets without the desktop runner.
- Keep input/audio/asset APIs the same at the Stasis level.

Target outcome:

- One C runtime core for both mobile platforms.

### P2: add an Android shell

Needed:

- `mobile/android/` checked into the repo
- minimal Gradle wrapper project
- minimal SDL activity integration
- CMake build that links:
  - mobile runtime
  - compiled game objects
  - bundled assets

First version should support:

- one game per app
- `arm64-v8a` only
- debug install to one connected device

### P3: add an iOS shell

Needed:

- `mobile/ios/` checked into the repo
- minimal Xcode app target
- native startup path that calls into the same runtime/game ABI as Android
- bundled assets in app resources

First version should support:

- one game per app
- `arm64` device builds
- simulator optional after device path works

### P4: add one common packaging step

Needed:

- a host-side packaging command that takes:
  - entry `.stasis` file
  - target platform
  - output app shell path
- it should compile AOT, copy assets, write manifest metadata, and hand off to platform build tools

This should be the only Stasis-specific mobile build command developers need to learn.

## What To Avoid If We Want This Lean

Do not do these in the first mobile pass:

- No mobile JIT.
- No hot swap.
- No "run arbitrary `.stasis` source on device" flow.
- No plugin or external code loading.
- No separate renderer stacks per mobile platform.
- No multi-ABI Android matrix initially.
- No attempt to keep the current desktop runner model on mobile.
- No extra mobile abstraction framework on top of SDL.

Each of those adds setup or maintenance that the current repo does not need in order to ship a first mobile game.

## Recommended First Shipping Scope

If the goal is the leanest useful result, the first real scope should be:

1. Android `arm64-v8a`
2. iOS `arm64`
3. One bundled sample game
4. AOT only
5. SDL-only runtime
6. Fixed assets in app bundle
7. No dev hot reload on device

That gives the shortest path to a working mobile build without committing to a large mobile platform surface.

## Concrete Recommendation

Build mobile around a new release-only path:

- `stasis package-mobile --target android-arm64 --entry samples/foo/main.stasis`
- `stasis package-mobile --target ios-arm64 --entry samples/foo/main.stasis`

Internally that should:

1. run the existing AOT compiler path
2. emit mobile-linkable objects
3. link those objects into a checked-in thin platform app shell
4. copy assets into the platform bundle

That is the smallest architecture that matches both:

- current StasisLang direction
- iOS platform rules
- Android native build expectations
- low setup pressure for contributors

## Source Links

Official sources:

- Rust Android target support: https://doc.rust-lang.org/rustc/platform-support/android.html
- Rust Apple iOS target support: https://doc.rust-lang.org/rustc/platform-support/apple-ios.html
- Android NDK CMake guide: https://developer.android.com/ndk/guides/cmake
- Android ABI guide: https://developer.android.com/ndk/guides/abis
- Apple App Store Review Guidelines: https://developer.apple.com/app-store/review/guidelines/
- SDL Android notes: https://github.com/libsdl-org/SDL/blob/main/docs/README-android.md
- SDL iOS notes: https://github.com/libsdl-org/SDL/blob/main/docs/README-ios.md

Repo references:

- `README.md`
- `docs/spec_implementation_status.md`
- `runtime/CMakeLists.txt`
- `runtime/README.md`
- `runtime/build_android.ps1`
- `runtime/stasis_runner.c`
- `crates/stasis_dynload/src/lib.rs`
