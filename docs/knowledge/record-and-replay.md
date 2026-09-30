# Record, replay, and captures

<!-- tags: replay, input, capture, record, deterministic -->

Use replay to reproduce a particular input-driven run with the same game and
toolchain identity. A recording stores the host observations the game read and
checks simulation state during playback. It does not save pixels or replay
recorded game-state mutations.

```text
stasis play src/main.stasis --record-replay runs/first.replay.json --ticks 600
stasis --workspace . replay runs/first.replay.json
stasis --workspace . record src/main.stasis --replay runs/first.replay.json --output artifacts/first.mp4 --width 1280 --height 720 --fps 60 --frames 600
```

The first command records a play session. The second executes normal ticks and
rendering while verifying state checkpoints. The third captures its normal
rendered frames; `--frames` must equal the replay's tick count. `--fps`
sets media timestamps and does not change the number of simulation ticks.
For a still image, record one frame to an extensionless output directory and
inspect its `frame-000001.png`. Inspect the actual capture when judging visible
behavior. MP4 encoding requires FFmpeg on `PATH`.

Replay begins after `main()` and requires exact source, layout, toolchain,
runtime, and prepared-asset identity. A source or vendor change can make an
old recording incompatible. Schema-v3 input usage is selected from reachable
HostFrame reads; unused fields do not enter the recording. Checkpoint and
final hashes detect divergent simulation, while rendering remains an
independent visual observation.

Input-only replay rejects reachable host observations it cannot reproduce:
wall clock and sleep, storage, clipboard, platform responses, network input,
asynchronous asset readiness, live code/data swaps, unknown externs, and
host-dependent audio queries. Game-owned deterministic RNG state is ordinary
simulation state. Use a focused Stasis test or a controlled host profile for
a feature outside replay's contract; do not interpret a replay hash as proof
of pixels, audio output, or physical presentation.

The [compiled input and display example](examples/src/display_spaces.stasis)
shows a `HostFrame` read feeding both game intent and a rendered projection.
For routine behavioral assertions, begin with [focused tests](testing.md).
