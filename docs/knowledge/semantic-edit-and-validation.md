# Semantic edit and validation

<!-- tags: symbols, references, source-hash, atomic-apply, compiler, tests, runtime -->

Stasis edits operate on compiler-discovered source items. The unit of change
is an import group, globals group, struct, function, or test declaration, not a
guessed text range. The compiler owns selection, source spans, import
reconciliation, validation, and rollback.

## Discover before editing

Start with a scoped symbol inventory using `stasis --json symbol list`. Narrow
it with `--file`, `--kind`, `--owner`, `--query`, and paging options. Use
`symbol find` when several declarations may share a name; use `symbol read`
when one exact item is required.

Before changing behavior, run `stasis --json symbol references SYMBOL` for the
relevant function, struct, global field, or qualified field path. Inspect the
definition, reads, writes, and callers. A name match is not enough evidence
that the target is the correct boundary.

Each source item has a stable selector:

| Field | Meaning |
| --- | --- |
| `kind` | `imports`, `globals`, `struct`, `function`, or `test` |
| `file` | Project-relative source or test file |
| `name` | Semantic declaration name |
| `owner` | Optional containing type or scope |
| `signature` | Optional normalized signature |
| `source_hash` | Deterministic identity of the current item source |

Use the full selector when a workspace contains similarly named items. Keep the
`source_hash` returned by `symbol read`; it is the concurrency check for the
edit target.

## Plan a semantic operation

Choose `add`, `update`, `delete`, or a related `apply` batch. For an update,
provide the complete replacement declaration and the expected source hash.
Do not reconstruct a partial declaration from a line offset. For related
changes, put all edits in one versioned request so the compiler can validate
their relationships together.

Use a dry run when the target, import ownership, or changed-file set is
uncertain. A dry run should answer:

- which semantic items match;
- which files will change;
- which imports will be retained, added, sorted, or removed;
- whether the expected source hashes still match;
- whether the complete replacement parses and type-checks.

## Apply atomically

`stasis symbol apply --request REQUEST.json` plans the complete batch in memory,
parses it again, reconciles imports, and compiler-validates the resulting
workspace before writing. A normal apply runs the project tests as part of the
validation boundary. If compilation or tests fail, every touched file is
restored. A successful operation writes a receipt that records the reversible
change.

Use `--no-tests` only when the surrounding workflow explicitly owns an
equivalent test gate. It weakens the normal behavior guarantee and should be
visible in the edit record.

After a successful apply, retain the receipt and the post-edit source hashes.
Revert through the semantic receipt; the revert operation verifies the expected
post-edit state before restoring the prior source.

## Validate behavior

Validation has distinct layers:

1. Run `stasis fmt --check` to catch formatting drift without mutating files.
2. Run `stasis check` for project compilation and diagnostics.
3. Run `stasis test` for deterministic state-transition and regression tests.
4. Use fresh `stasis validate` evidence for observable runtime state.
5. Inspect the final changed-file list and the receipt before reporting success.

Passing compilation proves that the source is valid. Passing tests proves only
the behavior covered by those tests. Fresh runtime validation proves the
observable path selected by that validation. Keep all three claims separate.

## Failure and recovery

| Failure | First check | Correct response |
| --- | --- | --- |
| No symbol or several symbols match | Scope, kind, file, owner, signature | Refine discovery; do not guess a text location |
| Source hash is stale | Current `symbol read` result | Re-read references and re-plan from the current source |
| Reference set is broader than expected | Reads, writes, callers, and import map | Include the affected declarations or narrow the intended change |
| Batch does not compile | First compiler diagnostic in the changed item | Correct the smallest semantic unit and rerun the dry run |
| Tests reject the batch | Failing invariant and first divergent tick | Repair behavior, preserve the regression, and reapply |
| Runtime evidence disagrees | Fresh validation setup and projection boundary | Inspect authoritative state and render projection separately |
| Apply fails after multiple edits | Receipt or failure observation | Confirm rollback restored every touched file before retrying |

Do not call an edit successful because source bytes changed. The success record
should include the semantic target, expected hash, changed files, compiler/test
result, runtime evidence when observable behavior changed, and the receipt path.
