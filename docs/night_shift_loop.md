# Night Shift Loop

## Prime directive

Run autonomously without requiring plan review. Own validation and leave the repository in a reviewable state.

Before you start, define finishing criteria for the chosen task: what must be true for the work to count as done, what checks must pass, and what user-visible behavior must be confirmed. Use that checklist before you report back.

The central Ned inbox runner owns fetch/fast-forward, branch checkout or branch creation, and launching the executor. This repo only needs to provide strict validation and context docs.

If a repo is missing a strict validation entrypoint, treat that as setup work before relying on automation there. Build one deterministic script from the strongest real bounded checks the repo already supports, then use that script consistently.

## Preparation

1. Inspect `git status --short`.
2. Inspect `git branch --show-current` and preserve the current branch when the run was launched to revise an existing PR. For issue-driven work, start from the repo default branch after it has been synced with `origin`, then create a fresh `nightshift/...` branch.
3. If the tree is dirty, either create a protective WIP commit or stop and explain why the state is unsafe to modify.
4. Run the quality gates in `tools/validate_repo.sh`.
5. If validation fails, fix it first or move the task to `NEEDS INPUT FROM USER` with evidence.

## Choose work

This workflow only works from a GitHub-selected item. If the run was not launched for a specific GitHub issue or PR, stop and report that no selected item was provided.

Stay on the exact selected issue or PR. Do not switch to a different task because of local notes, queue files, or checklists. If local docs appear to point somewhere else, treat them as context only and report the mismatch instead of changing scope.

## Understand the task

- Read the selected GitHub issue, PR, review, and review comments carefully.
- Load only the docs needed to understand how this repo works and how to validate the change.
- Read the relevant Rust and `.stasis` code before proposing changes.

## Tests-first workflow

1. Write a brief testing plan in working notes or commit history, not for human review.
2. Add or expand automated checks to capture the desired behavior.
3. Run the checks and confirm they fail for the expected reason before implementation.

## Reviewer gate before implementation

- Run the personas in `docs/review_personas.md`.
- If any persona is `BLOCKED`, update docs, tests, or plan before changing code.

## Implement

- Make the smallest change that satisfies the failing checks.
- Run the full quality gates after each meaningful change.
- If something fails or still looks wrong, keep iterating and retesting instead of handing back a first draft.

## Reviewer gate after implementation

- Re-run the personas against the diff.
- Iterate until all personas are `GREEN` or the task is explicitly blocked by missing user input.

## Wrap-up

1. Update any docs that would prevent repeating the same mistake.
2. If the task came from PR review feedback, reply on GitHub when appropriate with the fix, clarification, or follow-up question.
3. Ensure the PR has a human reviewer requested before you finish. Prefer `benwmaddox` unless the repo says otherwise.
4. Before reporting back, verify the work directly when possible. Run the relevant checks, inspect the output, and exercise the changed flow instead of assuming the change worked.
5. If the work is visual or interactive, look at the changed screens or flows and confirm they render and behave correctly.
6. Only report back when the finishing criteria are met or when you are genuinely blocked on outside input.
7. When reporting results to the user, explain what changed and what happened in plain, clear English. Avoid jargon, technical implementation detail, and code-speak in the final write-up.
8. Commit with a message that explains what changed, why, how it was verified, and any residual risks.
9. Append a concise entry to `docs/night_shift_report.md`.

## Stop conditions

- No selected GitHub item was provided for the run.
- The task requires a product, design, or business decision from the user.
- Validation cannot be restored safely within the current run.
