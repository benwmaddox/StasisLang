# Cranelift Generation Review

Date: 2026-03-05

Scope reviewed:
- `crates/stasis_compiler/src/backend/emit.rs`
- `crates/stasis_compiler/src/backend/jit.rs`
- `crates/stasis_compiler/src/backend/aot.rs`
- `crates/stasis_jit/src/lib.rs`
- `tools/cranelift-aot/Cargo.toml`
- `Cargo.toml`

Validation:
- Ran `cargo test -p stasis_compiler backend:: -- --nocapture`

## Findings

### 1. High: AOT incremental recompilation does not invalidate on analysis changes that already force JIT re-emission

Files:
- `crates/stasis_compiler/src/backend/aot.rs:144`
- `crates/stasis_compiler/src/backend/aot.rs:151`
- `crates/stasis_compiler/src/backend/jit.rs:189`
- `crates/stasis_compiler/src/backend/jit.rs:200`
- `crates/stasis_compiler/src/backend/emit.rs:353`
- `crates/stasis_compiler/src/backend/jit.rs:3802`

Why this matters:
- `AotProcess::compile()` only recompiles reachable functions when `function.dirty` is set or the stored body hash changed.
- JIT already knows that is not enough. It computes `compile_analysis_requires_reemit(...)` and forces re-emission when extern resolution, constants, global path types, collection info, or named-struct field types change.
- That means AOT can keep stale object files when a dependency changes without changing the caller body text. Imported constants are the clearest example, and JIT already has a regression test for that exact case.

Why this also blocks simplification:
- JIT and AOT now have different correctness rules for "what requires recompilation". That keeps the pipelines conceptually different even though they share most lowering code.

Recommended simplification:
- Move AOT onto the same compile-analysis cache / invalidation decision used by JIT.
- Reuse `select_emit_function_ids(...)` instead of maintaining a weaker AOT-only selection path.

### 2. High: AOT does not resolve externs; it just picks the last candidate name

Files:
- `crates/stasis_compiler/src/backend/aot.rs:92`
- `crates/stasis_compiler/src/backend/aot.rs:95`
- `crates/stasis_compiler/src/backend/jit.rs:489`
- `crates/stasis_compiler/src/backend/jit.rs:497`
- `crates/stasis_compiler/src/backend/emit.rs:216`

Why this matters:
- JIT walks `symbol_candidates` and selects the first symbol that actually resolves in the runtime.
- AOT does not do that. It takes `sig.symbol_candidates.last()`.
- Today the candidate list is order-sensitive and includes aliases like raw name, `stasis_*`, `stasis_jit_*`, plus special cases such as `time -> stasis_get_time_ms`.
- So AOT can emit a symbol name simply because it is last in the list, not because the linked runtime exports it. That is a correctness risk and it makes AOT behavior diverge from JIT for the same source program.

Recommended simplification:
- Centralize extern resolution in one shared function.
- For AOT, resolve against an explicit exported-symbol set for the runtime being linked, and fail early if no candidate matches.
- Avoid encoding policy in candidate ordering.

### 3. Medium: There are still two AOT object-generation stacks, and they use different Cranelift versions

Files:
- `crates/stasis_compiler/src/backend/aot.rs:533`
- `crates/stasis_jit/src/lib.rs:348`
- `crates/stasis_jit/src/lib.rs:382`
- `Cargo.toml:10`
- `Cargo.toml:31`
- `Cargo.toml:36`
- `tools/cranelift-aot/Cargo.toml:9`
- `tools/cranelift-aot/Cargo.toml:12`

Why this matters:
- The main compiler AOT path emits objects directly with `ObjectModule`.
- The helper path still writes CLIF to a temp file and shells out to `tools/cranelift-aot`.
- That helper is excluded from the workspace and pinned to Cranelift `0.126.1`, while the workspace compiler code is on `0.117`.
- This keeps two independent object-generation implementations alive, with different parser/emitter code and different backend versions. That is the opposite of the current simplification goal and creates real drift risk around CLIF syntax, ISA flags, and ABI expectations.

Recommended simplification:
- Pick one AOT object emitter as the source of truth.
- Prefer reusing the in-process Rust object-emission path where possible.
- If the helper must remain, bring it into the workspace and pin it to the same Cranelift version as the rest of the compiler.

## Simplification Direction

If the goal is "as simple as possible while still accurate", the shortest path is:

1. Share AOT/JIT invalidation rules.
2. Share extern resolution policy instead of letting JIT resolve and AOT guess.
3. Collapse to one AOT object-generation implementation, or at minimum one Cranelift version.

That keeps the existing shared lowering in `emit.rs`, but removes the remaining places where correctness currently depends on mode-specific behavior.
