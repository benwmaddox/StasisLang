# AOT Brickout Quality Gate (Windows)

This quality gate ensures the Rust-native AOT engine bundle for Brickout Revenge v1 can be
compiled, linked into a DLL, and executed headlessly for more than one tick.

## Command

```powershell
cd F:\StasisLang
$env:STASIS_AOT_QUALITY_GATE = "1"
cargo test -p stasis aot_brickout_revenge_v1_engine_bundle_executes_two_ticks -- --nocapture
```

## Prerequisites

- Windows
- Rust toolchain (`cargo`)
- `lld-link.exe` (Visual Studio 2022 Build Tools / Community is fine)

## What It Does

- Builds `stasis_dynload` as a Rust `staticlib` for AOT runtime shims
- Compiles `samples/brickout_revenge/brickout_revenge_v1.stasis` in `TargetMode::AotProd`
- Links emitted AOT objects into a temporary `brickout_aot_bundle.dll` via `lld-link`
- Loads the DLL, calls exported `main()`, then calls `tick()` twice (expects return `0`)

## Output Artifacts

Artifacts are written to a temp directory:

- `%TEMP%\\stasis_aot_brickout_exec_<stamp>\\`

Notable files:

- `brickout_aot_bundle.dll` (linked image)
- `aot_artifacts\\engine_bundle\\...\\manifest.json` (engine bundle manifest)
