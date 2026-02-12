# Bootstrap Compiler (Windows)

This folder contains a built bootstrap compiler pulled from `main` so Rewrite V1 can continue while self-hosting is in progress.

## Location

- Compiler CLI: `bootstrap/windows/stasis-cli/Stasis.Cli.dll`
- Batch launcher: `bootstrap/windows/stasisc.bat`

## Usage

```bat
bootstrap\windows\stasisc.bat run path\to\file.stasis --emit-ir
bootstrap\windows\stasisc.bat test --all
bootstrap\windows\stasisc.bat build path\to\file.stasis --backend cranelift
bootstrap\windows\stasis-cranelift-run.bat path\to\file.stasis
```

## Cranelift Run Helper

`bootstrap\windows\stasis-cranelift-run.bat` provides a reproducible local dev path for running `.stasis` files with Cranelift on this branch.

What it does:
- Builds `tools/cranelift-aot` if needed.
- Sets `STASIS_CRANELIFT_AOT` to the built helper binary.
- Sets `STASIS_BOOTSTRAP_NO_PREPROCESS=1` by default to avoid temp-copy execution policy issues.
- Adds a common local `clang.exe` path (LLVM or VS Build Tools) to `PATH` when present.
- Runs:
  - `stasisc run <file> --backend cranelift --no-cranelift-runner`

## Receiver-Style Compatibility Shim

`bootstrap\windows\stasisc.bat` runs a temporary preprocessing shim so receiver-style mutating conversion statements compile with the bootstrap compiler:

- `target.from_i32(expr);` -> `target = i32_to_f32(expr);`
- `target.from_f32(expr);` -> `target = f32_to_i32(expr);`
- `target.from_u32(expr);` -> `target = u32_to_i32(expr);`

Disable this shim only if needed:

```bat
set STASIS_BOOTSTRAP_NO_PREPROCESS=1
```

## Note

This is a temporary bootstrap path. Rewrite V1 targets an in-process Rust host with Stasis-owned compiler logic.
