# Stasis architecture characterization

This document describes the executable inventory in
`tests/characterization/manifest.json`.  The inventory is the evidence boundary
for the architecture work tracked by Maddox Task #382.  It is intentionally a
characterization gate, not a second compiler or host abstraction.

## What is frozen

The compiler fixture
`tests/stasis/characterization/compiler_pipeline_v1.stasis` is compiled through
the shared frontend and each current target path.  Its checked-in golden records
the facts that later simplification work must preserve:

- declaration names and source/signature ranges;
- one representative parse diagnostic, including its code, symbol, range, and
  message;
- data-flow reads, writes, calls, global type/collection metadata, and canonical
  `SymbolId` reachability;
- cold and edited patch sets, reuse, affected host entries, and reason chains;
- state layout records and the stable layout digest;
- JIT execution result, normalized CLIF blocks/opcode names/call targets;
- AOT artifact identities and normalized object symbol inventory; and
- structurally parsed Wasm section, import/export, function/memory, and opcode
  family facts.

Raw object bytes, raw Wasm bytes, raw CLIF text, code pointers, temporary paths,
timings, and host-dependent machine sizes are deliberately excluded.  The JIT,
AOT, and Wasm `ProgramSnapshot` checks compare semantic records; type IDs are
normalized to their type shapes where target initialization order can differ.

The same shared JSONL files under
`tests/characterization/live_protocol/v1/` are read by Rust and the VS Code
TypeScript tests.  They cover valid requests, semantic request failures,
success/failure/truncated/runtime-identity responses, and malformed envelopes.
Rust applies `LiveRequest::validate()` after JSON deserialization; TypeScript
checks only the JSON shape.  This keeps protocol-shape ownership distinct from
Rust semantic policy.

Web storage and network tests execute `runtime/web/game.js` in deterministic Node
VM adapters.  The opt-in test hook is absent from published pages.  Tests cover
scope/key isolation, corrupt and denied storage, checkpoint bounds and
credential isolation, bounded queue behavior, WebSocket send/poll, and malformed
checkpoint recovery.

## Evidence strength and lanes

Every manifest row names its owner, fixture paths, exact command, evidence
strength, execution lane, explicit `default_gate` selection, and expected evidence:

- `behavioral` means the command executes code and asserts an observable result
  or state transition.
- `structural-lint` means the command checks source, package, workflow, or
  manifest shape.  It is useful evidence, but it is not a runtime behavior
  proof.
- `fast-hermetic` is bounded local/PR evidence.  Run the inventory check with
  `python tools/ci/run_architecture_characterization.py --check`.  The six
  representative rows marked `default_gate: true` are the small PR/local gate;
  execute them with `--run-fast`.  Use `--run-lane fast-hermetic` when an
  intentional full fast-lane characterization run is needed.
- `platform-host` requires a native host, linker, or platform-specific runtime.
  It remains visible in the manifest and is not silently counted as a hermetic
  pass.
- `device-browser` requires an emulator, browser/extension host, or packaging
  machine.  Android Workshop, VS Code E2E, and iOS packaging stay in this lane.

The iOS row is deliberately packaging/link evidence only.  It does not claim
simulator or device behavior until such a lane exists.

## Running the gate

From the repository root:

```text
python tools/ci/run_architecture_characterization.py --check
python -m unittest tools.ci.test_run_architecture_characterization
python tools/ci/run_architecture_characterization.py --run-fast
# Optional full-lane run:
python tools/ci/run_architecture_characterization.py --run-lane fast-hermetic
```

The local validation script and default PR CI run the manifest check and the
small fast gate.  The VS Code row assumes its checked-in lockfile has already
been installed (`npm ci --prefix vscode-stasis`); PR CI performs that install.
The full workspace suite remains the broader regression gate; platform/device
rows are promoted by their owning jobs when their environment is available.

## Updating snapshots

Treat a golden change as an architectural decision, not generated churn.  A
snapshot update must:

1. change the fixture or implementation intentionally;
2. run the exact characterization test and inspect the semantic diff;
3. retain the no-unstable-data rules above; and
4. explain the changed invariant in the commit/PR and, when durable, here.

The compiler test supports a local-only update aid:

```text
STASIS_UPDATE_CHARACTERIZATION=1 python tools/cargo_cache.py run -- cargo test -p stasis_compiler --test architecture_characterization -- --test-threads=1
```

The environment variable must never be set in CI.  CI always compares against
the checked-in golden.

## Relationship to complexity and simplification

`docs/architecture_complexity_and_simplification.md` explains where StasisLang
complexity lies and proposes consolidation boundaries.  This document supplies
the executable memory needed before changing those boundaries: it tells a later
refactor which behavior belongs to the compiler, which behavior is a host seam,
and which facts are currently only structural evidence.

Task #382 establishes this characterization baseline.  Canonical cross-host
contract generation is intentionally deferred to Task #384; this gate records
the current contracts without introducing a universal host abstraction.
