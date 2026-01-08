# Self-host compiler gap closure plan

This document is the actionable plan to close the remaining gap between:

- Stage0 compiler (C# / `build/aot/Stasis.Cli.exe`) and
- Self-host compiler (Stasis / `build/stasis_release.exe`).

It focuses on getting to a long-term state where the self-host compiler is the default, stage0 becomes a bootstrap/reference implementation, and both produce equivalent semantics and artifacts for supported targets.

## Goals

1. Language parity: self-host compiles the same Stasis programs as stage0 (within defined platform feature gates).
2. Backend parity: self-host can emit and run through LLVM and can emit CLIF for Cranelift AOT with the same semantics as stage0.
3. Workflow parity: `stasis check/build/run/release/test/watch` behaviors match stage0 in user-visible ways.
4. Conformance: a shared test corpus passes on both compilers, and deltas are detectable and attributable.
5. Performance parity: compile+run times are within an agreed envelope for the common paths (especially `stasis test`).
6. Distribution: the self-host compiler is a standalone native executable with no .NET runtime requirement.

Non-goals (until the above land):
- Replacing all stage0 tooling (formatter, LSP, IDE integrations).
- Implementing a full Cranelift code generator in Stasis (prefer text CLIF + external AOT tool).
- Solving every runtime/platform feature at once (graphics/audio/IO are gated).

## Current state (as of `self-host` branch)

Self-host has:
- A native CLI written in Stasis and built as `build/stasis_release.exe` (`clang -O3`).
- Import graph loading with enforced limits (300 files, 1 MiB per file) and fixed-capacity compiler tables.
- Lexer, top-level decl scan, basic stmt parsing, and a growing LLVM IR emitter sufficient to compile and run the compiler itself.
- `stasis test <dir-or-file>` that can run the repo `tests/` suite; it uses `lli` when possible and otherwise links via `clang` (prefers `stasis_sys_static.lib`).

Stage0 remains ahead in:
- Full language surface (more statements, expressions, types, semantics).
- Backend breadth and polish (Cranelift runner/caching, LLVM tool detection).
- Diagnostics completeness and tooling (formatter, LSP).

## Definition of done (gap closed)

Self-host is considered the long-term compiler when:

1. `stasis test --all` (or equivalent) passes on:
   - `tests/`
   - non-interactive `samples/` test suites
   - a dedicated "compiler conformance" corpus (see below).
2. `stasis run/build/release` can build and run all non-interactive samples under LLVM.
3. The self-host compiler can compile itself in release mode reproducibly (same version input -> stable output hash within an acceptable tolerance for embedded timestamps/paths).
4. Stage0 can be treated as "bootstrap only" for normal contributors (self-host is the default path for day-to-day work).

## Workstreams

### 1) Conformance and parity harness

Purpose: detect regressions and quantify remaining gaps.

Deliverables:
- A shared corpus of `.stasis` programs that cover:
  - parsing corner cases
  - module/import resolution
  - type checking and conversions
  - memory layout rules (AoS->SoA lowering)
  - codegen correctness (control flow, calls, arrays, structs, enums, strings)
  - sys/runtime bindings behavior
- A runner that executes each corpus file under both compilers and records:
  - exit code
  - test pass/fail count
  - compile time and run time
  - backend used (LLVM/Cranelift)
  - whether `lli` vs `clang` was used (LLVM path)
- A stable output mode:
  - option to emit IR to a file and compare (normalized) IR between compilers
  - option to hash relevant artifacts per file

Milestones:
1. Expand existing timing scripts to support:
   - `--all` directory traversal equivalence
   - consistent environment variables (PATH tools) between runs
2. Add "IR normalization" rules (strip paths, module IDs, temp names) for comparisons.

### 2) Language frontend parity (parser + module system)

Primary target: accept the same syntax and module resolution rules as stage0.

Subtasks:
- Module/import:
  - ensure import resolution rules match stage0 (path normalization, module naming, duplicate handling)
  - enforce "module members available by default" with correct ambiguity diagnostics
  - ensure resolution order matches (locals, module members, qualified names)
- Parser:
  - close remaining expression grammar gaps (precedence, associativity, indexing, calls, member access)
  - add missing statements used by samples (foreach, while if present, break/continue if required by stage0 semantics)
  - unify `test` declaration parsing and signature rules (params/return expectations)
- Diagnostics:
  - move toward stage0-quality: one primary error per failure mode, stable spans, actionable hints

Constraints:
- Prefer iterative algorithms over recursion where feasible.
- Keep explicit memory use: fixed-size stacks/arenas with overflow diagnostics.

Milestones:
1. Enumerate "unsupported syntax" errors from self-host when compiling `samples/` and turn them into a prioritized list.
2. Close the top N blockers to compile all non-interactive samples.

### 3) Semantic parity (types, layout, effects)

Goal: same meaning for accepted programs.

Subtasks:
- Types:
  - numeric conversions and literal typing rules (including `123u8` and comparisons)
  - enum semantics (auto values, explicit values, comparisons)
  - slices/arrays rules (bounds, element typing)
  - string buffer layouts (`ascii[N]`, `utf8[N]`) and passing conventions
- Memory layout:
  - deterministic struct layout and alignment rules
  - AoS syntax -> SoA storage rules for globals and struct arrays
  - correct offset calculation and member addressing in IR
- Builtins:
  - align builtin lowering to stage0 for `print_*`, conversions, mem operations, `sys_*`

Milestones:
1. Add layout-focused fixtures that assert offsets and addressing (emit IR, compare patterns).
2. Add negative tests for rejected constructs and ensure diagnostics match expected messages.

### 4) Backend parity

#### LLVM path

Goal: match stage0 behavior for execution and linking.

Tasks:
- Execution selection:
  - `lli` fast path when no sys/runtime or graphics libraries are required
  - `clang` path otherwise
- Linking:
  - prefer `stasis_sys_static.lib` for sys runtime
  - link graphics bundle only when required
  - match stage0 flags where feasible (`-Wl,/NOLOGO`, `-Wno-override-module`, `-O3` in release)
- Tool discovery:
  - optional: add a syscall-based "find tool on PATH" or "spawn process without shell" to avoid `system()` overhead and quoting edge cases

#### Cranelift path

Goal: emit CLIF text that is accepted by the existing AOT tool and matches stage0 semantics.

Tasks:
- Keep CLIF emission feature-gated until semantics match.
- Ensure test harness works under Cranelift (or explicitly document unsupported cases).
- Integrate with `tools/cranelift-aot` invocation via syscalls.

Milestones:
1. Self-host can `--emit-ir` CLIF for the full test corpus accepted by stage0 Cranelift.
2. Self-host can `test` via Cranelift for the same subset (runner/AOT tool).

### 5) Runtime/syscall maturity for tooling

This is the main lever to remove remaining perf/robustness gaps that stage0 solves via real process APIs.

Highest impact additions:
- `sys_spawn(exe: utf8[], args: utf8[], ...) -> exit_code` or an argv-style API:
  - no shell quoting issues
  - predictable exit codes
  - optional capture of stdout/stderr for diagnostics
- `sys_cwd(out: utf8[])` to avoid writing temp outputs next to the compiler exe
- `sys_mkdir`, `sys_rmdir`, `sys_remove_file` (tooling hygiene)
- `sys_env_get` (configure tool paths like `lli`, `clang`, cranelift tools)

Milestones:
1. Replace `sys_exec(system())` usage in performance-critical loops with `sys_spawn`.
2. Add a "tool detection" path equivalent to stage0: prefer `lli`, else `clang`, with clear errors.

### 6) CLI parity and UX

Goal: users should not need to know whether they are using stage0 or self-host.

Tasks:
- Align flags and defaults:
  - `stasis test --all` behavior
  - watch loop behavior and output stability
  - `--backend` defaults and errors
  - standardized output formats for CI parsing
- Deterministic output:
  - stable "compiled in ..." / "test-time" lines
  - optional JSON output for automation (future)
- Docs:
  - update `README.md` and `docs/self-host.md` to point to the self-host CLI as default once ready

Milestones:
1. Document all CLI commands and the exact discovery rules for `test`.
2. Add a CI-friendly mode (`--quiet` plus stable summaries).

### 7) Bootstrap strategy and repo workflow

Goal: contributors can build the compiler without fragile steps.

Phases:
1. Stage0 builds self-host (debug and release).
2. Self-host builds self-host (sanity).
3. CI verifies:
   - stage0 -> self-host build
   - self-host -> self-host build
   - shared corpora pass under both

Milestones:
1. Add a single "bootstrap" command/script that produces `build/stasis_release.exe`.
2. Add CI jobs that run the conformance harness on PRs.

## Prioritized milestone list (recommended order)

1. Conformance harness + corpus expansion (so every gap is visible and measurable).
2. Frontend parity for `samples/` (compile all non-interactive samples).
3. Semantic parity tests for layout + builtins.
4. LLVM execution/linking parity (already mostly aligned; finish by adding `sys_spawn`).
5. Cranelift parity for a defined subset, expand to full.
6. CLI parity and documentation polish.
7. Make self-host the default in `README.md` and scripts; keep stage0 as bootstrap.

## Risks and mitigations

- Fixed-capacity tables overflow as feature coverage grows.
  - Mitigation: add explicit caps, diagnostics, and measured bump policy (with documented memory cost).
- Shell-based process execution (`system()`) causes perf spikes and quoting bugs.
  - Mitigation: add `sys_spawn` and switch all tool invocations to it.
- "IR text heuristics" drift (false positives/negatives on sys/runtime needs).
  - Mitigation: emit an explicit "link requirements" section in IR (comment markers) or track requirements during lowering.
- Divergent semantics between compilers.
  - Mitigation: corpus-first approach; block merges on conformance failures.

## Metrics to track

- Pass rate parity: `% files passing` in `tests/` and corpus (both compilers).
- Feature parity: count of "unsupported" diagnostics when compiling `samples/`.
- Performance: median and p95 compile+run time per file for `stasis test` (stage0 vs self-host).
- Bootstrap stability: `stage0 -> self-host` and `self-host -> self-host` success rate in CI.

