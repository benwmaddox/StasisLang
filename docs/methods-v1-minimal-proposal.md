# Methods V1 Minimal Proposal (Receiver-First, Low Parser Risk)

Date: 2026-02-09
Status: Proposal (not implemented)

## Goal

Define a minimal methods feature for Stasis that:

- Matches the receiver-first direction (`x.m(y)` centers behavior on data).
- Preserves deterministic/static semantics (no dynamic dispatch).
- Reuses the existing function-centric lowering pipeline.
- Requires only small, LL(1)-friendly grammar/parser changes.

This proposal is intentionally conservative. It favors "front-end sugar over existing ABI" so implementation risk stays low.

## Design summary

- Add a new top-level declaration: `method`.
- First parameter is always the receiver parameter.
- Method calls remain current syntax (`receiver.name(args)`), but now resolve as regular calls (not just special forms like `clear()`).
- Desugaring contract: `receiver.name(a, b)` -> resolved receiver-first function call shape.
- Extern methods are allowed and use fixed receiver-first ABI parameter ordering.

## Why this is minimal-risk

Benefits:

- Parser already supports member-call expression shapes.
- Lowering already handles ordinary function calls well; method calls can desugar to those.
- Reachability/tree-shaking can reuse current function symbol flow.

Pitfalls:

- New resolution ambiguity class (extension conflicts).
- Must define receiver type matching precisely.
- Must keep extern naming stable to avoid host ABI churn.

Mitigation:

- Hard ambiguity diagnostics.
- Exact match first, no implicit lookup coercions.
- Explicit link-name attributes for extern methods.

## Proposed source syntax

### Method declaration

```stasis
method damage(self: Enemy, amt: i32): void {
    self.hp -= amt;
}
```

Rules:

- Receiver is parameter index 0 (`self` above).
- Receiver must have an explicit type.
- Method name is a regular identifier.
- No overloading (same as functions today).

Benefits:

- Looks like current `function` signatures, so low learning cost.
- Keeps mutation/data flow explicit via a named receiver variable.

Pitfalls:

- Without receiver intent markers, it may be unclear if method mutates receiver.
- See "receiver intent" options below for follow-up.

### Extern method declaration

```stasis
extern method sleep_ms(sys: SysHost, ms: i32): i32;

method @extern("stasis_input_key_down") key_down(input: InputSnapshot, key: i32): bool;
```

Rules:

- `extern method ...;` is declaration-only (no body).
- Attribute form `@extern("symbol")` remains available to pin host symbol names.
- ABI argument order is receiver first.

Benefits:

- Unified host contract with method style.
- Stable, explicit linking with current attribute pattern.

Pitfalls:

- Requires pseudo-types (`SysHost`, etc.) for receiver-centric host APIs.
- Could feel artificial for host calls that are globally scoped in nature.

## Grammar patch (LL(1)-friendly)

Reference base: `docs/compilation.md`.

### 1) Top level item set

Current:

```ebnf
TopLevelItem     -> StructDecl
                 | ImportDecl
                 | EnumDecl
                 | GlobalDecl
                 | FunctionDecl
                 | TestDecl
```

Proposed:

```ebnf
TopLevelItem     -> StructDecl
                 | ImportDecl
                 | EnumDecl
                 | GlobalDecl
                 | FunctionDecl
                 | MethodDecl
                 | TestDecl
```

### 2) New method declaration production

```ebnf
MethodDecl       -> ExportOpt ExternOpt
                    "method" AttributeListOpt Identifier
                    "(" ParamListOpt ")"
                    ReturnTypeOpt
                    FunctionBody
```

Semantic requirement (not syntax):

- `ParamListOpt` must contain at least one parameter.
- Parameter 0 is the receiver.

Why LL(1) still works:

- Declaration starts with `method` keyword after optional `export`/`extern`, disjoint from existing `function`/`test`/`struct`/etc.
- No expression grammar change required for v1.

### 3) Lexer addition

- Add keyword token for `method`.

## Resolution and desugaring semantics

### 1. Method call resolution

Given call expression:

```stasis
recv.name(arg1, arg2)
```

Resolution algorithm:

1. Resolve `recv` type as `T`.
2. Gather visible methods with name `name`.
3. Filter to methods whose parameter 0 type is exactly `T`.
4. Verify arity/types of remaining parameters against `arg1..argN`.
5. If exactly one candidate remains, bind call.
6. If none remain, emit "unknown method" diagnostic.
7. If multiple remain, emit "ambiguous method" diagnostic.

Determinism constraint:

- Never tie-break by import order.

Benefit:

- Deterministic and easy to reason about.

Pitfall:

- Exact match only may feel strict; implicit widening can be added later if needed.

### 2. Desugaring contract

Once bound, lower as receiver-first function call shape:

```stasis
recv.name(a, b)
```

becomes equivalent to:

```stasis
name(recv, a, b)
```

or an internal mangled symbol (example):

```text
Enemy_name(recv, a, b)
```

Benefit:

- Reuses existing function ABI/lowering.

Pitfall:

- Must keep a clear source-to-internal mapping for diagnostics/debug symbols.

### 3. Direct function-form method calls

Optional compatibility behavior (recommended in v1):

- Allow calling bound methods in function form (`damage(enemy, 10)`) during migration.
- Method-form and function-form calls are semantically identical after binding.

Benefit:

- Easier incremental adoption.

Pitfall:

- If both free function `damage` and method `damage(self: Enemy, ...)` exist, diagnostics must force disambiguation.

## Symbol naming and uniqueness

Recommended internal symbol identity:

```text
<ReceiverType>_<MethodName>
```

Example:

- `method damage(self: Enemy, amt: i32): void` -> `Enemy_damage`

No-overload implication:

- Symbol identity by `(receiver type, method name)`.
- Duplicate declaration in scope is an error.

Benefit:

- Stable naming for backends and link debugging.

Pitfall:

- Future overload support would require symbol scheme expansion.

## Receiver mutability options (v1 and future)

### Minimal v1 (lowest risk)

- No new receiver mutability syntax.
- Mutability follows existing assignment legality (if body assigns through receiver path, normal assignment checks apply).

Benefit:

- No grammar expansion beyond `method` keyword.

Pitfall:

- Call site does not advertise "mutates receiver" explicitly.

### V1.1 option (recommended follow-up)

Add receiver intent marker in declaration only (example syntax):

```stasis
method apply_damage(mut self: Enemy, amt: i32): void { ... }
method hp(self: Enemy): i32 { ... }
```

Benefit:

- Better diagnostics and API clarity.

Pitfall:

- Requires parameter grammar update and additional semantic checks.

## Mutation-on-first-parameter policy

You asked whether mutating data should be constrained to the first parameter. Here are options for methods v1:

### Option A: hard rule

- Only parameter 0 may be mutated.
- Any mutation through later parameters is a compile error.

Benefits:

- Very clear side-effect model.
- Easier static effect reasoning.

Pitfalls:

- Overly restrictive for valid patterns (`swap(a, b)`, partitioning, some copy/move APIs).
- Can force awkward API shapes.

### Option B: default rule with explicit escape hatch (recommended)

- Parameter 0 is the default mutable target.
- Additional mutable parameters must be explicitly marked (future `mut`/`out` marker).

Benefits:

- Keeps the model clear in common cases.
- Still supports legitimate multi-mutation operations.

Pitfalls:

- Requires extra syntax and semantic checking for explicit mutable secondary parameters.

### Option C: no rule

- Any parameter can be mutated if language assignment rules allow it.

Benefits:

- Most flexible and closest to current behavior.

Pitfalls:

- Weak side-effect clarity at call sites.

Recommendation:

- Start with Option B intent, but ship v1 with minimal syntax first.
- Add receiver intent and explicit secondary-mutation markers in v1.1.

## Extern method ABI details

Rule:

- Extern method call ABI is exactly receiver-first argument order.

Example:

```stasis
method @extern("stasis_utf8_len") len(self: utf8[256]): i32;

let n: i32 = name.len();
```

Lowered host call shape:

```text
call stasis_utf8_len(name)
```

Benefit:

- Predictable host interop with no hidden dispatch.

Pitfalls:

- Receiver representation must be documented (pointer/index/descriptor) per type category.
- ABI mismatch risk if host and compiler disagree on receiver layout.

Mitigation:

- Document receiver lowering per type in `docs/spec.md` ABI section.
- Encourage explicit extern link names for stability.

## Examples

### 1. Game-state mutation

```stasis
struct Enemy {
    hp: i32;
    x: f32;
}

method damage(self: Enemy, amt: i32): void {
    self.hp -= amt;
}

method is_dead(self: Enemy): bool {
    return self.hp <= 0;
}

function tick_enemy(e: Enemy): void {
    e.damage(10);
    if (e.is_dead()) {
        e.hp = 0;
    }
}
```

Benefit:

- Reads naturally as behavior on `Enemy`.

Pitfall:

- `Enemy` is a view/reference style type in Stasis semantics; docs should remind users this is not heap object mutation.

### 2. String helper surface

```stasis
method clear(self: utf8[256]): void {
    // equivalent to existing explicit string clear strategy
}

method append_i32(self: utf8[256], value: i32): i32 {
    // returns new byte length
}

buffer.clear();
buffer.append_i32(score);
```

Benefit:

- Reduces flat helper namespace pressure.

Pitfall:

- Can collide with existing built-in names (`clear` special form). Must define precedence/migration behavior.

### 3. Extern host input method

```stasis
struct InputSnapshot {
    // fields omitted
}

method @extern("stasis_input_key_down") key_down(self: InputSnapshot, key: i32): bool;

if (input.key_down(32)) {
    // jump
}
```

Benefit:

- Host API looks like typed behavior instead of global syscall.

Pitfall:

- Requires clear documentation for host-side implementation ownership and symbol signatures.

### 4. Ambiguity example (should fail)

Module A:

```stasis
method normalize(self: Vec2): Vec2 { ... }
```

Module B:

```stasis
method normalize(self: Vec2): Vec2 { ... }
```

Use site:

```stasis
v.normalize();
```

Expected diagnostic:

- `Ambiguous method 'normalize' for receiver type 'Vec2'. Candidates: A.normalize, B.normalize. Hint: remove one import or call a qualified form if supported.`

Benefit:

- Hard fail preserves determinism.

Pitfall:

- Can be noisy in large module graphs unless disambiguation tools are ergonomic.

### 5. Multi-mutation operation with explicit intent (future syntax example)

```stasis
method swap(mut a: i32_ref, mut b: i32_ref): void {
    let t: i32;
    t = a.get();
    a.set(b.get());
    b.set(t);
}
```

Benefit:

- Demonstrates how the language can keep side effects explicit without banning useful patterns.

Pitfall:

- Depends on reference/intention syntax that is not part of minimal v1.

## Diagnostics to add (Elm-style)

1. Missing receiver parameter in declaration
- `Method 'name' must declare at least one parameter (receiver at parameter 0).`

2. Unknown method for receiver type
- `Type 'Enemy' has no method 'heal' with 1 argument. Hint: did you mean 'damage'?`

3. Ambiguous method
- `Call to 'normalize' on type 'Vec2' is ambiguous across imports.`

4. Non-callable member misuse
- `'.field(...)' is not callable. Hint: remove '()' or call a declared method.`

5. Extern body mismatch
- `Extern method 'foo' cannot have a body.`

6. Method body missing without extern
- `Method 'foo' is missing a body. Add a body or mark it extern.`

## Parser/compiler implementation sketch

### Parser

- Add `method` keyword token.
- Add `ParseMethod()` mirroring `ParseFunction()` shape.
- Add `MethodDeclarationSyntax` (or unify under callable declaration with a `Kind`).

Benefit:

- Small parser diff.

Pitfall:

- If AST split is too rigid, this can ripple through visitors/pattern matches.

Mitigation:

- Prefer shared callable abstractions where feasible.

### Semantic analysis

- Build method table keyed by `(receiver type, method name)`.
- Add member-call binding path for general methods (keep `clear()` handling defined and explicit).
- Enforce no-overload and ambiguity rules.

Benefit:

- Keeps all dispatch static and analyzable.

Pitfall:

- Interaction with module imports and existing symbol table scoping needs careful design.

### Lowering

- Desugar bound method calls to receiver-first function calls before backend-specific lowering.
- Reuse existing function call paths in LLVM and Cranelift backends.

Benefit:

- Limits backend churn.

Pitfall:

- Debug info/diagnostic spans must still point to original method call syntax.

## Migration plan

1. Implement method declarations + binding.
2. Enable method-call lowering for non-special methods.
3. Keep free functions fully supported.
4. Optionally allow function-form method calls during migration.
5. Add lints encouraging method form for receiver-centric APIs.

Benefits:

- Non-breaking adoption.

Pitfalls:

- Transitional dual style can cause style inconsistency.

Mitigation:

- Add formatter/lint guidance and examples in docs.

## Test plan (minimum)

1. Parse
- Method decl with body.
- Extern method decl with semicolon.
- Method decl missing receiver parameter (diagnostic).

2. Semantic
- Successful bind for `recv.m(args)`.
- Unknown method diagnostic.
- Ambiguous method diagnostic.
- Arity/type mismatch diagnostics.

3. Lowering
- Method call lowered as receiver-first function call shape.
- Extern method uses expected link name.

4. Regression
- Existing function calls unchanged.
- Existing operator-method behavior unchanged.
- Existing `clear()` behavior unchanged until intentionally generalized.

## Open questions

1. Do we permit qualified method calls for ambiguity resolution (`Module.method(recv, ...)` or similar)?
2. Should methods be allowed on primitive types immediately, or only structs/string/buffers first?
3. Do we reserve a small set of method names (`clear`, `memoryOffset`) as compiler special forms, or migrate all to normal methods over time?
4. Should tests support method declarations identically to function declarations from day one?

## Recommendation

Adopt this minimal v1 shape first:

- `method` declarations with receiver as parameter 0.
- Static, deterministic member-call binding.
- Receiver-first desugaring into current function ABI/lowering.
- Extern methods via existing attribute/link-name pattern.

Then iterate on receiver intent markers and disambiguation ergonomics once usage data arrives.
