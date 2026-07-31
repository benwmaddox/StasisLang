# Contributor Workflow

## Goal

Make the next useful change, verify it, and leave the repository in a reviewable state.

## Preparation

1. Inspect `git status --short`.
2. If the tree is dirty because a cross-repo inbox synced tracked docs, either commit that sync first or stop if the state is unsafe to modify.
3. Run the baseline validation command: `tools/validate_repo.sh`.
4. If validation fails, fix it first or move the task to `NEEDS INPUT FROM USER` with evidence.
5. Run `tools/install_git_hooks.ps1` once per clone. This repository's pre-commit hook blocks commits unless the Android Workshop JIT and Published AOT render-parity emulator gate passes.

## Choose work

1. Read `docs/bugs.md`; choose the highest-severity item in `READY`.
2. If no bug is ready, choose the highest-priority active item from `docs/build_checklist.md`.
3. If no implementation task is available, improve docs, validation, or task hygiene.

## Understand the task

- Read the chosen checklist or bug entry.
- If the task came from PR review feedback, use the included GitHub links and thread context to understand what needs a reply.
- Load only the docs needed for that task.
- Read the relevant Rust and `.stasis` code before proposing changes.

## Tests-First Workflow

1. Write a brief testing plan in working notes or commit history, not for human review.
2. Add or expand automated checks to capture the desired behavior.
3. Run the checks and confirm they fail for the expected reason before implementation.

## Reviewer Gate Before Implementation

- Run the personas in `docs/review_personas.md`.
- If any persona is `BLOCKED`, update docs, tests, or plan before changing code.

## Implement

- Make the smallest change that satisfies the failing checks.
- Run the full quality gates after each meaningful change.

## Reviewer Gate After Implementation

- Re-run the personas against the diff.
- Iterate until all personas are `GREEN` or the task is explicitly blocked by missing user input.

## Wrap-Up

1. Update any docs that would prevent repeating the same mistake.
2. If the task came from PR review feedback, reply on GitHub when appropriate with the fix, clarification, or follow-up question.
3. Commit with a message that explains what changed, why, how it was verified, and any residual risks.
4. Append a concise entry to `docs/night_shift_report.md`.

## Stop Conditions

- No `READY` bugs remain and no runnable checklist work remains.
- The task requires a product, design, or business decision from the user.
- Validation cannot be restored safely within the current run.
