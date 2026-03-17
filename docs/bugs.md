# Bugs

Allowed states: `READY`, `IN PROGRESS`, `DONE`, `NEEDS INPUT FROM USER`

If you use a cross-repo inbox, it may maintain a generated `NED-INBOX` block under `## READY`. Treat those entries like normal bugs. Those synced items should include direct GitHub links for the source PR and individual review threads so the agent can reply when appropriate.

## READY

<!-- NED-INBOX:START -->
- [P1][PR #246] Address review feedback for `Address issue #244: Rust compilation task list`
  - Source: https://github.com/benwmaddox/StasisLang/pull/246
  - Synced at: `2026-03-17T01:00:09.740536+00:00`
  - Review decision: `UNKNOWN`
  - When you fix or clarify this feedback, reply on the relevant GitHub review thread when appropriate.
  - Review by chatgpt-codex-connector at 2026-03-16T19:36:12Z: ### 💡 Codex Review

Here are some automated review suggestions for this pull request.

**Reviewed commit:** `ff3e9b8493`
    

<details> <summary>ℹ️ About Codex in GitHub</summary>
<br/>

Codex has been enabled to automatically review pull requests in this repo. Reviews are triggered when you
- Open a pull request for review
- Mark a draft as ready
- Comment "@codex review".

If Codex has suggestions, it will comment; otherwise it will react with 👍.




When you [sign up for Codex through ChatGPT](https://openai.com/codex), Codex can also answer questions or update the PR, like "@codex address that feedback".
            
</details>
  - `crates/stasis_compiler/src/backend/aot.rs:982` **<sub><sub>![P2 Badge](https://img.shields.io/badge/P2-yellow?style=flat)</sub></sub>  Avoid asserting Cranelift callee IDs in CLIF markers**

This fixture hard-codes `call fn39` and `call fn38`, but Cranelift function reference numbers are allocator/order-dependent and can change when imports or lowering order shift, even if generated behavior is still correct. As a result, unrelated backend refactors can make `parity_corpus_covers_shared_lowering_shapes` fail spuriously and block normal development; the assertion should target a stable marker (e.g., helper symbol intent or generic call presence) instead of exact `fnNN` IDs.

Useful? React with 👍 / 👎.
    - Reply on GitHub: https://github.com/benwmaddox/StasisLang/pull/246#discussion_r2942496338
<!-- NED-INBOX:END -->

## IN PROGRESS

- None.

## DONE

- None.

## NEEDS INPUT FROM USER

- None.
