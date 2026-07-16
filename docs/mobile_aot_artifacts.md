# Mobile AOT Artifacts

Mobile builds consume release-only AOT output. They do not use the desktop JIT,
hot swap path, dynamic loading, or on-device compilation.

The host command is:

```powershell
cargo run -p stasis --release -- mobile-aot-bundle --target android-arm64 --project-dir mobile\android\app\src\main\assets\workshop_sample --entry-file src\main.stasis --out-dir target\mobile-aot\android
cargo run -p stasis --release -- mobile-aot-bundle --target ios-arm64 --project-dir mobile\android\app\src\main\assets\workshop_sample --entry-file src\main.stasis --out-dir target\mobile-aot\ios
```

Supported first-scope targets:

- `android-arm64`: emits `aarch64-linux-android` position-independent object files.
- `ios-arm64`: emits `aarch64-apple-ios` position-independent object files.

Each bundle contains:

- `*.obj`: one mobile-linkable object per reachable Stasis function.
- `engine_bundle_manifest.json`: function symbols, required runtime entrypoints,
  string literal metadata, collection max lengths, and optimization profile.
- `published_aot_symbols.h`: C macros for `main`, `tick`, `render`, and optional
  `on_code_swap` symbols.
- `published_aot_bindings.c`: platform-neutral registration for linked function
  pointers and string literals consumed by the shared mobile runtime core.
- `mobile_aot_bundle_manifest.json`: package-level target, object, symbol-header,
  engine-manifest, asset-root, and asset-manifest paths for platform shells.
- `apk_assets/stasis_game/...` for Android assets or `ios_assets/stasis_game/...`
  for iOS assets.

The older `android-aot-bundle` command remains as an Android compatibility
wrapper and writes `published_aot_objects.cmake` for the Android shell. Android
published builds pass the descriptor-owned `entrySource` as `--entry-file`.

Ordinary users should use `stasis package-mobile`; the raw bundle subcommands
are compiler/shell integration seams. See `docs/mobile_packaging.md`.
