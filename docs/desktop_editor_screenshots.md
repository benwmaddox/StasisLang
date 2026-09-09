# Desktop editor game screenshots

The editor's **Attach game screenshot** command captures the independent live
game window, including its current rendered state while gameplay is paused.
It waits for verified PNG completion and shows a preview. Capture does not
send an AI request; the next task reply includes that task's attached images.

The selected transport and model must explicitly support image input. The
current supported pairing is the installed Codex transport with `gpt-5.6-sol`,
the repository's default and visual-critic model. Unknown model names and the
current OpenRouter transport fail closed. Capability is checked again when
sending, so an attachment cannot silently become a text-only request.

Each attachment retains its originating task, source, content SHA-256, upload
state, and analysis state. The preview also shows the runtime identity and
scheduling/completion ticks. The provider receives a task-scoped snapshot;
later attachments cannot be marked uploaded by an earlier request's result.
Changed image content is rejected before sending. Upload and analysis are
marked complete only after successful provider completion; failures,
cancellation, and retries preserve explicit state.

Runtime PNG verification has a five-second deadline. The editor waits at most
15 seconds for capture completion, and provider requests have a 120-second
timeout. Cancel, reconnect, and shutdown invalidate pending capture results.
A result for an inactive task is discarded; a late result is never attached
to another active task.
Runtime reconnection does not replay an old capture automatically: request a
new screenshot for the current game state.

Validation uses deterministic runtime, editor, and task-controller tests plus
`desktop_screenshot_capture`, which runs a real game against the native SDL
runtime, pauses it, verifies the completed PNG and hash, and retains
`target/task515-evidence/live-game.png` and `capture.json`. Build the runtime
fresh and set `STASIS_RUNTIME_DLL_PATH` to its `stasis_graphics.dll` before
running that integration test through `tools/cargo_cache.py`.

The `captured_tick` field denotes the boundary where verified capture evidence
became available. It does not assert an exact presentation timestamp. Paused
captures preserve the gameplay tick.
