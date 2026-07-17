# Recording the desktop live TUI

This runbook describes a repeatable desktop capture for the persistent live TUI. It is intentionally separate from the runtime protocol: recording should observe the real process and must not replace the interactive workflow with JSON playback.

## Prepare the session

1. Build the debug runner with `cargo build -p stasis`.
2. Start a small running game and the live workspace in two ordinary windows. Size the real TUI window entirely above the taskbar, with the game beside it.
3. Capture the TUI and game as direct window sources on a fixed canvas. Crop and scale those sources to fill the frame without capturing the desktop or taskbar. Use display capture only as a diagnosed fallback, with an explicit crop that excludes the taskbar. Run a short smoke recording first and inspect a frame before the full take.
4. Keep OBS minimized. Do not click the classic console while recording; QuickEdit can suspend its input/output and make Stasis appear frozen. Bring it to the foreground with the window API or keyboard focus only.
5. Stop and clean up the runner, terminal host, and OBS after every take. Reject any take containing an unrelated window, notification, JSON envelope, or a blank game surface.

## Walkthrough shape

Say this disclosure at the opening and show it briefly on screen:

> This demonstration was written, controlled, narrated, and recorded entirely by AI. The interactions are scripted for clarity, but the compiler, tests, live edits, and running game shown here are real.

The story is not merely that source can be edited while a window is open. Make four Stasis-specific claims visible and falsifiable:

1. A changed function becomes active in the existing native process without restarting the match.
2. Compiler validation and project tests gate the swap, and the accepted pointer change commits between ticks.
3. Runtime state such as the current score and rally count survives the function replacement.
4. Concise live values and visible gameplay distinguish the old behavior from the new behavior, while an invalid or incompatible edit leaves the accepted game running.

Use a real autonomous Pong baseline with aligned ball/paddle collision bounds, visible `0-0` score digits, scoring, and a small `obstacle_enabled()` function. The baseline returns `false`. The tick and render functions both call it, so replacing this one function can add two smaller center bumpers with a passable gap, plus their collision behavior, without changing state layout. Keep `left_score`, `right_score`, `rally_hits`, `obstacle_hits`, `speed`, and `swaps` visible in the concise state pane.

Record this sequence:

1. Let Pong play unattended until a point or several paddle contacts establish that the match is real. Point out that `obstacle_hits` remains zero in the concise live-state pane before the obstacle exists.
2. Open `obstacle_enabled` through completion. Change its return expression from `false` to `true`.
3. Press Ctrl+Enter once. Hold on the concise transcript confirmation, `Hot swapped <= N ms | tests passed`. A clean successful apply closes the editor automatically. Keep the game visible so the viewer can see that the obstacle appears while the ball and score do not reset.
4. Hold on the live game and concise state. The counter must advance after the ball visibly rebounds from a horizontal or vertical bumper face. Abort or revise the take if it remains zero.
5. Reopen `obstacle_enabled`, replace `return true;` with `return 1;`, and submit it. Show the deterministic bool-versus-i32 diagnostic while the last accepted game continues and its counters still advance. Discard the rejected buffer before finishing.
6. End on the accepted obstacle version running unattended, with the preserved match score and incremented `swaps` count visible.

Narrate each hypothesis before its action. The displayed duration is an end-to-end upper bound from Ctrl+Enter submission until the TUI processes the response confirming that the pointer is active; the replacement runs on the next tick. Do not present it as compiler-only or exact first-execution time. Connect visible gameplay to the concise live values, and do not use one-off `:inspect` calls as filler. Type briskly, while pausing long enough on completion choices, apply timing, live-state evidence, and rejection for a phone viewer to read them.

## Capture acceptance

- The TUI and game fill the capture together; no OBS chrome, unrelated desktop, or taskbar is visible. The real TUI window does not overlap the taskbar.
- The default live-state pane uses concise `name = value` rows. It omits source/type detail, spatial fields, and collection `length`/`max_length` bookkeeping; explicit `:inspect PATH` remains the detailed escape hatch.
- The accepted apply line includes the measured active-confirmation upper bound. The score/rally state visibly survives that apply.
- `obstacle_hits` remains zero before the swap and advances only after a visible bumper collision.
- The accepted recording contains audible narration and the AI disclosure. Generate one cached audio section per beat, keyed by text, voice, model, and settings; unchanged sections must not consume API credits again.
- Sample opening, experiment, rejection, and closing frames. Confirm the game changes and the TUI shows real completion, validation, and swap feedback.
- Copy the accepted MP4 to a phone-share directory and verify it with the same LAN URL used by the viewer.

For a fully scripted take, keep the action and narration scripts beside the captured artifact, record the exact command sequence and model disclosure, and retain rejected takes only as local QA evidence.
