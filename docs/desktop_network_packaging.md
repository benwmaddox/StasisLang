# Desktop LAN host packages

For optional Windows and Android native guests, see
[Native guest transport](native_network_client.md).

For bounded external-peer automation using the packaged authority and a
private invite handoff, see [Native authority supervision](network_supervision.md).

Windows, Linux, and macOS production packages can host browser guests when the project manifest
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
and staged with the native assets. The desktop monolith links the target-native Rust
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

Automatic IPv4 selection enumerates native interfaces without requiring an
Internet connection or default route. If multiple usable addresses are present
(including VPNs), select the intended LAN explicitly before launching:

```powershell
$env:STASIS_NETWORK_ADVERTISE_IPV4 = '192.168.1.25'
& './MyGame.exe'
```

Use the host's address on the same LAN as the browser device. The override
changes the advertised address, not the listening interface. Invalid address
syntax, unspecified, multicast and broadcast addresses fail startup. A
loopback override is useful only for a browser on the host itself; automatic
selection never silently falls back to loopback. Restart the
application after changing adapters or the override; existing private links
belong to the old host session.

See [portable LAN policy and platform acceptance](portable_lan.md) for selection
failure policy, optional discovery, and platform permission onboarding.

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

Network-enabled desktop packages use the shared monolith shell on Windows,
Linux, and macOS. The release matrix builds Windows x86_64, Linux x86_64,
and macOS arm64. Native library resolution also distinguishes macOS x86_64
and Linux arm64; those architectures require a matching toolchain archive.
Downstream Maddox and Friends application adoption is separate.

Nightly and bootstrap toolchain archives include one target-native library:

- `desktop/network/windows-x86_64/stasis_network.lib`
- `desktop/network/linux-x86_64/libstasis_network.a`
- `desktop/network/macos-arm64/libstasis_network.a`

Each archive includes `desktop/network/include/stasis_network.h`. Installed
packaging resolves these relative to the compiler executable and checks the
exact native pair and its recorded provenance hashes before linking. A library
for another architecture, a missing header, or a substituted payload fails
packaging. Source checkouts build a fresh release static library in package
staging; only Windows uses the static CRT build flag. Browser guest bundles
and their audit sidecars share the package root with game assets on Unix
(under `app/` on Windows). On macOS the executable lives inside its `.app`
bundle; its asset root resolves back to that package directory. Network-enabled packages do not need a separate Stasis runtime
shared library.

Linux and macOS use the same F1 join card and explicit copy action as Windows.
The macOS app includes a local-network usage description.
The displayed card excludes credentials; only the clipboard action receives the
private invite. Native interface selection and `STASIS_NETWORK_ADVERTISE_IPV4`
keep the shared LAN policy. For example, on a Unix shell:

```sh
STASIS_NETWORK_ADVERTISE_IPV4=192.168.1.25 ./MyGame
# macOS app executable:
STASIS_NETWORK_ADVERTISE_IPV4=192.168.1.25 ./MyGame.app/Contents/MacOS/MyGame
```

Use the operating system's local-network/application permission controls on the
intended trusted network. Packages do not change firewall rules or bypass
managed policy. Manual invitations, transport admission, and authoritative
application state retain the existing protocol and ownership boundaries.

Immutable publication remains owned by the nightly release workflow. Do not
substitute a development package for the official release contract required by
downstream #357. Publication coordination belongs to existing task #477. Record
the first successful immutable release containing these changes, including its
source revision and provenance manifest, before satisfying that downstream
dependency. Source validation and PR publication alone do not establish that
release contract.

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
tests on Windows. Linux and macOS jobs run the native link/lifecycle probe
(`bash tools/ci/test_desktop_network_link.sh`), package contract tests, and
provenance tests. Nightly packaging additionally builds a network-enabled
desktop package from the relocated release archive with source inputs detached.
PR CI also runs `bash tools/ci/test_unix_desktop_network_package.sh` on Linux
and macOS to build a native host package and audit its staged browser guest.
Both platforms run the HTTP and process-exit listener probe; macOS also checks
the app executable and local-network permission description.

The package provenance verifier also checks each guest bundle against its
sidecar length and SHA-256, rejecting missing or substituted payloads. Linux
nightly packaging runs `tools/ci/test_linux_desktop_network_package.py` against
the generated executable with software rendering: it starts outside the package
directory, requests the staged guest page, and checks listener release after
process exit. The harness records only boolean results, never private invites.
