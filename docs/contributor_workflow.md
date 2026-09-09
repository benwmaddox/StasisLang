# Contributor Workflow

## Scope and work selection

For ordinary work, follow the user's requested task. `docs/bugs.md`, `docs/build_checklist.md`, and other plans provide context; they do not select work.

For Night Shift or inbox automation, implement only the selected GitHub issue, pull request, review, or review comment. The central runner owns branch setup, fetch or fast forward, and executor launch. Commit, report, or GitHub reply actions belong only to an explicitly authorized task workflow.

## Before editing

1. Inspect `git status --short` and read only the relevant docs and source.
2. For nontrivial work, resolve the affected symbols and architecture, state a concise implementation and validation plan, then edit.
3. Use the repository Cargo cache command for Codex and automation: `python tools/cargo_cache.py run -- cargo ...`.

## Implement and validate

- Make the smallest change that satisfies the task. Add a deterministic regression test when behavior warrants one; when practical, observe the focused test fail before implementation.
- Run focused checks while developing. Give Cargo tests an owning target (`--lib`, `--bin <name>`, or `--test <name>`); an unexpected `running 0 tests` result is a failed selection.
- Keep Rust tests runnable by default; `tools/validate_repo.sh` rejects `#[ignore]` under product and test roots.
- Keep unsafe Rust inside audited platform boundary crates and follow `docs/unsafe_rust.md`. Do not leave test or compiler processes running after a timeout, cancellation, or suspected leak; inspect and clean only processes started by this task.
- For graphical behavior, capture and inspect a representative PNG, and an MP4 when the claim depends on motion, timing, input, animation, or a multi step interaction. If relevant media cannot be captured, state the limitation and record the validation gap.

## Review and wrap up

- Select only the risk relevant reviewers from `docs/review_personas.md`. Review the diff and run applicable final validation once, including `tools/validate_repo.sh` when the change warrants the full repository gate. Do not repeat unrelated full gates after each edit.
- Remove incidental changes and simplify the touched code. Update a canonical doc when the work establishes a durable invariant; keep isolated observations in the work record when useful.
- If a review task explicitly authorizes a GitHub response, reply after validation. Otherwise leave external communication, commits, and Night Shift reports to the authorized workflow.
- Every AI authored work summary includes `Visual evidence:` with inspected PNG or MP4 paths and what they prove, or `Visual evidence: not applicable`.
