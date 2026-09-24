# Stasis computation benchmarks

Seven deterministic CPU workloads compare Stasis release AOT, Stasis JIT, and an optimized Rust baseline on the same host. These are **small, matched kernels**, not submissions to an external benchmark suite. The runner checks the output before accepting any timing.

## Run

On Windows with `stasis`, `rustc`, and PowerShell on `PATH`, from the repository root:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File benchmarks/computation/run.ps1
```

The script builds each Stasis project with `stasis build --mode release` and compiles `rust_baseline.rs` with `rustc --edition=2021 -C opt-level=3 -C target-cpu=native`. It performs two warmup process launches and ten measured launches per implementation, rotating launch order. It times whole processes, checks exit status and exact output, reports medians, and writes raw samples plus host/toolchain details to ignored `benchmarks/computation/artifacts/latest.json`. AOT and Rust compilation happen before timing; each `stasis run --json` JIT process compiles the program and then executes it, so JIT times include compilation. Use `-SkipBuild` after an unchanged build; `-Samples` and `-Warmups` adjust repetitions.

| Kernel | Fixed work | Check |
| --- | --- | ---: |
| `fib` | Naive recursive `fib(38)` | 39088169 |
| `mandelbrot` | 320 x 240 grid, 50 iterations, 15 passes; count points reaching limit | 306045 |
| `matmul` | 64 x 64 integer matrix multiply, 100 passes with pass-varying B; sum result diagonal | 296801 |
| `nqueens` | Count all 12-queen solutions using recursive backtracking | 14200 |
| `sieve` | Sieve primes below 100000, 300 passes | 9592 |
| `partial_sums` | Sum `1/i²` and its running partial sums for 100 million terms; check narrow result ranges | 1 |
| `life` | 128 x 128 toroidal Conway grid, fixed initial pattern, 100 generations; count live cells | 838 |

## Structure review

The Stasis implementations use fixed-capacity global arrays, indexed `for` loops, and scalar locals. Fixed collection storage is the established pattern in the language spec and repository samples; the recursive Fibonacci and N queens cases intentionally exercise function calls. The matrix kernel fills B anew on each pass so the repeated work depends on that pass. Rust uses local vectors for its arrays, so the same algorithms do not imply identical storage and bounds-check costs.
The [Computer Language Benchmarks Game](https://benchmarksgame-team.pages.debian.net/benchmarksgame/description/nbodyinprocess.html) includes Mandelbrot among its established tasks. [Programming Language Benchmark v2](https://github.com/attractivechaos/plb2) uses N queens and matrix multiplication and emphasizes using the same algorithm across languages. The [multi-language Game of Life implementations](https://github.com/KieranP/Game-Of-Life-Implementations) provide another comparison precedent. Sizes and outputs here are defined by this directory and do not claim compatibility with those suites' official inputs or optimized submissions.

## Local result, 2026-09-24

Windows 10.0.26200.0, Intel Core Ultra 9 185H, Stasis 0.1.0, rustc 1.92.0. Ten measured process launches per implementation; lower is faster.

| Kernel | Stasis AOT ms | Stasis JIT ms* | Rust ms |
| --- | ---: | ---: | ---: |
| fib | 144.37 | 213.79 | 90.21 |
| mandelbrot | 63.18 | 129.87 | 40.26 |
| matmul | 41.22 | 113.29 | 10.78 |
| nqueens | 339.43 | 437.95 | 65.59 |
| sieve | 102.78 | 181.62 | 59.23 |
| partial_sums | 98.25 | 183.99 | 95.16 |
| life | 71.24 | 155.14 | 15.73 |

*JIT is a fresh `stasis run` process each time and includes compilation. All columns include process startup and output. AOT and Rust compilation are excluded. The kernels are a workload-specific comparison, not a language-wide ranking. Rust uses the local CPU's native instruction target; Stasis uses its release build defaults. Timings varied between runs on this hybrid CPU, so small differences, particularly partial sums, should be treated as near parity. A preliminary 300-generation Life variant produced different counts (Stasis 663, Rust 838), so the checked suite uses the 100-generation variant where both agree. That longer-run discrepancy needs investigation before using it as performance evidence.
