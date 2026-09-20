# Record and replay

Stasis record/replay captures the host observations visible to a game, then runs the normal JIT
`tick()` and `render()` entries again. It does not store graphics command buffers or rendered
pixels, and it does not apply recorded gameplay-state changes during playback.

Record an interactive or input-script-driven play session:

```powershell
stasis play src/main.stasis --record-replay runs/first.replay.json --ticks 600
```

Replay it with the same project and toolchain build:

```powershell
stasis --workspace . replay runs/first.replay.json
```

Replay directly to an existing fixed-rate PNG or MP4 capture:

```powershell
stasis --workspace . record src/main.stasis `
  --replay runs/first.replay.json `
  --output artifacts/first.mp4 `
  --width 1280 --height 720 --fps 60 --frames 600
```

`stasis record` also accepts `--record-replay PATH` to publish a replay session alongside its PNG
or MP4 output.

## Compact runtime contract

Recording starts after `main()` and data binding, at the first between-frame boundary. The header
contains exact source, persistent-state layout, compiler layout, toolchain release, target,
CLI/runtime-binary, effective prepared-asset-manifest, HostFrame-v4, and input-usage identities. The
initial simulation snapshot contains only canonical scalar or collection locations whose exact
bits differ from their type default. Zero integers, `false`, positive floating-point zero, and
zeroed collection lanes consume no entries; negative zero and NaN payloads remain bit-exact.

Schema v2 records a whole-game union of raw HostFrame fields read by reachable `main`, `tick`,
`render`, and their called helpers. Reads forwarded through collection parameters and local aliases
are included. A statically known index selects one exact slot; an unresolved dynamic index selects
the conservative keyboard, pointer, display, or raw-lane family. Unreachable helpers and HostFrame
fields the game never reads do not enter the file. Field selection is compile-time metadata, not
conditional per-tick tracking.

The input stream contains:

- the initial observed-input baseline;
- ordered segments with tick gaps/run lengths and only changed observed values;
- exact i32 values and f32 bit patterns, including changes back to zero;
- a post-simulation checkpoint hash every 256 ticks; and
- the explicit final tick and final post-simulation hash.

Long unchanged runs use one segment rather than one frame/hash object per tick. Playback still
executes every tick through the ordinary simulation path. It zeroes a complete HostFrame, projects
the recorded observed slots, runs `tick()`, and then runs `render()` for visual playback. Sparse
verification reports the bounded interval after the last successful checkpoint and stops at the
first checkpoint that proves divergence. Graphics buffers, host request mailboxes, and HostFrame
arrays are excluded from the simulation hash. Simulation-only verification does not invoke a
separate renderer; normal visual replay uses the normal renderer.

A synthetic 10,000-tick held-input run serializes to exactly 5,068 bytes in schema v2 versus 1,299,320 bytes in schema v1, making v1 about 256× larger. The v2 measurement includes all 39 required 256-tick checkpoints.

Replay requires consecutive simulation ticks beginning at one and the exact recorded identity.
The schema is bounded to 1,000,000 completed ticks, 4,096 checkpoints, and 256 MiB; recording
enforces the encoded-size budget incrementally. Files are staged, synced, and published without
replacing an existing recording. Schema-v1 files remain readable with their original per-tick hash
behavior.

For `record --replay`, `--frames N` is the number of replay simulation ticks and must equal the
recording's total tick count. `--fps` controls the output presentation timestamps and audio/video
container rate; it does not skip, duplicate, or interpolate simulation ticks. The existing PNG/MP4
pipeline receives the normally rendered replay frames and performs the same final-state check.

Compact input-only sessions reject reachable observations that are not captured: direct wall
clock/sleep calls, storage, clipboard and other platform responses, network/random network seeds,
code swap, unknown externs, asynchronous asset readiness, and host-dependent audio queries. Game-
owned deterministic RNG state is ordinary simulation state. Rendering/audio output and deterministic
asset measurements are permitted under the exact runtime/asset identity. Adding another host
observation requires an explicit recorded/virtualized profile; a state-hash failure is not used as
a substitute for declaring it reproducible.

Live code swaps, data reloads, and asset reloads abort a record/replay session. Direct
nondeterministic host operations outside the HostFrame snapshot are not virtualized in schema v1.
Schema v2 currently ships in the desktop JIT commands above. Packaged native, Web, and Android
hosts require the same frame-projection and identity contract before they can advertise replay
support; no cross-target bit-identity guarantee is implied.
