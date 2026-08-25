# Compiler Architecture

This document is the map of the active Rust compiler. The language contract remains
[spec.md](spec.md); this file explains how the implementation carries that contract to JIT, AOT,
and Web output.

## One compiler flow

```text
source files
  -> module graph and declaration index
  -> parse every function body into backend-independent HIR
  -> whole-program semantic and effect analysis
  -> resolved call graph (exact function identities)
  -> reachability from lifecycle and host-required roots
  -> target-independent ProgramSnapshot and PatchPlan
  -> JIT, AOT, or Web emission
  -> atomic publication at the runtime safe point
```

All compiler-facing entry points use this validity contract. An unreachable function may be
omitted from target emission, but it may not contain invalid source. This prevents an editor,
development build, release build, and Web package from disagreeing about whether one program is
valid.

## Ownership by layer

| Layer | Owner | Responsibility |
| --- | --- | --- |
| Source declarations | `frontend/parser.rs`, `frontend/indexer.rs`, `frontend/module_graph.rs` | Syntax, imports, declaration identity, and source ranges |
| Function bodies | `frontend/body_parser.rs` | Parse statements and expressions once |
| Compiler IR | `ir/hir.rs` | Backend-independent statements, expressions, types, and debug offsets |
| Semantic facts | `data_flow.rs` | Type-aware calls, effects, storage use, and exact callee identities |
| Orchestration | `compiler.rs` | Cache parsed bodies, construct the resolved graph, track invalidation, and lower functions to HIR |
| Planning | `backend/reachability.rs`, `backend/patch_plan.rs` | Select reachable identities and compute a coherent patch |
| Target lowering | `backend/emit.rs`, `backend/jit.rs`, `backend/aot.rs`, `backend/wasm.rs` | Translate validated HIR and snapshots into target artifacts |
| Publication | `stasis_runner` | Validate and atomically commit a complete generation between ticks |

The Cranelift emitter does not own Stasis syntax. The HIR contains no source-body string and can be
consumed without reparsing source.

## Identity and reachability

A function is identified by `SymbolId` and its collision-checked compact `FnId`. Names are for
diagnostics and lookup, not graph identity. Body analysis resolves calls against parameter types
and module visibility, then `Compiler` builds forward and reverse edges from those resolved
identities. Selecting one overload does not retain other overloads merely because they share a
name.

Default roots are `main`, `tick`, `render`, and `on_code_swap` when present. Hosts may add
required exported roots. If no lifecycle root exists, all functions remain reachable so a
library-only input can still be compiled deliberately.

Reachability controls backend work only. Semantic validity, effect checking, layout construction,
and diagnostics cover the complete program.

## Incremental compilation

The correctness unit is the complete changed file/program view. Parsed function bodies are cached
by source path, module context, signature hash, and body hash. Semantic hashes and the resolved
call graph decide which backend artifacts can be reused. JIT planning expands changed functions
through the required reverse-caller closure; AOT emits a coherent release generation.

A `ProgramSnapshot` is the immutable handoff between accepted compiler semantics and target/runtime
consumers. A `PatchPlan` explains why each function is emitted, reused, or retained. Publication
is all-or-nothing; a failed compile, incompatible layout/signature, relocation, or swap hook leaves
the accepted generation active.

## Failure policy

Unsupported syntax or lowering fails with a deterministic diagnostic. There is no placeholder
compiler, hash-return stub, fallback AOT body, or alternate analysis compiler. Instrumentation
adds observations to the same pipeline rather than selecting another compiler path.

## Reality gates

Repository validation compiles all Rust targets, runs Rust tests, executes the checked-in
`tests/stasis/*.test.stasis` behavior suite through the normal JIT compiler, and always compiles
Brickout Revenge v1 through the production AOT engine-bundle path. The Windows gate also links,
loads, initializes, and executes that bundle for two deterministic ticks. Source/ABI validators
prune generated, cached, vendored, and build directories so they inspect checked-in source rather
than stale copies.

When a test requires an external platform tool or credential, it belongs in an explicit smoke
workflow. Default correctness tests must remain hermetic and must not silently return early behind
an environment variable.
