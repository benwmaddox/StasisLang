# Windows game launch integration testing

`samples/windows_launch_smoke` is the canonical Windows startup fixture. It loads a PNG sprite,
an SVG sprite, and a TrueType font, advances the game loop, renders a known 320x180 frame, and
returns distinct nonzero startup codes when any required resource fails.

The Windows-only Rust integration test `apps/stasis/tests/windows_game_launch.rs` copies the fixture
under `target/windows-launch-tests`, gives every child process a 60-second timeout, and exercises:

- `stasis play ENTRY` and manifest-inferred `stasis play` from a nested project directory;
- `stasis run --watch`;
- `stasis tui ENTRY` with a deterministic live script;
- `stasis build --mode release`, followed by the generated AOT executable;
- `stasis package --target desktop --development-build`, followed by the packaged executable.

Every successful path must capture frame 2 as a PNG. The test decodes the capture, verifies its
320x180 dimensions, probes characteristic pixels from both the PNG and SVG sprites, requires the
font load and render-contract diagnostics, and fails boundedly when a process hangs. The exact
Windows Application Control error 4551 may prevent locally generated unsigned AOT DLLs from
loading on a hardened developer machine; only that exact denial may skip the two AOT render
assertions locally. GitHub Windows CI permits generated DLLs and therefore enforces all five paths.

Every graphical path must also report that it presented the shared asset-free `STASIS LOADING`
frame. The runtime pumps initial window events before that presentation, keeping the SDL renderer
responsive on Windows while using the same startup treatment on every native target.
Generated launchers anchor `STASIS_ASSET_ROOT` to their own directory and reject a graphics DLL
whose exported runtime ABI does not match the runner, preventing caller working-directory and
mixed-version installations from degrading into missing assets or undefined runtime behavior.
Windows desktop packages keep the game-named executable at the package root and resolve the
launch sidecar, game DLL, graphics DLL, assets, and metadata from the relative `app/` directory.
This leaves one obvious file for non-developers to launch while keeping the package portable.
Source-tree integration builds select the rebuilt runner and graphics DLL explicitly instead of
using file modification times to guess which copied native artifact belongs to the current build.

The same fixture ships in Windows release archives. `verify.ps1` reruns the five-path matrix using
the bundled `stasis.exe`, its bundled graphics runtime, and archive-local assets. This detects
assembly errors that a source-checkout test cannot, including omitted DLLs, tools, samples, or
runtime assets.

Headless `stasis run` is intentionally outside this graphical matrix and remains covered by CLI
and fresh-runtime tests. `stasis_runner.exe` is an implementation component beneath generated AOT
launchers rather than a separate supported game-start command; executing release and packaged
launchers covers that boundary.
