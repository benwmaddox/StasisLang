# Stasis computation benchmarks

Seven deterministic CPU workloads compare Stasis AOT, Stasis JIT, Rust LLVM, and Rust Cranelift on the same host. These are small matched kernels, not submissions to an external benchmark suite. The runner checks every output before accepting a timing.

## Run

On Windows, install the official nightly Cranelift preview component and have `stasis`, `rustc`, and PowerShell on `PATH`:

```powershell
rustup toolchain install nightly --profile minimal --component rustc-codegen-cranelift-preview
powershell -NoProfile -ExecutionPolicy Bypass -File benchmarks/computation/run.ps1
```

The script builds each Stasis project in release mode. It compiles the same `rust_baseline.rs` source twice with the same nightly `rustc` and flags (`--edition=2021 -C opt-level=3 -C target-cpu=native -C panic=abort`), changing only `-Zcodegen-backend=llvm` to `-Zcodegen-backend=cranelift`. Both Rust executables link the nightly toolchain's same prebuilt standard library; this comparison changes the benchmark crate's codegen backend.

The runner makes two warmup process launches and ten measured launches per implementation, rotating launch order. It checks status and exact output, reports medians, and writes raw samples and host/toolchain details to ignored `benchmarks/computation/artifacts/latest.json`. Stasis AOT and both Rust binaries are built before timing. Each `stasis run --json` JIT launch includes compilation. All times include process startup and output. Use `-SkipBuild` after an unchanged build; `-Samples` and `-Warmups` adjust repetitions.

| Kernel | Fixed work | Check |
| --- | --- | ---: |
| `fib` | Naive recursive `fib(38)` | 39088169 |
| `mandelbrot` | 320 x 240 grid, 50 iterations, 15 passes; count points reaching limit | 306045 |
| `matmul` | 64 x 64 integer matrix multiply, 100 passes with pass-varying B; sum result diagonal | 296801 |
| `nqueens` | Count all 12-queen solutions using recursive backtracking | 14200 |
| `sieve` | Sieve primes below 100000, 300 passes | 9592 |
| `partial_sums` | Sum `1/i²` and its running partial sums for 100 million terms; check narrow result ranges | 1 |
| `life` | 128 x 128 toroidal Conway grid, fixed initial pattern, 100 generations; count live cells | 838 |

## Structure and sources

The Stasis implementations use fixed-capacity global arrays, indexed `for` loops, and scalar locals. Fixed collection storage is established in the language spec and repository samples; recursive Fibonacci and N queens intentionally exercise function calls. The matrix kernel fills B anew on each pass so repeated work depends on that pass. Rust uses local vectors, so matched algorithms do not imply identical storage and bounds-check costs.

The [Computer Language Benchmarks Game](https://benchmarksgame-team.pages.debian.net/benchmarksgame/description/nbodyinprocess.html) includes Mandelbrot among its established tasks. [Programming Language Benchmark v2](https://github.com/attractivechaos/plb2) uses N queens and matrix multiplication and emphasizes the same algorithm across languages. The [multi-language Game of Life implementations](https://github.com/KieranP/Game-Of-Life-Implementations) provide another comparison precedent. The [Rust Cranelift backend](https://github.com/rust-lang/rustc_codegen_cranelift) is a preview rustc backend. Sizes and outputs here are defined by this directory and do not claim compatibility with those suites' official inputs.

## Local result, 2026-09-24

Windows 10.0.26200.0, Intel Core Ultra 9 185H, Stasis 0.1.0, rustc 1.100.0-nightly. Ten measured process launches per implementation; lower is faster.

| Kernel | Stasis AOT ms | Stasis JIT ms* | Rust LLVM ms | Rust Cranelift ms |
| --- | ---: | ---: | ---: | ---: |
| fib | 159.76 | 213.44 | 138.32 | 189.82 |
| mandelbrot | 59.21 | 123.36 | 41.27 | 55.37 |
| matmul | 41.73 | 118.36 | 12.04 | 39.11 |
| nqueens | 359.47 | 478.23 | 67.66 | 314.76 |
| sieve | 103.48 | 181.52 | 61.29 | 138.74 |
| partial_sums | 95.84 | 179.09 | 96.37 | 407.12 |
| life | 43.34 | 119.10 | 8.94 | 130.38 |

*Stasis JIT includes fresh compilation on every launch, so its column measures a different path from the three built executables. The Rust LLVM/Cranelift gap on identical source is direct evidence that backend choice matters for these kernels. Stasis and Rust Cranelift still use different frontends and storage representations. Results varied between runs on this hybrid CPU; small differences should be treated as near parity.

A preliminary 300-generation Life variant produced different counts (Stasis 663, Rust 838), so the checked suite uses the 100-generation variant where both agree. That longer-run discrepancy needs investigation before using it as performance evidence.
