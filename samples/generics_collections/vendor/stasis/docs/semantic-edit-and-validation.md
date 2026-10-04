# Semantic edit and validation

<!-- tags: symbols, references, source-hash, atomic-apply, compiler, tests, runtime -->

Edit compiler-discovered declarations: imports, globals, structs, functions,
and tests. Replace each complete declaration, retaining comments and attributes.

## Discover

```text
stasis --json symbol list --file src/main.stasis --kind function
stasis --json symbol read tick --kind function --file src/main.stasis
stasis --json symbol references tick
```

Default lists cover the entry and direct imports. A file selection does not
expand dependencies; follow `result.imports` explicitly. Lists default to 32
items (maximum 200), with zero-based paging. Imports and empty globals groups
are omitted from lists; read them by name and kind. `find` returns exact-name
matches; `read` requires one. Disambiguate with `file`, `kind`, `owner`, or
`signature`.
References default to 128 results, cap at 256, and have no paging or file filter;
supplement a capped response with targeted `rg`.

## Apply

Copy the current ID, name, and hash from `result.item` into a schema-v2 request:

```json
{
  "schema_version": 2,
  "edits": [{
    "operation": "update",
    "target": {
      "kind": "function", "file": "src/main.stasis", "name": "tick",
      "symbol_id": "SYMBOL_ID_FROM_READ"
    },
    "expected_source_hash": "HASH_FROM_READ",
    "new_source": "function tick(): i32 {\n    return 2;\n}"
  }]
}
```

Run `stasis symbol apply --request REQUEST.json`. Related edits belong in one
batch. For new declarations without IDs, set the entire batch to schema v1 and
select by `name`, `kind`, and `file`; omitted schema versions default to v2.
Direct add/update/delete commands use v1; update/delete require the current
`--expected-source-hash`.

Use a dry run when the target or changed-file set is uncertain. It compiles the
candidate without writing proposed source or running tests, though command setup
may synchronize vendor/cache data. Embedded imports are merged and unused
imports in touched files are pruned.

## Validate and recover

Normal apply compiles and runs tests. Compile failure preserves source;
test/receipt failure rolls touched sources back. Successful receipts live under
`<output>/semantic-edits/`. Inspect the returned plan, validation, and receipt.
Use `--no-tests` only when explicitly requested by the user.

Run `fmt --check`, `check`, and `test`; use fresh runtime validation or inspected
captures for observable behavior. Re-read IDs and hashes after changes or
formatting. Receipt revert checks whole-file post-edit hashes, so later changes
make it refuse. If rollback is incomplete, inspect each affected file against
the plan before retrying. Never discard a stale-hash guard.

Use text edits for new files/configuration, unsupported units, or parse errors
that block discovery; then return to semantic edits and the same validation.
