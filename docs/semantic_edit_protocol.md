# Semantic Edit Protocol

Stasis symbol editing is owned by the Rust compiler frontend. The desktop CLI and Android
Workshop use the same versioned JSON request, parser-derived source items, source hashes,
validation, and rollback plan. Neither surface scans source text to choose edit spans.

## Source items

Each editable `src/**/*.stasis` or `tests/**/*.stasis` file exposes these deterministic items:

1. `imports`: one item containing the file's imports, including an empty item when none exist.
2. `globals`: one item containing every top-level `const` and `global` declaration.
3. `struct`: one item per struct.
4. `function`: one item per function.
5. `test`: one item per test declaration.

A struct or function item starts at its immediately preceding `//` comment block and ends after
the newline following its closing brace. Blank lines before that comment block remain outside the
item. This makes comments move, update, and delete with the declaration they describe while
unrelated formatting remains untouched.

The stable selector fields are `kind`, project-relative `file`, `name`, optional `owner`, and
optional normalized `signature`. Every item also returns a deterministic `source_hash`. Apply and
delete requests may include the full SHA-256 `expected_source_hash`; a mismatch rejects a stale
edit. Android may delete one named constant/global through the shared protocol; Rust resolves its
exact parser span inside the aggregate globals item rather than performing a text replacement.

## Import ownership

Imports are always owned by the file's `imports` item. An import included in a globals, struct,
function, or test replacement is removed from that replacement and merged into the imports item.
Imports are sorted and deduplicated.

After an edit, the compiler collects identifier references from each touched file and compares
them with parser-derived exports from each imported file, including transitive exports. An import
is removed when none of its exported identifiers remain referenced. Imports whose target is not
available in the loaded workspace are retained because the compiler cannot prove them unused.
Imports that supply `main`, `tick`, `render`, or `on_code_swap` are also retained because those
host roots can be reachable without a textual call in the importing file.

## JSON request

```json
{
  "schema_version": 1,
  "edits": [
    {
      "operation": "update",
      "target": {
        "kind": "function",
        "file": "src/main.stasis",
        "name": "tick",
        "signature": "tick(): i32"
      },
      "expected_source_hash": "cd966a7b3b8d7e7bb02f7049e46b90493a6621f83de2a94f311618aa4bc24529",
      "new_source": "function tick(): i32 { return 2; }"
    }
  ]
}
```

`operation` is `add`, `update`, or `delete`. A batch is planned entirely in memory, parsed again,
and compiler-validated before any write. Apply writes all changed files, runs tests unless skipped,
and restores every changed file if validation fails. Successful applies write a receipt under
`build/semantic-edits/`; source files and receipts use flushed atomic replacement so interrupted
writes retain either the old or complete new contents. Revert verifies the post-edit hashes before
restoring the recorded source.

## CLI

```text
stasis symbol list [--kind KIND] [--file FILE] [--owner OWNER]
stasis symbol find NAME [--kind KIND] [--file FILE] [--owner OWNER] [--signature SIGNATURE]
stasis symbol read NAME [selection options]
stasis symbol add NAME --kind KIND --file FILE (--source SOURCE | --source-file PATH) [--dry-run] [--no-tests]
stasis symbol update NAME [selection options] (--source SOURCE | --source-file PATH) [--expected-source-hash HASH] [--dry-run] [--no-tests]
stasis symbol delete NAME [selection options] [--expected-source-hash HASH] [--dry-run] [--no-tests]
stasis symbol apply --request PATH [--dry-run] [--no-tests]
stasis symbol revert --receipt PATH [--dry-run] [--no-tests]
```

`KIND` is `imports`, `globals`, `struct`, `function`, or `test`. `--json` returns the same typed
items, selectors, edit plan, hashes, reload classification, and receipt contract used by Android.
`--source` accepts the complete replacement definition inline and is the preferred scripted path;
`--source-file` remains available for large definitions. Exactly one is required for add/update.

Symbol commands deliberately have no legacy-project fallback. They require the versioned
`stasis.json` workspace contract; run `stasis init` in an older source tree before editing it.
Existing legacy compiler/runner entrypoints remain separate and are never selected implicitly by
`stasis symbol`.

## Android capability audit

Android already provided symbol list/read/write/delete, compile-on-batch, test execution, and
rollback behavior. Its earlier private Java scanner and direct span mutation were not reused.
AI `write_symbol` and `delete_symbol` now resolve the corresponding Rust source item and submit
the shared semantic request through `stasis_android_bridge_semantic_edit`. The bridge exposes the
same source-item JSON through `stasis_android_bridge_source_items`, compiles once at the requested
transaction boundary, and restores the Rust-generated plan on failure. Android's manual editor
continues to provide draft/recovery UI, but compiler-owned parsing remains the authority for
scripted symbol transactions.
