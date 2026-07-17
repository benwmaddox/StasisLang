# Recording the desktop live TUI

This runbook describes a repeatable desktop capture for the persistent live TUI. It is intentionally separate from the runtime protocol: recording should observe the real process and must not replace the interactive workflow with JSON playback.

## Prepare the session

1. Build the debug runner with `cargo build -p stasis`.
2. Start a small running game and the live workspace in two ordinary windows. Snap the TUI to the left and the game to the right with Win+Left and Win+Right.
3. Use a monitor/display capture in OBS when OpenGL window capture is black. Run a short smoke recording first and inspect a frame before the full take.
4. Keep OBS minimized. Do not click the classic console while recording; QuickEdit can suspend its input/output and make Stasis appear frozen. Bring it to the foreground with the window API or keyboard focus only.
5. Stop and clean up the runner, terminal host, and OBS after every take. Reject any take containing an unrelated window, notification, JSON envelope, or a blank game surface.

## Narrated walkthrough shape

The opening must say, both aloud and on screen:

> This demonstration was written, controlled, narrated, and recorded entirely by AI. The interactions are scripted for clarity, but the compiler, tests, live edits, and running game shown here are real.

Then use a simple Pong-like game and make the experiment visible:

- establish the baseline for several rallies;
- try a speed increase, observe it, and undo it when it is less readable;
- add a visible obstacle plus its matching tick behavior, validate, and observe the swap;
- tune the obstacle once instead of endlessly adding features;
- attempt one unsafe state-layout change and show deterministic rejection;
- finish on the accepted version running unattended.

Narration should name the hypothesis before each edit, explain that completion is compiler-backed, and distinguish visual evidence in the game from concise semantic live state. It should never imply that a human operator is typing off camera. Pause three seconds on completion choices, validation, swaps, undo, and rejection so a phone viewer can read them.

## Capture acceptance

- The TUI and game fill the capture together; no OBS chrome or unrelated desktop is visible.
- The default live-state pane uses concise `name = value` rows. It omits source/type detail, spatial fields, and collection `length`/`max_length` bookkeeping; explicit `:inspect PATH` remains the detailed escape hatch.
- The recording contains audible narration and the AI disclosure, not just an empty audio track.
- Sample opening, experiment, rejection, and closing frames. Confirm the game changes and the TUI shows real completion, validation, and swap feedback.
- Copy the accepted MP4 to a phone-share directory and verify it with the same LAN URL used by the viewer.

For a fully scripted take, keep the action script and narration script beside the captured artifact, record the exact command sequence and model disclosure, and retain rejected takes only as local QA evidence.
