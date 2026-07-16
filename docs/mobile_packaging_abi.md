# Mobile Packaging ABI

This note defines the first Stasis mobile packaging contract for Android and iOS
work. Mobile is a release-only AOT target. It is not a port of the desktop
development runner.

## Scope

The v1 mobile contract supports:

- one compiled Stasis game per native app
- Android `arm64-v8a`
- iOS `arm64`
- AOT-compiled game code linked into the app at build time
- app-bundled project assets resolved from the Stasis asset manifest
- one shared SDL-only runtime shape for Android and iOS

The contract intentionally excludes:

- JIT on device
- hot swap on device
- runtime dynamic library loading
- plugin-style game loading
- a multi-game launcher
- a multi-ABI Android matrix
- on-device Stasis compilation

## Exported Game Entry ABI

A mobile app shell calls a fixed set of exported game entrypoints produced by
the AOT build:

| Stasis function | Required | Return | Purpose |
|-----------------|----------|--------|---------|
| `main` | yes | `i32` | initialize game state and runtime requests |
| `tick` | yes | `i32` | advance deterministic simulation by one tick |
| `render` | yes | `i32` | emit one frame of render commands |
| `on_code_swap` | no | `void` | ignored by mobile v1; accepted only as inert metadata |

Native signatures:

```c
typedef int32_t (*StasisMobileI32Entry)(void);
typedef void (*StasisMobileVoidEntry)(void);
```

Rules:

- A mobile package is invalid if `main`, `tick`, or `render` is missing.
- The exported native symbols are compiler-generated and recorded in generated
  metadata; app shells must not guess symbol names from Stasis source names.
- Entry calls use the platform C calling convention and take no parameters in
  mobile v1.
- `main`, `tick`, and `render` must resolve to `StasisMobileI32Entry`.
- `on_code_swap`, when present, must resolve to `StasisMobileVoidEntry`, but
  mobile v1 does not call it.
- Symbols must be link-visible to the app shell. They may be plain object-file
  symbols or static-library members, but they must not require runtime dynamic
  lookup.
- The mobile runtime owns platform input, window, audio, asset, and lifecycle
  state. Stasis code observes that state through the existing host API.
- Return value `0` means continue. Non-zero means the shell may request a
  graceful stop or surface a diagnostic, matching existing runner convention.

## AOT Output Contract

The mobile AOT build emits a linkable game bundle, not a desktop executable and
not a dynamically loaded plugin.

Required outputs:

- one or more target-native object files, or a static library containing them
- `stasis_mobile_abi.json`, a versioned metadata file described below
- packaged assets under the platform bundle root described below

The shared mobile AOT helper writes the fixed entry symbols, link inputs, and
asset metadata consumed by both generated platform shells. Neither shell
discovers symbols independently.

### Metadata Schema

`stasis_mobile_abi.json` is UTF-8 JSON. All paths use `/` separators and are
relative to the directory containing the metadata file unless explicitly noted.

Required shape:

```json
{
  "schema": "stasis.mobile_abi.v1",
  "target": "android-arm64",
  "objects": ["obj/game.o"],
  "staticLibraries": [],
  "entries": {
    "main": { "symbol": "aot_fn_...", "signature": "i32()" },
    "tick": { "symbol": "aot_fn_...", "signature": "i32()" },
    "render": { "symbol": "aot_fn_...", "signature": "i32()" },
    "on_code_swap": null
  },
  "assets": {
    "bundleRoot": "stasis_game",
    "manifest": "stasis_game/assets/manifest.json"
  }
}
```

Rules:

- `schema` must be exactly `stasis.mobile_abi.v1`.
- `target` must be `android-arm64` or `ios-arm64` for v1.
- `objects` and `staticLibraries` are ordered linker inputs. At least one must
  be non-empty.
- `entries.main`, `entries.tick`, and `entries.render` are required.
- `entries.on_code_swap` is either `null` or an object with signature `void()`.
- Valid entry signatures are only `i32()` and `void()` in v1.
- `assets.bundleRoot` is the packaged root visible to the runtime.
- `assets.manifest` points to the packaged asset manifest under that root.

## Asset Bundle Contract

Mobile packages include project assets in the app bundle. Stasis code continues
to refer to project-relative logical paths.

Rules:

- `assets/manifest.json` is the source of packaged asset identity.
- Packaged assets preserve the manifest-relative path layout under the
  `stasis_game` bundle root.
- Android places that root under the APK/AAB asset tree as
  `assets/stasis_game`.
- iOS places that root in the app resource bundle as `stasis_game`.
- Runtime asset metadata paths are written relative to the metadata file, while
  platform shell code maps `stasis_game` to the platform-specific bundle access
  API.
- The runtime resolves Stasis asset requests against the packaged manifest, not
  arbitrary host file paths.
- Missing, malformed, or hash-mismatched assets fail deterministically with a
  diagnostic.
- Sprite and audio assets use the same manifest identity on desktop AOT,
  Android, and iOS.

## Runtime Contract

Mobile shells link one shared SDL-only Stasis runtime core.

Rules:

- `STASIS_GRAPHICS_SDL_ONLY` is the mobile runtime direction.
- Android and iOS shells should share the same C runtime ABI wherever platform
  SDK differences do not force a thin adapter.
- The runtime drives the app lifecycle and calls `main` once, then `tick` and
  `render` on the fixed tick/frame loop.
- Platform pause, resume, focus, and shutdown events are runtime-owned and must
  not require dynamic game code replacement.
- Audio and graphics device unavailability must leave gameplay state coherent
  and report diagnostics instead of faking successful device setup.

## Non-Goals As Compatibility Rules

The v1 mobile package must not depend on:

- `stasis_dynload`
- desktop shared-library runner behavior
- `FnId -> code_ptr` hot-swap table updates at runtime
- file watching
- Stasis source files being writable or compiled on device
- downloading or installing code after app review or installation

If a later design adds a mobile development loop, it must be a separate opt-in
mode and must not weaken this release packaging contract.

## First Implementers

Follow-up tasks should use this note as their contract boundary:

- AOT emission work should produce the linkable outputs and metadata above.
- Android shell work should consume those outputs for one `arm64-v8a` app.
- iOS shell work should consume the same outputs for one `arm64` app.
- `package-mobile` is the thin orchestration layer around this contract; it
  copies the AOT bundle into a Gradle or Xcode shell without adding another
  compiler or runtime path.
