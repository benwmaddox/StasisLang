# Shared Cranelift Backend Contract

Date: 2026-03-05

Goal:
- JIT and AOT should be thin backend-specific shells over one shared Cranelift lowering pipeline.
- Shared code should own lowering correctness.
- Backend-specific code should own only binding/finalization policy.

## Why

The current codebase already shares most expression and statement lowering in
`crates/stasis_compiler/src/backend/emit.rs`, but important backend seams still
exist in:

- `crates/stasis_compiler/src/backend/jit.rs`
- `crates/stasis_compiler/src/backend/aot.rs`

That remaining duplication creates two problems:

1. Correctness can drift when JIT and AOT make different compile-analysis,
   extern-resolution, or function-compile decisions for identical source.
2. New features are harder to add because the safe path is no longer obvious.

The target architecture is: one lowering pipeline, two policy shells.

## End State

When this work is complete:

- `emit.rs` (or a renamed shared backend module) owns almost all Cranelift
  lowering and compile-analysis logic.
- `jit.rs` mostly owns:
  - host/runtime symbol addresses
  - JIT module finalization into executable memory
  - code pointer / dispatch-table publication
  - JIT-only runtime table refresh
- `aot.rs` mostly owns:
  - extern binding policy for the linked runtime
  - direct-call/import policy where needed
  - object emission
  - bundle/link finalization

If a normal language feature requires parallel lowering edits in both `jit.rs`
and `aot.rs`, that should be treated as a design smell.

## Shared Pipeline

The shared per-function compile path should own these phases:

1. Compile analysis
   - call signatures
   - resolved extern metadata
   - constants
   - global path types
   - collection metadata
   - named-struct field metadata
   - invalidation / re-emit decisions

2. Function setup
   - Cranelift function signature construction
   - block creation
   - block parameter append/setup
   - local binding initialization
   - struct-view binding initialization

3. Runtime import setup
   - declare runtime helper imports
   - declare extern imports
   - build `FuncRef`s used during lowering

4. Body lowering
   - extract body text
   - parse simple statements / expressions
   - lower statements, expressions, conditions, loops, calls, and state access

5. Shared diagnostics
   - unsupported shape messages
   - type mismatch messages
   - signature / binding validation

## Backend Policy Surface

The backend policy input should be narrow and explicit.

It should answer:

1. What module type is being compiled into?
   - `JITModule`
   - `ObjectModule`

2. How do internal calls lower?
   - JIT runtime dispatch helper path
   - AOT direct/imported call path

3. How are externs resolved?
   - JIT: resolve to concrete host addresses
   - AOT: resolve to the symbol names exported by the linked runtime

4. How is the compiled function finalized?
   - JIT: define, finalize, publish code pointer
   - AOT: define, finish module, emit object bytes

Everything else should stay in shared code.

## Ownership Rules

### Shared code must own

- `CompileAnalysisCache` construction and invalidation rules
- supported call-signature discovery
- type-to-ABI mapping
- local binding setup
- struct-view ABI handling
- runtime helper import declaration
- expression and statement lowering
- global / collection / struct-view access lowering
- control-flow lowering

### JIT-only code may own

- lookup of host symbol addresses
- registration of concrete symbol pointers in `JITBuilder`
- final code pointer extraction
- dispatch-table publication and runtime refresh

### AOT-only code may own

- mapping extern declarations to concrete link-time symbol names
- direct internal-call import/export policy
- object emission, engine bundle writing, and link steps

## Operation Families

The lowering model should stay organized around a small set of operation
families.

1. Local SSA values
   - `let`, local mutation, arithmetic temps
   - keep in Cranelift vars / SSA values

2. State reads and writes
   - scalar globals
   - indexed collections
   - struct field access
   - lower through shared state-access helpers

3. Calls
   - shared signature matching and argument lowering
   - backend policy decides only internal-call execution mode

4. Control flow
   - `if`, `for`, `foreach`, `return`, `continue`
   - shared block/branch lowering

This keeps feature work framed as:

- new operation family work in shared lowering, or
- a small backend policy extension

not separate JIT and AOT implementations.

## Target API Shape

The shared compile path should converge on something conceptually like this:

```rust
struct SharedCompileInputs<'a> {
    meta: &'a FunctionMeta,
    hir: &'a FunctionHIR,
    analysis: &'a CompileAnalysisCache,
    type_table: &'a mut TypeTable,
}

trait BackendCompilePolicy {
    type ModuleT: cranelift_module::Module;
    type Artifact;

    fn module(&mut self) -> &mut Self::ModuleT;
    fn internal_call_mode(&mut self, self_function_id: FunctionId, self_func_id: FuncId)
        -> InternalCallMode<'_>;
    fn finalize(
        &mut self,
        function_id: FuncId,
        context: &mut cranelift_codegen::Context,
    ) -> Result<Self::Artifact, String>;
}

fn compile_function_with_policy<P: BackendCompilePolicy>(
    inputs: SharedCompileInputs<'_>,
    policy: &mut P,
) -> Result<P::Artifact, String>;
```

The exact types can differ, but the seam should look like this:

- shared compile function owns compile mechanics
- backend policy owns only the narrow behavior that truly differs

## Current Remaining Divergences To Eliminate

1. Per-function compile shell duplication in `jit.rs` and `aot.rs`
   - signature setup
   - block param setup
   - local binding bootstrap
   - body lowering entry

2. Extern resolution differences
   - JIT resolves actual addresses
   - AOT still needs a stricter shared resolution policy for link targets

3. Internal-call mode differences
   - these should remain, but only as an explicit policy seam

4. Separate AOT helper tool path
   - if retained, it should not become a second lowering implementation

## Review Rule

Use this rule when adding backend/compiler features:

- If the change is about language semantics or lowering shape, default to
  shared code.
- If the change is about symbol binding, finalization, or runtime publication,
  it may belong in JIT/AOT-specific code.
- If a PR adds matching lowering logic in both `jit.rs` and `aot.rs`, stop and
  justify why the shared seam was not extended instead.

## Execution Order

Recommended implementation order:

1. Share compile-analysis and extern/import policy.
2. Extract one shared per-function compile pipeline.
3. Reduce internal-call divergence to an explicit policy switch only.
4. Add parity and architecture regressions to keep the split stable.

This order improves correctness first, then performs the larger mechanical
extraction, then locks the result in with tests.
