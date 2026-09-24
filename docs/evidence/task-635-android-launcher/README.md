# Android launcher resources: task 635

The package contract requires the game to declare `android.launcher_resources` for
production Android builds. A generated CI smoke game with project-owned icon
resources was packaged using a freshly compiled `stasis` CLI on Windows.

## Source and publication checks

- An iconless production `android-arm64` package failed with
  `release Android packaging requires android.launcher_resources`; the requested
  output directory was not published.
- The same iconless game packaged with explicit `--development-build`, and its
  generated manifest contained no icon attributes or unresolved token.
- The branded production `android-arm64` package receipt recorded
  `branding/android/res`; its manifest declared both `android:icon` and
  `android:roundIcon` as `@mipmap/ic_launcher`.

## Compiled artifact checks

| Artifact | SHA-256 | Result |
| --- | --- | --- |
| Fresh arm64 release AAB (unsigned) | `feab890b5010dda8eb26689f280e3a21fa4f32a19495b8af6aba4fc5528a3c25` | Gradle `:app:bundleRelease`; runtime-only payload; bundletool resolved five legacy densities and adaptive anydpi resource |
| Fresh x86_64 debug APK (debug signed) | `b1b502fff2b53f74ecc27f3e22d6a0800237e180aaea6c777b76523ce69b9c4c` | Gradle `:app:assembleDebug`; apksigner v2 verified; `aapt` badging names the adaptive launcher XML; compiled icon gate passed |

The final rebased APK was installed on the API 35 x86_64 emulator. Android's app drawer showed
`Stasis CI Smoke` with the blue-green project icon. UIAutomator found the icon
at `[293,1707][540,2022]`; tapping it focused
`org.stasislang.ci_smoke/.ReplayLaunchActivity`, and the app process remained
running. Existing signed Toddler Match APK and AAB artifacts, carrying its own
adaptive foreground drawable, also passed the compiled icon checker.

Focused validation: `cargo test -p stasis --bin stasis android_` (7 tests),
`cargo test -p stasis --bin stasis` (193 tests),
`python -m unittest tools.ci.test_android_release
 tools.ci.test_check_android_release_package
 tools.ci.test_check_android_launcher_icon` (36 tests),
`python tools/ci/check_android_shell.py`, YAML parse of the two modified
release workflows, `cargo fmt --all -- --check`, and `git diff --check`.

This evidence tests launchers on an emulator and compiled resources in an arm64
bundle. It does not claim a physical arm64 device or Play Console publication.
