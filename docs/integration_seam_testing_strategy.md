# Integration seam testing strategy

Status: integration test design; rollout and task tracking belong to the
[build checklist](build_checklist.md#integration-seam-testing-rollout).

## Purpose

Stasis has strong tests inside the Rust compiler/JIT, the C graphics and mobile
runtime, the desktop toolchain, and the Android Java renderer. The recurring
hard failures occur between those components: a buffer has the right values but
the wrong size, an exported function has the right name but the wrong calling
convention, a host snapshot is populated after `tick`, a packaged asset exists
but is rooted differently on Android, or a native failure is lost before Java
can report it.

This strategy defines tests at those seams. It does not replace unit tests or create
one slow end-to-end test that is hard to diagnose. Each proposed test crosses
one named boundary, uses the smallest real implementation on both sides, and
records a stable observable result.

## Current path and coverage

The representative rendering path is:

```text
Stasis source
  -> Rust parse/check/lower
  -> Cranelift JIT or AOT
  -> registered host/global ABI
  -> current gfx_cmd buffers (all downstream consumers advance in lockstep)
  -> native/Java command interpreter
  -> SDL or GLES platform adapter
  -> desktop window or Android surface
```

Input, lifecycle, and assets travel in the other direction:

```text
OS / Android Activity / SDL
  -> host event and asset adapters
  -> HostFrame, request mailbox, and host externs
  -> Stasis tick and state
  -> render commands
```

Existing coverage already proves important pieces:

- `parity_corpus_covers_shared_lowering_shapes` compares real JIT and linked
  AOT command traces for the canonical render fixture.
- `runtime/tests` verifies render-contract validation, display transforms,
  renderer lifecycle state, and the mobile runtime against C fakes.
- `apps/stasis/tests/toolchain_cli.rs` verifies mobile package assembly and
  generated paths/manifests.
- PR CI builds and links a generated Android package.
- Windows CI runs the desktop renderer and verifies a real capture.
- `mobile/android/test_render_emulator.ps1` runs the Workshop JIT renderer and
  verifies stable frames.

The missing coverage is mostly composition. The C mobile runtime test does not
call compiler-generated AOT objects. The Android package job does not install
or run the generated release shell. Desktop input, HostFrame, Stasis state, and
render output are not asserted as one transaction. Workshop Java, JNI C, the
dynamically loaded Rust compiler bridge, JIT globals, and the GLES renderer are
only crossed together by broad UI acceptance.

## Testing model

### One seam per test

Every test must declare:

1. the producer and consumer at the seam;
2. the exact data or control transfer being checked;
3. one primary oracle;
4. the fault mode it is intended to catch;
5. its maximum run time and CI lane.

Tests may exercise setup components outside the seam, but should not add
unrelated assertions. For example, the Android input test may render a colored
marker, but it should assert input-to-state-to-command behavior rather than all
font and sprite parity.

### Shared probe project, small scenario files

Add a framework-owned `tests/seams/` project with a small common module and
separate entry files per scenario. The common module should expose only stable
test observables:

- a monotonic tick counter;
- the last normalized input and display generations;
- a deterministic state checksum;
- a bounded lifecycle marker list;
- one command-frame builder with fixed colors and coordinates;
- deliberate entry return codes selected by a test global.

Do not turn the probe into a second sample game. Each test entry imports only
the behavior it needs. Asset scenarios use a tiny SVG, a deterministic test
font, a short PCM/WAV asset, and the checked manifest. Reuse the render-parity
assets when their behavior is the subject of the test.

### Four oracle levels

Use the cheapest oracle that proves the seam:

1. **State oracle:** inspect a named Stasis global or deterministic checksum.
2. **Protocol oracle:** compare HostFrame fields, ABI metadata, command counts,
   command trace, lifecycle order, or structured diagnostic JSON.
3. **Resource oracle:** verify resolved asset identity, handle generation, and
   bounded resource counts.
4. **Pixel/audio oracle:** verify named image regions or an offline mixed audio
   buffer only when platform output is the boundary under test.

Pixel hashes are not a substitute for state or protocol assertions. Exact
hashes are allowed only for a pinned backend and rasterizer. Portable tests use
named regions with tolerances. Audio tests inspect deterministic mixed samples
or queue state; CI must not depend on speakers.

### One evidence format

Every process or device test should write `stasis.seam_test.v1` JSON containing:

- test ID, fixture revision, target, backend, and build identity;
- command-buffer and HostFrame versions and capacities;
- entry symbols and signatures when AOT is involved;
- ordered lifecycle events;
- state checksum and command trace;
- logical/native dimensions, fitted drawable viewport, and generations when relevant;
- asset IDs, paths, and content hashes when relevant;
- bounded timing, exit status, and the first structured failure.

The test runner should print a short failure and retain the JSON, logs, and any
capture as CI artifacts. This gives failures the same vocabulary across Rust,
C, PowerShell, and Java.

### Contract values have one source of truth

`runtime/stasis_render_contract.h` remains authoritative for rendering, and
`src/stdlib/internal/host_frame_raw.stasis` remains authoritative for HostFrame. A
fast contract test should extract or generate one machine-readable descriptor
from those canonical definitions and compare every copied constant used by
Rust allocation code, generated AOT bindings, JNI direct buffers, and Java
rendering. Do not add another hand-maintained list of expected values.

### Real code on both sides

Mocks remain useful inside a component, but seam tests use the actual producer
and consumer wherever practical:

- compile the checked Stasis fixture rather than hand-fill command buffers;
- link generated AOT objects and bindings into the C mobile runtime harness;
- call the exported JNI entry rather than a Java facsimile;
- install the generated Android release shell rather than the Workshop flavor;
- route test input through SDL/Android when that adapter is under test.

Test-only fault injection is acceptable at an adapter boundary when the real
failure is otherwise nondeterministic. Hooks must be compile-time test-only,
named for the injected failure, and leave the production path unchanged.

### Determinism and flake policy

- Each command remains bounded by 900 seconds; normal PR tests should finish in
  under 120 seconds.
- Wait on explicit markers, frame counters, generation changes, or process
  state. Fixed sleeps may be used only as a polling interval under a deadline.
- Record the random seed, but prefer fixed inputs and ticks.
- A test that is too flaky for gating is a defect in the harness or product. Do
  not hide it with retries. Device infrastructure may retry provisioning before
  the app launches, never the product assertion after launch.
- Check for and terminate owned lingering test processes after each run.

## Execution lanes

| Lane | Trigger | Budget | Contents |
|---|---|---:|---|
| Fast contract | every PR and `tools/validate_repo.sh` | 2 min | descriptor parity, JIT HostFrame, buffer bounds, diagnostic schemas |
| Native integration | nightly, platform-sharded | 15 min | desktop real runtime, linked AOT/C runtime, package link and symbol audit |
| Android emulator | nightly | 15 min/test shard | Two concurrent isolated API35 x86_64 shards: generated release-shell IT-017-IT-023 and Workshop JNI/JIT IT-025-IT-027 |
| Physical device | optional release candidate and scheduled farm | 15 min/test shard | Supplemental OEM driver, density, lifecycle, and representative rendering evidence |

Tests should be promoted toward the faster lane when a deterministic lower
adapter becomes available. The hosted x86_64 emulator is the CI and readiness
gate. Nightly runs provision one emulator per shard so release-shell and
Workshop failures, timeouts, and evidence remain independently visible rather
than sharing a sequential device. Physical-device tests supplement release
confidence for OEM-specific surface, driver, and density behavior, but device
availability does not block ordinary CI or task readiness. Production Android
packaging remains ARM64; the x86_64 package target exists only for deterministic
development/emulator tests.

### CI placement rule

Ordinary Rust test targets belong only in the broad Cargo workspace lane. A
platform suite may name a target only when its cases share a genuine platform
prerequisite, such as MSVC and the built graphics DLL. Compiler seams remain in
the compiler-package suite. Package-link, device-acceptance, and editor
boundaries remain separate jobs so their environments and evidence stay
actionable. A focused command added for local debugging must not create a
second CI invocation of a test already owned by one of these lanes.

## Integration test catalog and rollout

The canonical [build checklist](build_checklist.md#integration-seam-testing-rollout)
owns the IT-001 through IT-032 catalog, priorities, rollout order, and implementation
tracking. Use its linked task table when selecting an already-authorized slice.
This document supplies test design and acceptance mechanics; the runtime sequence
of acceptance cases below does not establish implementation order.

### Workshop acceptance details

IT-029 runs in the Workshop acceptance build between IT-028 and IT-031. It creates
two registered render-parity projects whose sprite, font, and text handles collide,
but whose canonical roots, sprite bytes, and direct/cached text differ. The lane
captures project A, project B, project B after a real EGL context recreation, and
project A after switching back. `stasis.workshop_resource_scope.v1` binds each PNG
hash to the native frame handles, exact resolver identities, renderer generation,
stale-generation rejection count, restore uploads, and bounded atlas/text caches.
Numeric GLES texture names are deliberately excluded because drivers may reuse them.

IT-032 runs after IT-031 on the scheduled device lane. It publishes same-layout
constant revisions before frames 75, 150, 225, and 300, recreates the real EGL
surface before frames 100 and 200, and restores the packaged revision within frame
300. Each frame must present its unique JNI token through GLES, observe one coherent
runtime generation plus matching guest tick/render revision, reuse the same three
direct buffers, report zero dropped commands, remain within the declared command and
resource maxima, and leave no pending runtime candidate. The texture provider's
renderer generation must equal the lifecycle renderer generation; its surface
generation is the creation-time value exactly one before the lifecycle value added
by `onSurfaceChanged`. Each recreation advances those epochs by one and two,
respectively. Logs are bounded to seven
milestones and one compact `stasis.workshop_soak.v1` summary. The summary carries the
fixed schedule, source identities, traces, peaks, and a structured cleanup receipt;
the emulator driver still force-stops the app in `finally` on success or failure.

IT-030 runs immediately after IT-029 and before IT-031 in the Workshop acceptance
build. The Java runner captures the packaged project with
`WorkshopAiProjectTransaction`, changes the uniquely tagged, reachable
`IT028_TICK_REVISION` constant from packaged value 1 to IT-030 accepted value 3, and
creates `tests/it030_workshop_jni.test.stasis`. It invokes `nativeRunTests` for an
initial pass, publishes value 4 as a second compilable source revision that makes
that same test fail, restores the accepted snapshot through the production
transaction helper, and proves a subsequent pass. Because reachable tick code reads
the constant, every source transition publishes a real runtime generation without
changing an ABI or layout. After each compile, a native preview frame activates the
staged candidate before Java captures its generation and fingerprint; evidence is
rejected if Rust still reports a pending candidate. The C JNI shim converts the
complete Rust-owned JSON string with `NewStringUTF` and frees it only afterward; no
fixed-size transport buffer is allowed.

Three ordered `stasis.workshop_test_runner.v1` case records carry bounded counts and
the IT-030 result's exact file, line, column, name, and status. Each case also records
a SHA-256 editable-project identity and the live runtime generation/fingerprint. The
summary binds pass, failing revision, rollback, and cleanup identities without
duplicating result arrays. Cleanup must restore the packaged project exactly, advance
the runtime generation, and prove the temporary test no longer exists. The strict CI
verifier rejects missing or reordered records, truncated JSON, count/location/status
loss, rollback mismatches, a missing subsequent pass, or leaked test files.

## Implementation order

See the [canonical rollout order](build_checklist.md#integration-seam-rollout-order).
Update ordering and implementation progress in that checklist only.

## Definition of done for each task

A task is complete only when:

- the test fails for a demonstrated boundary defect or a deliberate test-only
  fault before the fix and passes after it;
- both sides of the named seam are real implementations;
- the test is registered in the appropriate bounded CI lane;
- failure output names the test ID, seam, expected/actual values, and evidence
  artifact;
- owned processes/apps are stopped and device settings restored;
- the touched test code receives a simplicity/cruft review;
- the work summary records Good, Bad, Adjustment, and `Theory gained:` per the
  repository process.

## Theory gained

The reliable unit of integration is not “desktop” or “Android”; it is one
versioned transfer observed on both sides. Existing evidence supports this:
command-trace tests catch compiler/render drift, while real device work has
found failures that file-existence and component tests could not, including
entry signature, asset-root, display-generation, and renderer-restoration
mistakes. The adjacent prediction is that a shared descriptor, fixture, and
evidence vocabulary will make future platform additions cheaper: a new host or
renderer adapter can reuse the same seam probes and only add tests for genuinely
new platform transfers.
