# Focused behavioral tests

Tests are specialized parameterless functions with display names, discovery,
test-result interpretation, and exclusion from production builds. Their bodies,
calls, and effect checking should use normal function machinery.

Use setup -> action -> deterministic progression -> observation. Setup may
directly establish minimal state; the action should exercise the real system
path rather than write the expected result. Keep tests in `.test.stasis` files
near the behavior and use small receiver-form helpers when they clarify intent.
Scenario structs should hold handles, indexes, initial comparison values, and
small bookkeeping rather than duplicate world state.

Use narrow function-level `@effects(...)` contracts on helpers. `@effects()`
permits reads and local mutation while rejecting global writes and host effects.
Contracts cover the complete resolved call tree; helper contracts add local
guarantees. Use the narrowest practical contract on tests as well as helpers.

Tests accept parameterless `bool` or `string` results and ordinary function
attributes including `@effects(...)`. For bool results, `true` passes and
`false` fails. For string results, prefer explicit first-failure control flow:

```stasis
if (condition_is_wrong) {
    return "describe the violated behavior";
}
return "";
```

An empty string means success; a non-empty string describes failure. Test tooling
lowers source tests to ordinary functions with generated internal names and
`@test("display name")` metadata while preserving their attributes and bodies.

Define `step(n)` as exactly n authoritative simulation updates with known inputs
and normal system order. These are project-local helpers, not new simulation
APIs. Complete input-edge, `HostFrame`, and lifecycle claims need full tick
tooling; simulation steps alone do not prove host behavior.

Prefer semantic expectations such as reaching a target, reward granted once,
or rejected actions preserving state. Assert exact values when price, damage,
reward, capacity, duration, or a boundary tick is contractual. Cover before/after
transitions, adjacent boundary values, ordering, idempotence, deterministic
tie-breaking, and full capacity. End-state checks alone cannot prove stage
ordering; observe meaningful boundaries or evidence that distinguishes the order.

Pure tests often have exact calculation contracts. System tests should focus on
one system or a small group. Integration tests cover host services, assets,
graphics, and full ticks and may need broader effects or inspected captures.

Avoid arbitrary intermediate coordinates, private-helper call assertions,
excessive gameplay history, broad effects without reason, and generic mandatory
assertion frameworks or scenario DSLs. Explicit state, deterministic completion,
small helper vocabulary, enforceable effects, and useful failures help both
humans and agents write maintainable tests.

The canonical extended standard lives at `docs/testing.md` in the StasisLang
source repository; this page is the offline project-local reference.
