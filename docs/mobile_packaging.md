# Lean mobile packaging

Stasis mobile releases are AOT-only, one game per app, and arm64-only. The one
Stasis-specific command is:

```text
stasis --workspace path/to/game package-mobile --target android-arm64
stasis --workspace path/to/game package-mobile --target ios-arm64
```

Official release archives verify their compiler and runtime sources against
`stasis_release_provenance.json` before packaging. A source checkout must add
`--development-build`; that output is explicitly labeled non-release. See
`release_provenance.md` for the manifest and repinning contract.

`stasis.json` supplies the entry source. Its optional Android object supplies
`application_id`, `label`, `orientation`, `version_code`, and `version_name`;
those values become the generated app's package, title, activity orientation,
and release version. Use `--entry path/to/main.stasis` to
select another project-relative import root and `--out path` to select a new,
nonexistent output directory. Packaging is atomic: compiler or file failures do
not publish a partial app project.

```json
"android": {
  "application_id": "com.example.game",
  "label": "Example Game",
  "orientation": "sensorLandscape",
  "version_code": 1,
  "version_name": "1.0.0"
}
```

Each output contains the same pieces:

- `aot/`: target-native game objects, generated entry bindings, ABI metadata
- `runtime/`: the shared SDL-only mobile runtime sources
- `common/`: the fixed `SDL_main` lifecycle adapter
- `stasis_mobile_package.json`: the versioned package receipt
- `stasis_provenance.json`: verified release identity and content hashes
- `android/` or `ios/`: a thin platform-native app project

Mobile packaging follows the selected entry module's import graph. It includes only
manifest assets referenced by static string literals in reachable production source,
plus their transitive manifest dependencies, and emits a manifest containing that same
subset. Relative literals use the selected entry file's directory as their base, even
when they appear in imported modules. Keep dynamically chosen release asset paths out of
string construction; declare the literal paths in reachable source instead. Android and
iOS use the identical rule.

The packaged runtime is the same canonical SDL command interpreter used by the
desktop distribution. The versioned guest buffer and deterministic trace
contract are documented in `shared_renderer_process.md`.

Potential Android release-shell additions are tracked in
`android_release_shell_backlog.md`; they must remain generic or opt-in adapters.

The runtime asset root is always the packaged `stasis_game` project root.
Canonical game paths therefore start with `assets/`. For compatibility with
older source-relative literals such as `../assets/foo.svg`, an explicit
packaged asset root normalizes `.` and leading parent segments without allowing
the resolved path to escape `stasis_game`.

No package contains the Stasis compiler, JIT, watcher, dynamic game loader, or
writable Stasis source.

Android and iOS also embed the provenance manifest below `stasis_game/`. Startup
diagnostics report the release/development label, tag, commit, and renderer ABI.

## Android arm64

Install JDK 17, Android SDK 35, NDK, CMake 3.22.1, Ninja, and Gradle 8.9. Keep
local SDL3 and SDL3_image source checkouts and set:

```text
STASIS_SDL3_SOURCE=/absolute/path/to/SDL
STASIS_SDL3_IMAGE_SOURCE=/absolute/path/to/SDL_image
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

The exact metric fields, aspect-fit input transform, safe viewport, cache keys,
and generation rules are documented in `display_metrics.md` and are shared with
desktop, Workshop JIT preview, and the generated release shell. On Android, a
three-finger tap toggles a rolling tick/render timing overlay in both Workshop
and the generated release app.

## iOS arm64

On macOS install Xcode and obtain device-capable `SDL3.xcframework` and
`SDL3_image.xcframework` bundles in one directory. From generated `ios/` run:

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
