# Semantic edit and validation

<!-- tags: semantic-edit, atomicity, workspace, references, validation, repair -->

## Edit contract

An edit is a semantic operation, not merely a text replacement. Describe the
intended change, its target, its dependencies, and its postconditions before
writing it.

| Stage | Record |
| --- | --- |
| Locate | File, symbol/section, and current identity/reference |
| Plan | Desired semantic change and affected consumers |
| Apply | One bounded edit with no unrelated formatting churn |
| Validate | Parse/build, targeted scenario, state inspection, and render check |
| Recover | Preserve failure evidence; roll back every file changed by the failed atomic batch |

Source: `docs/agent_workflow.md`, `docs/live_cli_workspace.md`,
`docs/semantic_edit_protocol.md`.

## Atomicity checklist

- Establish the current workspace and relevant file identity.
- Make one coherent edit that can be reviewed as a unit.
- Keep generated or unrelated files outside the edit scope.
- Validate the changed symbol before broad validation.
- Capture diagnostics with the exact source location.
- Do not call an edit successful because text was written; require behavior or
  validation evidence.

## References, hashes, and evidence

Use references to connect an edit to the object it changes. Use hashes to detect
that the expected input, state, or output changed. Store the human-readable
description beside the machine-checkable identity.

```text
edit target:   sprite.player
input hash:    <serialized source/state hash>
postcondition: player reference resolves and render parity holds
evidence:      scenario step + state inspection + render result
```

The angle-bracket values above are placeholders, so this is **pseudocode**, not
a compilable Stasis excerpt. Hashes are evidence of equality for the selected
representation; they do not prove that the representation contains every
required invariant.

Source: `samples/state_inspection/`, `docs/render_parity_gate.md`,
`samples/typed_sprite/`.

## Failure and recovery

| Symptom | First check | Recovery |
| --- | --- | --- |
| Target cannot be resolved | Reference and current source identity | Re-locate; do not patch by guess |
| Parse/build error | First diagnostic at changed symbol | Correct the smallest semantic unit |
| State changes but frame does not | Update/render boundary | Repair render projection or invalidation |
| Frame changes but state does not | Renderer mutation or stale fixture | Make render read-only; rerun inspection |
| Intermittent result | Unbounded work or time source | Fix tick/input ordering and bounds |

## Minimal edit record

**Pseudocode:**

```text
before = identify(target)
assert before.exists
apply_one_semantic_change(target, desired_change)
validate_parse_and_targeted_behavior()
if validation_failed:
    preserve_diagnostics()
    restore_atomic_batch_before_state()
else:
    record(before, desired_change, evidence)
```

This workflow is intentionally independent of an LLM: a human, CLI, or build
tool can follow the same locate/apply/validate/recover contract.
