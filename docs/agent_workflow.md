# Stasis agent workflow

Use the installed `stasis` CLI from the directory containing `stasis.json`. Do not invoke Cargo
for normal project work.

Read `PROJECT_ARCHITECTURE.md` before structuring game code. Use its input, tick, state, and
rendering boundaries as the default unless the project documents a concrete reason to differ.

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
`vendor/stasis`, use `stasis vendor status` to inspect it and the explicit `stasis vendor update`
command to prepare a missing, locally changed, or stale snapshot. If a project uses
`"stdlib": "toolchain"`, use the explicit `stasis prepare` command to materialize its cache.
A failed query leaves the project, vendor, and cache bytes unchanged.

1. Start a code task with `stasis --json symbol list`. Without `--file`, this returns a compact,
   source-free index for the manifest entry file and its direct imports, plus their import map.
2. If that page is truncated or dominated by unrelated imports, use the import map to select the
   likely implementation file. Prefer one file-scoped function inventory, for example
   `stasis --json symbol list --file src/main.stasis --kind function`, over a series of one-word
   queries. Request globals separately with the exact kind `--kind globals` only when state fields
   matter. The accepted kinds are `imports`, `globals`, `struct`, `function`, and `test`.
3. Otherwise, narrow follow-up discovery with `--query`, `--kind`, `--owner`, and repeated `--file`
   options. Do not enumerate every project symbol or read whole source files by default.
4. Read only likely targets with `stasis --json symbol read NAME` and disambiguate with `--file`,
   `--kind`, `--owner`, or `--signature`. Batch independent reads when the agent environment
   supports parallel tool calls; up to 50 deliberate reads in one turn is reasonable.
5. Before changing behavior, run `stasis --json symbol references SYMBOL` for the relevant
   function, global, or qualified field such as `PlayerState.health`. Inspect related callers,
   reads, and writes before editing.

For geometry or collision work, treat the rendered rectangle as the observable contract. Read the
render, movement, collision, scoring/reset, and existing test symbols together. Test the exact
contact boundary plus one value inside and outside it; do not infer physics extents from a name or
an old collision constant alone. For every changed inequality or threshold—including walls,
collision, scoring, clamping, and reset conditions—test equality and the adjacent value on each
side so `<` versus `<=` behavior is explicit.

## Edit semantically

Prefer `stasis symbol add`, `update`, `delete`, or `apply` over text-range edits. `symbol read`
returns `source_hash`; echo it as `--expected-source-hash` so a concurrent change fails instead of
being overwritten. Use the complete declaration as the replacement source.

For related changes, submit one atomic batch with `stasis symbol apply --request REQUEST.json`:

```json
{
  "schema_version": 1,
  "edits": [
    {
      "operation": "update",
      "target": {"kind": "function", "file": "src/main.stasis", "name": "tick"},
      "expected_source_hash": "HASH_FROM_SYMBOL_READ",
      "new_source": "function tick(): i32 { return 0; }"
    }
  ]
}
```

The compiler plans the entire batch, reconciles imports, compiles it, runs project tests, and
rolls every touched file back on failure. Do not use `--no-tests` unless the user explicitly asks.

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
- Finish with `stasis fmt --check`, `stasis check`, and `stasis test`. Semantic symbol edits already
  preserve untouched formatting; do not run mutating whole-project formatting as routine cleanup.
- Keep the generated `.githooks/pre-commit` active. `stasis new` configures it automatically; after
  cloning the project, run `git config --local core.hooksPath .githooks`. The hook formats source
  when necessary and blocks the first attempt so formatting changes can be reviewed and staged.
- Inspect the final changed-file list. Restore only unrelated changes created during the task and
  do not accept broad rewrites or empty placeholder files as incidental cleanup.

Use `stasis ai "PROMPT"` only when the user explicitly wants Stasis's subscription-backed nested
AI turn. An agent already performing the task should use the commands above directly.

Every AI-authored work summary must include a `Visual evidence:` line. Name each inspected PNG
and/or MP4 and state what it proves, or write `Visual evidence: not applicable` when the work has no
user-visible behavior. If relevant capture was not possible, report that limitation and do not imply
that visual validation passed.
