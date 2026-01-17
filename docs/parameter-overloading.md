# Parameter Overloading (First-Argument Dispatch) - Design Options

This note explores relaxing Stasis's current "no overloading" rule to enable
ergonomic, type-safe APIs such as `target.from_ascii(source)` for string
buffers, without making unrelated conversions ambiguous or "too implicit".

Context:
- `docs/spec.md` currently states: "No overloading."
- Stasis also emphasizes explicit boundaries between `ascii[N]` and `utf8[N]`.

## Goals

- Enable ergonomic, readable conversions scoped to a destination type, e.g.:
  - `dst_utf8.from_ascii(src_ascii)`
  - `dst_ascii.from_utf8(src_utf8)` (if permitted)
- Keep compilation deterministic (no runtime dispatch) and diagnostics crisp.
- Avoid "surprising conversions" where a single name like `from_ascii` starts
  applying to many unrelated destination types.

## Non-goals

- General-purpose overloading comparable to C++/C#.
- Implicit conversions between `ascii[N]` and `utf8[N]`.
- Return-type-driven resolution.

## The motivating example

We want this to make sense:

```stasis
let src: ascii[32];
let dst: utf8[64];

dst.from_ascii(src);
```

But we do not want `from_ascii` to become a "magic" conversion that (for
example) might also "convert ASCII to i32" or "ASCII to Enemy" depending on
what overloads exist.

## Option A: Keep "no overloading" (status quo) + explicit names

Example:

```stasis
utf8_from_ascii(dst, src);
ascii_from_utf8(dst, src);
```

Pros:
- Simple name resolution; aligns with the current spec.
- No ambiguity; easy to lower and mangle.

Cons:
- Verbose and less discoverable (harder to "see" what belongs to `utf8`).
- Doesn't support method-call ergonomics.

## Option B: Method-call sugar (receiver dispatch) with unique function names

Introduce method-call sugar that desugars:

```stasis
dst.from_ascii(src)
```

Into a uniquely named function call determined by the receiver type:

```stasis
utf8_from_ascii(dst, src)
```

Key property: there are still no overload sets. This is not "overloading";
it's a rewriting rule ("dot-call desugaring") that maps a method name to a
fully qualified function name chosen by the compiler based on the static type
of `dst`.

Pros:
- Keeps the "no overloading" rule intact.
- Strongly controls which types can have `from_ascii`.
- The receiver type is known at the call site; no ambiguity.

Cons:
- Requires adding/standardizing method-call syntax and lowering support.
- Needs a mapping rule from receiver type to a function symbol.

Notes:
- The parser already supports `a.b(c)` syntactically, but the current lowering
  path in the compiler primarily supports calls where the callee is a simple
  identifier. This option implies implementing a desugaring phase (AST rewrite)
  before lowering, or extending lowering to handle member-callee calls.

## Option C: First-argument overloading only (first-argument dispatch)

Allow multiple functions with the same name, but only if they differ by the
type of the first parameter.

This makes the "receiver dispatch" rule explicit at the function level:

```stasis
function from_ascii(dst: utf8[64], src: ascii[32]): bool { ... }
function from_ascii(dst: ascii[64], src: utf8[32]): bool { ... }
```

Then, if method-call sugar exists:

```stasis
dst.from_ascii(src)
```

Desugars to:

```stasis
from_ascii(dst, src)
```

And overload resolution picks the `from_ascii` variant based on `dst`'s type.

Even without method-call sugar, this still helps:

```stasis
from_ascii(dst, src);
```

### Proposed rules (tight, deterministic)

1. Overload sets are formed by name *within a module scope*.
2. Resolution uses:
   - exact match on arity first,
   - then exact match on first-parameter type.
3. If multiple candidates match the first-parameter type (e.g., via aliases),
   the call is ambiguous and is an error.
4. If no candidate matches the first-parameter type, it is an error.
5. The remaining parameters must match exactly (or match existing literal
   compatibility rules already used elsewhere, e.g. numeric literal fitting).
6. No return-type-based selection.

### Proposed "what counts as a type match"

To keep resolution deterministic and predictable, consider "match" to mean:

- Exact type identity after alias expansion:
  - `string` is an alias for `utf8` (and possibly `utf8[N]` depending on rules)
  - module identity matters (no structural typing)
- For arrays: element type + length are part of the identity (`utf8[64]` is not
  the same as `utf8[32]`).

If we want `from_ascii` to work across capacities, we probably want a separate
type concept for "any capacity", such as slices/views. Otherwise you end up
needing many overloads just for each `N`.

### Pros

- Still tightly constrained compared to general overloading.
- Works well with "receiver as first arg" conventions.
- Prevents accidental overloading on "conversion input types" (2nd, 3rd, ...)
  which is where conversions often become unclear.

### Cons / pitfalls

- `utf8[N]` vs `utf8[M]` means capacity is part of the type; naive overloading
  would not scale unless the language also has a slice/view type for string
  buffers.
- Imports + modules need a clear rule for overload sets (per-module? merged?).
- Diagnostics must explain *which overloads were considered* and why none/too
  many matched.

## Option D: General overloading (multiple parameters)

Allow overloads by any parameter types.

Pros:
- Maximum flexibility; common in many languages.

Cons:
- Harder to keep deterministic, especially with:
  - literal typing,
  - aliases (`string`),
  - future generics,
  - implicit coercions (even if Stasis tries to avoid them).
- Higher ambiguity risk, worse diagnostics, more complex implementation.

Given Stasis's focus on explicitness and predictable lowering, this option is
least aligned with current design goals.

## Recommendation for string conversion ergonomics

If the primary driver is `ascii[N]` <-> `utf8[N]` conversion APIs:

1. Prefer Option B (receiver-dispatch method sugar) if we want to keep the spec
   rule "No overloading" intact.
2. If we do want to relax the rule, Option C (first-argument dispatch) is the
   narrowest overloading model that still supports `dst.from_ascii(src)` via
   `from_ascii(dst, src)` desugaring.

To avoid a combinatorial explosion of overloads by capacity (`N`), introduce or
standardize view/slice types for string buffers (byte slice + length metadata),
then place conversion APIs on those types.

## Open questions

- Should overload sets be module-local, or can imports merge overloads?
- How should `string` aliasing interact with first-argument matching?
- Do we want to permit overloads that differ only by `utf8[N]` capacity, or
  require views/slices for capacity-agnostic APIs?
- Should there be an explicit disambiguation syntax if two overloads collide?
  (Example: `from_ascii::<utf8[64]>(dst, src)` would add generic syntax, which
  likely conflicts with current simplicity goals.)

