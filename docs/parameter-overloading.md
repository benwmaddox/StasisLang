# Parameter Overloading (First-Parameter Only?)

This note explores relaxing Stasis' current "no overloading" rule to support ergonomic receiver-style calls like `target.from_ascii(source)` without introducing broad, ambiguous overload resolution.

## Motivation

Stasis already leans on receiver-style operator methods (e.g. `a.+(b)`, `a.==(b)`), and it also has library functions where the "destination" is the natural receiver:

- `from_ascii(dst: utf8[], src: ascii[], dst_max: i32): i32`

It would be nice if this could be written as:

- `dst.from_ascii(src, dst_max)`

The key constraint: avoid surprising conversions or "messing up converting to other types". In practice this means:

- No implicit conversions during overload selection.
- Clear, deterministic resolution.
- Good diagnostics when ambiguous.

## Current Model (Baseline)

- A function name resolves to a single function symbol in scope (no overload sets).
- Dot-call syntax is used for operator-method style; any "method sugar" must not compromise determinism.

Pros:
- Simple name resolution and diagnostics.
- No ambiguity.

Cons:
- Libraries need long, type-specific names (`utf8_from_ascii`, `utf8_from_u32`, ...), or call sites look less natural (`from_ascii(dst, src, max)`).

## Option A: Keep "No Overloading" (Status Quo)

Make conversions explicit via naming and/or modules:

- `utf8_from_ascii(dst, src, max)`
- `utf8.from_ascii(dst, src, max)` (namespace/module qualifier, not overload)

If dot-call sugar exists, keep it purely syntactic:

- `dst.from_ascii(src, max)` rewrites to `from_ascii(dst, src, max)` and still requires a unique `from_ascii` in scope.

This is the safest option and keeps the compiler simple.

## Option B: First-Parameter Overloading for Dot-Calls Only

Allow multiple functions with the same name, but only participate in overload resolution when called via dot-call (receiver syntax), and only the first parameter is used to pick the candidate set.

### Rule sketch

Given a dot-call:

- `recv.name(arg1, arg2, ...)`

Lower to an overload search for `name(recv, arg1, arg2, ...)` with:

1. Candidate set = all functions named `name` visible in scope.
2. Filter by arity: must accept N+1 parameters.
3. Filter by exact type match on parameter 1 (the receiver parameter):
   - No numeric widening/narrowing.
   - No implicit pointer/slice conversions beyond what already exists as "exact".
4. After receiver filtering, do normal exact type checking for remaining parameters.
5. If exactly one candidate matches, select it; otherwise error.

### Example

Two functions can coexist:

- `from_ascii(dst: utf8[], src: ascii[], dst_max: i32): i32`
- `from_ascii(dst: ascii[], src: ascii[], dst_max: i32): i32`

Call sites:

- `utf8_buf.from_ascii(ascii_buf, 256)` selects the `utf8[]` receiver overload.
- `ascii_buf.from_ascii(ascii_buf2, 256)` selects the `ascii[]` receiver overload.

Non-dot call sites remain non-overloaded (or error if multiple exist):

- `from_ascii(...)` is either disallowed when overloaded, or requires disambiguation (see Option C).

### Why this fits the "first parameter" constraint

It keeps overload selection scoped to the receiver type, which is usually explicit at the call site and aligns with how operator methods feel today.

### Failure modes (diagnostics)

- No candidates: "no method `from_ascii` for receiver type `utf8[]` with parameters `(ascii[], i32)`".
- Multiple candidates after filtering: list candidates and suggest renaming or adding a module qualifier.

### Open questions

- How to handle methods on structs vs slices (`utf8[]` is a slice/pointer-like type)?
- Whether generics (if added) would interact with this rule.

## Option C: First-Parameter Overloading + Explicit Disambiguation for Non-Dot Calls

Same as Option B for dot-calls, but allow non-dot calls to resolve overloads via explicit qualification:

- `utf8.from_ascii(dst, src, max)` where `utf8` is a namespace/module/type qualifier.
- Or a cast-like qualifier: `from_ascii[utf8](dst, src, max)` (syntax TBD).

This keeps overload resolution deterministic while allowing call sites that cannot use dot-call (e.g., higher-order passing of a function).

## Option D: Full Overloading (Not Recommended Yet)

General overload resolution based on all parameters (and possibly literals) tends to require:

- A richer type-conversion story.
- More complicated and fragile diagnostics.
- More implementation surface area in semantic analysis.

Given Stasis' goals (determinism, explicit memory writes, minimal implicit behavior), this seems risky unless there is a strong need beyond receiver-style ergonomics.

## Recommendation (If Changing the Rule)

If we relax "no overloading", start with Option B:

- Only dot-call participates in overloading.
- Only the first parameter (receiver) is used to select candidates.
- All matches must be exact type matches (no implicit conversions).

This provides the desired ergonomics (`dst.from_ascii(src, max)`) while keeping resolution local, predictable, and easy to diagnose.

