# Release swap callback validation (Maddox #654)

## Contract and platform audit

The release policy removes the implicit `on_code_swap(): void` root and host
binding. Ordinary source calls follow resolved FunctionIds, including overloads
named `on_code_swap`. Explicit `@host_export` declarations retain their declared
contract. Shared functions, main/tick/render, graphics construction reset/finish,
and platform restoration retain their existing responsibilities.

| Surface | Code boundary | Policy and validation |
| --- | --- | --- |
| Windows/Linux/macOS standalone | `compiler_backend` self-host AOT and release engine package | Release; Windows executable and SDL smoke executed |
| Web | `package_web_workspace` | Release unless explicit development build; both final Wasm modules executed in Node |
| LAN/network guests | `stage_desktop_network_guest` / Web guest bundle | Same Web package policy; native network preflight now uses release AOT instead of JIT extern registration |
| Android arm64/x86_64 | `write_mobile_aot_engine_bundle` | Release even for development-labelled standalone shell; both target objects inspected, x86_64 APK executed |
| iOS arm64 | Same mobile AOT writer | Release; Mach-O objects, generated header/bindings and staged assets inspected by test |
| JIT and live AOT | JIT snapshot / incremental compiler backend | Development; real watched swap, failed compile and rejected callback regression passed |

Low-level AOT/Wasm APIs keep a development-compatible default because AOT also
serves live swapping. Package boundaries explicitly select release. Snapshot
revision salts separate the policies; AOT development -> release -> development
coverage verifies pruning and restoration without stale cache reuse. Emission
uses the snapshot's authoritative reachable IDs. AOT registered string literals
are restricted to emitted function references in release mode.

No ABI layout/version, frame ordering, runtime font/audio, vendor, deployment or
global access design changed. Runtime/host contract checks retain 810/975
comparisons; generation and layout oracles remain intact. Package provenance,
asset hashes and generated content identities are regenerated normally.

## Validation environment and commands

Canonical Windows checkout, baseline `676129cbce07eb700c99517381b7214032405bf2`.
All Cargo invocations use `python tools/cargo_cache.py run -- cargo ...`.
The baseline `tools/validate_repo.sh` passed its Python/web/architecture gates
and library/CLI tests, then stopped at a mismatched installed runtime fingerprint.
A fresh CMake SDL runtime and runner were built, signed with the workstation's
existing configured identity, and matched to fresh Rust binaries using explicit
`STASIS_RELEASE_ID` and `STASIS_BUILD_FINGERPRINT`. A stale sibling DLL was also
replaced in the ignored Cargo output directory. No signing identity was created.
The full aggregate was not rerun; bounded owning-target gates were used below.

- `cargo test -p stasis_compiler --lib -- --test-threads=1`: 872 passed.
- `cargo test -p stasis --lib --bin stasis --test web_package -- --test-threads=1`:
  334 library and 482 CLI unit tests passed. The Web suite exposed an unnecessary
  empty inferred asset identity; the corrected full `--test web_package` rerun
  passed all 17 tests.
- `cargo test -p stasis --bin stasis mobile_release_bundles_prune_reload_objects_bindings_and_assets -- --test-threads=1 --nocapture`:
  all Android arm64/x86_64 and iOS arm64 object/binding/asset assertions passed.
- `cargo test -p stasis --test toolchain_cli -- --test-threads=1`: all 41 passed,
  including release asset diagnostics/atomicity and mobile project generation.
  The explicit same-name call test ran an actual Windows executable returning 7;
  the unused no-argument callback produced no output.
- `python tools/ci/run_windows_platform_seams.py --suite DesktopSdl`: all six cases
  passed; covers input, display metrics, manifest assets, asset stress,
  graphics loss/recovery and real watched hot swap.
- `mobile/android/test_release_shell_emulator.ps1 -Serial emulator-5554 -ArtifactRoot build/654-android -TestId IT-020 -PerSeamTimeoutSeconds 660`:
  fresh APK passed launch, background/resume and activity/process recreation.
  Sprite, procedural fallback, direct and cached text pixel counts were preserved.
  Device state was restored and the temporary installation was removed.
- Fresh Windows `windows_launch_smoke` standalone package launched and exited 0
  after frame-2 screenshot; PNG/SVG/font startup and density replacement logged.
- Runtime ABI, host runtime and JIT generation contract scripts passed.
- `cargo fmt --all -- --check` and `git diff --check` passed. No test hosts lingered.

## Final artifact inspection and sizes

Paired fixture: `tests/stasis/seams/release_swap_roots.stasis.fixture`. It includes
a reload-only function/import/asset and a shared function/startup asset. Tests
inspect actual final Web exports/imports, invoke main/tick/render and the dev swap
callback, verify shared assets, and execute explicit same-name calls. Mobile tests
inspect actual generated objects, header, bindings, manifest and staged assets.

| Final Web fixture | Release | Development |
| --- | ---: | ---: |
| Wasm bytes | 594 | 938 |
| Wasm gzip bytes | 347 | 518 |
| Package raw file bytes | 152,072 | 201,831 |
| Package ZIP bytes (deflate level 9) | 43,288 | 50,591 |

Release Wasm has no `on_code_swap` export and no imports; development retains
`on_code_swap` and `print_i32`. Release stages only `assets/shared.svg` plus its
manifest; development stages both shared and reload SVGs. Release optimization
reduced its 705-byte input Wasm to 594 bytes. These are observed release/development
sizes, **not an isolated callback-only saving**: debug metadata, minification and
wasm-opt differ too. No pre-change release size baseline was measured.

The actual IT-020 APK is 4,539,473 bytes. Its final `lib/x86_64/libmain.so` dynamic
symbol table and engine manifest contain no implicit swap callback. Its ZIP asset
list contains resource/fallback SVGs, font, manifest and package/provenance markers.
The Windows final PE export table has no `on_code_swap`; its asset package retains
exactly the smoke PNG/SVG/font and manifest.
Raw inspection reports and logs are under ignored `build/654-*` directories.

## Evidence and limits

Visual evidence: inspected `build/654-desktop-startup.png` (standalone startup
sprites/text) and `build/654-android/android_resource_restore/e/stable-frame.png`
(resource presentation). Android `e/evidence.json` records matching target pixel
counts after resume and recreation. Real development IT-010 reported zero mixed
generations across 51 frames in the final rerun; compile failure and callback rejection retained
revision 2. These are local evidence paths, not checked-in binary artifacts.

Linux/macOS standalone linking/execution and iOS signing/device execution require
their host toolchains and were not run on this Windows host. Android arm64 object
emission was checked; device execution used the available x86_64 emulator.
Networking package routing was audited and existing package/unit tests cover it;
no separate multi-device LAN session was launched.

## Final review

Language Designer, Compiler Architect, Runtime Engineer, Code Expert, Performance
Expert and Human Advocate review: GREEN for the scoped diff. Policy is explicit at
package boundaries; no syntax or ABI change; ordinary calls remain identity-based;
no runtime hot-path work added; artifact and platform limits are stated above.
