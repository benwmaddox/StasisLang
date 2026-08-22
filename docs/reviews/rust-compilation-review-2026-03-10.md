# Rust Compilation Process Review

Date: 2026-03-10

Scope:
- `crates/stasis_compiler`
- `crates/stasis_jit`
- `apps/stasis`
- `tools/cranelift-aot`

Validation:
- Ran `cargo test -p stasis_compiler backend:: -- --nocapture`
- Result: 174 passed, 1 ignored, 0 failed
- Note: several executable-link parity smokes are environment-gated and were skipped because runtime link symbols were not available in the current environment

## Executive Summary

The active Rust compiler backend is in better shape than the older review trail suggests.

For the live in-process compiler path, JIT and AOT are now fairly close:
- both use the same compile-analysis invalidation rules
- both use the same reachable-function selection logic
- both lower functions through the same `compile_function_with_module(...)` path
- the remaining intentional IR difference is the internal-call policy split (`JIT` dispatch helper vs `AOT` direct calls)

That is the right architecture.

My assessment:
- In-process `stasis_compiler` backend: good, roughly `8/10`
- End-to-end Rust compilation story across the repo: mixed, roughly `6/10`

The downgrade comes from one major remaining divergence: `apps/stasis` still has an AOT/self-host path that emits textual CLIF and shells out to a separate helper compiler. That second backend stack is the biggest risk to long-term IR parity, clarity, and maintenance cost.

## What Is Working Well

### 1. JIT and AOT now share the important backend core

The main backend contract is substantially implemented:
- thin-wrapper guardrails are enforced in tests in `crates/stasis_compiler/src/backend/mod.rs:60`
- the shared per-function compile path lives in `crates/stasis_compiler/src/backend/emit.rs:1451`
- JIT routes through it in `crates/stasis_compiler/src/backend/jit.rs:987`
- AOT routes through it in `crates/stasis_compiler/src/backend/aot.rs:518`

This is the biggest positive result in the current design. On the live compiler path, IR-shape drift between JIT and AOT is now much harder to introduce by accident.

### 2. Incremental invalidation is shared and reasonably disciplined

The shared re-emit rules in `crates/stasis_compiler/src/backend/emit.rs:477` and reachable-function selection in `crates/stasis_compiler/src/backend/emit.rs:489` are used by both:
- JIT: `crates/stasis_compiler/src/backend/jit.rs:142`
- AOT: `crates/stasis_compiler/src/backend/aot.rs:85`

That is both clearer and safer than mode-specific invalidation logic.

### 3. The frontend compile loop is simple and easy to reason about

The `Compiler` flow is compact:
- indexing and dirty propagation: `crates/stasis_compiler/src/compiler.rs:219`
- emit only selected functions: `crates/stasis_compiler/src/compiler.rs:311`

This is a good baseline for an incremental compiler. The dirty/signature propagation rules are understandable without much indirection.

## Findings

### 1. High: there is still a second AOT compiler stack outside the shared backend

The main compiler path uses shared Cranelift lowering, but `apps/stasis` still has a separate AOT route that compiles emitted CLIF text through `compile_clif_to_object(...)`:
- CLIF object compile call: `apps/stasis/src/compiler_backend.rs:1363`
- helper process launch: `crates/stasis_jit/src/lib.rs:358`
- helper crate excluded from workspace: `Cargo.toml:10`
- workspace Cranelift version: `Cargo.toml:31`
- helper Cranelift version: `tools/cranelift-aot/Cargo.toml:9`

Why this matters:
- it creates a second compiler implementation boundary
- it uses a different Cranelift version (`0.126.1` vs workspace `0.117`)
- it makes "JIT and AOT should be as close as possible for IR output" true only for the in-process backend, not for the repo as a whole

Recommendation:
- make the in-process `AotProcess` object emission the source of truth for AOT
- keep the CLIF helper only as an explicitly deprecated fallback, or remove it
- if it must remain, bring it into the workspace and pin it to the same Cranelift version immediately

### 2. Medium: the compiler still reparses function bodies during emit because HIR is only source text

`FunctionHIR` is currently just a container for source strings:
- `crates/stasis_compiler/src/ir/hir.rs:1`
- `crates/stasis_compiler/src/compiler.rs:382`

Resolution (2026-08-22): function bodies now parse once into backend-independent HIR in
`frontend/body_parser.rs`; `FunctionHIR` no longer stores source text, and JIT/AOT/Web consume the
same structured body. The current flow is documented in `docs/compiler_architecture.md`.

At the time of this review, the pipeline was effectively:
1. index file and hash metadata
2. slice function source text
3. reparse statements during backend emit

This is simple, but it is not efficient, and it blurs the phase boundary between frontend lowering and backend codegen.

Why this matters:
- each emitted function pays parsing cost again
- backend clarity suffers because statement parsing and codegen are still interleaved
- IR comparison tooling is weaker because there is no stable structured HIR to diff before backend policy is applied

Recommendation:
- keep the current dirty/reachability model, but replace string-backed `FunctionHIR` with a typed statement/expression tree
- parse once, reuse in both JIT and AOT
- keep backend code focused on lowering structured nodes to Cranelift IR

### 3. Medium: `AotProcess` retains object bytes monotonically across recompiles

`AotProcess` stores object bytes in `object_bytes: Vec<Vec<u8>>`:
- field definition: `crates/stasis_compiler/src/backend/aot.rs:25`
- new bytes appended on each emit: `crates/stasis_compiler/src/backend/aot.rs:164`
- artifacts are pruned/replaced, but old `object_bytes` entries are not compacted: `crates/stasis_compiler/src/backend/aot.rs:168`

Why this matters:
- repeated incremental AOT compiles can grow memory usage indefinitely
- this is unnecessary for the current artifact model because only active artifacts need retained object bytes

Recommendation:
- replace `object_index` with a direct map keyed by `FunctionId`, or compact `object_bytes` after each compile
- if historical artifacts are needed later, store them explicitly with a retention policy rather than leaving unreachable bytes in the live cache

### 4. Medium: AOT extern resolution is shared now, but still heuristic rather than contract-based

Extern resolution is much better than before because both paths go through shared resolution helpers:
- shared resolver: `crates/stasis_compiler/src/backend/emit.rs:194`
- AOT preferred-symbol policy: `crates/stasis_compiler/src/backend/emit.rs:256`

The remaining issue is that AOT still resolves against a baked allowlist of "known runtime symbols" rather than an explicit export contract:
- `crates/stasis_compiler/src/backend/emit.rs:225`

Why this matters:
- it is still possible for the linked runtime surface and AOT compile-time assumptions to drift
- this is a contract problem, not a lowering problem

Recommendation:
- generate or maintain an explicit runtime export manifest
- resolve AOT externs against that manifest instead of a name heuristic
- fail early when the runtime contract and compile-time contract differ

### 5. Low/Medium: parity testing exists, but it is still narrow for the stated goal

There is a real JIT/AOT parity test:
- `crates/stasis_compiler/src/backend/aot.rs:1366`

That is good, but it currently only covers two internal-call fixtures and depends on link/runtime availability.

Recommendation:
- add a small parity corpus that covers:
  - extern calls
  - globals and collection accesses
  - control flow
  - struct-view ABI cases
  - string literal handling
- for each case, compare:
  - behavior
  - emitted Cranelift IR text shape for the shared backend path where practical

## Efficiency and Clarity Assessment

### Efficiency

Good:
- file hashing and dirty propagation are cheap and clear
- reachable-only emission reduces backend work
- compile-analysis cache reuse is sensible

Weaker:
- reparsing function bodies during emit leaves avoidable frontend work on the hot path
- the app/self-host AOT CLIF helper path adds file I/O and process-launch overhead
- `AotProcess` currently retains stale object bytes

### Clarity

Good:
- the live shared backend seam is now obvious
- wrapper-guard tests reduce accidental drift
- the compiler core is still small enough to reason about

Weaker:
- "HIR" is not really HIR yet
- repo-level AOT has two different stories: shared `ObjectModule` in `stasis_compiler`, and textual CLIF helper compilation in `apps/stasis` / `stasis_jit`

## Recommended Order of Work

1. Collapse the repo onto one AOT object-generation path.
2. Replace string-backed `FunctionHIR` with structured frontend output.
3. Fix `AotProcess` artifact retention so object bytes do not grow monotonically.
4. Replace AOT extern heuristics with an explicit runtime export contract.
5. Expand parity coverage with a small shared JIT/AOT fixture suite.

## Bottom Line

If the question is "are JIT and AOT close today?", the answer is:

- for the main in-process `stasis_compiler` backend: yes, mostly
- for the full Rust compilation ecosystem in this repo: not yet

The core backend architecture is pointed in the right direction now. The biggest remaining problem is not inside `emit.rs`; it is the existence of the separate CLIF-helper AOT path around it. Remove that divergence, then make the frontend output structured instead of string-backed, and the compilation process will be both clearer and materially more efficient.
