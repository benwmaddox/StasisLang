# Render parity gate

`samples/render_parity` is the framework-owned current gfx_cmd conformance scene. It
does not depend on a game. One frame contains a clear, two overlapping lines,
one translucent filled rectangle, five resolved atlas-backed sprite instances
(a full-canvas SVG, the same SVG repeated smaller, opaque and translucent SVGs,
and a rotated/scaled opaque SVG), direct UTF-8 text, cached text, and present.
Missing resources use the renderer's same-path placeholder region, but are tested
separately instead of being part of this normal resource fixture.

The checked-in test font is intentionally synthetic. Regenerate it with
`python tools/ci/generate_render_parity_font.py`; it contains only deterministic
bar glyphs and carries no third-party font data.

## Automated gates

Run the portable fixture and capture-manifest checks:

```text
python -m unittest tools.ci.test_verify_render_parity
python tools/ci/verify_render_parity.py
```

Run the real compiler-to-native trace gate through JIT and AOT:

```text
cargo test -p stasis_compiler parity_corpus_covers_shared_lowering_shapes -- --nocapture --test-threads=1
```

The `renderer_command_trace` case compiles
`samples/render_parity/trace.stasis`. It intentionally asserts the exact
current gfx_cmd trace recorded in `capture_manifest.json` as the unsigned and Stasis `i32` trace result, links and executes the AOT object where the host linker is
available, and requires both results to match. Display and density metadata are
present in the fixture but intentionally excluded from the backend-independent
trace.

The native contract/lifecycle matrix is:

```text
cmake -S runtime -B target/render-parity-native -DSTASIS_GRAPHICS_BUILD_SHARED=OFF -DSTASIS_GRAPHICS_BUILD_STATIC=OFF -DSTASIS_BUILD_RUNNER=OFF -DSTASIS_BUILD_SYS=OFF -DSTASIS_MOBILE_RUNTIME_BUILD_TESTS=ON
cmake --build target/render-parity-native --config Release
ctest --test-dir target/render-parity-native -C Release --output-on-failure
```

That matrix covers valid and invalid headers, bounded text spans, initial and
second frames, resize/density generation changes, pause/resume, renderer reset,
failed restore retry, and successful restoration. Renderer failures name the
stage, failure, backend, and surface/renderer generations. Resource diagnostics
also include the handle/path and logical/raster dimensions.

## Pixel captures

Build the canonical SDL-only runtime, then capture desktop frame 1 and frame 2:

```text
$env:STASIS_GFX_LOG_SPRITES = "1"
$env:STASIS_PARITY_CAPTURE_STAGE = "initial_launch"
cargo run -p stasis --release -- play samples/render_parity/main.stasis --watch-dir samples/render_parity --ticks 1 --screenshot target/render-parity/initial.bmp --screenshot-frame 1 --exit-after-screenshot 2>&1 | Tee-Object target/render-parity/initial.log
python tools/ci/verify_render_parity.py --capture target/render-parity/initial.bmp --runtime-log target/render-parity/initial.log --evidence target/render-parity/initial.json --write-evidence --require-load-details --stage initial_launch

$env:STASIS_PARITY_CAPTURE_STAGE = "second_frame"
cargo run -p stasis --release -- play samples/render_parity/main.stasis --watch-dir samples/render_parity --ticks 2 --screenshot target/render-parity/second.bmp --screenshot-frame 2 --exit-after-screenshot 2>&1 | Tee-Object target/render-parity/second.log
python tools/ci/verify_render_parity.py --capture target/render-parity/second.bmp --runtime-log target/render-parity/second.log --evidence target/render-parity/second.json --write-evidence --require-load-details --stage second_frame
```

Exact `sha256_rgba` profiles are appropriate only when the backend, rasterizer,
font asset, drawable size, and dependency versions are fixed. The checked-in
`windows_sdl_d3d11` profile records the exact proof capture from that fixed
configuration. The checked-in `portable` profile instead uses named pixel
regions with documented tolerances.
This prevents driver rounding from hiding a missing SVG, the atlas-backed canvas sprite, or
text layer while still producing a specific stage/region failure.

For Android, package the same project and use the existing device lifecycle
driver. Capture and verify the initial and second frames, then repeat after one
orientation change and one background/resume cycle using stages
`resize_or_density_change` and `resource_restore`. The renderer log must show
`resources restored` with advanced generations before those captures pass.
Device screenshots include system bars or letterboxing, so pass the measured
scene viewport explicitly; for example:

```text
python tools/ci/capture_android_render_parity.py --stage initial_launch --capture target/render-parity/android-initial.png --runtime-log target/render-parity/android-runtime.log
python tools/ci/verify_render_parity.py --capture target/render-parity/android-initial.png --runtime-log target/render-parity/android-runtime.log --evidence target/render-parity/android-initial.json --write-evidence --profile portable --stage initial_launch --viewport 0,896,1080,607
```

The verifier crops and nearest-neighbor normalizes that explicit viewport to
the fixture's 640x360 logical size before applying the named tolerances. It
never guesses a crop.
Runtime evidence is mandatory for captures. The gate verifies the exact command
trace/counts, backend, logical/native/drawable dimensions, font creation, three
restored SVG resources, surface-generation advancement for resize, and a newer
renderer generation after foreground restoration.
Each evidence sidecar records the raw capture hash plus the matching backend and
surface/renderer generation event. Keep one sidecar per stage; reusing an
earlier screenshot under a later lifecycle label fails the binding check.
The desktop renderer emits the provenance event itself while writing the image.
The Android capture driver takes the screenshot and immediately records the
current backend and generations in the same bounded operation. It never accepts
an existing image to relabel after the fact.
All app processes must be force-stopped after device testing.

The optional desktop GL build uses the same fixture and `portable`
profile. Any expected backend difference belongs in a separately named capture
profile with an explicit reason; command traces must remain exact.
