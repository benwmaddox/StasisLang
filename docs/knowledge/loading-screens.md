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

Capture your integration, including a failed asset run:

```text
stasis --workspace build/knowledge-examples record src/loading_screen.stasis --output loading.mp4 --width 640 --height 360 --fps 60 --frames 60
```

Verify that IO follows the loading frame, failure exits loading, and gameplay
never observes partially ready required resources.
