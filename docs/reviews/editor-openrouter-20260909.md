# Desktop editor first-message and OpenRouter review

RootbeerMaze3 failed before provider transport because its serialized editable sources exceeded
256 KiB. Its standalone trial also lacked the `.env` that the earlier live demo launcher supplied
through process environment. Finally, endpoint routing discarded the valid `cerebras/fp16` tag.

The editor now sends a new objective once, loads workspace-scoped provider settings, and starts
with symbol metadata rather than every source body. Read-only symbol requests retrieve the exact
local snapshot. Proposal acceptance, atomic application, and focused validation remain intact.
The desktop prompt carries the successful demo's local-language, live-state preservation, and
related source/test batching guidance.

OpenRouter defaults to a 400 tokens/second endpoint qualification threshold. The live workspace
smoke selected `openai/gpt-oss-120b` on `cerebras/fp16`. Authenticated endpoint metadata reported
673 tokens/second median. The small connectivity request took 826 ms overall and measured about
212 completion tokens/second including startup overhead. Endpoint qualification is not a
guarantee of every request's measured rate. No user source was changed by that smoke.

Validation covers the 93 focused editor tests, 130 AI unit tests and five task-action tests,
including large source snapshots, exact symbol reads, a simulated read/propose round trip,
first-message/retry identity, scoped configuration, endpoint variants, and secret-safe failures.

Visual evidence: [native failure fixture](../evidence/editor-send-failure-20260909.png), inspected
after capture. It shows the objective once in the thread, a wrapped actionable routing error,
and the Reconnect action. This is a deterministic failure fixture, not a live failed request.
Reproduce with `STASIS_EDITOR_EVIDENCE_FAILURE=1`, `STASIS_EDITOR_EVIDENCE_PNG=<output.png>`, and
`python tools/cargo_cache.py run -- cargo test -p stasis --bin stasis capture_native_task_timeline -- --test-threads=1`.

Theory gained: fast interactive editing requires both a qualified exact endpoint and a focused
source request. Project size should not prevent transport; source reads and proposal validation
remain separate bounded steps. A restored task keeps its message and never replays a request
automatically on reopening the editor.
