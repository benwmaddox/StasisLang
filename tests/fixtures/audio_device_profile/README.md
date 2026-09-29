# Headless audio device profile fixtures

`broken.stasis` disables its stream after the first refused push. `retry.stasis`
uses `AudioStream.frames_wanted()` and keeps trying after a transient refusal.
The version-one profiles deliberately run different producer and device rates,
pause and resume the virtual device, and inject a producer stall.
`tick-120-callback-50-short-pause.json` covers a 10..30 ms pause whose state
changes cross 120 Hz guest ticks and 50 Hz callbacks.
`resume-once.stasis` pushes only on the first guest tick after that pause; its
expected first audible callback frame is 960.

The CLI integration test copies one source and one profile into a temporary
toolchain-stdlib project and checks that the broken producer fails audio
validation while the retrying producer passes.
