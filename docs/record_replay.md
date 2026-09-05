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

## Runtime contract

Recording starts after `main()` and data binding, at the first between-frame boundary. The header
contains exact source, state-layout, toolchain-release, target, and HostFrame-size identities. The
initial simulation snapshot contains only canonical scalar or collection locations whose exact
bits differ from their type default. Zero integers, `false`, positive floating-point zero, and
zeroed collection lanes consume no entries; negative zero and NaN payloads remain bit-exact.

Each completed tick contains:

- the tick number;
- only changed `host_i32` values since the prior reconstructed HostFrame;
- only changed `host_f32` bit patterns since the prior reconstructed HostFrame; and
- one post-`tick()`/post-`render()` simulation-state SHA-256 hash.

A change back to zero is an ordinary stored change. Playback begins with zeroed HostFrame arrays,
applies each tick's sparse changes, publishes the complete reconstructed arrays, runs `tick()`,
runs `render()`, and compares the resulting simulation hash. The first mismatch stops playback
with its tick and expected/actual hashes. Graphics buffers, host request mailboxes, and HostFrame
arrays are excluded from the simulation hash.

Replay requires consecutive ticks beginning at one and an exact match for the recorded source,
state layout, Stasis version/release identity, target OS/architecture, and HostFrame dimensions.
The schema is bounded to 1,000,000 completed ticks and 256 MiB. Files are staged, synced, and
published without replacing an existing recording.

Live code swaps, data reloads, and asset reloads abort a record/replay session. Direct
nondeterministic host operations outside the HostFrame snapshot are not virtualized in schema v1;
their effects will either produce a state-hash divergence or require a later schema extension.
