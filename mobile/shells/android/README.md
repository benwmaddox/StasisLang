# Android arm64 package

Install JDK 17, Android SDK 35, NDK, CMake 3.22.1, Ninja, and Gradle 8.9.
Set `ANDROID_HOME`, `STASIS_SDL3_SOURCE`, and `STASIS_SDL3_IMAGE_SOURCE` to
local SDL3 and SDL3_image source checkouts, then run:

```text
gradle :app:assembleDebug
gradle :app:installDebug
```

Only `arm64-v8a` is built. The app links the AOT objects under `../aot`, the
shared SDL-only Stasis runtime under `../runtime`, and bundled assets under
`app/src/main/assets/stasis_game`. No Stasis compiler, JIT, watcher, dynamic
game loader, or writable source is included.

The Android activity adds one release diagnostic: a three-finger tap toggles a
five-second rolling tick/render timing overlay with average, p50, p95, and
60-fps frame-budget usage. It is hidden when the game starts. The same
safe-inset-aware overlay layer presents startup/runtime resource failures, and
startup verifies every packaged asset against its manifest SHA-256 before
replacing the last validated app-private copy. Future candidates are recorded
in `docs/android_release_shell_backlog.md`.
