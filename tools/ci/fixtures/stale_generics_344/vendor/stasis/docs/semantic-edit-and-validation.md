# Semantic edit and validation

<!-- tags: symbols, references, source-hash, atomic-apply, compiler, tests, runtime -->

Stasis edits operate on compiler-discovered source items. The unit of change
is an import group, globals group, struct, function, or test declaration, not a
guessed text range. The semantic tooling owns source spans, declaration
replacement, import handling, candidate compilation, receipts, and rollback.

## Discover before editing

Start with a scoped symbol inventory using `stasis --json symbol list`. Narrow
it with `--file`, `--kind`, `--owner`, `--query`, and paging options. Use
`symbol find` when several declarations may share a name; use `symbol read`
when one exact item is required.

Before changing behavior, run `stasis --json symbol references SYMBOL` for the
relevant function, struct, global field, or qualified field path. Inspect the
definitions, reads, writes, and calls. A name match is not enough evidence
that the target is the correct boundary.

`symbol list` returns compact metadata. `symbol find` adds `source_hash`, and
`symbol read` returns the complete source item, including `symbol_id`, source
spans, source, and source hash. These fields have different roles:

| Field | Meaning |
| --- | --- |
| `symbol_id` | Canonical semantic identity; required by schema-version 2 batch selectors |
| `kind` | `imports`, `globals`, `struct`, `function`, or `test` |
| `file` | Project-relative source or test file |
| `name` | Semantic declaration name |
| `owner` | Optional containing type or scope |
| `signature` | Normalized declaration signature; optional in a selector |
| `source_hash` | Hash of the current item source used for optimistic concurrency |

For direct CLI selection, add `kind`, `file`, `owner`, and `signature` as needed
to disambiguate the name. For a schema-version 2 `symbol apply` request, use the
`symbol_id` returned by `symbol read`. Keep that read's `source_hash` as the
expected hash for an update or delete.

## Plan a semantic operation

Choose `add`, `update`, `delete`, or a related `apply` batch. For an update,
provide the complete replacement declaration and the expected source hash.
Do not reconstruct a partial declaration from a line offset. For related
changes, put all edits in one versioned request so the compiler can validate
their relationships together.

Use a dry run when the target, import ownership, or changed-file set is
uncertain. A dry run compiler-validates the candidate without writing files or
running tests. The response reports successful validation, and its plan records:

- the normalized edits;
- each changed file's complete before and after source plus hashes;
- the expected reload classification.

Embedded imports in replacement source are merged, and unused imports in
touched files are pruned. Inspect the plan's after-source when import changes
matter.

## Apply atomically

`stasis symbol apply --request REQUEST.json` plans the complete batch in memory,
handles imports, and compiler-validates the resulting workspace before writing.
A normal apply then writes the batch and runs the project tests. Candidate
compilation failure leaves source files untouched. A write, test, or receipt
failure triggers rollback of touched source files; an error reports separately
if rollback is incomplete. A successful operation writes a receipt that records
the reversible change.

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
4. Use fresh `stasis validate PATH OP VALUE --frames N` evidence for observable
   runtime state.
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
| Apply fails after multiple edits | Error plus current source hashes | Confirm rollback restored every touched file before retrying |

Do not call an edit successful because source bytes changed. The success record
should include the semantic target, expected hash, changed files, compiler/test
result, runtime evidence when observable behavior changed, and the receipt path.
