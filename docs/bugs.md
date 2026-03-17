# Bugs

Allowed states: `READY`, `IN PROGRESS`, `DONE`, `NEEDS INPUT FROM USER`

If you use a cross-repo inbox, it may maintain a generated `NED-INBOX` block under `## READY`. Treat those entries like normal bugs. Those synced items should include direct GitHub links for the source PR and individual review threads so the agent can reply when appropriate.

## READY

<!-- NED-INBOX:START -->
- [P1][PR #247] Address review feedback for `Preserve PR branches during Night Shift runs`
  - Source: https://github.com/benwmaddox/StasisLang/pull/247
  - Synced at: `2026-03-17T12:49:50.785669+00:00`
  - Review decision: `UNKNOWN`
  - When you fix or clarify this feedback, reply on the relevant GitHub review thread when appropriate.
  - Review by chatgpt-codex-connector at 2026-03-17T12:40:34Z: ### 💡 Codex Review

Here are some automated review suggestions for this pull request.

**Reviewed commit:** `290660d65b`
    

<details> <summary>ℹ️ About Codex in GitHub</summary>
<br/>

Codex has been enabled to automatically review pull requests in this repo. Reviews are triggered when you
- Open a pull request for review
- Mark a draft as ready
- Comment "@codex review".

If Codex has suggestions, it will comment; otherwise it will react with 👍.

 


When you [sign up for Codex through ChatGPT](https://openai.com/codex), Codex can also answer questions or update the PR, like "@codex address that feedback".
            
</details>
  - `tools/nightshift.sh:43` **<sub><sub>![P2 Badge](https://img.shields.io/badge/P2-yellow?style=flat)</sub></sub>  Reject preserve mode on detached HEAD**

When `NIGHTSHIFT_BRANCH_MODE=preserve`, the script accepts an empty branch name and continues (`${CURRENT_BRANCH:-DETACHED}`), which means runs started from a detached HEAD will still execute and create commits not attached to any branch. In that scenario the intended “preserve PR branch” behavior is lost and the resulting fixes are easy to strand or lose; this path should fail fast unless a real branch is checked out (or unless `EXPECTED_BRANCH` is provided and matches).

Useful? React with 👍 / 👎.
    - Reply on GitHub: https://github.com/benwmaddox/StasisLang/pull/247#discussion_r2946496401
<!-- NED-INBOX:END -->


* Work through items in docs/reviews/rust-compilation-task-list-2026-03-10.md. All should be completed.



## IN PROGRESS

- None.

## DONE

- None.

## NEEDS INPUT FROM USER

- None.
