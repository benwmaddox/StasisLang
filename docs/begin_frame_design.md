# Host-owned frame construction lifecycle

Status: approved in Task 410 and implemented by Task 562; source audit 2026-09-10.
The historical call graph below records the pre-migration behavior that motivated
the v8 construction contract. The lifecycle and compatibility sections are the
normative implementation design.

## Recommendation

Make guest frame reset internal and host-triggered at entry to each actual render
invocation. Keep `clear(r, g, b, a)` as optional background intent. Retain an
explicit publication request (`end_frame()` during migration); it is useful for
discarding an incomplete render and is not a synchronous GPU fence. Do not replace
`begin_frame()` with `clear()`, and do not introduce render passes in this change.

The necessary operation is **reset the command builder**, not start the graphics
device. The host knows when it invokes rendering; the shared guest helper knows
the command layout and writer generations. Have the host invoke that helper
through a generated entry wrapper, rather than duplicate its stores in each
host. Backend submission preparation remains host-private and runs even when
there is no clear. This is the implemented v8 contract.

## Evidence and current call graph

Paths and named functions below identify the audited code; the companion
[occurrence inventory](begin_frame_inventory.md) records every tracked matching
definition, use, fixture and documentation occurrence, including vendor snapshots.
Searches also followed `gfx_cmd_begin`, submission, render entry and presentation
helpers rather than relying only on public spelling.

1. Application `render()` (sometimes `main()`/`tick()` or a fixture helper) calls
   [`graphics.stasis`](../src/stdlib/graphics.stasis), `begin_frame` ->
   [`gfx_cmd.stasis`](../src/stdlib/internal/gfx_cmd.stasis), `gfx_cmd_begin`.
   Drawing helpers store into persistent `gfx_cmd_i32/f32/u8` globals.
   `clear` -> `gfx_cmd_clear`; `end_frame` -> `gfx_cmd_mark_present`.
   There is no BeginFrame opcode in the order stream.
2. Compiler JIT/AOT/Wasm compile these ordinary Stasis functions and stores.
   The audit found no BeginFrame-specific lowering. The apparent no-op definition
   in [`aot.rs`](../crates/stasis_compiler/src/backend/aot.rs) is inside the
   audio playback test's substituted graphics declarations, not production
   lowering. [`sealed_display_list.rs`](../crates/stasis_compiler/tests/sealed_display_list.rs)
   executes the real helpers in JIT and compiles them for Wasm, asserting reset
   counts, order and patched sprite coordinates. Frontend ownership restrictions
   are tested in [`module_graph.rs`](../crates/stasis_compiler/src/frontend/module_graph.rs);
   rendering does not need a new parser construct.
3. Native [`stasis_runner.c`](../runtime/stasis_runner.c) calls tick/render then
   `gfx_submit_u8` in both Windows and POSIX paths. Its tick-only bulk path calls
   `stasis_host_bulk_step` in [`stasis_graphics.c`](../runtime/stasis_graphics.c).
   [`stasis_mobile_runtime.c`](../runtime/stasis_mobile_runtime.c),
   `stasis_mobile_runtime_step`, calls the generated tick/render entries and
   `stasis_gfx_submit_u8`; nonzero lifecycle results return before submission.
   [`stasis_dynload`](../crates/stasis_dynload/src/lib.rs) binds the same submit
   export and canonical global arrays for JIT. These are host boundaries, not
   side effects of the guest `begin_frame` call.
4. SDL `stasis_gfx_submit_frame` validates, records acceptance/trace and display
   metadata, resets submission metrics, calls native `stasis_begin_frame`,
   optionally clears, replays ordered commands, resets clipping, and calls
   native `stasis_end_frame` only for `PRESENT`.
5. Web [`game.js`](../runtime/web/game.js), `frame`, invokes Wasm tick/render,
   resets workload metrics, then `executeCommands` -> `executeStasisBuffer` ->
   WebGL2 batcher. The separate legacy `web_begin_frame(r,g,b)` import clears a
   JavaScript command list and inserts a background command. That legacy list
   is replayed before the canonical buffer. It is not the stdlib API.
6. Android [`stasis_android_bridge_run_render_frame`](../crates/stasis_android_bridge/src/lib.rs)
   executes lifecycle code, copies active canonical lanes through
   `copy_jit_render_active` in dynload and stamps display/frame-token metadata.
   [`StasisPreviewRenderer.java`](../mobile/android/app/src/main/java/com/stasislang/workshop/StasisPreviewRenderer.java)
   validates the frame, prepares/restores resources, then draws only when
   `FLAG_PRESENT` and resource readiness permit. GLSurfaceView owns scheduling.

### What each operation actually owns

| Operation / source | Current responsibilities and limits |
| --- | --- |
| Guest `gfx_cmd_begin` | Increment positive writer frame generation; invalidate active token; write magic/version; zero flags, line/rectangle/sprite/run/text/clip/order counts, dropped counters and text bytes used. Does not zero payload arenas, clear background floats, reset next-token identity, or overwrite host display slots 10..21. Old payload becomes unreachable through zero counts. |
| Guest transient memory | Fixed arrays are reused by resetting published counts/text-byte cursor. No heap, GPU allocation, atlas eviction, synchronization or resource release happens at begin. Writer generation/token checks protect unfinished reservations; see `gfx_cmd_sprite_run_*` helpers and [writer probe](../tests/stasis/seams/sprite_run_writer_public_probe.stasis). |
| Guest `gfx_cmd_clear` | Assign flags to `CLEAR` (1), store four floats. Does not reset any count, writer, clip descriptor, header identity or text cursor. Assignment also removes a previous `PRESENT` request. |
| Guest `end_frame` / internal `gfx_cmd_submit` | Add 2 to flags; neither freezes nor submits arrays. Repeated calls are not idempotent: e.g. 3 becomes 5, losing the present bit. Later drawing still mutates the buffer. |
| Native `stasis_begin_frame` | Reset debug hash, apply asset-watch changes, pump events if needed, attempt resource restore, reset queued line count and clip state, set SDL blend/clip defaults. Called by submission independently of guest begin. |
| Native `stasis_end_frame` | Gate on resource readiness, flush queued lines, capture before present, call SDL present, finish timing, advance debug frame counter and reset event-pump bookkeeping. No guest array reset. |
| Web batcher `beginFrame` | Check context, disable scissor, set viewport, clear color buffer. Called only for clear intent; misleadingly named backend clear operation. Metrics reset separately in `frame`. |
| Workshop `drawFrame` | Reset pipeline and clipping, set surface/logical viewport, clear letterbox bars, optionally clear logical canvas, replay/batch. `onDrawFrame` owns resource restore, counters, captures and deferred sprite releases. |
| `ui_begin_frame` | Reset UI layout rectangle, status, stack, hot/pointer and scroll scratch state in [ui_single_pass.stasis](../src/stdlib/ui_single_pass.stasis). It does not call graphics begin. Keep this independently scoped UI operation; multiple UI roots need not imply multiple graphics frames. |

The renderer resource generations are distinct from the sprite writer generation.
Resource lifetimes and restoration already belong to hosts, as implemented by
native `stasis_restore_renderer_resources`, Web context handlers/atlas ownership,
and Workshop resource lifecycle/texture preparation. See
[resource lifecycle](renderer_resource_lifecycle.md) and
[lifecycle tests](../runtime/tests/stasis_renderer_lifecycle_test.c).

## Clear semantics, independently of lifecycle

Current canonical clear is a single background declaration applied **before all
geometry**, not an ordered draw command. Multiple clears overwrite the same four
floats: last value wins even if drawing occurred between calls. Clearing neither
discards previous geometry nor begins a new pass. A clear after end currently
removes the publication flag; this is a flag API defect, not useful clear
semantics. Evidence: `gfx_cmd_clear`, `gfx_cmd_mark_present`, and each consumer's
clear-before-order-loop implementation cited above.

Proposed contract: retain last-background-wins behavior; clear changes only
background intent, never lifecycle flags. Specify finite RGBA components in
0..1 for portable input. It replaces the logical canvas background rather than
alpha-blending a rectangle with previous contents; letterbox treatment is host
display policy. Native currently clears black then fills the logical rectangle
with blend disabled; Workshop clears a scissored logical viewport; Web clears
the backing framebuffer and clamps alpha. Out-of-range/NaN conversion equivalence
is not established here and must not be promised by the new contract.

No clear means no requested background replacement. It does **not** promise
persistent pixels across swaps/context loss. A frame that needs deterministic
full-canvas pixels must clear or cover the canvas. Retained *commands* and retained
framebuffer *contents* are different features; a future persistent render target
would need a separate explicit contract. This portability recommendation avoids
depending on unspecified prior backbuffer contents; it is not a measured claim
that all current backends preserve or discard those pixels identically.

## Backend findings and risks

| Backend | Observation | Migration risk / required boundary |
| --- | --- | --- |
| Desktop SDL JIT and native runner | Validation precedes native begin; begin owns event/resource setup. Missing PRESENT still replays draws but skips swap. Submission counters and actual successful display are not interchangeable (`stasis_end_frame` can withhold for restore). | Do not remove native setup when removing the guest call. Wrap actual guest render entry, not the submit call, which is too late. Preserve screenshot-before-present and event pumping on withheld frames. |
| Generated SDL mobile AOT | Runtime binds/initializes arrays, invokes lifecycle entries, skips submission on nonzero entry result and while paused. Same SDL consumer. [Mobile runtime test](../runtime/tests/stasis_mobile_runtime_test.c) counts begin/end/submit and checks early-stop paths. | Generated wrapper must be retained/exported and used on device; resetting only desktop JIT would leave stale mobile commands. Pause/resume and renderer restore remain host-owned. |
| Web Wasm/WebGL2 | Canonical replay checks magic/version and sprite/run validity; no PRESENT gate in `executeStasisBuffer`. It calls batcher `beginFrame` only with CLEAR. `frame` catches replay errors, but guest tick/render calls occur outside that catch. Legacy command list reset is separate. | Separate viewport/scissor preparation from clear; gate publication consistently; do not claim existing error handling is atomic. Reset both producers only under their negotiated contract; migrate legacy imports separately. Context loss cannot be repaired by guest begin. Tests use mocked GL, not a real compositor. |
| Android Workshop / preview GLES2 | Java validates before clearing or preparing resources. PRESENT gates resource preparation/drawing. Active-lane copy is not an immutable guest-side seal. Frame tokens, synchronization, captures and deferred releases are host mechanisms. | Reset before bridge render and keep host snapshot/copy ownership; never reset arrays while GL consumes them. Preserve token/capture rejection behavior, bounded restore and releases after successful presentation. Test invalid/unfinished new frames against the last accepted snapshot. |
| Legacy/conformance adapters | `stasis_begin_frame` remains a native exported symbol in [stasis_graphics.def](../runtime/stasis_graphics.def). Two web smoke samples import `web_begin_frame`; Web executes their commands through the same WebGL2 renderer. [Resource docs](renderer_resource_lifecycle.md) mention historical desktop GL, but the audited production C begin/submit is SDL. | Do not infer a separate shipping GL lifecycle from historical prose. Preserve exported C compatibility until explicitly retired; rename private Web method independently of its legacy import. |

Neither the canonical order kinds nor v7 run metadata define multiple passes:
`pass` is reserved and nonzero values are rejected. Evidence:
[direct frame contract](direct_graphics_frame_contract.md),
[C validator](../runtime/stasis_render_contract.h), Web run validation, and
Workshop `isValidFrame`. Native replay without present is not a portable multipass
API. A future pass design belongs above ordered draws with explicit target/load/
store intent; repeated BeginFrame would erase commands rather than model it.

## Alternatives and edge cases

| Model | Benefit | Why accept/reject |
| --- | --- | --- |
| Keep explicit begin + clear | Current reset is cheap and allows manual fixture builders. | Safe only with a precise builder contract; repeated/missing begin can erase/append work, unrelated to actual backend frame start. Not preferred for normal render lifecycle. |
| Clear alone | One fewer visible call for common samples. | Reject: stale geometry/text/counts and writer token survive. Reset-on-clear would break multiple clears and no-clear rendering. |
| Lazy start on first graphics operation | Omits explicit begin. | Reject: reserve, clear, draw, replay and empty publication all need guards; a host epoch is still needed to distinguish invocations. Empty/failed frames become harder to reason about. |
| Host-owned builder reset | One authoritative invocation boundary, independent of background choice. | Recommend, using a shared generated wrapper; explicit publication still distinguishes finished work from abandonment. |
| Rename public begin to reset commands | Truthful and small change. | Useful internal/manual-builder name, but keeps normal application bookkeeping and stale-state hazards. Transitional option, not endpoint. |
| New frame/pass object | Can model targets and multiple submissions. | Defer until a real non-default-pass requirement exists; v7 explicitly rejects that state. |

| Case | Current result / dependency | Proposed result |
| --- | --- | --- |
| Begin + clear + draws + end | Fresh counts and background, then present request. | Same picture with reset at entry and explicit end request. |
| Clear alone on reused globals | Old geometry remains; first use may lack magic/version. | Entry reset makes it a fresh clear-only builder; publish explicitly. |
| No clear | Begin still required for fresh geometry. Backend prep differs on Web. | Reset and backend prep always run; no previous-pixel guarantee. |
| Multiple clears / begin calls | Last clear wins; second begin discards earlier commands. | Clear still last-wins. Migrated applications have one host reset; no public mid-render reset. |
| Empty render with end | Fresh empty valid command frame; display depends on clear. | Publish empty frame explicitly; no clear is not an erase-screen request. |
| Early return before begin | Previous flags/counts can be submitted again if host considers invocation successful. | Fresh unpublished builder is discarded; old accepted frame remains eligible for host replay. |
| Early return after begin, before end | Native may replay without swap; Web may display; Workshop skips drawing. | No new publication on every backend. Explicit end followed by normal return requests adoption; error/nonzero host failure aborts even after end. |
| Clean/skipped render | Current Web and listed native loops invoke render; they do not establish a general dirty-frame scheduler contract. | If scheduling later skips invocation, do not reset/rebuild accepted commands. Re-present only an explicit host-owned accepted snapshot. |
| Retained/replayed list | `PresentationList.replay` appends into current builder; the JIT test calls begin for each replay. | Replay into a fresh invocation builder; keep logical lists independent of canonical lanes. |
| Surface recreation | Resource restore and viewport generations are host-owned, not repaired by guest reset. | Re-prepare resources and replay accepted logical commands or show host loading state; never publish the discarded working builder. |
| Error during replay | Validation is backend-specific; source does not prove rollback of every GPU-side error. | Validate before adoption; never replace accepted snapshot on guest/schema failure. Resource/GPU failure withholds presentation and retries; do not promise restoration of already-modified pixels without an offscreen transaction. |

## Lifecycle and compatibility

The host invokes `reset -> guest render -> finish` once per scheduled construction.
Reset executes the existing bounded stores, preserves display metadata and
invalidates writer tokens. Finish rejects unfinished writers and unsuccessful
invocations; otherwise it validates and publishes only if requested. Published
storage must survive until the consumer finishes. A later reset cannot alias the
accepted snapshot: synchronous hosts may consume before reset, but any replay or
asynchronous consumer needs owned retained storage. Preserve payload capacity and
use authoritative counts; no arena-wide zeroing is needed. Invalidate writer
capabilities on abort and hot-generation changes as well as reset.

`end_frame()` remains the compatible spelling for request-publication, set
idempotently rather than arithmetically added. It is not a resource lifetime
fence or physical-presentation acknowledgement. Keeping it avoids silently
publishing half-built work on normal early returns. Source checks may later
rename it to `publish_frame`, but that rename is not required to remove begin.
See [loading screens](knowledge/loading-screens.md): callers must still return
to a host that presents before starting blocking IO on a later tick.

Migration is a semantic ABI change even if all v7 offsets stay fixed. Current
flags do not encode whether a producer expects implicit reset or whether PRESENT
means render gating on Web. Negotiate a generated runtime/lifecycle capability
in package metadata and require a matching host; if the existing package ABI
cannot distinguish it, bump its version. Do not silently reinterpret old v7
packages. A render-schema bump is required if flags/validation meanings change
under the same consumers; an explicit capability may separate lifecycle from
the unchanged lane layout. Decide the concrete version number during approved
implementation, auditing [mobile ABI](mobile_packaging_abi.md) and
[ABI checker](../tools/ci/check_runtime_abi_contract.py).

Affected contracts: compiler-required roots/generated lifecycle exports and AOT
entry manifests; dynload entry trampolines and global/writer ownership; native
bulk tick-only and regular render paths; mobile binding headers; Wasm export and
memory metadata; Android JNI active-lane copy, Java schema and presentation token;
C validation/trace fixtures; vendored stdlib copies; templates, snippets, loading
guidance and exact-string CI assertions listed in the inventory. The native C
export is not the guest wrapper and must not be deleted as a side effect.

Keep legacy explicit behavior for old packages. Do not make old begin silently
no-op: some fixtures and applications use it to restart a list or build outside
render. Migrate these to a supported manual builder in tests or into a render
entry; tick-only hosts require their own explicit construction wrapper, not
reset-before-tick for all programs. Graphics outside the negotiated construction
scope must fail deterministically. Update vendor packages through their normal
generation flow, not independent edits to snapshots. Rollback consists of
retaining old package/host contract support and rebuilding with explicit calls;
never downgrade the interpretation of already-negotiated new packages.

## Implementation sequence and focused validation

The implementation follows these bounded work packages and gates.

1. Characterize reset/clear/publication: add a shared fixture covering stale
   geometry/text, repeated clears/end, unfinished writer and early returns;
   assert exact flags, counts, order and writer rejection in JIT, Wasm execution
   and a fresh AOT executable. Freeze old-package compatibility oracles.
2. Introduce negotiated construction wrappers and abort/finalize behavior in
   compiler/dynload/native/mobile entries. Preserve layout and audit all generated
   roots/exports/version fields. Test tick-only/manual construction separately,
   hot swap rejection, accepted snapshot lifetime and reset-after-consumption.
3. Wire Web and Workshop to the same publication policy and move Web viewport/
   scissor setup out of clear. Exercise empty/no-clear, multiple clear, context
   loss, malformed frames, early returns and replay. Do not advertise the new
   package contract on any backend until it passes the shared fixture.
4. Migrate samples, templates, vendor snapshots and documentation; replace
   exact-string begin assertions with lifecycle behavior checks; deprecate/remove
   guest begin only for the negotiated contract. Preserve native export adapters.

Each command must stay within 900 seconds. The focused validation commands are:

```text
python tools/cargo_cache.py run -- cargo test -p stasis_compiler --test sealed_display_list -- --test-threads=1
python tools/cargo_cache.py run -- cargo test -p stasis_dynload active_render_copy -- --test-threads=1
node --test runtime/web/tests/render_pipeline.test.mjs
python tools/ci/check_runtime_abi_contract.py
ctest --test-dir <fresh-runtime-build> -C Release --output-on-failure -R "render|mobile_runtime"
powershell -NoProfile -ExecutionPolicy Bypass -File mobile/android/test_emulator.ps1 -Headless
```

Shard device acceptance if its full flow exceeds the budget. Run relevant Java
schema/lifecycle tests and fresh generated mobile AOT acceptance too. Capture and
inspect native/Web/Android PNGs for no-clear coverage, clear color and clipping;
MP4s for early-return retention, pause/resume and surface restoration. Compare
logical command traces as well as pixels: traces alone cannot prove presentation.
An actual multipass feature would need separate target/load/store tests.

## Validation performed for this design

- Tracked-source occurrence audit and direct inspection of the call graph above.
- `node --test runtime/web/tests/render_pipeline.test.mjs`: 42 passed, including
  ordered clipping/batching, reserved-state rejection, context loss, resource
  lifetime and timing. These are mocked WebGL tests, not device evidence.
- `python tools/ci/check_runtime_abi_contract.py`: 797 comparisons passed;
  generated evidence at `target/seam-tests/it-001-runtime-abi.json`.
- Inventory: 168 matching occurrences in 88 tracked files. All 114 local links
  across this note and its inventory resolve; both files are ASCII. Whitespace
  checks pass, and the post-test process check found no lingering test binaries.
- No renderer implementation changed; native/device visual capture and new
  lifecycle executable tests are migration gates, not completed experiments.

Visual evidence: not applicable; documentation-only proposal, no visible behavior
changed or claimed validated.

Theory gained: command construction and backend submission are separate epochs;
the guest reset only stores counts/tokens while native submit independently calls
its own begin. Therefore moving the reset requires invocation ownership, and a
new backend should not need a public BeginFrame to restore resources. Good: source
tracing distinguished three differently scoped begin operations. Bad: matching
names alone would have missed backend PRESENT differences. Adjustment: use the
same early-return/no-clear fixture across consumers before migrating lifecycle.
