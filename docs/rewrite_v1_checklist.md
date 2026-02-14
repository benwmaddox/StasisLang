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
3. When compiler behavior can be tested in `.stasis`, prefer `.stasis` tests over Rust-host reimplementation tests.
4. Remove dead code and temporary paths before slice completion.
5. Update docs in the same PR when behavior changes.
6. Preserve deterministic tick-based behavior.

## Tooling Note

Current canonical workspace commands:
- `cargo build`
- `cargo test`

Bootstrap compiler tooling is seed-only for initial bring-up.
It is not part of the steady-state incremental JIT update loop.

## Slice Plan

### Current Snapshot (2026-02-13)
- Completed slices (baseline): `S0`, `S1`, `S2`, `S3`, `S4`, `S5`, `S6`, `S7`, `S8`, `S9`, `S11`.
- Partially complete/in progress: `S8b`, `S10`.
- New self-host track started: `S10b` (minimal `.stasis` AOT CLI core orchestration).
- Priority override (2026-02-13):
- Single top priority is `S10b` (`SH1 -> SH2 -> SH3`) until `.stasis` self-host AOT CLI core is running end-to-end.
- All other in-progress tracks (`S8b`, `S10`, and remaining `R*`/`H*` slices) are lower priority and should only be touched if they directly unblock `S10b`.
- Main integration gap: real backend compile path is now default, but emitted function patches are metadata-only (`FnId` mapping from hashes) and are not yet executing newly generated machine code through the pointer table.
- Ownership enforcement: compiler frontend semantics are routed through `.stasis` (`compiler/incremental_compiler.stasis`), and Rust is constrained to host/runtime glue.
- Rust semantic analyzer paths were removed from `crates/stasis_compiler`; ownership guard test now enforces zero reintroduction (`tests/compiler_logic_ownership_guard.rs`).

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
- Rust: host invocation/test harness and in-process compiler host bindings.
- `.stasis`: lexer, parser, diagnostics emission, and incremental parse orchestration in `compiler/incremental_compiler.stasis`.
- Scope:
- Implement lexer/parser for minimum executable subset:
- `function`, `return`, integer/string literals, call expression, extern declaration.
- Deliverable:
- Parser accepts minimal program containing `main`.
- Tests:
- Parser fixtures for positive/negative cases (`tests/stasis/parser_valid_main.stasis`, `tests/stasis/parser_invalid_missing_semicolon.stasis`).
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
- End-to-end test asserts returned status code (`tests/stasis/run_main_returns_7.stasis`).
- Done gate:
- Exit status path is stable and deterministic.
- Status: `completed`

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
- Status: `completed`

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
- Status: `completed`

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
- Seed-compiler compatibility path exists only for bring-up; steady-state path is self-hosted intrinsic implementation.
- Deliverable:
- `enemy.damage(5)` and `damage(enemy, 5)` both resolve correctly.
- Conversion semantics follow spec examples.
- Tests:
- Overload resolution tests, conversion tests, negative diagnostics.
- Current parser/execution coverage fixture:
- `tests/stasis/run_parser_s5_receiver_and_function_calls.stasis` (receiver-form and function-form call parsing baseline).
- Added semantic-level regression coverage in `crates/stasis_compiler`:
- receiver-scoped signature distinction test for overloads by parameter0 type
- conversion misuse diagnostic test for invalid `from_*` expression usage (`4001`)
- Done gate:
- Receiver-form preferred but both call forms behave consistently and deterministically.
- Status: `completed`

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
- Status: `completed`

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
- Status: `completed`

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
- Status: `completed`

### S8b - Cranelift AOT Production Path
- Language:
- `Rust`
- Scope:
- Add production AOT compilation path and artifact wiring using Cranelift AOT outputs.
- Current implementation:
- `DevHotSwapPipeline` now supports explicit `TargetMode` (`JitDev` or `AotProd`) dispatch.
- `apps/stasis` runner config/CLI can request `AotProd` compile requests (`--target-mode aot` / `--aot-prod`).
- `crates/stasis_jit::compile_clif_to_object` invokes `tools/cranelift-aot` to produce native object artifacts.
- `apps/stasis::IncrementalCompilerBackend` now emits per-function AOT object artifacts for changed functions when `TargetMode::AotProd` compile requests are processed in the real backend path.
- Real backend now writes `last_patch_manifest.json` alongside emitted AOT object artifacts to persist request/artifact mapping for runtime handoff.
- `crates/stasis_jit` now provides optional object-bundle link support (`link_objects_to_dynamic_library`) and the real backend can emit linked bundle artifacts when `STASIS_AOT_LINK_ARTIFACTS=1` and linker tooling is available.
- `SwapCommitRequest` now carries explicit `target_mode` so runtime safe-point commit can apply mode-specific gating deterministically.
- Compile/commit contracts now carry optional `aot_linked_image_path`, and runtime commit gate rejects `AotProd` commits when linked-image metadata is missing or the declared linked image is missing at commit time.
- Runner events now include explicit `aot_linked_image_validation` success/failure records for commit-time artifact handoff diagnostics.
- Compile/commit contracts now also carry optional `aot_linked_image_size_bytes`; runtime commit gate validates expected linked-image size to catch artifact drift between compile and commit.
- Compile/commit contracts now also carry optional `aot_linked_image_sha256`; runtime commit gate validates linked-image content hash to catch artifact substitution/drift between compile and commit.
- Compile/commit contracts now also carry optional `aot_function_symbols`; runtime commit gate requires symbol mapping coverage for all patched `FnId`s in `AotProd`.
- Runtime now supports optional exported-symbol resolution for `AotProd` pointer-table overrides (`STASIS_AOT_RESOLVE_EXPORTS=1`), resolving code pointers from linked-image export tables when available.
- Runtime now supports optional in-process dynamic loader resolution for `AotProd` pointer-table overrides (`STASIS_AOT_USE_LOADER=1`), loading linked artifacts and resolving exported symbol addresses via OS loader APIs.
- AOT link step now forwards emitted function symbol exports to the linker (Windows `/EXPORT:` flags) so linked bundles can expose compiled symbol entrypoints when toolchain/linker supports export emission.
- In `AotProd`, runtime commit gate now requires complete linked-image metadata (`path + size + sha256`) and rejects incomplete handoff payloads.
- Linked-image SHA-256 computation/validation now uses buffered streaming I/O (chunked reads) in compile + commit paths to avoid full-file memory spikes on large artifacts.
- Runtime now caches successful linked-image validation tuples (`path + size + sha256 + probe mode`) to avoid redundant hash/format/probe work across unchanged commits.
- Runtime now applies `AotProd` pointer-table updates with explicit code-pointer overrides derived from linked-image symbol handoff metadata (`FnId -> symbol`), instead of generation-only placeholder pointer synthesis.
- Default `AotProd` pointer override behavior remains deterministic metadata-derived when export resolution mode is disabled, preserving stable bring-up behavior while export-resolution path matures.
- Runtime commit gate now performs safe linked-image format validation (`MZ/PE` on Windows, `ELF` on Linux, `Mach-O` magic on macOS) before allowing `AotProd` swap.
- Runtime now records successful linked-artifact activation (`AotLinkedImageActivated`) and tracks the active linked-image path in runner summary state for downstream runtime ownership.
- Optional runtime loadability probe support is now available (`RunnerConfig.aot_probe_loadability` / CLI `--aot-probe-load`) to attempt OS-level load/free validation of linked artifacts at commit time.
- Runtime now tracks AOT linked-image lifecycle (active + retired artifacts) through `AotArtifactRegistry` and emits retirement events (`AotLinkedImageRetired`) when pointer-table generations exit the safe-retire window.
- Runtime now tracks loaded AOT module lifecycle by pointer-table generation and unloads retired generations by dropping generation-bound loader handles after safe-retire.
- `AotArtifactRegistry` now bounds retained retirement history (`DEFAULT_MAX_RETIRED_IMAGES`) to avoid unbounded memory growth during long watch-mode sessions.
- AOT artifact lifecycle is now generation-bound: activation/retirement metadata is recorded against the committed pointer-table generation and surfaced through runner events/summary state.
- Runtime relaunch path now passes `--no-runtime-launch` to spawned child processes to prevent recursive process trees during watch-mode swap iteration.
- Simple `i32` return-body extraction now supports deterministic `if`/`else if`/`else` branch evaluation with branch-local `let`/assignment handling and fallthrough continuation to later top-level `return`.
- AOT simple-body lowering now preserves conditional return chains as expression-level select trees (`SimpleI32Condition` + `SimpleI32ReturnExpr::Select`) instead of only compile-time branch selection.
- Incremental compiler function metrics now include declared return type metadata, and AOT stub emission uses return-type-aware signatures (`void` functions emit `return` without value; `i32` functions keep value-return lowering).
- Deterministic simple-body condition evaluation now supports logical composition in `if` conditions (`&&`, `||`, `!`, and parenthesized grouping) in addition to comparison operators.
- Symbolic simple-body condition extraction now preserves logical condition trees (`And`/`Or`/`Not`) for `if` return-chain lowering, enabling AOT condition emission beyond comparison-only predicates.
- Top-level conditional return emission now lowers through explicit branch blocks (`brif` + branch-return blocks) in AOT stub CLIF output instead of select-only expression lowering.
- Deliverable:
- Production mode runs from AOT artifacts without requiring runtime JIT.
- Tests:
- AOT compile-and-run smoke tests for representative fixtures.
- Added backend regression test coverage for AOT helper failure diagnostics (`apps/stasis::compiler_backend::tests::aot_compile_reports_missing_helper_binary`).
- Added backend regression test coverage for AOT artifact manifest generation with deterministic fake helper success path (`apps/stasis::compiler_backend::tests::aot_compile_writes_manifest_with_artifacts_on_success`).
- Added backend + JIT regression coverage for optional AOT link step and linked-image manifest field (`apps/stasis::compiler_backend::tests::aot_compile_can_link_bundle_and_record_linked_image_in_manifest`, `crates/stasis_jit::tests::aot_linker_can_be_driven_by_configured_fake_linker`).
- Added runner regression coverage for AOT commit-time linked-image validation (`apps/stasis::tests::aot_commit_rejects_missing_linked_image_path`).
- Added runner regression coverage for `AotProd` metadata-presence gating (`apps/stasis::tests::aot_commit_rejects_missing_linked_image_metadata_path`).
- Added runner regression coverage for AOT linked-image size mismatch rejection (`apps/stasis::tests::aot_commit_rejects_linked_image_size_mismatch`).
- Added runner regression coverage for linked-image hash mismatch rejection (`apps/stasis::tests::aot_commit_rejects_linked_image_hash_mismatch`).
- Added runner regression coverage for missing linked-image size/hash metadata rejection (`apps/stasis::tests::aot_commit_rejects_missing_linked_image_size_metadata`, `apps/stasis::tests::aot_commit_rejects_missing_linked_image_hash_metadata`).
- Added deterministic SHA-256 utility coverage in both backend and runtime paths (`apps/stasis::compiler_backend::tests::compute_file_sha256_hex_matches_known_value`, `apps/stasis::tests::compute_file_sha256_hex_matches_known_value`).
- Added runtime validation-cache tuple semantics coverage (`apps/stasis::tests::aot_validation_cache_hits_only_on_exact_metadata_tuple`).
- Added contract/pipeline coverage for AOT function-symbol propagation (`crates/stasis_runner::swap::contracts`, `crates/stasis_runner::swap::pipeline::compile_hook_symbol_propagates_to_commit_request`).
- Added pointer-table override commit coverage (`crates/stasis_jit::tests::commit_patch_set_with_code_ptrs_applies_override_pointers`).
- Added runtime export-resolution coverage on Windows (`apps/stasis::tests::resolve_aot_symbol_export_code_ptr_finds_kernel32_export`, `apps/stasis::tests::resolve_aot_symbol_export_code_ptr_rejects_missing_export`).
- Added loader-resolution coverage on Windows (`apps/stasis::tests::build_aot_code_ptr_overrides_loader_mode_resolves_export_address`, `apps/stasis::tests::build_aot_code_ptr_overrides_loader_mode_rejects_missing_export`, `crates/stasis_dynload::tests::can_load_kernel32_and_resolve_export`).
- Added loader native-entry invocation coverage on Windows (`crates/stasis_dynload::tests::can_invoke_get_tick_count_export`).
- Added runtime commit-loop coverage for AOT loader + native hook execution path (`apps/stasis::tests::runner_aot_loader_native_hook_executes_and_reports_return_value`).
- Added runner regression coverage for invalid linked-image format rejection (`apps/stasis::tests::aot_commit_rejects_invalid_linked_image_format`) and dedicated validator unit coverage (`apps/stasis::aot_validation::tests::rejects_non_binary_payload`).
- Added positive runner regression coverage for `AotProd` commit acceptance when linked-image metadata is present and valid (`apps/stasis::tests::aot_commit_accepts_valid_pe_linked_image_metadata` on Windows).
- Added optional loadability-probe coverage on Windows (`apps/stasis::aot_validation::tests::loadability_probe_accepts_system_library`, `apps/stasis::tests::aot_commit_accepts_system_library_when_probe_enabled`).
- Added lifecycle coverage for AOT artifact retirement when generations retire (`apps/stasis::tests::aot_activation_retires_previous_image_after_generation_safe_window`, `apps/stasis::aot_artifacts::tests::*`).
- Added bounded-retirement-history coverage (`apps/stasis::aot_artifacts::tests::retired_history_is_bounded`).
- Added lifecycle coverage for generation-bound activation/retirement (`apps/stasis::tests::aot_commit_accepts_valid_pe_linked_image_metadata`, `apps/stasis::aot_artifacts::tests::activate_same_path_updates_generation_without_retiring`).
- Done gate:
- Production pipeline uses AOT artifacts with deterministic behavior.
- Status: `in_progress`
- Remaining:
- Priority update (2026-02-13):
- `S8b` hardening/parity work is explicitly lower priority than `S10b` self-host AOT CLI core.
- Do not schedule additional `R*` slices unless required to unblock `SH1`, `SH2`, or `SH3`.
- Slice R1: Add real branch/join block emission for runtime-dependent `if/else` in `AotProd` (no select-only fallback for supported bodies). (completed 2026-02-13)
- Slice R2: Add short-circuit boolean control-flow lowering (`&&`, `||`, `!`) in real emitted branch blocks. (completed 2026-02-13)
- Slice R3a: Lower direct no-arg `i32` return-call bodies (`return callee();`) in emitted AOT bodies when callee dispatch resolves uniquely from compiler metadata in the patch set. (completed 2026-02-13)
- Slice R3b1: Extend call-site lowering to direct `i32` return-call bodies with constant additive offsets (`return callee() +/- <int_literal>;`). (completed 2026-02-13)
- Slice R3b2a: Extend call-site lowering to simple no-arg two-call `i32` return bodies (`return lhs() +/- rhs();`) with resolver gating/diagnostics. (completed 2026-02-13)
- Slice R3b2b: Extend call-site lowering to argument-bearing `i32` call-return bodies. (completed 2026-02-13)
- Slice R3b2c1: Extend call-site lowering to nested one-arg call-return bodies (`return callee(arg_fn());`) with resolver gating/diagnostics. (completed 2026-02-13)
- Slice R3b2c2a: Extend call-site lowering to one-arg call-return additive-offset bodies (`return callee(<int_literal>) +/- <int_literal>;`, `return callee(arg_fn()) +/- <int_literal>;`). (completed 2026-02-13)
- Slice R3b2c2b: Extend call-site lowering to broader nested/mixed call-heavy `i32` return expressions and resolver sources.
- Slice R4a: Lower simple void host extern print calls (`print_i32(<int_literal>)`) in emitted AOT bodies and add deterministic side-effect verification fixtures. (completed 2026-02-13)
- Slice R4b1: Extend host-side-effecting extern-call lowering to simple call-argument print bodies (`print_i32(callee())`) with resolver gating and diagnostics. (completed 2026-02-13)
- Slice R4b2a: Extend host-side-effecting extern-call lowering to additive-offset call-argument print bodies (`print_i32(callee() +/- <int_literal>)`). (completed 2026-02-13)
- Slice R4b2b1: Extend host-side-effecting extern-call lowering to folded literal-expression print bodies (`print_i32(<int_literal> +/- <int_literal>)`). (completed 2026-02-13)
- Slice R4b2b2a: Extend host-side-effecting extern-call lowering to one-arg call print bodies (`print_i32(callee(<int_literal>))`). (completed 2026-02-13)
- Slice R4b2b2b1: Extend host-side-effecting extern-call lowering to nested one-arg call print bodies (`print_i32(callee(arg_fn()))`). (completed 2026-02-13)
- Slice R4b2b2b2a: Extend host-side-effecting extern-call lowering to one-arg literal additive-offset print bodies (`print_i32(callee(<int_literal>) +/- <int_literal>)`). (completed 2026-02-13)
- Slice R4b2b2b2a2: Extend host-side-effecting extern-call lowering to nested one-arg additive-offset print bodies (`print_i32(callee(arg_fn()) +/- <int_literal>)`). (completed 2026-02-13)
- Slice R4b2b2b2a3: Extend host-side-effecting extern-call lowering to two-call print bodies (`print_i32(lhs() +/- rhs())`) via `.stasis` shape detection routed through shared two-call metadata channels. (completed 2026-02-13)
- Slice R4b2b2b2b: Extend host-side-effecting extern-call lowering/parity to broader `print_i32` body shapes beyond literal/direct-call/additive-offset/one-arg-call/nested-one-arg-call/one-arg-additive forms.
- Slice R5: Lower runtime-dependent local mutation/update flows in emitted bodies (non-constant locals and assignment chains).
- Slice R6a: Add compatibility-gate coverage for unresolved direct-call dispatch in real lowered AOT bodies. (completed 2026-02-13)
- Slice R6a2: Add compatibility-gate coverage for unresolved two-call dispatch in real lowered AOT bodies. (completed 2026-02-13)
- Slice R6a3: Add compatibility-gate coverage for unresolved one-arg direct-call dispatch in real lowered AOT bodies. (completed 2026-02-13)
- Slice R6a4: Add compatibility-gate coverage for unresolved one-arg direct-call argument-target dispatch in real lowered AOT bodies. (completed 2026-02-13)
- Slice R6b1: Add compatibility-gate coverage for unresolved void print-call dispatch in real lowered AOT bodies. (completed 2026-02-13)
- Slice R6b2a: Add compatibility-gate coverage for `AotProd` commit-time missing function-symbol mapping metadata (patched `FnId` dispatch compatibility). (completed 2026-02-13)
- Slice R6b2b: Add compatibility-gate coverage for `AotProd` commit-time missing symbol entries for patched `FnId`s. (completed 2026-02-13)
- Slice R6b2c1: Add compatibility-gate coverage for `AotProd` loader-mode mapped-symbol export resolution failures at commit time. (completed 2026-02-13)
- Slice R6b2c2a: Add compatibility-gate coverage for `AotProd` commit-time duplicate symbol-mapping entries for the same patched `FnId`. (completed 2026-02-13)
- Slice R6b2c2b: Add compatibility-gate coverage for additional real lowered body signature/layout mismatch classes at commit time. (deferred post dev/jit + self-host)
- Slice R7a: Add rollback-path coverage for unresolved direct-call compile failure (compile failure must skip commit and preserve previous generation state). (completed 2026-02-13)
- Slice R7a2: Add rollback-path coverage for unresolved two-call compile failure (compile failure must skip commit and preserve previous generation state). (completed 2026-02-13)
- Slice R7a3: Add rollback-path coverage for unresolved one-arg direct-call compile failure (compile failure must skip commit and preserve previous generation state). (completed 2026-02-13)
- Slice R7a4: Add rollback-path coverage for unresolved one-arg direct-call argument-target compile failure (compile failure must skip commit and preserve previous generation state). (completed 2026-02-13)
- Slice R7b1: Add rollback-path coverage for unresolved void print-call compile failure (compile failure must skip commit and preserve previous generation state). (completed 2026-02-13)
- Slice R7b2a: Add rollback-path coverage for `AotProd` commit failure on missing function-symbol mapping metadata (no partial commit, previous generation preserved). (completed 2026-02-13)
- Slice R7b2b1: Add rollback-path coverage for `AotProd` commit failure on missing symbol entries for patched `FnId`s (no partial commit, previous generation preserved). (completed 2026-02-13)
- Slice R7b2b2a: Add rollback-path coverage for second-commit `AotProd` loader-mode mapped-symbol export resolution failures preserving previous active artifact/generation. (completed 2026-02-13)
- Slice R7b2b2b1: Add rollback-path coverage for second-commit `AotProd` missing linked-image path metadata preserving previous active artifact/generation. (completed 2026-02-13)
- Slice R7b2b2b2a: Add rollback-path coverage for second-commit `AotProd` duplicate-symbol-mapping commit failures preserving previous active artifact/generation. (completed 2026-02-13)
- Slice R7b2b2b2b1: Add rollback-path coverage for second-commit `AotProd` missing linked-image size metadata preserving previous active artifact/generation. (completed 2026-02-13)
- Slice R7b2b2b2b2a: Add rollback-path coverage for second-commit `AotProd` missing linked-image hash metadata preserving previous active artifact/generation. (completed 2026-02-13)
- Slice R7b2b2b2b2b: Add rollback-path coverage for additional real lowered body commit failure classes beyond unresolved direct-call/void-print-call compile rejection. (deferred post dev/jit + self-host)
- Slice R8: Add emitted-symbol runtime parity tests for runtime-dependent control-flow bodies in loader mode. (deferred post dev/jit + self-host)
- Slice R9a: Add emitted-symbol runtime parity coverage for direct no-arg `i32` call-return bodies in loader mode. (completed 2026-02-13)
- Slice R9b: Extend emitted-symbol runtime parity coverage to broader call-heavy bodies (arguments, nested calls, mixed control flow). (deferred post dev/jit + self-host)
- Slice R10: Add emitted-symbol runtime parity tests for local-mutation-heavy bodies in loader mode. (deferred post dev/jit + self-host)
- Slice R11: Add Brickout-oriented gameplay dispatch parity coverage using real emitted exported entrypoints in watch loop. (prioritize dev/jit path first; AOT/export parity deferred post dev/jit + self-host)
- Slice R12 (optional): Perform long-session watch-mode stability/perf pass and memory-growth checks after R1-R11.

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
- Status: `completed`

### S10 - `on_code_swap` Hook
- Language:
- `Rust + .stasis`
- Rust: hook invocation boundary and rollback/error propagation.
- `.stasis`: hook definition and rule enforcement semantics.
- Scope:
- Run optional `function on_code_swap(): void` before pointer swap.
- Current implementation:
- `CompileResult` now carries optional `hook_symbol`; pipeline forwards it into `SwapCommitRequest` (instead of hardcoded symbol) so hook execution is compiler-declared.
- `CompileResult`/`SwapCommitRequest` now also carry optional `hook_fn_id`, and real backend resolves/populates hook function identity for commit-time hook handling metadata.
- Incremental compiler host now reports `hook_symbol` from full tracked program state (not only changed-function emission), so `on_code_swap` remains commit-visible across non-hook edits in the same file.
- Real backend watch-mode regression now verifies hook runs across subsequent commits even when `on_code_swap` body is unchanged (`apps/stasis::tests::real_backend_runs_hook_on_subsequent_commits_when_hook_body_unchanged`).
- Contract regression coverage now verifies hook symbol + function-id propagation (`crates/stasis_runner::swap::contracts` and `crates/stasis_runner::swap::pipeline::compile_hook_symbol_propagates_to_commit_request`).
- Runner hook events now include optional `hook_fn_id` telemetry to keep commit-time hook outcomes tied to compiled function identity.
- Runtime commit gate now rejects hook execution when hook symbol metadata is present but `hook_fn_id` is absent, preventing ambiguous hook dispatch contracts.
- Runtime now resolves hook dispatch through pointer-table staged code-pointer preview (`FunctionPointerTable::preview_code_ptr_after_commit`) and rejects commit when hook dispatch cannot be resolved for `hook_fn_id`.
- Runner hook events now include optional `hook_code_ptr` telemetry so commit-time hook outcomes are tied to both function identity and staged dispatch target.
- Runtime now supports optional native hook-entry invocation for `AotProd` loader mode (`STASIS_AOT_EXECUTE_NATIVE_HOOK=1`), executing the staged hook address before swap and aborting commit on invocation failure.
- Native hook-entry invocation now uses a `void` ABI call shape aligned with `on_code_swap(): void` semantics to avoid mismatched return-signature invocation risk.
- Runner hook events now include optional `hook_return_value` telemetry for native-invocation mode.
- Deliverable:
- Explicit state adjustment point between ticks.
- Tests:
- Hook success/failure transactional tests.
- Added runner regression for hook metadata consistency rejection (`apps/stasis::tests::runner_rejects_hook_symbol_without_hook_fn_id_metadata`).
- Added runner regression for unresolved hook dispatch rejection (`apps/stasis::tests::runner_rejects_hook_when_pointer_table_has_no_dispatch_entry`).
- Added pointer-table unit coverage for staged hook dispatch preview (`crates/stasis_jit::tests::preview_code_ptr_after_commit_*`).
- Added native hook-invocation coverage on Windows (`apps/stasis::tests::maybe_execute_native_hook_invokes_loaded_export_for_aot_loader_mode`, `apps/stasis::tests::maybe_execute_native_hook_skips_when_native_execution_disabled`).
- Added runtime commit-loop native hook telemetry coverage in `AotProd` loader mode (`apps/stasis::tests::runner_aot_loader_native_hook_executes_and_reports_return_value`).
- Added transactional abort coverage for native hook invocation failure in `AotProd` loader mode (`apps/stasis::tests::runner_aot_loader_native_hook_failure_aborts_swap`).
- Added multi-commit transactional preservation coverage for `AotProd` loader mode (`apps/stasis::tests::runner_aot_loader_second_commit_failure_preserves_previous_active_artifact`) to verify a failed subsequent swap preserves the previously active linked artifact generation.
- Added real-backend emitted-artifact loader-mode commit coverage (`apps/stasis::tests::real_backend_emitted_aot_artifact_commits_via_loader_mode`) using deterministic emitted AOT artifact handoff plus runtime linked-image metadata validation/activation.
- Added compiler-backend emitted-symbol fidelity coverage for `AotProd` (`apps/stasis::compiler_backend::tests::aot_compile_emits_hook_fn_symbol_mapping_and_patch_coverage`) to verify symbol mapping covers all patched functions and includes `hook_fn_id`.
- Added linker export-argument propagation coverage (`crates/stasis_jit::tests::aot_linker_includes_configured_export_symbols`) and opportunistic real-toolchain export validation (`apps/stasis::compiler_backend::tests::aot_compile_with_real_linker_exports_emitted_symbols_when_available`).
- Added opportunistic runtime loader-resolution coverage for true emitted AOT symbols (`apps/stasis::tests::emitted_aot_symbols_resolve_via_loader_when_real_link_available`) to verify emitted-symbol `SwapCommitRequest` payloads resolve to non-zero loaded addresses when real toolchain output is available.
- AOT stub emission now uses parsed simple `i32` return-body semantics when available (local `let` bindings, local reassignment with `= += -= *= /= %=`, and arithmetic expression trees with precedence, parentheses, unary minus, and `+ - * / %`) with deterministic body-hash fallback for unsupported bodies, and opportunistic real-toolchain coverage verifies emitted symbol invocation reflects source-body changes across recompiles (`apps/stasis::compiler_backend::tests::aot_emitted_symbol_return_changes_when_body_changes_if_real_link_available`).
- Simple `i32` return-body extraction now also supports deterministic `if`/`else if`/`else` chains with branch-local `let`/assignment evaluation and fallthrough continuation to later top-level `return` (`crates/stasis_compiler::tests::simple_i32_return_expr_supports_else_if_and_fallthrough_return`, `crates/stasis_compiler::tests::simple_i32_return_expr_supports_branch_local_statements_before_return`).
- AOT stub CLIF emission now lowers conditional return trees through `icmp` + `select` when extracted simple-body conditions are available (`apps/stasis::compiler_backend::tests::aot_stub_uses_icmp_and_select_for_conditional_expression`), and compiler extraction coverage now includes nested return-chain select generation (`crates/stasis_compiler::tests::simple_i32_return_expr_builds_nested_select_for_else_if_return_chain`).
- Added opportunistic real-toolchain emitted-symbol execution coverage for conditional branch semantics in `AotProd` (`apps/stasis::compiler_backend::tests::aot_emitted_symbol_executes_if_else_select_semantics_if_real_link_available`), asserting expected true/false branch return values via loader invocation.
- Added opportunistic runtime loader-override invocation coverage for conditional branch semantics (`apps/stasis::tests::emitted_aot_loader_overrides_execute_if_else_semantics_when_real_link_available`), validating `build_aot_code_ptr_overrides` dispatch addresses execute expected true/false branch values across recompiles.
- Added AOT stub signature regression coverage for `void` return functions (`apps/stasis::compiler_backend::tests::aot_stub_uses_void_signature_for_void_return_type`) and compiler metadata coverage for parsed return types (`crates/stasis_compiler::tests::parse_records_functions_and_hashes`).
- Added compiler semantic regression coverage for logical condition operators in simple `if` extraction (`crates/stasis_compiler::tests::simple_i32_return_expr_supports_logical_condition_operators`).
- Added AOT condition-lowering coverage for logical predicates in select conditions (`apps/stasis::compiler_backend::tests::aot_stub_uses_logical_condition_ops_for_select_conditions`) validating CLIF logical condition op emission (`bor`, `bnot`) before `select`.
- Added AOT branch-block lowering coverage for top-level conditional returns (`apps/stasis::compiler_backend::tests::aot_stub_uses_branch_blocks_for_top_level_conditional_expression`) validating `brif` branch emission and removal of top-level `select` fallback for supported conditional bodies.
- Added explicit short-circuit branch-block coverage for logical `&&` conditions in top-level conditional lowering (`apps/stasis::compiler_backend::tests::aot_stub_uses_short_circuit_branching_for_and_conditions`), validating multi-block `brif` lowering without `band` fallback.
- Incremental compiler metadata now records direct no-arg `i32` return-call target hashes for simple function bodies (`return callee();`) and surfaces this through host compile metrics (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler`).
- AOT stub emission now lowers direct no-arg `i32` return-call bodies to real CLIF calls when callee dispatch resolves uniquely from the patch-set metadata (`apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_when_simple_return_call_target_is_resolved`, `apps/stasis::compiler_backend::tests::resolve_simple_i32_return_call_target_symbol_returns_unique_match`).
- Added opportunistic emitted-symbol runtime parity coverage for direct no-arg call-return lowering (`apps/stasis::compiler_backend::tests::aot_emitted_symbol_executes_direct_call_semantics_if_real_link_available`) to verify `main` dispatch matches `callee` export invocation in linked `AotProd` output.
- Added AOT compatibility-gate rejection coverage for unresolved direct-call dispatch (`apps/stasis::compiler_backend::tests::aot_compile_rejects_unresolved_direct_call_target`) so unresolved direct-call bodies fail compile instead of silently falling back.
- Added runner rollback-path coverage for unresolved direct-call compile rejection in real backend `AotProd` mode (`apps/stasis::tests::real_backend_aot_compile_failure_skips_commit_for_unresolved_direct_call_target`), asserting compile failure increments while swap commit remains skipped.
- Incremental compiler metadata now records direct `i32` return-call additive-offset bodies (`return callee() +/- <int_literal>;`) and surfaces signed offset deltas through host compile metrics (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_add_delta`).
- AOT stub emission now lowers direct `i32` return-call additive-offset bodies to emitted call + integer add/sub sequence (`apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_add_delta_when_metadata_is_resolved`).
- Incremental compiler metadata now records simple two-call `i32` return bodies (`return lhs() +/- rhs();`) via left/right callee id-hash and operation code capture (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_two_call_metadata`).
- AOT stub emission now lowers simple two-call `i32` return bodies to emitted left/right call + `iadd/isub` sequence, with compatibility-gate rejection when either callee dispatch cannot be resolved (`apps/stasis::compiler_backend::tests::aot_stub_uses_two_call_add_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::aot_stub_uses_two_call_sub_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::aot_compile_rejects_unresolved_two_call_target`).
- Added runner rollback-path coverage for unresolved two-call compile rejection in real backend `AotProd` mode (`apps/stasis::tests::real_backend_aot_compile_failure_skips_commit_for_unresolved_two_call_target`), asserting compile failure increments while swap commit remains skipped.
- Incremental compiler metadata now records simple one-arg `i32` return-call bodies (`return callee(<int_literal>);`) and surfaces callee id-hash + literal payload through host compile metrics (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_metadata`).
- AOT stub emission now lowers simple one-arg `i32` return-call bodies to emitted direct-call `external callee(i32) -> i32`, with compatibility-gate rejection when target dispatch cannot be resolved to a unique `i32 <- (i32)` signature (`apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_one_i32_arg_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::aot_compile_rejects_unresolved_one_arg_direct_call_target`).
- Added runner rollback-path coverage for unresolved one-arg direct-call compile rejection in real backend `AotProd` mode (`apps/stasis::tests::real_backend_aot_compile_failure_skips_commit_for_unresolved_one_arg_direct_call_target`), asserting compile failure increments while swap commit remains skipped.
- Incremental compiler metadata now records nested one-arg `i32` return-call bodies (`return callee(arg_fn());`) via outer callee id-hash + argument-call target id-hash capture (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_call_arg_metadata`).
- AOT stub emission now lowers nested one-arg `i32` return-call bodies to emitted `arg_fn()` call feeding `callee(i32)` call, with compatibility-gate rejection when the argument-call target dispatch is unresolved (`apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_one_call_arg_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::aot_compile_rejects_unresolved_one_arg_direct_call_arg_target`).
- Added runner rollback-path coverage for unresolved one-arg argument-target compile rejection in real backend `AotProd` mode (`apps/stasis::tests::real_backend_aot_compile_failure_skips_commit_for_unresolved_one_arg_direct_call_arg_target`), asserting compile failure increments while swap commit remains skipped.
- Incremental compiler metadata now records additive-offset variants for one-arg call-return bodies (`return callee(<int_literal>) +/- <int_literal>;`, `return callee(arg_fn()) +/- <int_literal>;`) through the shared direct-call add-delta metric (`crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_literal_add_delta`, `crates/stasis_compiler::tests::compile_records_simple_i32_return_call_one_arg_call_arg_add_delta`).
- AOT stub emission now lowers additive-offset one-arg call-return bodies to emitted callee-call + integer add/sub (and arg-call prelude where applicable) (`apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_one_i32_arg_and_add_delta_when_metadata_is_resolved`, `apps/stasis::compiler_backend::tests::aot_stub_uses_direct_call_with_one_call_arg_and_add_delta_when_metadata_is_resolved`).
- Incremental compiler metadata now records simple void `print_i32(<int_literal>)` bodies and surfaces literal payloads through host compile metrics (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler`).
- AOT stub emission now lowers simple void `print_i32(<int_literal>)` bodies to explicit extern calls with deterministic CLIF coverage (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_for_simple_void_print_metadata`).
- Incremental compiler metadata now records simple void `print_i32(callee())` bodies via callee id-hash capture and surfaces this through host compile metrics (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_call_target_hash`).
- AOT stub emission now lowers simple void `print_i32(callee())` bodies to emitted direct call + host extern call sequence, with compatibility-gate rejection when callee dispatch cannot be resolved (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_direct_call_target_for_simple_void_metadata`, `apps/stasis::compiler_backend::tests::aot_compile_rejects_unresolved_void_print_call_target`).
- Incremental compiler metadata now records additive-offset simple void print-call bodies (`print_i32(callee() +/- <int_literal>)`) via void-print call add-delta metrics (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_call_target_add_delta`).
- AOT stub emission now lowers additive-offset simple void print-call bodies to emitted direct call + integer add/sub + host print call (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_direct_call_target_and_add_delta_when_metadata_is_resolved`).
- Incremental compiler metadata now folds simple literal-expression void print bodies (`print_i32(<int_literal> +/- <int_literal>)`) into literal print payloads (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_literal_add_expression`).
- Incremental compiler metadata now records one-arg simple void print-call bodies (`print_i32(callee(<int_literal>))`) via callee id-hash + argument literal capture (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_one_arg_call_target_and_literal`).
- AOT stub emission now lowers one-arg simple void print-call bodies to emitted `callee(i32)` call + host print call (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_direct_one_i32_arg_call_target_for_simple_void_metadata`).
- Incremental compiler metadata now records nested one-arg simple void print-call bodies (`print_i32(callee(arg_fn()))`) via callee id-hash + argument-call target id-hash capture (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_one_arg_call_target_with_arg_call`).
- AOT stub emission now lowers nested one-arg simple void print-call bodies to emitted `arg_fn()` call feeding `callee(i32)` then host print call (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_direct_one_call_arg_call_target_for_simple_void_metadata`).
- Incremental compiler metadata now records one-arg literal additive-offset simple void print-call bodies (`print_i32(callee(<int_literal>) +/- <int_literal>)`) via shared void-print call add-delta metrics plus one-arg call metadata (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_one_arg_call_target_literal_add_delta`).
- AOT stub emission now lowers one-arg literal additive-offset simple void print-call bodies to emitted `callee(i32)` call + integer add/sub + host print call (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_direct_one_i32_arg_call_target_and_add_delta_for_simple_void_metadata`).
- Incremental compiler metadata now records nested one-arg additive-offset simple void print-call bodies (`print_i32(callee(arg_fn()) +/- <int_literal>)`) via one-arg argument-target metadata + shared void-print call add-delta metrics (`compiler/incremental_compiler.stasis`, `crates/stasis_compiler::tests::compile_records_simple_void_print_i32_one_arg_call_target_with_arg_call_add_delta`).
- AOT stub emission now lowers nested one-arg additive-offset simple void print-call bodies to emitted `arg_fn()` call feeding `callee(i32)` + integer add/sub + host print call (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_direct_one_call_arg_call_target_and_add_delta_for_simple_void_metadata`).
- Incremental compiler metadata now records simple void two-call print bodies (`print_i32(lhs() +/- rhs())`) in `.stasis` and routes them through existing shared two-call channels (`function_simple_i32_return_two_call_*`) to avoid new Rust-side compiler schema ownership (`compiler/incremental_compiler.stasis`).
- Added `.stasis` fixture coverage for void two-call print metadata routing (`tests/stasis/run_incremental_void_print_two_call_metrics.stasis`) and host-bridge regression coverage (`crates/stasis_compiler::tests::compile_records_simple_void_print_i32_two_call_metadata`).
- AOT stub emission now lowers void two-call print bodies to emitted left/right call + add/sub + host print call (`apps/stasis::compiler_backend::tests::aot_stub_uses_print_i32_call_with_two_call_targets_for_simple_void_metadata`).
- Added runner rollback-path coverage for unresolved void print-call compile rejection in real backend `AotProd` mode (`apps/stasis::tests::real_backend_aot_compile_failure_skips_commit_for_unresolved_void_print_call_target`), asserting compile failure increments while swap commit remains skipped.
- Added `AotProd` commit compatibility-gate coverage for missing function-symbol mapping metadata (`apps/stasis::tests::aot_commit_rejects_missing_function_symbol_mapping_metadata`).
- Added `AotProd` commit compatibility-gate coverage for missing symbol entries for patched `FnId`s (`apps/stasis::tests::aot_commit_rejects_missing_symbol_for_patched_function_id`).
- Added `AotProd` commit compatibility-gate coverage for loader-mode missing export resolution when symbol mapping exists (`apps/stasis::tests::aot_commit_rejects_loader_mode_when_symbol_export_is_missing`).
- Added `AotProd` commit compatibility-gate coverage for duplicate symbol-mapping entries targeting the same `FnId` (`apps/stasis::tests::aot_commit_rejects_duplicate_symbol_mapping_for_fn_id`).
- Added rollback-path coverage for second-commit `AotProd` missing-symbol-mapping failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_missing_symbol_mapping_preserves_previous_active_artifact`).
- Added rollback-path coverage for second-commit `AotProd` missing patched-`FnId` symbol failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_missing_symbol_for_patched_fn_preserves_previous_active_artifact`).
- Added rollback-path coverage for second-commit `AotProd` loader-mode missing export resolution failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_missing_loader_export_preserves_previous_active_artifact`).
- Added rollback-path coverage for second-commit `AotProd` missing linked-image path metadata failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_missing_linked_image_path_preserves_previous_active_artifact`).
- Added rollback-path coverage for second-commit `AotProd` duplicate-symbol-mapping commit failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_duplicate_symbol_mapping_preserves_previous_active_artifact`).
- Added rollback-path coverage for second-commit `AotProd` missing linked-image size metadata failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_missing_linked_image_size_metadata_preserves_previous_active_artifact`).
- Added rollback-path coverage for second-commit `AotProd` missing linked-image hash metadata failure with previous active artifact/generation preservation (`apps/stasis::tests::runner_second_aot_commit_missing_linked_image_hash_metadata_preserves_previous_active_artifact`).
- Migration gate M0/H0 completed: host compile analysis now runs through `.stasis` harness output only; Rust-side analyzer/simple-expression parsing removed.
- Added `.stasis` semantic validation for invalid `from_*` expression usage (`4001`) in `compiler/incremental_compiler.stasis`.
- Done gate:
- Hook errors abort swap with clear diagnostics.
- Status: `in_progress`
- Remaining:
- Slice H1: Execute `on_code_swap` from real lowered hook bodies in `AotProd` loader mode (remove simple-extraction dependency for supported hook bodies).
- Slice H2: Add hook parity fixture for deterministic state mutation (`on_code_swap`) and verify match vs current JIT/dev behavior.
- Slice H3: Add hook parity fixture for branch-dependent state mutation (`on_code_swap` with runtime condition paths).
- Slice H4: Add hook parity fixture for hook-side intra-program call effects where supported by real lowered hook codegen.
- Slice H5: Add hook parity fixture for hook-side extern/host-call effects where supported by real lowered hook codegen.
- Slice H6: Add rollback coverage for real lowered hook execution call-failure/unresolved-dispatch modes with previous generation preservation checks.
- Slice H7: Add rollback coverage for explicit hook failure-signal modes with previous generation preservation checks.
- Slice H8: Add compatibility-gate rejection coverage specific to real lowered hook dispatch (hook signature/layout/body incompatibility combinations).

### S10b - Self-Hosted AOT CLI Core
- Language:
- `.stasis` (orchestration) + minimal `Rust` host extern bridge
- Scope:
- Introduce a `.stasis` compiler CLI core entry that owns compile orchestration policy:
- enumerate source files from project dir via host extern bridge
- load each `.stasis` source via host extern bridge
- stage sources into `compiler/incremental_compiler.stasis` file DB
- run compile/entry validation and emit diagnostics
- delegate AOT artifact emission through a single host bridge call for now
- Current implementation:
- Added `compiler/stasis_aot_cli_core.stasis` with `compiler_cli_compile_project(...)` orchestration and host-extern bridge declarations (`host_source_file_count`, `host_load_source_file`, and staged AOT bridge calls `host_emit_ir_from_compiler_state`, `host_run_cranelift_aot`, `host_link_executable_from_objects`, `host_write_aot_cli_summary`, `host_set_summary_file`).
- Added `compiler/stasis_aot_cli_entry.stasis` with argv-driven CLI entry orchestration (`compiler_cli_main_from_argv` + `main`) and host-extern CLI argument bridge declarations (`host_cli_arg_count`, `host_cli_arg_value`) to define stage1 executable argument contract in `.stasis`.
- Added `.stasis` parser fixture coverage for bridge surface shape (`tests/stasis/run_parser_self_host_aot_cli_bridge_counts.stasis`).
- Added runnable host bridge command path `stasis aot-cli --project-dir <dir> --out <exe>` that:
- enumerates `.stasis` files from directory
- compiles via `.stasis`-driven incremental analysis path
- links emitted Cranelift AOT objects to a runnable executable using resolved `main` entry symbol
- Host bridge implementation now runs through explicit staged functions in order:
- `host_emit_ir_from_compiler_state` (produces staged IR-bundle metadata)
- `host_run_cranelift_aot` (produces staged object-bundle metadata from emitted Cranelift AOT objects)
- `host_link_executable_from_objects` (links runnable executable with resolved `main` entry symbol)
- Added deterministic fake-toolchain coverage for executable linker path and CLI flow (`crates/stasis_jit::tests::aot_executable_linker_can_be_driven_by_configured_fake_linker`, `apps/stasis::compiler_backend::tests::self_host_aot_cli_links_runnable_executable_with_main_entry_symbol`).
- `stasis aot-cli` now supports writing a machine-readable summary artifact (`--summary-file <path>`) containing staged bundle paths, entry symbol, and object layout contract for stage parity checks.
- `stasis aot-cli` now supports optional `--entry-file <file.stasis>` host contract routing (`STASIS_AOT_ENTRY_FILE`) so self-host AOT can compile a selected program when a project directory contains multiple `main()` declarations; backend source discovery now resolves project-local import closure from that entry file instead of compiling every `.stasis` file under the directory.
- `stasis aot-cli` now supports optional `--quality-gate` (`STASIS_AOT_QUALITY_GATE=1`) which rejects outputs when the selected entry symbol is still fallback-stub lowered, preventing shipping non-playable placeholder executables as "quality" game builds.
- `.stasis` incremental parser compatibility was extended for Brickout `_v1` source forms used by self-host AOT CLI input: top-level `import`, `const`, `enum`, `struct`, and `global name: Type;` declarations, `function @inline ...` signatures, float literal tokenization compatibility (`123.45` treated as a contiguous numeric primary token), and optional `for` clauses (`for (; cond; )`).
- Verified self-host `.stasis` compile path advances through Brickout `_v1` parsing/incremental analysis and now fails at host linker invocation stage when linker tooling is unavailable (`link.exe`/`STASIS_AOT_LINKER`), indicating parser-side `_v1` compatibility blockers are removed for this slice.
- With `STASIS_AOT_LINKER` set to `lld-link.exe`, Brickout `_v1` self-host compile advances past linker discovery and currently fails on runtime-bridge object unresolved runtime symbols (`core::panicking::*`, `memset`) during final executable link, making runtime-bridge/object-link contract the active blocker.
- Runtime bridge executable-link fallback now retries with CLIF bridge object when rustc-emitted bridge object fails to link on Windows toolchains (e.g., `lld-link` unresolved runtime symbols), unblocking self-host AOT executable emission for Brickout `_v1`.
- Added host regression coverage for entry-file CLI parsing and project-local import closure selection (`apps/stasis::tests::parse_aot_cli_contract_args_accepts_entry_file_flag`, `apps/stasis::compiler_backend::self_host_file_selection_tests::self_host_project_entry_selects_project_local_import_closure`).
- Deliverable:
- `.stasis`-owned core compile CLI orchestration exists and is test-covered independently of runtime watch loop.
- Tests:
- `.stasis` parser/incremental fixture coverage for host bridge extern surface and core function declarations.
- Done gate:
- Core compile orchestration policy remains in `.stasis`; Rust bridge contains no compile-policy branching.
- Status: `in_progress`
- Remaining:
- Slice SH1: Wire minimal host bridge implementations for `S10b` externs in CLI path and execute `compiler_cli_compile_project`. (completed 2026-02-13; current host command path is `stasis aot-cli`)
- Slice SH2a: Replace monolithic `.stasis` host bridge declaration with staged AOT extern contract (`emit_ir`, `run_cranelift_aot`, `link_executable`) while preserving `.stasis` orchestration ownership. (completed 2026-02-13)
- Slice SH2b: Wire host bridge implementations for staged AOT extern calls and route CLI execution through them end-to-end. (completed 2026-02-13; current host `aot-cli` path executes staged bridge sequence)
- Slice SH2c: Add summary-bridge extern contract from `.stasis` (`host_write_aot_cli_summary`) and host sidecar summary emission parity (`<exe>.summary.json`) for staged contract introspection. (completed 2026-02-13)
- Slice SH3a: Add deterministic repeated-run self-host smoke on fixed source input (same staged bridge path, stable summary/output contract). (completed 2026-02-13)
- Slice SH3b1: Add stage1->stage2 metadata-contract smoke (independent stage1/stage2 builds on identical sources produce matching staged bundle contract: entry symbol + object layout metadata). (completed 2026-02-13)
- Slice SH3b2: Add true stage1->stage2 self-host smoke by executing stage1 compiled `.stasis` compiler binary to produce stage2, then verify diagnostics + function metadata summary parity.
- Slice SH3b2e: Add executable CLI contract bridge for self-host compiler main (`argv` extern surface + runtime host binding path) so compiled stage1 `.exe` can receive `aot-cli --project-dir --out --summary-file` arguments directly from process launch.
- Slice SH3b2e1: Define `.stasis` CLI-entry extern contract and parser/runtime fixture coverage for argument loading and compile invocation wiring. (completed 2026-02-13)
- Slice SH3b2e2: Wire host runtime extern implementations for CLI-entry contract in produced AOT executable path.
- Slice SH3b2e2a: Align host CLI parser path with `.stasis` argv contract semantics and summary override bridge (`--summary-file` routed via host summary path override) with unit coverage. (completed 2026-02-13)
- Slice SH3b2e2b: Bind runtime extern dispatch for compiled stage1 executable (`host_cli_arg_count`, `host_cli_arg_value`, `host_set_summary_file`) in produced AOT process path.
- Slice SH3b2e2b1: Link self-host runtime extern bridge object into produced AOT executable with explicit stub symbol coverage for CLI bridge externs and existing staged host externs (host/runtime glue only). (completed 2026-02-13)
- Slice SH3b2e2b1a: Publish process-backed CLI argument env contract (`STASIS_SELF_HOST_ARG_COUNT`, `STASIS_SELF_HOST_ARG_<n>`, `STASIS_AOT_SUMMARY_FILE`) from host `aot-cli` path with restore semantics for runtime bridge consumption. (completed 2026-02-13)
- Slice SH3b2e2b1b: Add host-boundary guard coverage in `apps/stasis` CLI path to keep host responsibilities limited to argv/env glue + self-host entry delegation (no direct compile orchestration in `main.rs`). (completed 2026-02-13)
- Slice SH3b2e2b2: Replace runtime bridge stubs with live process-backed extern implementations and validate stage1 executable argv-driven compile flow.
- Slice SH3b2e2b2a: Add `aot-cli --entry-file` contract path and project-local import-closure source selection so self-host AOT can target one program in multi-sample directories without multi-main rejection. (completed 2026-02-13)
- Slice SH3b2e2b2b: Add deterministic linker discovery/fallback guidance + coverage for self-host AOT CLI executable link step on Windows dev environments lacking `link.exe` on `PATH` (use `STASIS_AOT_LINKER` override contract and document expected toolchain paths).
- Slice SH3b2e2b2a: Add process-backed runtime bridge function implementations (`host_cli_arg_count`, `host_cli_arg_value`, `host_set_summary_file` semantics) in host module with unit coverage, ready to bind into emitted runtime extern bridge. (completed 2026-02-13)
- Slice SH3b2e2b2b: Bind emitted runtime bridge extern symbols to process-backed implementations (replace CLIF stubs) and validate compiled stage1 executable argument-driven behavior.
- Slice SH3b2e2b2b1: Emit Windows runtime bridge object via live `no_std` rustc object with process-backed extern semantics for CLI arg/env bridge, with explicit mode marker and fallback path to CLIF stubs. (completed 2026-02-13)
- Slice SH3b2e2b2b1a: Add opt-in real-toolchain runtime-bridge executable smoke (`STASIS_RUN_REAL_RUNTIME_BRIDGE_ARGC_SMOKE=1`) verifying compiled executable observes `STASIS_SELF_HOST_ARG_COUNT` via live `host_cli_arg_count` binding. (completed 2026-02-13)
- Slice SH3b2e2b2b1b: Extend live runtime bridge extern coverage for source staging env contract (`host_source_file_count`/`host_load_source_file`) and add opt-in executable smoke for `STASIS_SELF_HOST_SOURCE_FILE_COUNT` binding. (completed 2026-02-13)
- Slice SH3b2e2b2b2: Execute compiled stage1 executable through argv path and verify stage2 summary parity using live runtime bridge extern binding.
- Slice SH3q1: Add consolidated `aot-cli` end-to-end quality harness (stage contract parity + invalid-program diagnostic coverage + per-run time-budget assertions), gated as opt-in (`STASIS_RUN_E2E_SELF_HOST_QUALITY=1`) until bootstrap harness/env interactions are fully isolated. (completed 2026-02-13)
- Slice SH3b2a: Add opt-in real-toolchain runnable-exe smoke hook for self-host AOT CLI output (`STASIS_RUN_REAL_AOT_EXE_SMOKE=1`) to exercise executable startup path while startup ABI hardening is in progress. (completed 2026-02-13)
- Slice SH3b2b: Add stage parity harness using `aot-cli --summary-file` outputs so stage1/stage2 compiler executions can be compared via stable machine-readable contract outputs once stage1 executable invocation is wired.
- Slice SH3b2b1: Add CLI summary-parity integration test for repeated `aot-cli --summary-file` runs on identical source, asserting stable `source_file_count`, `entry_symbol`, and `object_file_names` contract fields. (completed 2026-02-13)
- Slice SH3b2b2: Execute the same summary-parity harness through stage1 compiled self-host executable invocation once true stage1->stage2 execution is wired.
- Slice SH3b2d: Add strict self-host lowering gate option (`STASIS_AOT_STRICT_SELF_HOST=1`) that rejects `aot-cli` outputs when emitted `i32` functions rely on stub fallback codegen (temporary bypass via `STASIS_AOT_ALLOW_STUB_FALLBACK=1`). (completed 2026-02-13)
- Slice SH3b2c: Add signed self-host AOT artifact flow (post-link signing hook + signer/policy validation checklist) so Windows security policy allows stage1/stage2 executable invocation in default dev environments.
- Slice SH3b2c1: Add optional post-link signing hook (`STASIS_AOT_SIGN_TOOL`) in self-host AOT CLI bridge and deterministic fake-signer test coverage for signer invocation contract. (completed 2026-02-13)
- Slice SH3b2c2: Add signed-artifact policy validation smoke for stage1 executable invocation path on Windows dev environments with Application Control enabled.
- Slice SH3b2c2a: Add opt-in signed real-toolchain executable smoke hook (`STASIS_RUN_SIGNED_SELF_HOST_SMOKE=1` + `STASIS_AOT_SIGN_TOOL`) to validate signed self-host startup path when signing toolchain/certs are configured. (completed 2026-02-13)
- Slice SH3b2c2b: Run signed stage1->stage2 parity smoke in a cert-trusted environment and record policy outcome/requirements.
- Priority:
- This is the active front-of-line work. Complete `SH1` before any additional non-blocking `R*`/`H*` slices.

### S11 - Swap Indicator (Tick-Based)
- Language:
- `.stasis` (feature logic) + `Rust` (draw host bridge only)
- Scope:
- Integrate `DebugUI.swapFlashTicks` behavior in Stasis game code.
- Current implementation:
- `samples/brickout_revenge/brickout_revenge_v1.stasis` now defines `on_code_swap()` to arm `swap_flash_ticks` and renders/decrements indicator ticks in `draw_swap_indicator()`.
- Deliverable:
- Successful swaps trigger visible deterministic indicator.
- Tests:
- Tick countdown behavior tests and no-indicator-on-failure tests.
- Current runtime coverage fixture:
- `tests/stasis/run_swap_indicator_tick_behavior.stasis` (arms `on_code_swap`, decrements `swapFlashTicks` over 180 ticks, and verifies no indicator once expired).
- Done gate:
- Indicator follows tick policy and does not fire on failed swap.
- Status: `completed`

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
- Current runtime coverage:
- `apps/stasis` scenario run path uses the real incremental backend and drives runtime launch in graphics mode for Brickout.
- Transitional note: `crates/stasis_compiler` now uses a Rust-native in-process incremental analyzer path and does not invoke `Stasis.Cli.exe` for compile requests.
- Next step remains replacing this transitional Rust analyzer with the self-hosted `.stasis` compiler execution path inside the same process.
- Done gate:
- Brickout runs with correct proportion and swap loop remains stable.
- Real compile -> function patch -> commit path updates patch identity on source edit.
- Status: `completed`

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

- (none currently tracked for compiler ownership migration)
