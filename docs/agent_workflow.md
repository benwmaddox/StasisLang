# Stasis agent workflow

Use the installed `stasis` CLI from the directory containing `stasis.json`. Do not invoke Cargo
for normal project work.

## Inspect narrowly

1. Start a code task with `stasis --json symbol list`. Without `--file`, this returns a compact,
   source-free index for the manifest entry file and its direct imports, plus their import map.
2. Narrow follow-up discovery with `--query`, `--kind`, `--owner`, and repeated `--file` options.
   Do not enumerate every project symbol or read whole source files by default.
3. Read only likely targets with `stasis --json symbol read NAME` and disambiguate with `--file`,
   `--kind`, `--owner`, or `--signature`. Batch independent reads when the agent environment
   supports parallel tool calls; up to 50 deliberate reads in one turn is reasonable.
4. Before changing behavior, run `stasis --json symbol references SYMBOL` for the relevant
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
- Finish with `stasis fmt --check`, `stasis check`, and `stasis test`. Semantic symbol edits already
  preserve untouched formatting; do not run mutating whole-project formatting as routine cleanup.
- Inspect the final changed-file list. Restore only unrelated changes created during the task and
  do not accept broad rewrites or empty placeholder files as incidental cleanup.

Use `stasis ai "PROMPT"` only when the user explicitly wants Stasis's subscription-backed nested
AI turn. An agent already performing the task should use the commands above directly.
