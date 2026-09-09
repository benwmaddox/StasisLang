# Night Shift Loop

This is the automation companion to [docs/contributor_workflow.md](contributor_workflow.md). It applies only when the central inbox runner starts a Night Shift run for a selected GitHub item.

## Scope

- Work only on the selected GitHub issue, pull request, review, or review comment. If no item was selected, report that fact and stop.
- Treat local notes, queues, bugs, checklists, and plans as repository context; they do not change the selected scope.
- The central runner owns fetch or fast forward, branch checkout or creation, and executor launch. Preserve the prepared branch and report a mismatch instead of changing it locally.

## Run

- Define finishing criteria before editing: the behavior that must hold, checks that must pass, and user visible evidence required.
- Inspect the status and selected GitHub context, then load only the relevant repository docs and source.
- Follow the focused implementation and validation workflow in docs/contributor_workflow.md. Make the smallest change, run focused checks during work and applicable final validation once (including tools/validate_repo.sh when the change warrants it), and keep each command within 900 seconds.
- Select review personas by risk when useful; there is no mandatory reviewer set or repeated full gate.
- For graphical or interactive work, inspect the relevant output. Record the checks, evidence artifacts, and any validation limitation before reporting.

## Finish

- Verify the finishing criteria directly and report the result in clear English.
- Commit, append docs/night_shift_report.md, request a reviewer, or reply on GitHub only when the selected run workflow explicitly authorizes that action.
- Stop when the selected item is complete, a required user decision is missing, or validation cannot be restored safely.
