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

Each supervisor invocation owns its child processes and private handoff. The
host chooses an ephemeral port and a fresh production pairing secret, so
concurrent instances do not share ports, credentials, or readiness files.
Application persistence remains application-owned: use separate package/data
directories when an application writes persistent state.

## Consumer validation and release handoff

From a Stasis source checkout on Windows:

```powershell
python tools/cargo_cache.py run -- cargo test -p stasis_network --test supervision
powershell -NoProfile -ExecutionPolicy Bypass -File tools/ci/test_network_supervision.ps1
```

The harness enables the `supervision-cli` Cargo feature when building the
supervisor binary. This leaves ordinary Android and iOS network-library builds
unchanged.

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
