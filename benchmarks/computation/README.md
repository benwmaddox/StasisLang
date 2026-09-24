# Stasis computation benchmarks

Seven deterministic CPU workloads compare Stasis release AOT executables with an optimized Rust baseline on the same host. These are **small, matched kernels**, not submissions to an external benchmark suite. The runner checks the output before accepting any timing.

## Run

On Windows with `stasis`, `rustc`, and PowerShell on `PATH`, from the repository root:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File benchmarks/computation/run.ps1
```

The script builds each Stasis project with `stasis build --mode release` and compiles `rust_baseline.rs` with `rustc --edition=2021 -C opt-level=3 -C target-cpu=native`. It performs two warmup process launches and ten measured launches per implementation, alternating launch order. It times whole executable processes after compilation, checks exit status and exact output, reports medians, and writes raw samples plus host/toolchain details to ignored `benchmarks/computation/artifacts/latest.json`. Use `-SkipBuild` after an unchanged build; `-Samples` and `-Warmups` adjust repetitions.

| Kernel | Fixed work | Check |
| --- | --- | ---: |
| `fib` | Naive recursive `fib(38)` | 39088169 |
| `mandelbrot` | 320 x 240 grid, 50 iterations, 15 passes; count points reaching limit | 306045 |
| `matmul` | 64 x 64 integer matrix multiply, 100 passes with pass-varying B; sum result diagonal | 296801 |
| `nqueens` | Count all 12-queen solutions using recursive backtracking | 14200 |
| `sieve` | Sieve primes below 100000, 300 passes | 9592 |
| `partial_sums` | Sum `1/i²` and its running partial sums for 100 million terms; check narrow result ranges | 1 |
| `life` | 128 x 128 toroidal Conway grid, fixed initial pattern, 100 generations; count live cells | 838 |

The [Computer Language Benchmarks Game](https://benchmarksgame-team.pages.debian.net/benchmarksgame/description/nbodyinprocess.html) includes Mandelbrot among its established tasks. [Programming Language Benchmark v2](https://github.com/attractivechaos/plb2) uses N queens and matrix multiplication and emphasizes using the same algorithm across languages. The [multi-language Game of Life implementations](https://github.com/KieranP/Game-Of-Life-Implementations) provide another comparison precedent. Sizes and outputs here are defined by this directory and do not claim compatibility with those suites' official inputs or optimized submissions.

## Local result, 2026-09-24

Windows 10.0.26200.0, Intel Core Ultra 9 185H, Stasis 0.1.0, rustc 1.92.0. Ten measured process launches per side; lower is faster. The ratio is Stasis median divided by Rust median.

| Kernel | Stasis ms | Rust ms | Ratio |
| --- | ---: | ---: | ---: |
| fib | 200.69 | 134.94 | 1.49x |
| mandelbrot | 107.76 | 62.57 | 1.72x |
| matmul | 76.14 | 17.50 | 4.35x |
| nqueens | 518.08 | 131.34 | 3.95x |
| sieve | 172.91 | 93.03 | 1.86x |
| partial_sums | 191.29 | 168.19 | 1.14x |
| life | 77.68 | 16.79 | 4.63x |

The timings include process startup and output but exclude compilation. They are a workload-specific comparison, not a language-wide ranking. Rust uses the local CPU's native instruction target; Stasis uses its release build defaults. A preliminary 300-generation Life variant produced different counts (Stasis 663, Rust 838), so the checked suite uses the 100-generation variant where both agree. That longer-run discrepancy needs investigation before using it as performance evidence.
