# Stasis Project Guidance

## Theory-Building Practice

- Treat programming as building and maintaining an explainable theory of how real-world behavior maps through Stasis source, compiler, JIT/AOT, runtime, and user experience. Code, tests, and documentation are evidence and memory cues; they are not substitutes for understanding.
- Before a nontrivial change, observe one representative path end to end and explain:
  - Mapping: what real-world behavior is represented, where it is represented, and what is deliberately outside the model.
  - Rationale: why the present structure and invariants were chosen, including the nearest tempting alternative that would violate them.
  - Extension: where one plausible adjacent requirement should fit naturally.
- Predict the result of a focused test, trace, or sample before running it. Treat a different result as evidence that the working theory is incomplete.
- When a change creates pressure for a detector, fake fallback, duplicated path, or special case, pause and determine whether the requirement fits the existing theory or requires an explicit theory revision.
- For surprising or consequential work, use a critical-incident review: reconstruct the decision, cues noticed, alternatives considered, observed result, and a counterfactual that would have changed the decision.
- A handoff is complete when the next contributor can teach back the mapping, rationale, and extension point and can diagnose or implement one representative case.
- End substantial work with `Theory gained:` stating the learned invariant or mapping, the observation supporting it, and one adjacent prediction it makes. Promote repeated durable lessons into project guidance; leave isolated hypotheses in the work summary.

## Stasis Practice

- Preserve deterministic, tick-based gameplay semantics; do not make gameplay progression depend on variable `dt`.
- Keep simulation state explicit and render as a projection of current state.
- Trace feature changes through state definition, initialization/reset, tick/update, render, and tests.
- Prefer representative `.test.stasis` behavior tests and keep failure paths explicit.

## Stasis vendor update policy

- Treat `stasis.json` fields `vendor.stasis.release_id` and `vendor.stasis.sha256`
  together with the complete `vendor/stasis` tree as one release snapshot. Intentional
  release or checksum pin changes must include the matching vendor update in the same
  change. Select the intended Stasis release executable, inspect `stasis vendor status`,
  then use `stasis vendor update` to refresh the complete snapshot and manifest together.
  Do not hand-edit a checksum to bless local vendor edits or copy only selected files.
- Validate vendored release fidelity with the intended release executable: run
  `stasis --json vendor status --workspace .` from the project directory and require
  `ok: true` and `result.current: true`, with `result.local_changes: false` and
  `result.update_available: false`. Confirm the recorded and installed release IDs and
  SHA-256 values match the intended release and `result.actual_sha256` matches the
  manifest checksum. A successful command exit alone does not prove fidelity. Inspect
  the complete manifest/vendor diff, including additions and deletions, then run
  `stasis fmt --check`, `stasis check`, and `stasis test`.
- Require explicit review before accepting backward pins (downgrades), checksum/release
  mismatches, partial vendor updates, or vendor changes without manifest changes.
  Report the old and proposed release/checksum, affected paths, reason, and fidelity
  validation results; do not silently normalize these cases. Release labels alone do
  not establish ordering or content identity, especially for `development` builds.
- For each affected project, report its manifest path, matching vendor path, validation
  outcome, and any reviewed exception. Projects using `"stdlib": "toolchain"` have no
  checked-in vendor snapshot: use `stasis prepare` and validate against the selected
  toolchain instead. Unpinned projects without `vendor/stasis` have no snapshot to
  update; if adopting vendoring, introduce the manifest pin and full tree together.
  Examples nested inside `vendor/stasis` belong to the enclosing release snapshot;
  do not update them independently.
