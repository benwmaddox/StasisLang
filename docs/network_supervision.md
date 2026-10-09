# Bounded native authority supervision

Windows toolchain archives ship `stasis-network-supervise.exe` beside
`stasis.exe`. Package the application's normal desktop entry with
`capabilities.network` and its existing `web.entry`, using the same toolchain
release. The supervisor launches that packaged executable; it does not host a
replacement authority, decode application commands, or change game rules.

```powershell
./stasis-network-supervise.exe --authority C:/games/MyGame/MyGame.exe --peer C:/tests/peer.exe --startup-ms 15000 --action-ms 60000 --shutdown-ms 5000
```

The peer receives these UTF-8 lines on its private standard input:

```text
stasis-network-supervision-v1
<complete private native join URL>
```

Both lines end in LF; the supervisor then closes stdin. The URL is at most
512 bytes and contains no NUL, CR, or LF.

Read the invite in memory, connect to the URL's `/session` endpoint through
the existing authenticated WebSocket protocol, and use the application's
normal framed commands and viewer projections. Exit zero only after the
expected application outcome is verified. Exit nonzero on failure. Do not
write the invite to a file or pass it through command-line arguments. The
supervisor discards child stdout and stderr, including failures, so child
diagnostics cannot accidentally expose credentials in its output.

All three bounds are required and accept 1 through 900000 milliseconds.
Startup bounds authority readiness; action bounds the external peer; shutdown
bounds process cleanup. Arguments after `--` are forwarded to the peer, so a
Node consumer can use `--peer C:/node/node.exe ... -- C:/tests/peer.mjs`.
Forwarded arguments must contain no credentials. The peer must read its invite
from stdin rather than requiring a private URL argument.

Completion terminates the authority and any remaining child descendants;
this is a bounded test-session lifetime, not an application save/quit request.
Peer failure, authority failure, and timeouts also terminate the child job.
The job's kill-on-close policy covers abrupt supervisor termination.
Credential-free stderr stages are `authority-ready`, `peer-started`, and
`complete`, prefixed by `stasis-network-supervise:`. Exit zero means the peer
succeeded and cleanup completed; any startup, action, or cleanup failure exits
nonzero. These stages report process readiness, not a game-specific lobby or
turn condition; the peer must verify those through the application's protocol.

The native shell publishes readiness only after graphics, the network host,
runtime binding, and application `main` have initialized successfully. It
copies the invite through the existing native host API into an inherited
anonymous pipe. This opt-in path replaces the initial modal join card; it
does not require clipboard access, a test-only build flag, or injected input.
The invite is never placed in deterministic Stasis state. An invalid private
handoff fails startup rather than falling back to interactive discovery.

## Supervised live authority

Windows developers can exercise a real JIT live workspace instead of a packaged
application. The existing supervisor remains the parent process and launches
the Stasis CLI as the live authority; the CLI does not create a hidden child
process. It builds and verifies a fresh network guest bundle, initializes the
native host and guest, and publishes the private readiness frame only after all
fallible startup setup is complete.

```powershell
$evidenceRoot = Join-Path $env:TEMP ("stasis-session-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $evidenceRoot | Out-Null
./stasis-network-supervise.exe --toolchain C:/stasis/stasis.exe --workspace C:/games/MyGame --peer C:/tests/peer.exe --evidence-root $evidenceRoot --startup-ms 60000 --action-ms 30000 --shutdown-ms 10000
```

The live mode requires `--toolchain`, `--workspace`, and `--evidence-root`;
packaged mode continues to use `--authority` with its existing behavior. The
evidence root must already exist as an empty, dedicated directory beneath the
system temporary directory. The supervisor and CLI reject a root that is a
reparse point, a symlink, the temporary directory itself, or a different path
from the one inherited by the authority.

In live mode the supervisor also reads a bounded JSONL control stream from its
own standard input and writes one fixed-schema JSONL response per accepted
request to standard output. Schema version 1 allows only `pause`, `resume`,
single-tick `step`, `set_input_state`, `capture_frame`, and `quit`. Pointer
input is limited to eight unique nonnegative pointer IDs and checked against
the current sampled game viewport before it is acknowledged. A resize clears a
stale supervised pointer override before it can be applied to a later frame.
The `step_scheduled` response confirms that one tick was scheduled; it does not
claim the tick has run. A consumer that needs to prove execution must observe
the resulting application state through the peer protocol.

Requests are limited to 512 per session and 120 per second. Capture IDs are
unique in a session and limited to 0-15. Each saved PNG is limited to 16 MiB,
and total capture bytes are limited to 256 MiB. Capture output uses only fixed
names (`supervised-capture-00.png` through `supervised-capture-15.png`); the
response contains dimensions, byte length, and SHA-256 but no filesystem path.
Peer receipts are projected to a fixed numeric-field allowlist and retained as
`peer-receipts.jsonl`. The retained artifacts contain no invite URL, secret,
source path, or arbitrary child diagnostic. Keep or remove the dedicated
evidence directory according to the consuming test's retention needs.

The upstream real-game acceptance fixture is
`tests/fixtures/network_supervision/live_ttt/`. It vendors four hash-pinned
Maddox and Friends rules/protocol modules unchanged and drives legal moves
through those production game functions with a real authenticated `NetworkClient`
peer. The script checks host pointer input, private join/authentication,
acknowledgements, malformed and duplicate command rejection, win/rematch/draw,
return to the catalog, exact state projections and captures. It also runs a
separate process-level backpressure case with supervisor stdout unread and
verifies that the supervisor exits within its bound and both child processes
are gone. This proves the Stasis-owned authority/supervisor bridge against the
vendored TTT rules; it is not a claim that the downstream Maddox and Friends
application's complete ten-game matrix has been integrated.

For a source checkout, first build a fresh native graphics runtime and runner
with the same release identity and build fingerprint as the CLI. Set
`STASIS_RUNTIME_DLL_PATH`, `STASIS_RUNTIME_LIBRARY_PATH`, and
`STASIS_RUNTIME_RUNNER_PATH` to those selected outputs. The Cargo commands below
build the Rust CLI, supervisor, and peer; they do not build the native DLL or
runner. The legacy packaged supervision test stages those explicitly selected
native files beside its isolated CLI and retains the CLI's release/fingerprint
checks. See [Windows game launch testing](windows_game_launch_testing.md) for
the source-tree native runtime selection rule.

Then, from a Windows source checkout, build fresh matching Rust artifacts and
run the acceptance script:

```powershell
python tools/cargo_cache.py run -- cargo build -p stasis --bin stasis
python tools/cargo_cache.py run -- cargo build -p stasis_network --features supervision-cli --bin stasis-network-supervise
python tools/cargo_cache.py run -- cargo build -p stasis_network --example supervision_live_ttt_peer
powershell -NoProfile -ExecutionPolicy Bypass -File tests/fixtures/network_supervision/live_ttt/run_acceptance.ps1 -Toolchain build/codex-cargo-target/debug/stasis.exe -Supervisor build/codex-cargo-target/debug/stasis-network-supervise.exe -Peer build/codex-cargo-target/debug/examples/supervision_live_ttt_peer.exe
```

The script retains its evidence root and prints its path on success or failure.
Inspect the retained PNGs and `acceptance-summary.json` alongside the
`peer-receipts.jsonl` when reviewing an acceptance run.

Each supervisor invocation owns its child processes and private handoff. The
host chooses an ephemeral port and a fresh production pairing secret, so
concurrent instances do not share ports, credentials, or readiness files.
Application persistence remains application-owned: use separate package/data
directories when an application writes persistent state.

## Consumer validation and release handoff

From a clean Stasis source checkout in an x64 MSVC developer shell on Windows,
first create a fresh matching graphics runtime and runner. The release ID is
unique to the source commit, and the same computed fingerprint is embedded in
the CLI and native graphics runtime:

```powershell
$env:STASIS_SOURCE_COMMIT = (git rev-parse HEAD).Trim()
$env:STASIS_RELEASE_ID = "local-network-supervision-$($env:STASIS_SOURCE_COMMIT.Substring(0, 12))"
$env:STASIS_BUILD_TARGET = "x86_64-pc-windows-msvc"
$env:STASIS_BUILD_FINGERPRINT = (python tools/compute_toolchain_fingerprint.py --source-commit $env:STASIS_SOURCE_COMMIT --release-id $env:STASIS_RELEASE_ID).Trim()
$generator = (& powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./tools/windows/select-cmake-vs-generator.ps1).Trim()
cmake -S runtime -B target/network-supervision-runtime -G $generator -A x64 `
  -DSTASIS_GRAPHICS_BUNDLE_SDL=ON `
  -DSTASIS_GRAPHICS_BUILD_SHARED=ON `
  -DSTASIS_GRAPHICS_BUILD_STATIC=OFF `
  -DSTASIS_BUILD_RUNNER=ON `
  -DSTASIS_BUILD_SYS=OFF `
  -DSTASIS_RELEASE_ID="$env:STASIS_RELEASE_ID" `
  -DSTASIS_BUILD_FINGERPRINT="$env:STASIS_BUILD_FINGERPRINT"
cmake --build target/network-supervision-runtime --config Release --target stasis_graphics stasis_runner
$runtime = (Resolve-Path target/network-supervision-runtime/bin/Release/stasis_graphics.dll).Path
$runner = (Resolve-Path target/network-supervision-runtime/bin/Release/stasis_runner.exe).Path
$env:STASIS_RUNTIME_DLL_PATH = $runtime
$env:STASIS_RUNTIME_LIBRARY_PATH = $runtime
$env:STASIS_RUNTIME_RUNNER_PATH = $runner

python tools/cargo_cache.py run -- cargo test -p stasis_network --test supervision
powershell -NoProfile -ExecutionPolicy Bypass -File tools/ci/test_network_supervision.ps1
```

The test script builds fresh CLI and supervisor binaries with this identity,
stages the selected native DLL and runner beside its isolated CLI, and enables
the `supervision-cli` Cargo feature for the supervisor. The package command
continues to verify the graphics runtime identity. This leaves ordinary
Android and iOS network-library builds unchanged.

Use a short Windows checkout path (or a temporary `subst` drive pointing at
the checkout) when running the harness. Nested SDL dependency projects can
otherwise exceed native build tools' 260-character path limit, including some
Ninja versions. A drive alias keeps all generated files in the checkout.

To test an extracted official toolchain, pass `-Toolchain <stasis.exe>` and
`-Supervisor <stasis-network-supervise.exe>` plus `-InstalledToolchain` to the
PowerShell command. The installed mode packages without `--development-build`.
The nightly release workflow runs this check before publication, using the
extracted release archive. The fixture exercises real packaged Stasis code and an
external authenticated socket peer; it is not a replacement for downstream
game-specific assertions.

Task #527 supplies this interface for the
[Maddox and Friends computer player](https://github.com/benwmaddox/maddox-and-friends/blob/main/docs/computer_player.md).
Its game-specific integration and physical-phone LAN evidence belong to parent
#344. Consumers must use an official Windows release containing this document
and supervisor, then record that release tag with their validation. The release
workflow gates publication on the installed-toolchain check above. Source-build
acceptance establishes readiness for that publication flow; it does not establish
that an official release has shipped.
