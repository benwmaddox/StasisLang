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
