# Loading screens around content IO

Prepare an IO-free loading state, present at least one loading frame, and only
then start the bounded content IO batch on a later tick. Enter gameplay only
after **every required operation has settled**, and only if the required results
are usable. Failed and cancelled operations are settled, not ready.

Apply this rule whenever sprite, font, audio, level, save, configuration, or other
content IO can block a frame. File reads, decoding, font rasterization, texture
uploads, and synchronous resource creation can all stall. Do not hide those
operations in `main()`, the first `render()`, or a loading-state constructor.

Host/window/context initialization is different: a host must create a surface
before Stasis can draw anything. A native splash or the web package's HTML loading
shell covers that interval. It does not replace the in-game loading state for
later content work. `init_window` requests window configuration; it does not load
content or prove that a frame was displayed.

## Presentation boundary

`begin_frame` and `end_frame` build a command buffer; `end_frame` marks it for
presentation, rather than synchronously swapping the display. The example records
submission in `render`, returns to the host, and allows IO only on a subsequent
`tick`. Its first-frame-presented gate relies on a host that consumes and presents
that render before the next simulation tick. Repeated ticks without a render do
not open the gate; another call in the same tick does not open it either.

The current public `HostFrame` API has no physical-presentation acknowledgement.
Do not describe this flag as a GPU acknowledgement. A custom host that skips
presentation, runs simulation ahead of rendering, or renders offscreen must enforce
the presentation boundary itself before permitting the later tick. A minimized
window cannot promise user-visible pixels. Headless tests below verify guest
ordering, not display presentation.

## Bulk versus incremental

Start a small, bounded batch together when measured content cost is acceptable.
Requesting the image and music together lets the host overlap work. Even a
synchronous batch belongs after the loading frame; that frame remains visible
while the batch blocks. Do not insert sleeps, artificial minimum loading times,
or one-fast-file-per-tick schedules to make progress look animated.

Use genuinely incremental work for large or unpredictable batches: limit requests
or decode/upload work per tick, poll outstanding requests, and return to render.
An asynchronous request does not guarantee that every later poll/upload is cheap.
Measure on target hardware. Incremental scheduling cannot interrupt one blocking
host call; a watchdog checked on later ticks cannot rescue a hung synchronous
operation. Such a host needs bounded IO, cancellation, or worker-thread support.

Use nominal enums for phases and stages (for example `LoadingPhase`), not numeric
phase IDs. Numeric fields are for counts, progress, and elapsed ticks. Progress
means settled / total; failure is not successful progress. `LoadingProgress.complete()`
counts failures, whereas `successful()` additionally rejects them. Never gate
readiness on its smoothed `displayed_percent`: smoothing is presentation only.

## Complete example

[Source](examples/src/loading_screen.stasis) and
[regressions](examples/tests/loading_screen.test.stasis) accompany this guide.
The examples manifest selects `"stdlib": "toolchain"`. To copy the source into a
normal vendor-backed generated project, change its import to
`/vendor/stasis/stdlib/audio.stasis`. Supply `assets/hero.svg`
and `assets/music.wav` in the project assets directory (tiny fixtures are bundled).
The tests inject outcomes without requiring an audio device; asset validation checks the files.
The public API calls are checked against the bundled stdlib.

The loading UI uses only rectangles, so even a missing font cannot prevent its
first frame. Blue means waiting/loading, red means failure; two segments show
settled operations. A bulk load may finish before any intermediate percentage is
rendered, which is fine. For text, use an already-resident font, or load the desired
font with `load_font(path, size)` after the gate and count `font <= 0` as failure.
Do not load that font merely to draw the first "Loading" label. Pre-create cached
`TextRun` labels after the font succeeds; do not recreate them each frame.

The example treats both assets as required. A failed request still lets the other
settle. A request that never settles is cancelled after 600 loading ticks and
enters a terminal error state. This is an example tick budget, not a wall-clock
IO timeout. The error screen remains responsive; an explicit retry should release
old resources and call `prepare_loading` again, requiring a fresh loading frame.

```stasis
import "/.stasis_cache/toolchain/src/stdlib/audio.stasis";

enum LoadingPhase {
    AwaitingFrame,
    Loading,
    Gameplay,
    Error,
}

struct LoadingGate {
    phase: LoadingPhase;
    frame_submitted: bool;
    submitted_tick: i32;
    loaded: i32;
    failed: i32;
    total: i32;
    waited_ticks: i32;
}

global gate: LoadingGate;
global loading_tick: i32;
global hero: ImageAsset;
global music: AudioAsset;
global music_voice: AudioVoice;
global level_number: i32;

function prepare_loading(self: LoadingGate, total: i32): void {
    self.phase = LoadingPhase.AwaitingFrame;
    self.frame_submitted = false;
    self.submitted_tick = 0;
    self.loaded = 0;
    self.failed = 0;
    self.total = total;
    self.waited_ticks = 0;
}

function loading_frame_submitted(self: LoadingGate, tick_number: i32): void {
    if (self.phase == LoadingPhase.AwaitingFrame && !self.frame_submitted) {
        self.frame_submitted = true;
        self.submitted_tick = tick_number;
    }
}

function begin_loading_batch(self: LoadingGate, tick_number: i32): bool {
    if (self.phase != LoadingPhase.AwaitingFrame || !self.frame_submitted || tick_number <= self.submitted_tick) {
        return false;
    }
    self.phase = LoadingPhase.Loading;
    return true;
}

function settle_loading(self: LoadingGate, loaded: i32, failed: i32): void {
    self.loaded = loaded;
    self.failed = failed;
    if (self.loaded + self.failed >= self.total) {
        if (self.failed > 0) {
            self.phase = LoadingPhase.Error;
        } else {
            self.phase = LoadingPhase.Gameplay;
        }
    }
}

function start_content_batch(): void {
    hero.load_image("assets/hero.svg", 64, 64);
    music.load_audio("assets/music.wav");
}

function poll_content_batch(): void {
    let loaded: i32 = 0;
    let failed: i32 = 0;
    if (hero.ready()) {
        loaded += 1;
    } else if (hero.failed()) {
        failed += 1;
    }
    if (music.ready()) {
        loaded += 1;
    } else if (music.failed()) {
        failed += 1;
    }
    gate.waited_ticks += 1;
    // Bounded policy for a request that never reaches a terminal state.
    if (gate.waited_ticks >= 600 && loaded + failed < gate.total) {
        hero.release();
        music.release();
        gate.settle_loading(0, gate.total);
        return;
    }
    gate.settle_loading(loaded, failed);
    if (gate.phase == LoadingPhase.Gameplay) {
        music_voice.play(music, true, 0.5, 0.0);
    }
}

function next_level(): void {
    if (gate.phase == LoadingPhase.Gameplay) {
        // This example's levels are in-memory rules, sharing hero and music.
        level_number += 1;
    }
}

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

function render(): i32 {
    begin_frame();
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
    end_frame();
    gate.loading_frame_submitted(loading_tick);
    return 0;
}
```

`load_image` / `load_audio` returning true means a request was accepted, not that
its result is ready. Immediate rejection sets `Failed`. `ready()` and `failed()`
poll status; `failed()` includes `Cancelled`. Audio playback failure after a
successful decode is a separate device/playback policy: this example tolerates
silence. If audible playback is mandatory, give device initialization/playback
its own settled success/failure operation instead of retrying forever.

For later levels, `next_level` changes in-memory rules and reuses image and audio
ownership; it does not reload common assets. If a transition needs a new level or
save file, prepare a new loading gate first, render it, then read/validate the new
data in a bounded batch. Keep the old level intact until all required results
succeed, and commit the new level atomically. A failed transition should show an
error or return to the old level. Shared assets should remain loaded; release only
resources whose lifetime actually ended. A nominal `LoadingStage` enum can select
startup versus level-specific batches without assigning magic numeric stages.

## Regression coverage and limits

Run `stasis test` in `examples/` (vendored copies have the same layout). The tests
exercise the production gate with deterministic outcomes: no render, same-tick
submission, later-tick admission exactly once, partial success, full success,
failure/cancellation through the real polling path, tick-budget exhaustion, retry
reset, and asset reuse during an in-memory transition.
They do not perform disk decoding or prove physical display presentation. When
integrating with a host, also capture a loading-frame PNG and a startup/transition
MP4 and inspect them, including a missing asset run. Verify that the first content
request occurs after the loading frame, failure exits loading, and gameplay never
observes partial required resources.
