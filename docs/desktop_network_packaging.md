# Desktop LAN host packages

Windows production packages can host browser guests when the project manifest
declares both the network capability and a web entry point:

```json
{
  "capabilities": { "network": true },
  "web": { "entry": "src/guest.stasis" }
}
```

These fields extend the normal project manifest; the desktop `entry` remains
the authoritative native application. Run `stasis package --target desktop`
from the project, or add `--development-build` for a source-built toolchain.
The guest is compiled through the existing web packaging pipeline. Its Wasm,
JavaScript, HTML and reachable assets are encoded into `network_guest.bundle`
and staged with the native assets. The Windows monolith links the Rust
`stasis_network` static library and uses the existing bounded native mailbox ABI.
Non-network packages do not start a listener or stage a guest bundle.

The native runtime starts the host after graphics initialization and AOT runtime
binding, before guest main, and stops it before runtime teardown. The native
runtime owns the host handle; the shell owns join-card presentation. Guest code polls and sends
application messages through `network_client.stasis`. Transport admission and
resume identity are native responsibilities. Application ACKs, snapshots and
commands remain part of the application's protocol; the transport does not
invent game state or replay a game command automatically.

## Joining and address selection

The native join card offers an explicit copy action. Its visible text contains
the selected address and port, with no pairing or resume credential. Press F1
to reopen it. The copied private URL carries pairing data in
its fragment; share it only with intended players. The browser adapter owns
pairing and resume credentials, while Stasis receives only bounded semantic
mailbox data. Do not paste private links into logs, bug reports or screenshots.

Automatic IPv4 selection consults the host routing table. On machines with
multiple adapters, VPNs, or no default route, set the advertised address before
launching the package:

```powershell
$env:STASIS_NETWORK_ADVERTISE_IPV4 = '192.168.1.25'
& './MyGame.exe'
```

Use the host's address on the same LAN as the browser device. The override
changes the advertised address, not the listening interface. Invalid address
syntax, unspecified, multicast and broadcast addresses fail startup. A
loopback fallback is useful only for a browser on the host itself. Restart the
application after changing adapters or the override; existing private links
belong to the old host session.

## Windows diagnostics

If the browser cannot connect, check the displayed address and port, then run
the read-only diagnostic helper with that port:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/diagnose_desktop_network.ps1 -Port 45678
```

It reports TCP listeners, network categories and effective firewall profile
defaults. Missing permissions or unavailable Windows networking providers are
reported as unavailable, without dumping exception details or process command
lines. A listener alone does not prove remote reachability: firewall rules,
managed policy, VPN routing and Wi-Fi client isolation can still prevent access.
Use Windows' application firewall permission flow for the packaged executable
on the intended trusted network. The package and helper never change firewall
policy automatically. Do not disable the firewall to diagnose pairing.

## Platform boundary and validation

Windows is the first production monolith target. The network runtime lifecycle
and native copy callback are portable C seams for later Linux and macOS shells;
this milestone does not claim production monolith packaging for those targets.
Downstream Maddox and Friends application adoption is separate.

Windows nightly toolchain archives include
`desktop/network/windows-x86_64/stasis_network.lib` and
`desktop/network/include/stasis_network.h`. Installed packaging resolves these
relative to the compiler executable. Source checkouts build a fresh release
static library with the static CRT in package staging. Missing installed
support fails packaging rather than producing an executable with a disabled
network capability.

Focused validation covers manifest requirements, bundle staging and native link
configuration, start/stop paths, explicit native copying, address selection,
and browser join/ACK/snapshot/command/reconnect behavior. The browser acceptance
harness retains credentials only in memory and captures a page with semantic
status, excluding the browser address bar and private links. Real multi-device
LAN access and machine-specific Windows firewall policy still require testing
on the intended deployment network.

Run the native boundary tests with
`python tools/cargo_cache.py run -- cargo test -p stasis_network` and, on
Windows, `powershell -NoProfile -ExecutionPolicy Bypass -File tools/ci/test_desktop_network_link.ps1`.
For browser acceptance, build the `browser_acceptance_host` example from
`stasis_network`, set `STASIS_NETWORK_HOST_EXECUTABLE` to that executable, and run
`node tools/run_network_browser_acceptance.mjs` with Chrome or Edge and FFmpeg
available. The harness writes PNG, MP4 and JSON evidence under
`target/network-browser-acceptance`. The MP4 records the asserted protocol
stages; it is not a timing or animation benchmark. The focused
`network-browser-acceptance.yml` workflow runs these gates plus package-content
tests on Windows.
