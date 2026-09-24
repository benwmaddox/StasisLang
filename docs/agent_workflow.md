# Stasis agent workflow

Use the installed `stasis` CLI from the directory containing `stasis.json`. Do not invoke Cargo
for normal project work.

Read `PROJECT_ARCHITECTURE.md` before structuring game code. Use its input, tick, state, and
rendering boundaries as the default unless the project documents a concrete reason to differ.

## Quick task loop

1. If the target is unknown, start with `stasis --json symbol list`. If its file is known,
   start with a file-scoped list; if the exact target is established, read it directly.
   Inspect references before changing behavior.
2. Use semantic edits with complete declarations. For an update or delete, copy the current
   `source_hash`; for a schema-v2 batch, copy both `symbol_id` and `name` from `symbol read`.
3. Review the plan and retain the successful receipt. Run `stasis fmt --check`,
   `stasis check`, and `stasis test`; add fresh runtime validation when observable
   behavior changes.

For related edits, use `stasis symbol apply --request REQUEST.json`. This v2 example uses
the `symbol_id` and hash returned by `symbol read tick --kind function --file src/main.stasis`.
Adapt the full replacement to preserve the existing declaration, including attached comments and
attributes:

```json
{
  "schema_version": 2,
  "edits": [
    {
      "operation": "update",
      "target": {
        "kind": "function",
        "file": "src/main.stasis",
        "name": "tick",
        "symbol_id": "SYMBOL_ID_FROM_READ"
      },
      "expected_source_hash": "HASH_FROM_SYMBOL_READ",
      "new_source": "function tick(): i32 {\n    return 2;\n}"
    }
  ]
}
```

For an add with no known ID, explicitly use schema v1 for the whole batch and select by `name`,
`kind`, and `file`; never invent an ID. An omitted `schema_version` defaults to v2.
Use `--no-tests` only when the user explicitly asks for it.

## Offline vendor documentation

Generated projects keep the selected toolchain's documentation beside its standard library at
`vendor/stasis/docs`. Read `vendor/stasis/docs/README.md` for the offline project-local knowledge
library; its examples and guidance are available without a network connection, and source imports
continue to use `vendor/stasis/stdlib`.

The documentation and standard library are one vendor snapshot. `stasis vendor update` replaces
both directories and their manifest release ID and hash in one transaction. Automatic vendor
synchronization uses that same transaction, so it repairs missing or stale documentation together
with the standard library. Stasis owns `vendor/stasis`; use the update command to repair it rather
than editing the snapshot by hand. New vendor pins use hash version 2, which treats CRLF and LF as
equivalent for valid UTF-8 Stasis source, Markdown, JSON, and SVG text without NUL bytes. It still
detects other text changes, added or missing paths, and exact binary byte changes. Older manifests
without a hash version use the legacy raw-byte contract; a converted tree is accepted only when its
release baseline can be authenticated. Otherwise `stasis vendor status` and read-only symbol queries
report the pin as unverified without claiming local edits or modifying files. Mutating project
commands retain automatic synchronization and may replace a stale or unverified vendor snapshot
with the selected release; a clean, verified same-release v1 pin remains untouched. An explicit
`stasis vendor update` migrates even a clean v1 pin to version 2.
Generated Git and editor settings keep text at LF by default (with CRLF for Windows batch files);
status remains stable if another checkout converts text to CRLF.

## Theory-building practice

- Treat programming as building and maintaining an explainable theory of how real-world behavior maps through Stasis source, explicit state, deterministic tick systems, rendering, tests, and the packaged user experience. Code, tests, and documentation are evidence and memory cues; they are not substitutes for understanding.
- Before a nontrivial change, observe one representative path end to end and explain:
  - Mapping: what user or world behavior is represented, where it is represented, and what is deliberately outside the model.
  - Rationale: why the present structure and invariants were chosen, including the nearest tempting alternative that would violate them.
  - Extension: where one plausible adjacent requirement should fit naturally.
- Predict the result of a focused test, trace, simulation, or capture before running it. Treat a different result as evidence that the working theory is incomplete.
- When a change creates pressure for a detector, fake fallback, duplicated path, or special case, pause and determine whether the requirement fits the existing theory or requires an explicit theory revision.
- For surprising or consequential work, use a critical-incident review: reconstruct the decision, cues noticed, alternatives considered, observed result, and a counterfactual that would have changed the decision.
- A handoff is complete when the next contributor can teach back the mapping, rationale, and extension point and can diagnose or implement one representative case.
- End substantial work with `Theory gained:` stating the learned invariant or mapping, the observation supporting it, and one adjacent prediction it makes. Promote repeated durable lessons into this file or the relevant canonical document; leave isolated hypotheses in the work summary.

## Inspect narrowly

Semantic symbol queries (`list`, `find`, `read`, and `references`) are read-only and never
reconcile the checked-in vendor snapshot or materialize the toolchain cache. If a project tracks
`vendor/stasis`, use `stasis vendor status` and the explicit `stasis vendor update` command to
prepare a missing or stale snapshot. If a project uses `"stdlib": "toolchain"`, use the explicit
`stasis prepare` command. A failed query leaves project, vendor, and cache bytes unchanged.

1. Start with a file-scoped list when the file is known, or read an established target directly.
   Otherwise, use `stasis --json symbol list`. The default list results cover the manifest entry file and its direct
   imports, with an `result.imports` map for selected files. An explicit `--file` selects
   that file without expanding its imports; follow the map with additional file selections when
   discovery needs to go deeper.
2. If results are truncated or dominated by unrelated imports, use the map to choose a likely
   implementation file. Prefer a file-scoped inventory such as
   `stasis --json symbol list --file src/main.stasis --kind function` over one-word searches.
   `--query` matches a substring of name or signature. `--page` is zero-based and defaults to 0;
   `--limit` defaults to 32 and is capped at 200. List results omit `imports` and empty
   `globals` items; read those groups directly with `symbol read imports --kind imports --file FILE`
   or `symbol read globals --kind globals --file FILE`. Kinds are `imports`, `globals`,
   `struct`, `function`, and `test`.
3. Use `stasis --json symbol find NAME` for an exact name across the loaded workspace and
   `stasis --json symbol read NAME` when one exact item is required. `find` and `read` accept
   one `--file`; use `--kind`, `--owner`, and `--signature` as needed. Batch a small set of
   relevant independent reads when parallel calls are available. Avoid reading whole source files
   or enumerating every project symbol by default.
4. Before changing behavior, run `stasis --json symbol references SYMBOL` for the relevant
   function, global, or qualified field such as `PlayerState.health`. It defaults to 128
   results, caps at 256, and has no file filter or paging. Treat a response at the cap as
   potentially truncated and supplement it with targeted `rg` searches. Inspect related callers,
   reads, and writes.

For geometry or collision work, treat the rendered rectangle as the observable contract. Read the
render, movement, collision, scoring/reset, and existing test symbols together. Test the exact
contact boundary plus one value inside and outside it; do not infer physics extents from a name or
an old collision constant alone. For every changed inequality or threshold—including walls,
collision, scoring, clamping, and reset conditions—test equality and the adjacent value on each
side so `<` versus `<=` behavior is explicit.

## Container-derived UI geometry

- Derive draw, hit, and content geometry from the real safe/current container using
  `ui_single_pass`. For screens and nested rows/columns, prefer `ui_begin_frame`,
  stack scopes (`ui_vstack_begin`/`ui_hstack_begin` with matching ends), fixed/rest children
  (at most one rest child, last), `ui_inset`, `ui_anchor`/`ui_anchor_current`, and
  `ui_current_x`, `ui_current_y`, `ui_current_width`, and `ui_current_height`.
- Use the same resolved rectangle for drawing and hit testing. Recompute ephemeral rectangles each frame
  and retain only semantic interaction state; do not add a measurement pass or retained widget tree.
- Use `ui_place_x`/`ui_place_y` with `UiHorizontal`/`UiVertical` for isolated known-size placement.
  Do not replace a suitable container recipe with repeated hand-derived offsets.
- Size and place button labels from measured or intentionally cached text width and the
  actual inner content box after icon/padding allocation. Keep expensive measurement outside render hot paths
  when required; text metrics must be known when the layout recipe encounters the child.
- Place icons, thumbnails, badges, sprites, and status art from
  nominal display bounds with aspect/alpha-safe padding and container-derived origins,
  not source bitmap dimensions or opaque-trim guesses.
- Allow direct offsets only for deliberate local decoration, fixed spacing inside an
  already-derived rectangle, authored world coordinates, or a tested pixel adjustment.
- When affected UI changes, require deterministic geometry/hit tests and
  inspected desktop and phone evidence. Check container boundaries and draw/hit agreement;
  inspect PNG stills and MP4 for motion or interaction before reporting visual validation.

## Edit semantically

Prefer semantic add/update/delete/apply operations over text-range edits. Replace the complete
item returned by `symbol read`, retaining attached comments and attributes. JSON reads return
`result.item`; copy its `source_hash` to `--expected-source-hash` for direct update/delete commands
or `expected_source_hash` in a batch. Use `--source-file PATH` for a complete replacement saved
outside `src/` and `tests/`, or `--source SOURCE` for an inline declaration. Schema-v2 batch
selectors require both `target.symbol_id` and `target.name`, copied from `symbol read`. If an add has no known ID, set
`schema_version` to 1 for the whole batch and use tuple selectors; do not invent an ID. Direct
symbol commands still require the versioned `stasis.json` workspace, even though their generated
requests use schema v1.

Use `--dry-run` when target scope, imports, or changed-file scope is uncertain; for a clear
single-item change, apply once and inspect the returned plan. Dry-run compiler-validates without
writing the proposed project source or running tests. Normal setup may still synchronize vendor
files or cache data. A normal apply runs tests. Candidate compile failure leaves source unchanged;
test or receipt failure rolls back touched files. If rollback is incomplete, inspect current file contents and hashes before retrying.
Keep the successful receipt for hash-guarded revert. Re-read affected items after apply or
formatting before later edits; use fresh IDs and hashes. Receipt revert checks whole-file
post-edit hashes, so later changes such as formatting make it refuse rather than overwrite them.

Use direct text edits only for unsupported units, new files or configuration, or correcting a
parse error that prevents symbol discovery. Then resume semantic edits and the same validation
gates. A failed semantic edit is not a reason to bypass compiler or source-hash checks. Use
`--no-tests` only when the user explicitly asks for it.

See `vendor/stasis/docs/semantic-edit-and-validation.md` for detailed discovery limits,
schema examples, and recovery behavior.

## Testing standard

Read `vendor/stasis/docs/testing.md` for the standard and the selected toolchain's
language support. For gameplay and system tests, use:

```text
setup known state -> perform the real action -> advance deterministically -> observe
```

Setup may directly construct minimal state. After setup, exercise the actual
system path rather than write the expected result. Prefer small receiver-form
helpers over repeated setup when they clarify behavior; keep only handles,
indexes, initial comparison values, and small bookkeeping in scenario structs.

Use the narrowest practical `@effects(...)` contracts on tests and helpers.
Prefer `@effects()` for read-only observations
and pure calculations. The outer test contract constrains the complete call
tree; helper contracts add local guarantees. Tests are specialized parameterless
functions and use ordinary attributes, body semantics, and compile-time checking.

For string-result tests, report the first violated behavior explicitly:

```stasis
if (condition_is_wrong) {
    return "describe the violated behavior";
}
return "";
```

An empty string means success. For bool tests, return `true` for success
and `false` for failure. Test transitions, boundaries, ordering, one-time actions,
idempotence, tie-breaking, capacity, and preservation after rejected actions.
Prefer semantic expectations over incidental positions, counters, or indexes.
Assert exact values when cost, damage, reward, duration, capacity, or the exact
boundary tick is the rule. Define simulation steps as exact authoritative updates;
use complete tick tooling when input edges or host lifecycle work are relevant.

This gives agents and humans explicit starting conditions, a small vocabulary,
deterministic completion conditions, compiler-enforced effect boundaries, and
useful failure feedback. Keep tests focused and avoid a mandatory assertion
framework or a generic scenario DSL.

## Prove behavior

- For deterministic logic, add or update a real `.test.stasis` regression test. When practical,
  run `stasis test` before the implementation to observe the requested test fail, then run it again
  after the edit.
- For observable runtime state, use `stasis validate PATH OP VALUE --frames N`. It starts a fresh,
  isolated runtime, so it is suitable for an integration-style red/green check without depending
  on a currently running game's state. Do not report an observable change complete without a
  passing fresh validation or an equivalent focused integration test.
- For user-visible graphical behavior, supplement assertions with media that a human or AI reviewer
  can inspect. Capture PNG for a representative still state. Capture MP4 when the claim depends on
  motion, timing, animation, input, state transitions, or a multi-step interaction. Inspect the
  resulting pixels or recording; merely producing the file does not validate the behavior. Prefer
  deterministic `stasis record` output when available; see `docs/headless_recording.md`.
- Finish with `stasis fmt`, `stasis fmt --check`, `stasis check`, and `stasis test`. Treat formatter
  changes as part of the implementation, review them, and stage them deliberately. CI keeps the
  nonmutating `--check` verification.
- Keep the generated `.githooks/pre-commit` active. `stasis new` configures it automatically; after
  cloning the project, run `git config --local core.hooksPath .githooks`. The hook formats source
  when necessary and blocks the first attempt so formatting changes can be reviewed and staged.
- Inspect the final changed-file list. Restore only unrelated changes created during the task and
  do not accept broad rewrites or empty placeholder files as incidental cleanup.


Every AI-authored work summary must include a `Visual evidence:` line. Name each inspected PNG
and/or MP4 and state what it proves, or write `Visual evidence: not applicable` when the work has no
user-visible behavior. If relevant capture was not possible, report that limitation and do not imply
that visual validation passed.
