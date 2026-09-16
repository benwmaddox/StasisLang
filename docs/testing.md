# Standard Stasis testing guidance

Set up the state you need, exercise the real behavior, advance deterministically,
and check the behavior that matters. This is the default style for gameplay and
system tests, using ordinary Stasis control flow and small receiver-form helpers.

A test is a specialized parameterless function. Its display name, discovery,
result interpretation, and exclusion from production builds distinguish it;
its body, calls, receiver-form helpers, and effect checking use normal function
semantics. Test tooling should preserve function attributes when preparing a test
for compilation rather than introduce a separate execution language.

The internal representation is an ordinary function with test metadata:

```stasis
function @test("enemy reaches target") @effects(state.world) __stasis_test_0(): string {
    ...
}
```

`@test(...)` here is generated metadata, not a new required source spelling. The
display name stays separate from the internal function symbol so labels do not
need to be valid identifiers and cannot collide through identifier conversion.

## Language support and test results

See [the testing construct in the spec](spec.md#10-testing-construct) for current
language semantics. Tests accept parameterless `bool` or `string` results:
`true` passes and `false` fails for bool tests. Tests can use function attributes,
including `@effects(...)`, through ordinary function compilation. The scenario
examples below require project-defined structs and helpers; they are not
self-contained programs.

For string-result tests, the contract is an empty string for success
and a non-empty string describing the first meaningful failure. Prefer explicit
control flow rather than a mandatory assertion framework:

```stasis
if (actual != expected) {
    return "describe the violated behavior";
}

return "";
```

Failure messages should explain intent, such as "enemy should be defeated after
attack resolves", rather than only "expected 0". Include exact expected values
when they explain a contractual requirement. No new assertion syntax or scenario
DSL is required by this standard.

## Setup, action, progression, observation

Keep each test focused on one behavior and its first useful failure reason:

1. **Setup:** establish a small, explicit starting state. Direct assignments are
   appropriate here; do not simulate minutes of unrelated gameplay to create a
   low-health enemy. Use named helpers when they explain nontrivial setup.
2. **Action:** exercise the normal semantic path used by the system. A combat
   test should queue or apply the real attack, rather than subtract HP or write
   the expected death state itself.
3. **Progression:** run a bounded, deterministic number of authoritative updates
   or advance to a condition with an explicit maximum step count.
4. **Observation:** read resulting state directly or through read-only helpers.
   Check meaningful behavior, transitions, and invariants.

Setup creates the starting state. Actions exercise the behavior being tested.
Tests belong in focused `.test.stasis` files near their implementation when
practical. Helpers should stay close to the system they describe.

## Effect contracts and receiver-form helpers

Use the narrowest practical `@effects(...)` contract on tests and test helpers.
For example, `state.world.actors` permits writes to
that collection and its nested fields; `state.world` permits the whole world.
`@effects()` allows reads and local mutation but rejects global mutation and
host effects. It is a restriction on effects, not on reads.

Effects are compiler-enforced boundaries, not comments or runtime assertions.
The outer test contract is authoritative over the complete resolved call tree,
including unannotated, imported, and receiver-form helpers. A helper's own
contract adds a local guarantee and a diagnostic boundary when it evolves.
Parameter-relative writes are checked against the actual caller's state region.
See [effect contract semantics](spec.md#78-opt-in-effect-contracts).

Illustrative helper declarations, with game-specific bodies omitted:

```stasis
function @effects(state.world) setup(self: CombatTest): void { ... }
function @effects(state.world) attack(self: CombatTest): void { ... }
function @effects() enemy_dead(self: CombatTest): bool { ... }
function @effects() player_xp(self: CombatTest): i32 { ... }
```

Receiver-form helpers can use existing game structs or a small test-specific
struct holding entity indexes, identifiers, handles, initial comparison values,
and bookkeeping. They should not duplicate the game world. A small vocabulary
such as `s.setup()`, `s.attack()`, and `s.enemy_dead()` is often enough; direct
reads and assignments remain appropriate for simpler tests.

Do not routinely permit `graphics`, `audio`, `storage`, `platform`, or unrelated
global state in a world-only test. Broader contracts need a reason tied to the
behavior being verified.

## Simulation steps and complete ticks

A project-local `s.step(n)` should mean exactly n calls to the authoritative
simulation update, with known inputs and the normal system order. It must not
mean elapsed wall time or an approximate number of rendered frames. Document
what it calls and which lifecycle work it omits. These helpers are conventions,
not new standard-library APIs.

When input edges, `HostFrame` refresh, lifecycle boundaries, or host behavior
matter, use tooling or a helper that performs the complete relevant tick
protocol. Calling a simulation update alone does not prove that protocol.
Likewise, a helper named `tick(n)` must state whether it calls the game entry
function or drives a full host tick. Rendering is separate from simulation;
graphics claims require appropriate integration tests and inspected captures.

## Choose expectations that express the contract

Prefer "actor reached destination", "projectile hit enemy", "reward granted
once", or "rejected purchase preserved state" over arbitrary intermediate
coordinates, animation counters, cache contents, or entity indexes. A valid
implementation should be able to change those details while preserving behavior.

Exact values are appropriate when the value itself is the contract: a 50-gold
price, 12 damage, 15 XP, a 30-tick cooldown, capacity 64, a defined fixed-point
result, or a transition on an exact boundary tick. Equality with a captured
initial value is also meaningful when rejection must preserve that value.

Check transitions as well as end states: alive before an attack, dead after it
resolves, then rewarded and cleaned up. Cover before/after boundaries and
adjacent values, one-time actions, idempotence, deterministic tie-breaking,
full capacity, and rejected actions leaving relevant state unchanged.

For ordered systems such as movement -> collision -> damage -> death -> reward
-> cleanup, observe the relevant stage boundaries when order is contractual.
Checking only that reward and cleanup both happened at the end does not prove
which happened first. Use the real stage entry points or observable evidence
that would differ if order were wrong; do not merely assert private helpers were
called.

## Examples of string-result tests

These examples assume project-defined scenario structs and helpers.

```stasis
test @effects(state.world) `enemy reaches target`(): string {
    let s: MovementTest;
    s.setup();
    s.move_enemy_to_target();
    s.step(30);

    if (!s.enemy_at_target()) {
        return "enemy should reach target";
    }
    return "";
}
```

Here the exact reward is the rule, and the second step checks it is not repeated:

```stasis
test @effects(state.world) `slime awards 15 xp once`(): string {
    let s: CombatTest;
    s.setup();
    let xp_before: i32 = s.player_xp();
    if (!s.enemy_alive()) {
        return "slime should begin alive";
    }

    s.attack();
    s.step(1);
    if (!s.enemy_dead()) {
        return "slime should die after the lethal attack resolves";
    }
    if (s.player_xp() != xp_before + 15) {
        return "slime death should award exactly 15 xp";
    }

    s.step(1);
    if (s.player_xp() != xp_before + 15) {
        return "slime death should not award xp twice";
    }
    return "";
}
```

Configure `CombatTest.setup()` for a lethal normal attack; define `enemy_dead()`
to remain meaningful after cleanup, for example using a retained death event.

```stasis
test @effects(state.world) `unaffordable purchase preserves state`(): string {
    let s: ShopTest;
    s.setup_without_enough_gold();
    let gold_before: i32 = s.gold();

    s.buy_sword();
    s.step(1);
    if (s.gold() != gold_before) {
        return "rejected purchase should not spend gold";
    }
    if (s.has_sword()) {
        return "rejected purchase should not grant a sword";
    }
    return "";
}
```

The setup must establish that no sword is initially owned. Also check any other
state the rejection contract promises to preserve, such as inventory or stock.

## Granularity and agent guidance

Pure/helper tests cover calculations, conversions, predicates, and algorithms.
Prefer `@effects()` where possible; exact transformation results are often their
contract. System tests cover one system or a small cooperating group through
setup -> action -> deterministic progression -> observation. Integration/runtime
tests cover full ticks, input edges, graphics, assets, platform services, or
other host interaction and may require broader effects and specialized tooling.
See [integration seam testing](integration_seam_testing_strategy.md) for that work.

For agents, explicit starting state, a small receiver vocabulary, deterministic
progression, enforceable effect boundaries, and concise failure feedback reduce
the architectural decisions needed to produce a useful test. They give humans
the same correctness and maintainability benefits. An allowed world-only call
tree cannot quietly expand into storage, audio, or unrelated global writes.

Avoid large generic test frameworks, recreating excessive gameplay history,
writing the expected result during the action phase, asserting private call
structure, and broad effects without a behavioral reason. Keep control flow
visible and test the observable completion condition.
