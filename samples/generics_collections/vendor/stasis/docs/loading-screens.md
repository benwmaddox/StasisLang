# Loading screens around content IO

<!-- tags: loading, assets, presentation, failure, bounded-io -->

Present an IO-free loading frame before starting content IO on a later tick.
Enter gameplay only when every required operation has settled and succeeded.
Failed or cancelled operations are settled, but not ready.

## Ordering

1. Prepare loading state without requesting content.
2. Submit a loading frame from `render()`.
3. On a later tick, start the bounded batch.
4. Poll completion; enter gameplay on success or an error state on failure.

The host owns graphics construction. Do not call `begin_frame` or `end_frame`.
The [example](examples/src/loading_screen.stasis) records submission and waits
for a later tick before requesting its image and audio. This gate assumes the
host presents that render before the next tick. `HostFrame` has no physical
presentation acknowledgement; custom hosts must enforce that ordering.
Ticks without a render never open the gate.

## Wire it into a game

`LoadingGate`, `prepare_loading`, and the other helpers below are defined in
the [example source](examples/src/loading_screen.stasis), not built-in APIs.
Copy that implementation into your project and replace its asset paths and
gameplay drawing. Initialize the gate with the number of required assets;
keep requests out of `main()` and `render()`:

```stasis
function main(): i32 {
    init_window(640, 360, "Loading example");
    loading_tick = 0;
    level_number = 1;
    gate.prepare_loading(2);
    return 0;
}

function tick(): i32 {
    loading_tick += 1;
    if (gate.begin_loading_batch(loading_tick)) {
        start_content_batch();
    }
    if (gate.phase == LoadingPhase.Loading) {
        poll_content_batch();
    }
    return 0;
}
```

The guard admits the batch once, after a render on an earlier tick:

```stasis
function begin_loading_batch(self: LoadingGate, tick_number: i32): bool {
    if (self.phase != LoadingPhase.AwaitingFrame || !self.frame_submitted || tick_number <= self.submitted_tick) {
        return false;
    }
    self.phase = LoadingPhase.Loading;
    return true;
}
```

The admitted batch uses the asset APIs directly:

```stasis
function start_content_batch(): void {
    hero.load_image("assets/hero.svg", 64, 64);
    music.load_audio("assets/music.wav");
}
```

`poll_content_batch()` checks each asset's `ready()` and `failed()` state,
updates settled counts, and enforces a timeout. Its `settle_loading()` helper
selects `Gameplay` only when both assets succeed, or `Error` when a required
asset fails. Add gameplay updates under the `Gameplay` phase in `tick()`.

Draw an IO-free loading state, then record submission at the end of `render()`:

```stasis
function render(): i32 {
    clear(0.04, 0.06, 0.1, 1.0);
    if (gate.phase == LoadingPhase.Gameplay) {
        draw_sprite(hero.sprite_ref, 288.0, 148.0, 64.0, 64.0, 0, 255);
    } else {
        // IO-free status: blue = waiting/loading, red = error.
        let red: f32 = 0.1;
        if (gate.phase == LoadingPhase.Error) {
            red = 1.0;
        }
        fill_rect(120.0, 140.0, 400.0, 12.0, red, 0.3, 0.6, 1.0);
        // One segment per settled operation, including failures.
        let segment_x: f32 = 120.0;
        for (let i: i32 = 0; i < gate.loaded + gate.failed; i += 1) {
            fill_rect(segment_x, 170.0, 192.0, 20.0, red, 0.7, 0.6, 1.0);
            segment_x += 200.0;
        }
    }
    gate.loading_frame_submitted(loading_tick);
    return 0;
}
```

## Bounded work and progress

Start a small batch together when its measured cost is acceptable. For large
batches, bound requests and decode/upload work per tick, poll outstanding
operations, and return to render. One blocking host call cannot be interrupted
by a watchdog checked on later ticks. Avoid sleeps or artificial loading delays.

Use enums for phases. Show progress as settled / total; gate readiness on
successful results, not smoothed display progress. Bound waiting and release
failed or cancelled resources. Window/context setup needs the host's native
splash or Web shell before the first drawable frame.

## Example and verification

Copy and prepare the example workspace using [these instructions](README.md#executable-backing).
The bundled `assets/hero.svg` and `assets/music.wav` are its content fixtures.
Run `stasis --workspace build/knowledge-examples test`; the
[tests](examples/tests/loading_screen.test.stasis) cover no-render and same-tick
rejection, later-tick admission, partial/full success, failure, timeout, retry,
and asset reuse. They verify guest ordering; disk decoding and presentation
also need host captures.

The inspected [success](media/loading-screen/success.mp4) and
[failure](media/loading-screen/failure.mp4) recordings show loading, partial
progress, then gameplay or an error. Inspect the first three frames: these
small assets finish quickly. Stills: [loading](media/loading-screen/loading.png),
[progress](media/loading-screen/progress.png), [gameplay](media/loading-screen/gameplay.png),
and [error](media/loading-screen/error.png).

From the project root, play the copied example or capture it. Repeat the
capture with an invalid asset path to check the error state:

```text
stasis play build/knowledge-examples/src/loading_screen.stasis
stasis --workspace build/knowledge-examples record src/loading_screen.stasis --output loading.mp4 --width 640 --height 360 --fps 60 --frames 60
```

Verify that IO follows the loading frame, failure exits loading, and gameplay
never observes partially ready required resources.
