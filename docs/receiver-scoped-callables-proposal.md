# Receiver-Scoped Callables Proposal

Date: 2026-02-09
Status: Draft proposal

## Intent

Keep function declarations as the core model, but resolve name conflicts by receiver type at call sites.

- Allow both forms:
  - Receiver form: `enemy.damage(5)` (recommended)
  - Function form: `damage(enemy, 5)` (supported)
- Same callable name may exist for different receiver types without conflict.

## Core rule

A callable is identified by:

- receiver type (parameter 0 type)
- callable name
- arity

Examples:

- `damage(self: Enemy, amt: i32)`
- `damage(self: Hero, amt: i32)`

Both are valid and distinct.

## Declaration shape

No new declaration keyword required if you keep `function`.

```stasis
function damage(self: Enemy, amt: i32): void {
    self.hp -= amt;
}

function damage(self: Hero, amt: i32): void {
    self.hp -= amt;
}
```

Optional future syntax sugar:

```stasis
method damage(self: Enemy, amt: i32): void { ... }
```

Not required for v1.

## Call resolution

For receiver form:

```stasis
enemy.damage(5)
```

Resolution steps:

1. Resolve `enemy` type (`Enemy`).
2. Find callables named `damage` whose parameter 0 type is `Enemy`.
3. Filter by arity/type of remaining arguments.
4. Require exactly one match.

For function form:

```stasis
damage(enemy, 5)
```

Resolution steps:

1. Resolve first argument type (`Enemy`).
2. Find callables named `damage` whose parameter 0 type is `Enemy`.
3. Filter by remaining argument types.
4. Require exactly one match.

Determinism rule:

- Never tie-break by import order.

## Recommendation level

Language support:

- Receiver form and function form are both valid.

Style guidance:

- Recommend receiver form for receiver-centric operations.
- Keep function form for utility helpers and migration compatibility.

## Internal lowering

Both call forms lower to the same internal function symbol.

Example symbols:

- `Enemy_damage`
- `Hero_damage`

Desugaring equivalence:

- `enemy.damage(5)` == `damage(enemy, 5)` after binding.

## Conflict behavior

Valid:

- same name, different receiver type.

Invalid:

- same module scope defining duplicate `(receiver type, name, arity)`.

Ambiguous import case:

- if two imports define identical `(receiver type, name, arity)`, emit hard ambiguity diagnostic.

## Diagnostics

Unknown receiver method:

- `Type 'Enemy' has no callable 'heal' with 1 argument. Hint: did you mean 'damage'?`

Ambiguous callable:

- `Call to 'damage' for receiver type 'Enemy' is ambiguous across imports.`

Function-form non-receiver mismatch:

- `First argument of 'damage' determines receiver type. Expected 'Enemy', got 'Hero'.`

## Pros

- Eliminates verbose type-prefixed names in user code.
- Keeps current function-centric compiler architecture.
- Preserves deterministic static resolution.
- Smooth migration path with no immediate breakage.

## Cons

- Requires clearer diagnostics when both forms exist and imports clash.
- Function form can hide receiver-centric intent if overused.
- True method discoverability is weaker than explicit method declarations unless tooling surfaces receiver-scoped callables clearly.

## Suggested rollout

1. Implement receiver-scoped resolution using parameter 0 type.
2. Keep both call forms valid.
3. Add lint: prefer receiver form when first parameter is a structured/game state type.
4. Update stdlib samples to receiver form for readability.

## Example

```stasis
struct Enemy { hp: i32; }
struct Hero { hp: i32; }

function damage(self: Enemy, amt: i32): void {
    self.hp -= amt;
}

function damage(self: Hero, amt: i32): void {
    self.hp -= amt;
}

function tick(enemy: Enemy, hero: Hero): void {
    enemy.damage(5);   // preferred
    damage(hero, 3);   // still valid
}
```
