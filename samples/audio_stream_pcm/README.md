# Public AudioStream PCM fixture

Package with `stasis package --target web --out build/web --development-build`
from this directory.
The fixture uses the public AudioStream and AudioVoice APIs and retains all eight
streaming imports plus five effect voice imports. Calls with an unloaded effect
exercise the binding surface without adding audio to the PCM measurement.
Each 800-frame block contains a 480 Hz square wave at 48 kHz. Left
amplitude is 0.5 and right amplitude is 0.25; the right channel must remain half
the left channel. `muted` produces zero PCM and `volume` sets the left amplitude.
The swap hook closes and reopens the stream. No asset audio is used.

From the repository root, run `node tools/run_audio_stream_browser_acceptance.mjs`
after packaging to `build/web` (or pass the package directory as the first argument).
Set `STASIS_BROWSER_EXECUTABLE` when Chrome is not at its standard Windows path.
The bounded CDP harness starts Chrome with autoplay restricted, sends a real input
gesture, and captures rendered stereo PCM from a WebAudio graph tap. It asserts
the 480 Hz period, channel ratio, mute and volume changes, reload, close/reopen,
page lifecycle suspension, and injected device failure/recovery. The receipt and
interleaved float32 captures are under `target/audio-stream-browser`; the receipt
binds the exact package and provenance files with SHA-256. This tests browser
rendering, not physical speakers. Visibility and queue ownership also have isolated
tests in `runtime/web/tests/audio_suspended_queue.test.mjs`.
For an optimized package, pass its directory, an evidence directory, and
`--startup-only` to the harness. This checks real startup/audio and reload without
requiring the development-only mutable global exports used by control tests.
Coordinate physical Android listening separately; this fixture does not prove it.

For native conformance, first build the compiler and runtime from the same source
using the repository-pinned SDL dependencies, then run:

```
python tools/run_audio_stream_native_acceptance.py --stasis path/to/stasis.exe --runtime path/to/stasis_graphics.dll
```

This runs the fixture through native JIT and the runtime's deterministic recording
path using SDL dummy drivers. It requires FFmpeg, asserts two seconds of 48 kHz
stereo PCM, checks 480 Hz and the 2:1 channel ratio after MP3 decoding, and records
compiler/runtime/fixture/capture checksums. The selected runtime must be the
compiler's canonical sibling library (for example, `stasis_graphics.dll` beside
`stasis.exe`); stage the intended pair together first. The harness verifies the
pair with `editor-info` before recording and refuses a receipt if either binary
changes during capture. An environment override cannot substitute another runtime.
Use a fresh `--output` directory for
each run. This does not test physical output or claim an official release.

The compiler, Web runtime, and stdlib must ship together through the official
release provenance workflow in `docs/release_provenance.md`. Do not transplant
this runtime into a consumer's existing release or vendor snapshot.
The platform conformance follow-up can reuse this fixture and the import/PCM
receipts; the Android listening follow-up should compare the same 480 Hz stereo
signal through physical output after repinning to that matching official release.
Marble Run and Gambit Guard should both rebuild against that release; the fixture
guards their reported imports but does not replace consumer deployment acceptance.
