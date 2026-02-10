# Callable Resolution Review Checklist

This checklist is for changes touching callable resolution, symbol naming, and extern linkage.

## Scope

Use this checklist when changing any of:

- Receiver-scoped callables (`foo(value: T, ...)`)
- Receiver form (`value.foo(...)`) and function form (`foo(value, ...)`)
- Extern callables and `@extern("...")` naming
- LLVM/Cranelift symbol emission and call-site resolution
- Reachability and overload selection

## Reviewer Checklist

1. **Lexical shadowing correctness**
- Local values must win over global callables in both semantic analysis and lowering diagnostics.
- Verify block-local declarations cannot shadow call sites outside their scope in backend-only maps.

2. **Arity and receiver resolution**
- Receiver form must validate receiver type + receiver-form arity.
- Function form must validate full arity and first-argument receiver matching behavior.
- Ambiguous overloads should emit focused diagnostics.

3. **Extern naming invariants**
- Extern overloads with shared default link names must produce semantic errors.
- Extern link names must avoid collisions with emitted non-extern symbols (for example, receiverless `foo`).
- Explicit `@extern("name")` should still be honored when unique.

4. **Backend parity**
- LLVM and Cranelift must choose the same callable targets for the same source.
- Callable symbol naming rules must be shared, not duplicated ad-hoc in each backend.

5. **Reachability and declaration safety**
- Reachability should not force unrelated overload extern declarations.
- Emitted declarations/calls must reference compatible signatures.

6. **Diagnostics and tolerant lowering**
- Programs with semantic diagnostics should still avoid malformed call emission in tolerant lowering paths.

## Test Matrix

The canonical matrix is implemented in:

- `Stasis.Compiler.Tests/CallableResolutionParityTests.cs`

Current matrix scenarios:

1. Receiver overload dispatch by first parameter type.
2. Primitive receiver overload dispatch (`ping(i32)` + `ping(u8)`).
3. Extern overloads with explicit distinct link names.
4. Receiverless + extern receiver callable coexistence via explicit link name.
5. Explicit extern link name mixed with non-extern receiver overload.

## Commands

Smoke suite:

```powershell
.\scripts\callable-smoke.ps1
```

Pre-push gate:

```powershell
.\scripts\pre-push-gate.ps1
```

The pre-push gate runs:

1. Callable smoke suite
2. Full `Stasis.Compiler.Tests` suite (unless `-Quick`)
3. `.\stasis.bat test --all` (unless `-SkipStasisAll`)
