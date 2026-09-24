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

## JSON request and schema compatibility

A schema-v2 request requires `target.symbol_id` on every edit. Include `target.name` too; it is
required by the request shape even when the canonical ID selects the item. For an update or delete,
copy both fields and `source_hash` from `stasis --json symbol read NAME`. An omitted request
version defaults to v2.

Schema v1 remains supported for tuple selectors using `name`, `kind`, `file`, and optional
`owner` and `signature`. If a new add has no ID from `symbol read`, explicitly set
`schema_version` to 1 for the whole batch and use tuple selectors for its edits; do not fabricate an
ID. Direct `symbol add`, `update`, and `delete` commands currently construct v1 requests.
This request compatibility does not enable a legacy-project fallback: symbol commands still
require the versioned `stasis.json` workspace. Accepted v1 requests produce a plan using the
current schema version.

### Existing-symbol v2 example

For a fixture where `src/main.stasis` contains:

```stasis
function main(): i32 { return tick(); }
function tick(): i32 { return 1; }
```

Read `tick` and copy both its `symbol_id` and `source_hash`:

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

Adapt the complete replacement to the existing item, retaining attached comments and attributes.

### New add in a mixed v1 batch

The request version applies to the whole batch. When a new item has no ID to copy from a read, use
v1 tuple selectors for that add and related edits. This update depends on the helper added in the
same batch:

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

A batch is planned and compiler-validated before source writes. Apply then runs project tests unless
the user explicitly asks for `--no-tests`. Candidate compilation failure leaves sources unchanged;
test or receipt failure rolls touched sources back. If rollback is reported incomplete, inspect
current file contents and hashes before retrying. Successful applies write a receipt under
`<output>/semantic-edits/` (`build/semantic-edits/` by default). Each source file and receipt uses flushed atomic replacement.

Revert verifies whole-file post-edit hashes before restoring source. A later change, including
formatting, makes it refuse rather than overwrite that change. Re-read affected items after apply
or formatting before later edits and use fresh IDs and hashes. If tests fail during revert, the
edited sources are reapplied.

Use `--dry-run` when target, imports, or changed-file scope is uncertain. It returns the
compiler-validated plan without writing the proposed project source or running tests. Normal
command setup may still synchronize vendor files or cache data. For a clear single-item change,
apply once and inspect the returned plan instead of compiling the same candidate twice. Do not
remove hash guards or bypass compiler validation after a failed edit.

## CLI

```text
stasis --json symbol list [--kind KIND] [--file FILE ...] [--owner OWNER] [--query TEXT] [--page N] [--limit N]
stasis --json symbol find NAME [--kind KIND] [--file FILE] [--owner OWNER] [--signature SIGNATURE]
stasis --json symbol read NAME [--kind KIND] [--file FILE] [--owner OWNER] [--signature SIGNATURE]
stasis --json symbol references SYMBOL [--limit N]
stasis --json symbol add NAME --kind KIND --file FILE (--source SOURCE | --source-file PATH) [--dry-run] [--no-tests]
stasis --json symbol update NAME [selection options] (--source SOURCE | --source-file PATH) [--expected-source-hash HASH] [--dry-run] [--no-tests]
stasis --json symbol delete NAME [selection options] [--expected-source-hash HASH] [--dry-run] [--no-tests]
stasis --json symbol apply --request PATH [--dry-run] [--no-tests]
stasis --json symbol revert --receipt PATH [--dry-run] [--no-tests]
```

`KIND` is `imports`, `globals`, `struct`, `function`, or `test`.
`--query` is a substring match on name or signature. List pages are zero-based; `--page`
defaults to 0 and `--limit` defaults to 32 (maximum 200). `list` defaults to the manifest entry
and its direct imports. Its `result.imports` map reports dependencies for every selected file. Passing
`--file` selects the named file without expanding its imports; repeated `--file` values are supported
for `list`. List results omit `imports` and empty `globals` items, which can be read directly
using those names with their matching kinds. `find NAME` matches an exact name across the loaded
workspace; `read NAME` requires exactly one item. `find` and `read` accept one `--file`.

`references` defaults to 128 results and is capped at 256. It has no file filter or paging; a
response at the cap may be truncated, so supplement it with targeted `rg` searches. A successful
`read` returns `result.item` with `symbol_id`, `name`, and `source_hash`. Inspect
`symbol apply`'s `result.plan`, `result.validation`, and `result.receipt`.

`--source` accepts the complete replacement inline; `--source-file` is available for larger
definitions. Exactly one is required for add/update. The CLI flags list `--no-tests`, but agents
must use it only when the user explicitly asks.

Symbol commands require the versioned `stasis.json` workspace contract. Run `stasis init` in
an older source tree before using them. Existing legacy compiler/runner entrypoints remain
separate and are never selected implicitly by `stasis symbol`.

## Android capability audit

Android already provided symbol list/read/write/delete, compile-on-batch, test execution, and
rollback behavior. Its earlier private Java scanner and direct span mutation were not reused.
AI `write_symbol` and `delete_symbol` now resolve the corresponding Rust source item and submit
the shared semantic request through `stasis_android_bridge_semantic_edit`. The bridge exposes the
same source-item JSON through `stasis_android_bridge_source_items`, compiles once at the requested
transaction boundary, and restores the Rust-generated plan on failure. Android's manual editor
continues to provide draft/recovery UI, but compiler-owned parsing remains the authority for
scripted symbol transactions.
