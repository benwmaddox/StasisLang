# Android package

Install JDK 17, Android SDK 35, NDK, CMake 3.22.1, Ninja, and Gradle 8.9.
Set `ANDROID_HOME`, `STASIS_SDL3_SOURCE`, and `STASIS_SDL3_IMAGE_SOURCE` to
local SDL3 and SDL3_image source checkouts, then run:

```text
gradle :app:assembleDebug
gradle :app:installDebug
```

Each generated project builds exactly one ABI: production `android-arm64`
selects `arm64-v8a`, while development-only `android-x86_64` selects `x86_64`
for emulator tests. The app links the AOT objects under `../aot`, the
shared SDL-only Stasis runtime under `../runtime`, and bundled assets under
`app/src/main/assets/stasis_game`. No Stasis compiler, JIT, watcher, dynamic
game loader, or writable source is included.

The Android activity adds one release diagnostic: a three-finger tap toggles a
five-second rolling tick/render timing overlay with average, p50, p95, and
60-fps frame-budget usage. It is hidden when the game starts. The same
safe-inset-aware overlay layer presents startup/runtime resource failures.

Packaged assets are copied into the app-private directory only on a cold cache
path. The cache reads the small packaged `assets/manifest.json` on every
activity creation, then reuses a matching extracted tree when its versioned
marker agrees on the package name, release identity, manifest SHA-256, and
verified file inventory (size and modification time). A cold path copies the
tree, SHA-256 verifies every declared asset, writes the marker last, and
publishes by rename with rollback protection. This metadata inventory avoids
rehashing all asset bytes on ordinary recreation while remaining inside the
app-private trust boundary: missing, truncated, mutated, extra, partial, stale,
or corrupt state is rejected and rebuilt. The marker seals a fully verified
tree inside the app-private trust boundary; the inventory rejects observable
metadata/tree changes without claiming cryptographic detection of a same-
privilege rewrite that restores every recorded metadata value. Startup logs
cold/reuse elapsed time and packaged/cache read-write byte counters.

When preparation rejects a package, the cache returns a stable
`code=<cause> path=<asset> detail=<reason>` diagnostic. The Java overlay and
native SDL gate preserve that same diagnostic, and `SDL_main` returns before
AOT binding, game initialization, or frame submission. The IT-022 emulator
seam builds missing, tampered, traversal, duplicate, oversized, and malformed-
manifest variants, checks that staging is never published, then launches the
pristine package as a recovery proof. The oversized case uses a seam-only
one-byte bound override while retaining the production 128 MiB default, so CI
does not need to carry or package a 128 MiB fixture.

iOS does not use this extraction cache. Its immutable app-bundle assets are
opened directly by the iOS shell; the Android cache is not forced onto that
platform.

Future candidates are recorded in `docs/android_release_shell_backlog.md`.

The generated shell also supports an opt-in integration-test launch extra,
`stasis.seam_test_id`. It enables bounded `stasis.seam_test.v1` log markers for
initialization, the first frame, stable frame 30, and fixture-owned probe
sequence changes; ordinary app launches do not compile or enable the marker
hooks. CI runs IT-017 through IT-022 on a hosted API 35 x86_64 emulator. The same driver can be
run against an ARM64 device with the default target, or an x86_64 emulator with
`-Target android-x86_64`:

```powershell
mobile/android/test_release_shell.ps1 -Serial <device-serial>
```

The driver builds a fresh generated package, verifies lifecycle/checksum/trace
markers and named capture regions, retains JSON/log/screenshot evidence, then
force-stops the app, removes a test-only install, and restores the device's
prior immersive-confirmation setting.

IT-018 reuses that driver with a portrait logical fixture on the landscape
surface. It injects Android touchscreen gestures in the real pillarbox and
content regions, then verifies ordered SDL/HostFrame pointer edges, logical and
normalized coordinates, one Stasis state transition, and the resulting frame:

```powershell
mobile/android/test_release_shell.ps1 -Serial <device-serial> `
    -ProjectPath samples/android_touch_seam
```

IT-019 drives an odd `1001 x 1601` display override through portrait,
landscape, and restored portrait. Each stage waits for the AOT guest to observe
the new HostFrame display generation during `tick`, injects a logical-coordinate
touch, and verifies the same frame's guest metrics, pointer transform, command
trace, and named pixel regions. Native dimensions must match the configured
surface; drawable dimensions must match the fitted 360 x 720 SDL letterbox
viewport:

```powershell
mobile/android/test_release_shell.ps1 -Serial <device-serial> `
    -ProjectPath samples/android_orientation_seam
```

The driver independently restores any prior display-size override, user and
accelerometer rotation settings, immersive confirmation, package installation,
and process state even when an assertion fails.

`mobile/android/test_release_shell_emulator.ps1` is the CI entrypoint. It
requires exactly one ready emulator, rejects physical-device serials, verifies
`x86_64`, and runs IT-017, IT-018, and IT-019 sequentially. The GitHub
workflow owns AVD startup and shutdown. Physical-device runs remain useful
supplemental release evidence but do not gate CI readiness.
