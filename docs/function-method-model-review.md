# Function Model Review: Top-Level Functions vs Methods-First

Date: 2026-02-09

Assumption for this revision: method calls are first-parameter-centric (`x.m(y)` => function-form call with `x` as parameter 0).

## What exists today

Current behavior is a mixed model:

- Declarations are top-level `function` declarations (`docs/spec.md`, section 9).
- External symbols are top-level `extern function ...;` or `function @extern ...;`.
- Expressions support method-like operator calls (`x.+(y)`, `x.==(y)`) plus special forms like `clear()` on receivers.
- Parser syntax allows general member/call chains, but semantic/codegen behavior is largely function-call-centric:
  - Regular calls are expected to be identifier callees (`foo(...)`).
  - Member-call lowering is effectively special-cased for `clear()` today.

So the language *looks* partially method-oriented in expressions, but declaration/lookup/lowering is still mostly free-function based.

## Your proposed direction

"Treat functions as methods always" means every callable is attached to a type, including host/extern entry points.

Conceptually:

- `damage(enemy, amt)` becomes `enemy.damage(amt)`.
- Host APIs become methods too, potentially on dedicated host/system types.
- "Extension" style methods can be declared outside the type's file.

## Pros of methods-first

- Consistent mental model with existing operator-method style.
- Better API discoverability by type (`Enemy.*`, `utf8.*`, `InputSnapshot.*`).
- Reduces global-name pressure from imports because lookup can be receiver-typed first.
- Aligns nicely with SoA/AoS usage style where data is central and behavior hangs off data views.
- Gives a clean path for host APIs as explicit typed surfaces instead of a large flat `sys_*` namespace.

## Cons / risks of methods-only

- Some operations are not naturally "owned" by a type (math helpers, process/global orchestration, test helpers).
- Extension methods across modules can create ambiguity and conflicts if two imports define the same method for the same receiver type.
- Compiler complexity increases:
  - Receiver-based lookup and ambiguity diagnostics.
  - Call rewriting and symbol mangling conventions.
  - Better type inference/validation at call sites.
- ABI complexity for extern methods (receiver passing convention, link-name stability, host binding expectations).
- Mutability can become less explicit unless receiver intent is encoded (`mut`, `in`, etc.), which matters for deterministic reasoning.

## Key design decisions to settle first

1. Receiver ownership/coherence rule.
   - Can any module add methods to any type, or do you enforce an "owner" rule?
2. Mutability contract.
   - How does a method communicate whether it mutates receiver-backed storage?
3. Non-owned behavior.
   - Do "utility" operations still exist as free functions, or do you introduce namespace/pseudo-types (`Math`, `Sys`, `Io`)?
4. Extern ABI mapping.
   - Lock `receiver.method(a, b)` to a receiver-first ABI call shape (`symbol(receiver, a, b)`), with stable link-name control.
5. Import conflict behavior.
   - What happens when two modules provide identical method signatures on the same receiver type?

## Recommendation

Use a **hybrid transition**, not a hard methods-only cutover.

- Keep free functions valid.
- Add first-class method declarations and method-call resolution.
- Lower methods to the existing function ABI using receiver-first call shape (`Type_method(receiver, ...)`) to preserve backend stability.
- Allow extern methods with explicit link-name attributes.
- Re-evaluate "methods-only" only after real-world usage data.

This preserves current compiler architecture while giving you the ergonomic and organizational wins you want.

## Suggested migration plan

1. **Call resolution stage**
   - Implement regular member-call lowering (`obj.method(args)`) by desugaring to receiver-first function form.
2. **Method declaration syntax**
   - Add method declarations tied to receiver type and compile to mangled function symbols.
3. **Extern methods**
   - Support extern methods with explicit stable link names.
4. **Conflict/coherence diagnostics**
   - Add Elm-style actionable errors for ambiguous or conflicting methods.
5. **Gradual deprecation (optional)**
   - If desired later, lint against new free-function declarations instead of breaking immediately.

## Concrete guardrails for Stasis constraints

- No hidden allocation/copying in receiver passing.
- Receiver mutability must be explicit in syntax/semantics.
- Deterministic name resolution order must be specified.
- Tree-shaking/reachability should treat desugared methods and free functions identically.

## Bottom line

The direction is strong and matches Stasis' existing "behavior-on-data" feel. The risky part is not syntax; it is coherence, ambiguity, and ABI stability. A staged hybrid model gives you the upside without destabilizing the compiler/runtime contract.

## Additional consistency checks with Stasis principles

This section focuses on whether methods-first is philosophically and mechanically consistent with Stasis as currently defined.

### 1. Static global memory and no hidden allocation

- Good fit if method calls are pure syntax over existing storage (no implicit object allocation, no hidden copies).
- Risk appears when receiver passing semantics are unclear; avoid value-copy receiver defaults for large structs/arrays.
- Recommendation: define method receiver lowering explicitly as address/index-based where applicable, aligned with current global-memory model.

### 2. AoS syntax -> SoA storage

- Strong fit: methods on struct views can improve ergonomics without changing SoA internals.
- Important guardrail: method bodies must not imply AoS materialization.
- Recommendation: specify that receiver access still lowers through existing flattened/SoA paths; methods are naming/dispatch sugar, not storage-model changes.

### 3. Determinism and explicit side effects

- Potentially positive if mutating APIs become clearer by receiver (`state.apply_damage(...)`).
- Potentially negative if method resolution becomes import-order-sensitive or ambiguous.
- Recommendation: lock deterministic method lookup order and reject ambiguities with hard diagnostics (not "first match" behavior).

### 4. Operator-method heritage

- Very consistent with current language identity: arithmetic/comparison already has method form.
- Extending the model to normal callables can make the language feel less split-brain (today declarations are free-function-centric).
- Recommendation: keep infix arithmetic/comparison as primary style while preserving operator-method compatibility.

### 5. Extern/host boundary clarity

- Methods can improve host API readability if attached to explicit host-facing types instead of many global `sys_*` symbols.
- ABI risk is real unless lowering rules are fixed and simple.
- Recommendation: extern methods should always lower to stable symbol calls with explicit link-name controls, and receiver position in ABI should be fixed (for example, first parameter).

### 6. Elm-style diagnostics and developer UX

- Methods-first introduces new error classes (ambiguous extension, missing receiver method, wrong receiver mutability).
- This is still consistent with Stasis goals if diagnostics stay specific and actionable.
- Recommendation: add diagnostics that suggest disambiguation imports or explicit fully qualified calls when conflicts occur.

### 7. Simplicity and implementation pragmatism

- Full methods-only immediately is less consistent with current compiler architecture, which is identifier-call centric.
- Incremental desugaring is more consistent with Stasis' practical, deterministic engineering style.
- Recommendation: treat method syntax as front-end sugar that reuses current symbol/lowering pipelines first, then evolve internals after stability.

## First-parameter-centric method model

Assumption: most methods operate on the first parameter or return data derived from it.

That maps cleanly to a simple rule:

- `receiver.method(a, b)` desugars to `method(receiver, a, b)`.
- The receiver is always ABI parameter 0.
- Resolved method symbols may still be mangled (`Type_method`) for uniqueness/linking.
- Methods that "query" state can return any value type; methods that mutate can return `void` (or explicit status values where needed).

### Why this is a strong fit

- Minimal backend change: existing function call/lowering pipeline can stay mostly intact.
- Clear host interop: extern methods are just extern functions with receiver-first ABI.
- Preserves deterministic/static dispatch: no dynamic method tables required.
- Aligns with existing operator-method behavior where the left side is already the receiver.

### What to watch carefully

- Symmetric operations (`distance(a, b)`) may look arbitrary if one operand is forced as receiver.
- Utility/global orchestration APIs still need a home (free function or namespace-like type).
- Extension method conflicts remain a major ambiguity source.
- Without explicit receiver intent, mutating vs read-only methods can become unclear.

### Recommended rules under this assumption

1. Desugaring contract
- Method syntax is purely surface sugar over first-parameter functions.
- No semantic difference between `x.m(y)` and the resolved receiver-first function call after name resolution.

2. Receiver matching
- Method resolution uses exact receiver type (plus explicitly specified compatibility rules only).
- Avoid implicit numeric/type coercions during method lookup.

3. Receiver intent
- Add explicit receiver mode in method declarations (for example read-only vs mutating).
- Diagnostics should state when a mutating method is called on a non-assignable/non-mutable receiver.

4. Extern ABI stability
- Extern method linkage uses stable symbol naming and fixed receiver-first calling convention.
- Link-name overrides (`@extern("...")`) remain the authority for host symbol binding.

5. Ambiguity policy
- If multiple imported methods match `(receiver type, method name)`, emit a hard ambiguity error with disambiguation guidance.
- Do not use import order as a tie-breaker.

### Practical implication

If this is your default model, Stasis can stay architecturally simple: methods become ergonomic syntax over the current function ABI, while still feeling consistent with operator-method language identity.

