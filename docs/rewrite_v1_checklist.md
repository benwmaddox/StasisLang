# Rewrite V1 Build Checklist

This checklist is the implementation plan for Rewrite V1 and is aligned with:
- `docs/spec.md`
- `docs/live-compilation-prd.md`

Locked decisions:
- Entrypoint is `function main(): i32`.
- Initial host externs are `print_i32` and `print_string`.
- Function-form calls remain supported indefinitely (receiver-form still preferred).
- Planned compiler orchestration file path is `compiler/incremental_compiler.stasis`.
- Backend modes are:
- Cranelift JIT for development/watch/hot-swap runtime
- Cranelift AOT for production builds

## Language Ownership Legend

- `Rust`: Host app/runtime boundary, platform integration, Cranelift integration, process/watch plumbing.
- `.stasis`: Compiler pipeline orchestration and language-level compile logic (lexer/parser/semantics/incremental policy source of truth).
- `Rust + .stasis`: Rust provides execution/binding substrate; `.stasis` defines compiler behavior/policies.

Boundary rule:
- Rust must not become a second source of truth for lexer/parser semantics; frontend behavior lands in `compiler/incremental_compiler.stasis`.

## Execution Rules

1. Build in feature slices only. Each slice must be shippable.
2. Every slice requires tests in the same PR.
3. Remove dead code and temporary paths before slice completion.
4. Update docs in the same PR when behavior changes.
5. Preserve deterministic tick-based behavior.

## Tooling Note

Current canonical workspace commands:
- `cargo build`
- `cargo test`

Legacy bootstrap compiler tooling remains available for smoke/reference:
- `bootstrap\\windows\\stasisc.bat run path\\to\\file.stasis --emit-ir`
- `bootstrap\\windows\\stasisc.bat test --all`
- `cargo test -p stasis_compiler bootstrap_compiles_incremental_compiler_source -- --nocapture` (Windows bootstrap smoke path)

## Slice Plan

### S0 - Workspace Bootstrap
- Language:
- `Rust`
- Scope:
- Create real crate/app sources for `apps/stasis`, `crates/stasis_compiler`, `crates/stasis_jit`, `crates/stasis_runner`.
- Create/verify required `Cargo.toml` and `src/` roots for each workspace member referenced by root `Cargo.toml`.
- Deliverable:
- `cargo build` and `cargo test` pass with scaffold smoke tests.
- Tests:
- Workspace compile smoke.
- Done gate:
- Clean build/test on branch with no placeholder dead modules.
- Status: `completed`

### S1 - Minimal Front-End Parse
- Language:
- `Rust + .stasis`
- Rust: host invocation/test harness only for bootstrap and integration boundaries.
- `.stasis`: lexer, parser, diagnostics emission, and incremental parse orchestration in `compiler/incremental_compiler.stasis`.
- Scope:
- Implement lexer/parser for minimum executable subset:
- `function`, `return`, integer/string literals, call expression, extern declaration.
- Deliverable:
- Parser accepts minimal program containing `main`.
- Tests:
- Bootstrap-backed parser fixtures for positive/negative cases (`tests/stasis/parser_valid_main.stasis`, `tests/stasis/parser_invalid_missing_semicolon.stasis`).
- Done gate:
- Parses minimal valid program and emits actionable diagnostics on failures.
- Status: `completed`

### S2 - Minimal Execution (`main(): i32`)
- Language:
- `Rust + .stasis`
- Rust: lowering bridge to Cranelift and execution harness.
- `.stasis`: compile pipeline decisions selecting/validating `main`.
- Scope:
- Wire parser output into minimal lowering and JIT execution (dev mode) for:
- `function main(): i32 { return <int>; }`
- Deliverable:
- Runner executes `main` and returns process status code.
- Tests:
- End-to-end test asserts returned status code (`tests/stasis/run_main_returns_7.stasis` via `bootstrap/windows/stasis-cranelift-run.bat`).
- Done gate:
- Exit status path is stable and deterministic.
- Status: `in_progress`

### S3 - Console Externs
- Language:
- `Rust + .stasis`
- Rust: host extern ABI implementation (`print_i32`, `print_string`).
- `.stasis`: extern symbol declarations and compile-time binding checks.
- Scope:
- Add stable host extern ABI for:
- `print_i32(value: i32)` and `print_string(value: string)`.
- Ensure console path supports `string`, `ascii[]`, and `utf8[]` call sites for `print_string`.
- Deliverable:
- Stasis program can print deterministic output through host boundary.
- Tests:
- End-to-end golden stdout tests.
- Done gate:
- Output is deterministic and ABI contract is documented.
- Status: `in_progress`

### S4 - Core Statements and Expressions
- Language:
- `Rust + .stasis`
- Rust: expression lowering/eval codegen primitives.
- `.stasis`: semantic rules and compile pipeline ordering.
- Scope:
- Add `let`, assignment, infix arithmetic/comparison, block scopes, and `if`/`else if`/`else`.
- Deliverable:
- Small real programs beyond single return execute correctly.
- Tests:
- Semantic and codegen unit tests plus end-to-end fixtures.
- Added parser coverage fixtures:
- `tests/stasis/parser_s4_valid_control_flow.stasis`
- `tests/stasis/parser_s4_invalid_let_missing_init_or_type.stasis`
- Added runtime smoke fixtures that execute `compiler/incremental_compiler.stasis` parse counts and failure paths:
- `tests/stasis/run_parser_s4_counts.stasis`
- `tests/stasis/run_parser_invalid_let_missing_init_or_type.stasis`
- Done gate:
- Behavior matches `docs/spec.md` operator and assignment rules.
- Status: `in_progress`

### S5 - Call Model and Conversion Semantics
- Language:
- `Rust + .stasis`
- Rust: overload resolution engine and IR lowering support.
- `.stasis`: receiver-preference policy, conversion semantics checks, and diagnostics policy.
- Scope:
- Implement receiver-scoped resolution key `(function_name, parameter0_type)`.
- Keep function-form calls supported indefinitely.
- Implement conversion semantics:
- Mutating `from_*` operations.
- Pure `to_*` operations.
- Explicit enum conversion surface `enum_to_i32(value: EnumType): i32` (no implicit enum/int conversion).
- Bootstrap compatibility path: compiler wrapper builtin rewrite with same call shape; self-hosted compiler path: intrinsic implementation.
- Deliverable:
- `enemy.damage(5)` and `damage(enemy, 5)` both resolve correctly.
- Conversion semantics follow spec examples.
- Tests:
- Overload resolution tests, conversion tests, negative diagnostics.
- Current parser/execution coverage fixture:
- `tests/stasis/run_parser_s5_receiver_and_function_calls.stasis` (receiver-form and function-form call parsing baseline).
- Done gate:
- Receiver-form preferred but both call forms behave consistently and deterministically.
- Status: `in_progress`

### S6 - Global Memory and Layout
- Language:
- `Rust + .stasis`
- Rust: layout computation primitives and stable hashing implementation.
- `.stasis`: layout-policy checks and rejection rules.
- Scope:
- Implement global declarations and deterministic layout metadata/hashing.
- Deliverable:
- Stable layout hash for unchanged declarations.
- Tests:
- Layout determinism tests across repeated compiles.
- Current runtime coverage fixtures:
- `tests/stasis/run_layout_hash_deterministic.stasis`
- `tests/stasis/run_layout_hash_changes_on_layout_update.stasis`
- `tests/stasis/run_layout_hash_file_db_change_detection.stasis`
- Done gate:
- Layout-affecting changes are detected reliably.
- Status: `in_progress`

### S7 - Incremental Compiler V1
- Language:
- `Rust + .stasis`
- Rust: in-memory file DB, cache storage, and invalidation substrate.
- `.stasis`: whole-file semantic pass orchestration and per-function codegen gating policy.
- Scope:
- Add in-memory file database, file-level invalidation, whole-file semantic check.
- Gate codegen per function using semantic hashes.
- Deliverable:
- Unchanged function bodies skip backend regeneration.
- Tests:
- Incremental cache hit/miss tests and file-level invalidation tests.
- Current runtime coverage fixture:
- `tests/stasis/run_incremental_file_db_counts.stasis` (exercises `compiler_upsert_file` parse + reuse counters).
- `tests/stasis/run_incremental_function_hash_metrics.stasis` (exercises per-function reused/changed/codegen hash gating counters).
- Done gate:
- Semantic pass always runs per changed file; backend work is correctly gated.
- Status: `in_progress`

### S8 - Function Pointer Table ABI
- Language:
- `Rust`
- Scope:
- Implement stable `FnId -> code_ptr` indirection and generation-based code regions.
- Current implementation:
- `crates/stasis_jit::FunctionPointerTable` now owns `FnId -> CodePtr` mapping, generation increments, and safe-window retirement bookkeeping.
- `apps/stasis` swap commit path now sources commit generation IDs from `FunctionPointerTable`.
- Deliverable:
- Runtime dispatch goes through pointer table only.
- Tests:
- ABI and indirect-call tests.
- Done gate:
- No direct raw-address calls from runtime callsites.
- Status: `in_progress`

### S8b - Cranelift AOT Production Path
- Language:
- `Rust`
- Scope:
- Add production AOT compilation path and artifact wiring using Cranelift AOT outputs.
- Current implementation:
- `DevHotSwapPipeline` now supports explicit `TargetMode` (`JitDev` or `AotProd`) dispatch.
- `apps/stasis` runner config/CLI can request `AotProd` compile requests (`--target-mode aot` / `--aot-prod`).
- Deliverable:
- Production mode runs from AOT artifacts without requiring runtime JIT.
- Tests:
- AOT compile-and-run smoke tests for representative fixtures.
- Done gate:
- Production pipeline uses AOT artifacts with deterministic behavior.
- Status: `in_progress`

### S9 - Two-Phase Swap Commit
- Language:
- `Rust + .stasis`
- Rust: commit transaction mechanism and thread-safe pointer swap.
- `.stasis`: swap eligibility policy inputs and diagnostics policy.
- Scope:
- Implement background compile patch generation and between-ticks commit.
- Implement typed boundary contracts for dev flow messages:
- `FileChangeEvent`, `CompileRequest`, `CompileResult`, `SwapCommitRequest`, `SwapCommitResult`.
- Current implementation:
- `DevHotSwapPipeline` now rejects compile/commit payloads with mismatched `contract_version` and surfaces explicit failure diagnostics/errors instead of partially proceeding.
- Deliverable:
- Atomic swap behavior: all-or-nothing commit.
- Tests:
- Swap success, swap rejection, and no-partial-commit tests.
- Boundary contract tests for message ordering and failure propagation.
- Done gate:
- On failure, old code/data remain active.
- Runtime/compiler ownership boundaries enforced in code paths.
- Status: `in_progress`

### S10 - `on_code_swap` Hook
- Language:
- `Rust + .stasis`
- Rust: hook invocation boundary and rollback/error propagation.
- `.stasis`: hook definition and rule enforcement semantics.
- Scope:
- Run optional `function on_code_swap(): void` before pointer swap.
- Deliverable:
- Explicit state adjustment point between ticks.
- Tests:
- Hook success/failure transactional tests.
- Done gate:
- Hook errors abort swap with clear diagnostics.
- Status: `in_progress`

### S11 - Swap Indicator (Tick-Based)
- Language:
- `.stasis` (feature logic) + `Rust` (draw host bridge only)
- Scope:
- Integrate `DebugUI.swapFlashTicks` behavior in Stasis game code.
- Deliverable:
- Successful swaps trigger visible deterministic indicator.
- Tests:
- Tick countdown behavior tests and no-indicator-on-failure tests.
- Done gate:
- Indicator follows tick policy and does not fire on failed swap.
- Status: `in_progress`

### S12 - Brickout Revenge End-to-End
- Language:
- `.stasis` gameplay/compiler script + `Rust` host runner/JIT integration
- Scope:
- Run `samples/brickout_revenge/brickout_revenge_v1.stasis` through incremental compiler and hot-swap loop.
- Validate intended vertical window proportion.
- Deliverable:
- Real sample runs in watch/compile/swap workflow.
- Tests:
- End-to-end scenario test with window config assertion.
- Done gate:
- Brickout runs with correct proportion and swap loop remains stable.
- Status: `in_progress`

## PR Sequence

1. PR-A: S0-S2
2. PR-B: S3-S5
3. PR-C: S6-S8
4. PR-D: S8b-S10
5. PR-E: S11-S12

Each PR must include:
- tests for that slice set
- docs updates for changed behavior
- removal of obsolete paths introduced during the slice

## Backlog

- Add `schema_version` field to every JSONL runner event payload for strict editor/tool compatibility negotiation.
- Add dedicated `Stasis.Compiler.Tests` coverage for chained array-field load/store paths (`a[i].field[j]`) to keep the bootstrap backend fix pinned.
