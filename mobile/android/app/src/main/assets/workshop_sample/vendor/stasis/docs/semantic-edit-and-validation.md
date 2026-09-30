# Semantic edit and validation

<!-- tags: symbols, references, source-hash, atomic-apply, compiler, tests, runtime -->

Stasis edits operate on compiler-discovered source items. The unit of change
is an import group, globals group, struct, function, or test declaration, not a
guessed text range. The semantic tooling owns source spans, declaration
replacement, import handling, candidate compilation, receipts, and rollback.

## Discover before editing

Start with a scoped `symbol list` when the target is unknown. If its file is known, start with a
file-scoped list; if an exact target is already established, read it directly. Default list results
cover the manifest entry and its direct imports. The `result.imports` map reports dependencies for selected
files; an explicit `--file` selects that file without expanding imports, so follow the map with
additional file selections to walk deeper. Prefer a focused inventory such as
`stasis --json symbol list --file src/main.stasis --kind function` over one-word searches.

`--query` matches a substring of name or signature. `--page` is zero-based and defaults to 0;
`--limit` defaults to 32 and is capped at 200. List results omit `imports` and empty `globals`
items; read those groups by their names and matching kinds.

Use `symbol find NAME` for an exact name across the loaded workspace; it returns every match.
`symbol read NAME` requires exactly one match. Both accept one `--file`, with `--kind`,
`--owner`, and `--signature` available to disambiguate.

A JSON read returns the complete editable source item under `result.item`, including its
`symbol_id`, source spans, and `source_hash`. For direct update/delete commands, pass the hash
with `--expected-source-hash`. The fields have distinct roles:

| Field | Meaning |
| --- | --- |
| `symbol_id` | Canonical semantic identity; required by schema-v2 batch selectors |
| `kind` | `imports`, `globals`, `struct`, `function`, or `test` |
| `file` | Project-relative source or test file |
| `name` | Semantic declaration name; required in a batch selector |
| `owner` | Optional containing type or scope |
| `signature` | Normalized declaration signature; optional in a selector |
| `source_hash` | Current source-item hash for optimistic concurrency |

Before changing behavior, run `stasis --json symbol references SYMBOL` for the relevant function,
struct, global field, or qualified field path. It defaults to 128 results, caps at 256, and has
no file filter or paging. Treat a response at the cap as potentially truncated and supplement it
with targeted `rg` searches. Inspect definitions, reads, writes, and calls.

## Plan a semantic operation

For an existing symbol in a schema-v2 `symbol apply` request, copy both `symbol_id` and required
`name` from `symbol read`. Include that read's `source_hash` as `expected_source_hash` for
updates and deletes. Schema v1 tuple selectors remain supported. When an add has no ID to copy
from a read, explicitly set `schema_version` to 1 for the whole batch and use `name`, `kind`,
and `file` selectors. Never invent IDs; omitted `schema_version` defaults to v2. Direct CLI
add/update/delete commands use v1 requests.

For an update, provide the complete replacement declaration, retaining attached comments and
attributes. Do not reconstruct a partial declaration from a line offset. Related edits belong in
one batch so the compiler can validate them together. This v1 example updates `tick` to call a
new `helper`; because the new declaration has no ID yet, both edits use v1 tuple selectors:

```json
{
  "schema_version": 1,
  "edits": [
    {
      "operation": "update",
      "target": {
        "kind": "function",
        "file": "src/main.stasis",
        "name": "tick"
      },
      "expected_source_hash": "HASH_FROM_SYMBOL_READ",
      "new_source": "function tick(): i32 {\n    return helper();\n}"
    },
    {
      "operation": "add",
      "target": {
        "kind": "function",
        "file": "src/main.stasis",
        "name": "helper"
      },
      "new_source": "function helper(): i32 {\n    return 2;\n}"
    }
  ]
}
```

This schema-v2 update request shows the required canonical ID and name. Copy their values, and the
hash, from a fresh read:

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

Use a dry run when the target, import ownership, or changed-file set is uncertain; for a clear
single-item edit, apply once and inspect the returned plan instead of compiling the same candidate
twice. Dry-run returns the compiler-validated plan without writing the proposed project source or
running tests. Normal command setup may still synchronize vendor files or cache data. Inspect
normalized edits, changed files and before/after hashes, and reload classification. Embedded
imports are merged into the imports item and unused imports in touched files are pruned.

Use direct text edits only for unsupported units, creating a new file or configuration, or
correcting a parse error that prevents symbol discovery. Then return to semantic edits and the same
validation gates. A failed semantic edit is not a reason to bypass compiler or hash checks.

## Apply atomically

`stasis symbol apply --request REQUEST.json` validates the candidate workspace before writing. A
normal apply runs project tests. Candidate compilation failure leaves source unchanged; test or
receipt failure rolls touched sources back. If an error reports incomplete rollback, inspect each
affected file and compare its current hash with the plan before retrying. Successful applies write
a receipt under `<output>/semantic-edits/` (`build/semantic-edits/` by default). Revert verifies whole-file post-edit hashes before
restoring source; formatting or another later change makes it refuse rather than overwrite that
change. If tests fail during revert, the edited sources are reapplied. Re-read affected items after
apply or formatting before later edits and use fresh IDs and hashes.

Use `--no-tests` only when the user explicitly asks for it, even if another workflow owns a
test gate. Inspect `result.plan`, `result.validation`, and `result.receipt` before reporting
success.

## Validate behavior

Validation has distinct layers:

1. Run `stasis fmt --check` to catch formatting drift without mutating files.
2. Run `stasis check` for project compilation and diagnostics.
3. Run `stasis test` for deterministic state-transition and regression tests.
4. Use fresh `stasis validate PATH OP VALUE --frames N` evidence for observable
   runtime state.
5. Inspect the final changed-file list and the receipt before reporting success.

Passing compilation proves that the source is valid. Passing tests proves only
the behavior covered by those tests. Fresh runtime validation proves the
observable path selected by that validation. Keep all three claims separate.

## Failure and recovery

| Failure | First check | Correct response |
| --- | --- | --- |
| No symbol or several symbols match | Exact name and `kind`, `file`, `owner`, or `signature` scope | Refine discovery; read the intended item |
| V2 selector lacks an ID | Whether the target already exists | Re-read an existing item; for a new add, set the entire batch to v1 |
| Source hash is stale | Current `symbol read` result and references | Re-read and rebuild with the new hash; do not drop the guard |
| Reference response reaches its cap | Result count and relevant source files | Supplement with targeted `rg`; do not assume completeness |
| Candidate does not compile | First diagnostic in the changed item | Correct the smallest semantic unit and rerun the plan |
| Tests reject the batch | Failing invariant and first divergent tick | Repair behavior, preserve the regression, and reapply |
| Runtime evidence disagrees | Fresh validation setup and projection boundary | Inspect authoritative state and render projection separately |
| Apply or receipt reports rollback incomplete | Each touched file's current hash versus the plan | Recover inconsistent source state before retrying |
| Receipt revert hash check fails | Changes since the recorded edit, including formatting | Resolve the conflict; do not force the old receipt |

Do not call an edit successful because source bytes changed. Report the semantic target, expected
hash, changed files, compiler and test results, runtime evidence when behavior changed, and receipt
path.
