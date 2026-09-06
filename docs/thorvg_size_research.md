# ThorVG size research (task 518)

## Recommendation

The requested GL-only SVG profile is a useful prototype candidate, but it is not
a drop-in size switch for Stasis: it replaces the current pixel-buffer backend
and requires GL context/texture integration. Its size advantage is unmeasured.
For the lowest-effort change to the existing runtime, Stasis already
custom-compiles a small ThorVG feature set. Keep CPU rasterization,
SVG parsing, the raw loader, C API, and file I/O. First measure release compiler
and linker size settings, then disabling partial rendering. Treat removing worker
threads as a size-versus-asset-load-latency experiment. There is no measured byte
saving in this report: the available Windows CMake installation could not find a
C/C++ compiler. Do not interpret omitted source volume as installed binary savings.

## What the application needs

The pinned version is 1.2.0, commit
`5654bbbb13c518c93ce159569838b329c0af85a7`, under MIT; see
[provenance](../runtime/third_party/thorvg/STASIS_PROVENANCE.md).
[stasis_svg.cpp](../runtime/stasis_svg.cpp) loads a file or an in-memory SVG,
determines its natural size, applies contain-fit sizing and translation, draws
once into a fresh straight-alpha RGBA buffer, synchronizes, and destroys the
canvas. Texture upload happens outside ThorVG. Four persistent workers perform
internal work; a bridge mutex serializes asset bakes.

This is asset rasterization, not a retained animated ThorVG scene. However,
removing SVG itself would break runtime assets and density-dependent rebaking
described in [the runtime README](../runtime/README.md). Prebaking all SVGs would
be a separate asset-pipeline/product change, trading native code for packaged
rasters and constraining resolution and live asset changes.

## Requested highly stripped GL profile

The proposed path is `SVG asset -> SVG loader -> ThorVG GL renderer`. Evaluate
it separately from the CPU baseline. Upstream's currently published
[Meson options](https://github.com/thorvg/thorvg/blob/main/meson_options.txt)
accept `gl`, `svg`, thread/partial booleans, and an empty extra-feature array.
The following is a candidate command for a **full upstream source checkout**,
not for Stasis's reduced vendor directory; verify options against the chosen
commit before using it:

```powershell
meson setup target/thorvg-gl-size path/to/full/thorvg --buildtype=minsize --default-library=static '-Dengines=gl' '-Dloaders=svg' -Dthreads=false -Dpartial=false '-Dextra=[]' '-Dbindings=capi' '-Dsavers=[]' -Dfile=true -Dsimd=false -Dlog=false -Dtests=false
meson compile -C target/thorvg-gl-size
```

Keep the C binding for the current bridge API style and file I/O for existing
callers. For an OpenGL ES target, investigate `-Dextra=opengl_es` instead of the
empty array; desktop GL and GLES builds must be tested separately. Do not add
CPU to `engines` merely to make the existing smoke test pass: that would no
longer measure the requested GL-only profile. LTO is a separate comparison.

The local checkout cannot build that profile as supplied: it contains only
`src/renderer/cpu_engine`, no GL engine sources or upstream Meson project.
Its CMake file explicitly selects CPU files. Enabling `THORVG_GL_ENGINE_SUPPORT`
alone would reference missing `tvgGlRenderer.h`; disabling CPU alone makes
`SwCanvas::gen()` return null in `src/renderer/tvgCanvas.cpp`. The existing
`stasis_svg.cpp` calls `tvg_swcanvas_create`, so that change would fail asset
loading rather than transparently select GL. A future prototype must obtain the
full source at the pinned commit, preserve MIT provenance, and use its GL source
closure and platform dependencies. Updating ThorVG versions is a separate
compatibility variable.

The pinned C API header documents `tvg_glcanvas_create` and
`tvg_glcanvas_set_target`: the latter accepts a GL context and framebuffer ID,
not a host pixel buffer. It requires a suitable current context if display and
surface handles are omitted. The header also says GL does not support smart
render mode; disabling partial rendering fits this candidate.

Two integration choices need different measurements:

| Route | Required work | Consequence |
| --- | --- | --- |
| Render into an offscreen FBO, read RGBA back, retain the current bridge contract | Context/FBO lifecycle, synchronization, readback, orientation and alpha checks | Preserves downstream SDL texture upload but adds GPU-to-CPU transfer and upload; not evidence of a speed improvement. |
| Render into a GPU texture consumed directly by Stasis | Explicit compatible renderer/context ownership, texture sharing, GL state restoration, render-thread scheduling, context-loss recovery | Avoids readback but changes resource publication and the bridge boundary. |

The shipping renderer calls `SDL_CreateRenderer(g_window, NULL)` in
`runtime/stasis_graphics.c`; that does not establish a GL-only backend contract.
Do not assume SDL textures are shareable GL texture names on all platforms.
Android GLES and iOS renderer compatibility require explicit validation before
claiming this as a common desktop/mobile solution. Drawing SVGs directly each
frame is another architectural choice, unlike the current cached asset bake.

Acceptance for the GL prototype: first link and run a context-owning harness
that loads the existing viewport fixture, draws into an FBO, synchronizes, and
checks readback dimensions, clipping, contain-fit padding, orientation and alpha.
Compare CPU/GL corpus images with a documented pixel tolerance for rasterizer
differences, then exercise real SDL texture publication, reload, density changes,
and context recreation. Measure stripped linked size including integration code
and platform dependencies, GPU memory, and bake latency. Library-only compilation
does not establish application compatibility or total package savings.

No GL build or rendering result is claimed here: Meson was not found on PATH,
the GL source closure is absent, and the earlier isolated CMake configure found
no C/C++ compiler. The supplied options have been researched, but compiling and
running this candidate remains an explicit follow-up experiment. The evidence
does not yet support calling GL-only the lowest-effort choice for this runtime.

## Current feature budget

The [custom CMake target](../runtime/third_party/thorvg/CMakeLists.txt) lists 36
C++ translation units and builds a static library. Its
[config.h](../runtime/third_party/thorvg/config.h) enables CPU, SVG, C API, file
I/O, partial rendering, and threading. Raw loading is unconditional in
`src/renderer/tvgLoaderMgr.cpp`; there is no raw-loader feature switch.

| Feature | Current state | Decision |
| --- | --- | --- |
| CPU renderer, SVG | Included | Required for current runtime SVG assets. |
| GL/WebGPU backends | Omitted | No further savings from disabling them. GPU texture presentation does not require a ThorVG GPU backend. |
| Lottie, PNG/JPEG/WebP, font/media loaders, GIF saver | Omitted | Keep omitted; generic animation/text/saver source files do not mean their format loaders are enabled. |
| Tools, examples, upstream tests, logging, SIMD/OpenMP macros | Omitted or undefined | No additional feature-toggle savings in this build. |
| Partial rendering | Enabled | Best feature-removal candidate: every bridge call creates a fresh canvas and requests a clear/full draw. Verify identical output after disabling. |
| Thread scheduler | Enabled, four workers | Optional experiment; may reduce code and worker resources but increase cold-load/rebake latency. Preserve bridge locking. |
| File I/O | Enabled and used | Keep: desktop and Android file callers exist. Memory-only loading needs caller changes and a policy for relative SVG resources. |
| C API | Included and used | Keep initially. A C++ bridge rewrite is possible, but compare linked bytes before spending maintenance effort. |
| Raw loader | Included | Keep the existing source closure; deleting its file leaves direct loader-manager references. |

Upstream also exposes engine, loader, binding, partial-render, thread, file,
SIMD, and extra-feature selection in its
[Meson options](https://github.com/thorvg/thorvg/blob/main/meson_options.txt)
(consulted September 2026). That page is moving upstream context, not the pinned
build contract. Stasis does not invoke Meson: passing `-Dpartial=false` to its
CMake configure does not disable the macro. Local pinned source is authoritative.
The pinned upstream page was unavailable during research.

## Experiments, in priority order

1. Compare Release with MinSizeRel on the same compiler, architecture, CRT, and
   final target. Evaluate function/data sections plus linker garbage collection,
   and then interprocedural optimization separately. Candidate toolchain settings
   are `/O1`, `/Gy`, `/Gw`, `/OPT:REF`, `/OPT:ICF`, `/GL` and `/LTCG` on MSVC;
   `-Os` (or supported `-Oz`), `-ffunction-sections`, `-fdata-sections`,
   `--gc-sections` and LTO on ELF; use Apple linker equivalents such as
   `-dead_strip` on iOS. These are proposed experiments, not verified invocations
   for every shipping toolchain. Check effective flags first to avoid counting
   already-enabled defaults as new savings. Keep symbols separately.
2. In an isolated copy of the vendored sources, remove the definition of
   `THORVG_PARTIAL_RENDER_SUPPORT` and rebuild all objects. Predict unchanged
   smoke-test pixels because no canvas is reused across bakes. Do not define it
   to zero: the source uses `#ifdef`, so zero still enables guarded code.
3. Independently remove `THORVG_THREAD_SUPPORT`, using the existing synchronous
   scheduler branch. Set the bridge worker count to zero for an explicit contract
   and update its documentation in any eventual implementation. Predict identical
   pixels; measure cold-load time and density rebakes on representative mobile
   hardware before adoption. The bridge's own mutex remains necessary.
4. Only if the link map shows material retained wrapper code, evaluate C++ calls
   in the bridge instead of the broad `tvgCapi.cpp` object. Prefer linker removal
   of unused functions over maintaining a hand-pruned API fork. Animation, text,
   scene effects, gradients, masks, and clipping can have internal dependencies;
   do not delete renderer files merely because the bridge does not call them.

Non-MSVC builds already disable exceptions and RTTI in the ThorVG target; this is
not a new saving there. Avoid changing exception policy on MSVC without checking
the bridge and all affected objects. Avoid fast-math as a size experiment because
pixel behavior and finite-value validation matter.

Any adopted config change must cover desktop CMake, Android Workshop CMake,
exported Android shells, and the explicit ThorVG source list in
`mobile/shells/ios/StasisMobile.xcodeproj/project.pbxproj`. Update provenance and
the worker-count documentation together. Do not assume CMake source-list edits
automatically update Xcode.

## Reproducible measurement and acceptance

Start with this dependency-isolated baseline from the repository root:

```powershell
cmake -S runtime -B target/thorvg-size-research -DSTASIS_GRAPHICS_BUILD_SHARED=OFF -DSTASIS_GRAPHICS_BUILD_STATIC=OFF -DSTASIS_BUILD_RUNNER=OFF -DSTASIS_BUILD_SYS=OFF -DSTASIS_BUILD_MOBILE_RUNTIME=OFF -DSTASIS_SVG_BUILD_TESTS=ON
cmake --build target/thorvg-size-research --config Release --target stasis_svg_smoke
ctest --test-dir target/thorvg-size-research -C Release -R '^stasis_svg_smoke$' --output-on-failure --timeout 120
```

Use separate fresh build directories for baseline, size optimization, partial-off,
threads-off, and the winning combination. Bound each build/test to 900 seconds.
Record compiler/version, flags, architecture, source hash, and enabled macros.
Measure the linked smoke executable as a controlled proxy, then the actual
stripped runtime DLL/ELF/Mach-O and per-ABI package. Record `.text`, read-only
data, total binary bytes, and compressed package bytes independently; archive
size and debug symbols are not the runtime contribution. Use linker maps to
attribute retained ThorVG symbols. Report absolute bytes and percent versus the
same baseline; never extrapolate x64 savings directly to arm64.

The existing smoke test asserts file/memory/repeat equality, contain-fit viewport
clipping, straight-alpha RGBA, natural dimensions, and invalid-input failure.
Before shipping any candidate, also compare a representative asset corpus with
curves, strokes, gradients, masks, clipping, and transparency; measure median and
tail bake latency and peak memory. Exercise density rebaking and asset reload.
Capture and inspect representative PNGs for a rendering implementation change.
Reject a candidate that changes required pixels or breaks existing asset loading;
report latency tradeoffs explicitly rather than declaring the smallest build best.

## Validation performed and limitations

- Inspected pinned configuration, source closure, bridge, callers, platform build
  wiring, and smoke assertions; no runtime code or feature switches changed.
- Baseline CMake configure failed: Visual Studio 17 2022 generator reported no
  C or C++ compiler. No fresh executable or size comparison was produced.
- `tools/validate_repo.sh` via Git Bash could not run its checks: `dirname` and
  `python3` were unavailable in that shell environment. This is a documentation
  research result, not a claim that runtime validation passed.
- Visual evidence: not applicable; this report changes no user-visible behavior.
- Theory gained: ThorVG owns one-shot CPU SVG baking, while Stasis owns texture
  lifetime. Fresh canvas creation supports testing partial-render removal;
  persistent worker use predicts a latency tradeoff from thread removal.
