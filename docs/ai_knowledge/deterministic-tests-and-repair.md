# Deterministic tests and repair

<!-- tags: deterministic, tests, scenarios, hashes, bounds, repair, parity -->

## Deterministic contract

For the same program, saved initial state, scenario, and exact tick count, a
headless run should produce the same inspected simulation state and
compiler-owned simulation hash. Render trace or capture evidence is a separate
check over the projected state.

```text
(program, saved state, scenario, tick count) -> simulation state + simulation hash
(simulation state, render configuration) -> render trace or capture evidence
```

This is a **pseudocode** contract. Use repository scenario and inspection
facilities for the executable form.

Source: `docs/spec.md`, `docs/toolchain_cli.md`, `samples/headless_scenario/`,
`samples/state_inspection/`, `docs/render_parity_gate.md`.

## Test layers

| Layer | Proves | Example evidence |
| --- | --- | --- |
| Parse/build | Syntax and types are valid | Build diagnostics are empty |
| State transition | Rules advance correctly | Inspection at a named tick |
| Bounds/performance | Work and collections stay bounded | `bounded_performance` result |
| Render parity | Projection matches expected presentation | `render_parity` result |
| End-to-end scenario | A bounded tick sequence reaches expected state | Headless scenario receipt |

Source: `samples/bounded_performance/`, `samples/headless_scenario/`,
`docs/render_parity_gate.md`.

## Invariants worth naming

- One logical input is consumed at most once per tick.
- One update tick applies one declared timing policy.
- Positions and velocities remain finite and within declared bounds.
- A removed entity is not updated or rendered later in the same pass.
- A cooldown with a nonnegative contract is clamped at zero.
- A terminal phase does not continue spawning or scoring.
- State inspection and rendering read the same authoritative state.

Write invariants as assertions or scenario expectations where the repository
harness supports them. Give each assertion a stable label so a failure points to
the rule rather than only to a line number.

## Bounded work

Avoid a repair that merely makes one case pass by adding unbounded retries,
unbounded entity growth, or repeated full-world scans. Put explicit ceilings on
work, collection size, retries, and scenario steps. If a bound is reached, return
a diagnosable failure state.

Source: `samples/bounded_performance/`.

## Repair loop

1. Reproduce from the same initial state and input trace.
2. Find the first tick where inspected state diverges.
3. Compare the transition inputs and authoritative state at that tick.
4. Fix the smallest boundary or invariant violation.
5. Rerun state checks, then bounds, then render parity, then the full scenario.
6. Preserve the regression scenario and expected hashes/reference IDs.

## Hash discipline

Keep hash domains distinct. A semantic `source_hash` protects the exact source
being edited. A headless simulation hash covers the compiler-owned simulation
state selected by that harness. Render parity uses render traces or captures,
not the source or simulation hash. Within each domain, use a canonical
representation with stable field order and explicit inclusion/exclusion rules.
A changed hash localizes a difference; structured evidence explains it.

Source: `docs/semantic_edit_protocol.md`, `docs/spec.md`,
`samples/state_inspection/`, `docs/render_parity_gate.md`.
