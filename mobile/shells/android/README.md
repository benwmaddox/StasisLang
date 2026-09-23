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

## Packaged replay documents

The generated Android shell offers `Play` and `Import replay` from its
launcher dialog before `MainActivity` loads the native library. Import opens
`ACTION_OPEN_DOCUMENT` for JSON or octet-stream documents. Java copies the
selected URI into app-private storage with a 256 MiB bound, flushes it, and
publishes it with a same-directory rename before native startup consumes the
path through the bounded `--replay` loader. The same startup bridge accepts a
cold `ACTION_VIEW` URI or the `stasis.replay_uri` intent extra. A warm
`ACTION_VIEW` is also bounded and staged, then Android displays a restart
message and keeps the selection queued for the next launch instead of dropping
it. Failed copies leave the previous published replay intact; failed rollback
keeps the prior document in private recovery storage.

The Java status overlay polls the packaged host's replay receipt and displays
verified completion or divergence; replay load failures remain on the runtime
error surface. Android also opens a result dialog from the terminal receipt
when SDL closes, so completion or divergence stays visible after the game exits.
`requestReplayExport()` writes a copy of the staged imported replay
through `ACTION_CREATE_DOCUMENT`, with the same size bound and a durable
provider close. It does not create a new recording or claim that it does. SAF
providers own the final external-document transaction, so an interrupted
provider write is not reported as a successful export.

`StasisReplaySafTest` covers the exact size boundary, rejection of empty and
oversized input, temporary-file cleanup, and preservation of the previous
published replay when the source fails during copying.

The generated shell also supports an opt-in integration-test launch extra,
`stasis.seam_test_id`. It enables bounded `stasis.seam_test.v1` log markers for
initialization, the first frame, stable frame 30, and fixture-owned probe
sequence changes; ordinary app launches do not compile or enable the marker
hooks. CI runs IT-017 through IT-024 on a hosted API 35 x86_64 emulator. IT-024
packages separate main, tick, and render failures, requires the exact entry and
code in the native log and Java accessibility overlay, verifies zero submitted
or presented frames, and holds the original process alive on the error surface.
The shared mobile runtime ABI is version 2 because shells can now read both the
last entry identity and its exact signed result before shutdown clears runtime state.
The same driver can be
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
surface; native and drawable dimensions must match the complete renderer
backing. The driver independently derives and validates the fitted 360 x 720
SDL letterbox viewport and its content/raster scale:

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

## Android storage persistence seam

`IT-023` packages `samples/android_storage_seam` and verifies the AOT storage
stdlib across three fresh Android processes. The lane checks the exact scoped
file through `run-as`, corrupts only that file, requires the guest fallback on
the next launch, and proves unrelated scope/key and traversal paths are absent.
