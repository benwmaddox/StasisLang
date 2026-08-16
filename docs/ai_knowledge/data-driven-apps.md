# Data-driven applications

<!-- tags: data-driven, tables, configuration, records, state, references -->

## Split policy from data

Keep stable rules in code and variable content in data-shaped records or tables.
Use configuration for deployment/workshop choices, tables for repeated domain
content, and runtime state for facts that change while the program runs.

| Kind | Examples | Mutability |
| --- | --- | --- |
| Configuration | Bounds, feature switches, asset names, limits | Loaded at startup or rebound at a defined boundary |
| Table data | Levels, products, cards, enemy definitions, menu items | Read by rules |
| Runtime state | Selection, progress, score, cooldown, active rows | Mutated by update |
| Derived view | Labels, filtered rows, sprite placement | Recomputed for render |

Source: `samples/typed_sprite/`,
`docs/android_workshop_game_architecture_recommendations.md`,
`docs/android_workshop_codex_harness.md`, `samples/brickout_revenge/`.

Development hot reload may rebind configuration between ticks. Capture the
exact data bytes or disable rebinding for deterministic replay; iteration-time
reload and strict replay are different operating modes.

## Table shape

Give each row a stable key and keep references explicit. Validate required fields
and ranges at the boundary where data enters the program.

**Pseudocode:**

```text
item_table = [
  { id: "starter", label: "Starter", price: 0, enabled: true },
  { id: "pro", label: "Pro", price: 10, enabled: true }
]

state = { selected_id: "starter", query: "", page: 0 }
```

The example is **pseudocode**: adapt record syntax to the cited repository
language/source before compiling. The important contracts are stable `id`,
separate runtime selection, and a renderable projection.

## Update and render

```text
command -> validate command -> update state -> derive visible rows -> render
```

Do not store a second mutable copy of the selected row unless the copy is an
intentional snapshot. Prefer `selected_id` plus lookup so edits to table data do
not silently diverge from selection state.

## Data validation checklist

- Every row has a unique stable key.
- References resolve before the first interactive tick.
- Numeric limits use one declared unit and boundary convention.
- Optional fields have an explicit default.
- Unknown IDs produce a controlled diagnostic, not a null cascade.
- Filters and pagination have bounded output and work.
- Serialized order is stable when it participates in hashes.

## Application pattern

For a CRUD-like screen, model commands such as `select(id)`, `set_query(text)`,
`next_page`, and `save`. Apply them to state through one update path; let the UI
render state and dispatch commands. This makes the same data model usable in a
headless harness and a renderer.

Source: `samples/headless_scenario/`, `samples/state_inspection/`,
`docs/live_cli_workspace.md`.
