# Lean mobile packaging

Stasis mobile releases are AOT-only, one game per app, and arm64-only. The one
Stasis-specific command is:

```text
stasis --workspace path/to/game package-mobile --target android-arm64
stasis --workspace path/to/game package-mobile --target ios-arm64
```

`stasis.json` supplies the entry source. Use `--entry path/to/main.stasis` to
select another project-relative import root and `--out path` to select a new,
nonexistent output directory. Packaging is atomic: compiler or file failures do
not publish a partial app project.

Each output contains the same pieces:

- `aot/`: target-native game objects, generated entry bindings, ABI metadata
- `runtime/`: the shared SDL-only mobile runtime sources
- `common/`: the fixed `SDL_main` lifecycle adapter
- `stasis_mobile_package.json`: the versioned package receipt
- `android/` or `ios/`: a thin platform-native app project

No package contains the Stasis compiler, JIT, watcher, dynamic game loader, or
writable Stasis source.

## Android arm64

Install JDK 17, Android SDK 35, NDK, CMake 3.22.1, Ninja, and Gradle 8.9. Keep
local SDL2 and SDL2_image source checkouts and set:

```text
STASIS_SDL2_SOURCE=/absolute/path/to/SDL
STASIS_SDL2_IMAGE_SOURCE=/absolute/path/to/SDL_image
```

From the generated `android/` directory run `gradle :app:assembleDebug` or
`gradle :app:installDebug`. Gradle/CMake builds only `arm64-v8a`, compiles the
shared runtime, links the generated AOT objects, and packages
`assets/stasis_game`. No vcpkg installation is used.

The SDL shell preserves the logical dimensions requested by the game while
rendering into the device's native drawable surface. Original SVG files remain
in the packaged assets and are rasterized locally for the ratio between the
game's logical resolution and the device drawable. The shared runtime caches
those GPU rasters by source and logical target size, scales TrueType atlases for
the same drawable density, maps touch input back to logical coordinates, and
refreshes density-dependent caches after surface-size changes. Games should
author layout in logical coordinates rather than applying Android density
multipliers themselves.

## iOS arm64

On macOS install Xcode and obtain device-capable `SDL2.xcframework` and
`SDL2_image.xcframework` bundles in one directory. From generated `ios/` run:

```text
xcodebuild -project StasisMobile.xcodeproj -scheme StasisMobile \
  -configuration Debug -sdk iphoneos -arch arm64 \
  STASIS_SDL_FRAMEWORKS=/absolute/path/to/frameworks \
  DEVELOPMENT_TEAM=YOUR_TEAM_ID build
```

The checked-in Xcode shell compiles the same runtime, links the iOS AOT objects,
and copies `StasisMobile/stasis_game` into the app resources. Device arm64 is
the v1 target; simulator and multi-architecture packaging are intentionally out
of scope.
