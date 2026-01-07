# Self-Hosted Compiler: Advantages Over Stage0 (C#)

This repo currently has two compiler implementations:

- Stage0: C# compiler (AOT `build/aot/Stasis.Cli.exe`) used to bootstrap.
- Self-host: Stasis compiler (`stasis`, built from `src/stasis/main.stasis`) intended to become the long-term compiler.

This document lists improvements that become practical (or uniquely enabled) when the compiler is written in Stasis and follows Stasis' constraints (static global state, explicit memory, deterministic layout), compared to a managed C# implementation.

## What Self-Hosting Enables (Uniquely)

### 1) True no-.NET distribution and deployment
The self-host compiler can ship as a single native executable without a .NET runtime dependency.

What this unlocks:
- Portable compiler toolchain for minimal environments (CI images, game-jam setups, offline machines).
- Fewer moving parts in "stasis installs stasis" workflows.
- A clearer story for cross-platform packaging once the runtime backend is stable.

Stage0 can be AOT, but it still depends on managed/runtime concerns and tends to be heavier operationally.

### 2) Strict determinism from the same rules the language enforces
Stasis' design expects:
- A single global state struct holding everything.
- Static memory only (no hidden allocations).
- Deterministic memory layout decisions.

A self-host compiler can be written to obey those rules by construction:
- All compiler caches and buffers are explicit arrays/slices within global state.
- All "resets" can be bulk-zeroed (memset-like) instead of relying on GC and object graphs.
- Performance behavior is more predictable because memory and control flow are explicit.

In C#, avoiding allocations and enforcing determinism is possible, but it is not the default and requires constant discipline and profiling.

### 3) Bootstrapping pressure improves the language and the standard library
Self-hosting forces the language to be good at building real programs:
- Lexer/parser/IR builder patterns are validated by a large, complex Stasis codebase.
- Diagnostics become more actionable because the compiler itself needs them daily.
- The stdlib gets exercised under real workload patterns (string builders, path ops, IO glue).

Stage0 can evolve the language, but it does not automatically dogfood every feature in the same way.

### 4) A single implementation model from user code to compiler code
When the compiler is in Stasis, the same constraints and idioms apply everywhere:
- Iteration-first parsing and scanning patterns can be standardized.
- Module import semantics are enforced by the same model the language uses.
- "Target is first parameter" conventions can be consistently applied across compiler and user code.

This reduces conceptual overhead for contributors: learning the compiler also teaches the language, and vice versa.

### 5) Backend parity work becomes mechanical and test-driven
With a self-host compiler producing IR in a deterministic way, it becomes easier to:
- Define a stable "core IR" representation (even if just textual) with golden-file tests.
- Run the same test fixtures against LLVM and Cranelift and compare output/behavior.
- Add compiler passes that are explicit array transforms over IR nodes, which fits Stasis well.

Stage0 can do this too, but the self-host can make "IR as data" extremely concrete and easy to snapshot because the representation is already explicit and allocation-free.

### 6) Stronger guarantees against accidental recursion/stack blowups
If the self-host implementation prefers iterative algorithms (explicit stacks, queues, index-based loops), then:
- Deep import graphs and large files are handled without risking recursion limits.
- Failure modes are more predictable, and memory usage is bounded by explicit buffers.

This is doable in C#, but self-hosting encourages it because Stasis code naturally leans on explicit stacks and fixed-size storage.

## Practical Improvements We Can Build Next (Self-Host Friendly)

These are concrete areas where the self-host approach should pay off:

### A) Faster "watch" and incremental compilation
With global-state caches and explicit invalidation, the self-host compiler can:
- Persist token streams and signature tables across watch ticks.
- Re-lex only changed files and re-run only dependent phases.
- Avoid re-allocating AST/IR structures by reusing arenas and bulk-clearing ranges.

Stage0 can implement incremental compilation, but managing allocations and cache lifetimes is typically harder and less transparent.

### B) Built-in profiling hooks with near-zero overhead
Because the compiler owns its memory and timing points explicitly, it can:
- Collect per-phase timings and counts (tokens, nodes, diagnostics) into fixed arrays.
- Emit a single deterministic summary that is stable across runs.

In C#, profiling often skews results due to GC, JIT/AOT differences, and allocation noise.

### C) More aggressive compile-time limits and guarantees
The compiler can enforce hard ceilings deterministically:
- File count limits (e.g., 300 files) and file size limits (e.g., 1 MiB).
- Upper bounds on token count, IR instruction count, and string pool sizes.
- Clear, early errors that do not depend on OS memory pressure.

In managed environments, memory behavior can be less predictable under load.

### D) Unified CLI behavior across platforms
Self-hosting pushes toward:
- Stable process spawning (no-shell `sys_spawn`) for reproducible builds.
- Reduced reliance on platform-specific shell quoting rules.
- A consistent toolchain invocation model for `build`, `run`, `release`, `test`, and `watch`.

## Things Stage0 Still Does Well

Self-hosting is not automatically better at everything. Stage0 remains valuable for:
- Bootstrapping and regression-checking the self-host compiler.
- Rapid iteration when changing syntax/semantics (richer host ecosystem, libraries).
- Debugger experience and tooling in C# for complex semantic work.

## Summary

The self-host compiler is viable as a long-term direction when it:
- Compiles all supported Stasis programs with behavior parity across backends, and
- Provides a faster, more deterministic CLI experience (especially in watch/test loops).

The biggest unique advantage is not "rewriting in Stasis" itself, but that Stasis' explicit-memory model
lets the compiler be predictable, allocation-free, incremental, and easy to benchmark and constrain.

