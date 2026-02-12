# Bootstrap Compiler (Windows)

This folder contains the bootstrap compiler runtime used while Rewrite V1 self-hosting is in progress.

Bootstrap source in this branch:

- `Stasis.Compiler/`
- `Stasis.Cli/`

These are retained specifically as bootstrap compiler sources for compatibility/testing while self-hosted compiler logic continues to move into `.stasis`.

## Location

- Compiler CLI binaries: `bootstrap/windows/stasis-cli/`
- Batch launcher: `bootstrap/windows/stasisc.bat`
- Cranelift helper launcher: `bootstrap/windows/stasis-cranelift-run.bat`

## Usage

```bat
bootstrap\windows\stasisc.bat run path\to\file.stasis --emit-ir
bootstrap\windows\stasisc.bat test --all
bootstrap\windows\stasisc.bat build path\to\file.stasis --backend cranelift
bootstrap\windows\stasis-cranelift-run.bat path\to\file.stasis
```

## Stable Temp Output Path

Bootstrap execution writes transient CLIF/object/exe files to:

- `.<repo>/.stasis_cache/tmp`

This avoids `%TEMP%` execution-path issues on stricter Windows Application Control setups.

The wrapper sets:

- `STASIS_TEMP_DIR=<repo>/.stasis_cache/tmp`

## Build and Refresh Bootstrap Binaries

Rebuild + copy fresh binaries into `bootstrap/windows/stasis-cli`:

```powershell
powershell -ExecutionPolicy Bypass -File bootstrap/windows/build-bootstrap.ps1
```

Optional local signing:

```powershell
powershell -ExecutionPolicy Bypass -File bootstrap/windows/build-bootstrap.ps1 -Sign -CreateCert -TrustLocalCert
```

## Local Signing Script

`bootstrap/windows/sign-bootstrap.ps1` signs `.exe/.dll` files in `bootstrap/windows/stasis-cli`.

- Uses a local code-signing cert (default subject `CN=Stasis Local Dev`).
- Can create a cert (`-CreateCert`) if missing.
- Can trust cert for current user (`-TrustLocalCert`), which helps SmartScreen/App Control trust prompts.

## Cranelift Run Helper

`bootstrap/windows/stasis-cranelift-run.bat`:

- Builds `tools/cranelift-aot` if needed.
- Sets `STASIS_CRANELIFT_AOT` to the built helper binary.
- Adds common local `clang.exe` locations to `PATH` when present.
- Runs `stasisc run <file> --backend cranelift --no-cranelift-runner`.

## Receiver-Style Compatibility Shim

`bootstrap/windows/stasisc.bat` uses a preprocessing shim for bootstrap compatibility:

- `target.from_i32(expr);` -> `target = i32_to_f32(expr);`
- `target.from_f32(expr);` -> `target = f32_to_i32(expr);`
- `target.from_u32(expr);` -> `target = u32_to_i32(expr);`

Disable only if needed:

```bat
set STASIS_BOOTSTRAP_NO_PREPROCESS=1
```

## Distribution Policy

Generated bootstrap binaries should be distributed via CI/release artifacts (Windows/Linux) rather than committed as routine source diffs.

This branch keeps local bootstrap binaries only as a temporary development bridge.
