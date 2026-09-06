# Native guest transport

Windows and Android production packages can join an existing native host with:

```json
{
  "capabilities": { "network_client": true }
}
```

The normal package entry is the guest application. Import the existing
`network_client.stasis` standard library and use its mailbox operations. A
client-only package links `stasis_network` without starting a listener or
requiring `web.entry` or a browser bundle. Host `capabilities.network` and
`network_client` are mutually exclusive. Web guests continue to use the browser
adapter. Native client packaging currently supports Windows and Android.

The native shell receives the host's existing private join link. On Windows,
set `STASIS_NETWORK_JOIN_URL` in the launching process environment. Android's
production activity accepts the private string Intent extra
`stasis.network_join_url`; the activity removes that extra after provisioning.
Supply credentials through a trusted launcher; do not include private links in
logs, screenshots, package manifests, source, or deterministic game state.
The initial native client supports the host's `http://IPv4:port/#secret=...`
LAN links. It does not add DNS resolution, TLS, or a new browser protocol.

## Transport and application ownership

The opaque native client owns the socket, pairing secret, random resume
credential, reconnect timing, and queues. It joins `/session` using the same
three WebSocket subprotocols as the browser: `stasis-v1`, the pairing secret,
and `stasis-resume-v1.<credential>`. It sends the host's expected Origin and
validates the selected response protocol. No transport error includes a
request, private URL, pairing secret, or resume credential.

The existing `stasis_web_network_*` import names also resolve in native AOT
packages. They expose bounded payload bytes, status, seat, and last sequence.
Messages are limited to 64 KiB; queues are limited to 1 MiB and 256 records.
An undersized receive buffer leaves its message queued. The C client ABI is
additive and has its own version query; the host ABI remains unchanged.

Application envelopes, join ACKs, per-seat snapshots, command ACKs, and
version checks within application messages remain application responsibilities.
The transport forwards their bytes without inventing state or replaying an
already-sent command. After reconnect, the application requests or applies a
fresh seat snapshot and uses its existing sequence/ACK rules. Checkpoints
contain only seat and last sequence; they do not contain credentials.

## Lifecycle

Connection status uses the existing mailbox convention: `0` disconnected,
`1` connected, and `2` connecting or waiting for a retry. Negative results
report bounded argument, transport, capacity, or configuration failures.
The adapter retries transient connection failures with bounded backoff.
Explicit disconnect and background suspension close the socket. Foreground
resumes a connection that was desired before suspension. Shutdown destroys
the adapter before runtime teardown.

Resume identity and the semantic checkpoint survive reconnect and background
within the same adapter lifetime. Replacing the private link or terminating
the process creates a new identity; this milestone does not persist native
credentials to disk. Late work from an old connection must not enter the new
mailbox. An application remains responsible for reconciling uncertain command
delivery from its authoritative snapshot and ACK sequence.

## Validation

Run all Cargo commands through `tools/cargo_cache.py`. In a restricted worktree,
set `CARGO_TARGET_DIR` to a path inside that worktree.

```text
python tools/cargo_cache.py run -- cargo test -p stasis_network --test native_client
node --test runtime/web/tests/network_mailbox_contract.test.mjs
powershell -NoProfile -ExecutionPolicy Bypass -File tools/ci/test_desktop_network_link.ps1
python tools/ci/test_android_network_client.py --serial emulator-5554 --ndk C:/Android/Sdk/ndk/27.0.12077973
```

The Windows and Android probes build a fresh static library, link the same C
client ABI fixture, and assert join, guest command, isolated seat snapshots,
checkpoint retention, and background/resume identity against a real host.
The Android probe runs on-device and records a credential-free result under
`target/android-network-client/result.txt`. It also builds and runs the native
AOT mailbox bridge contract on-device. It is a transport integration
probe, not graphical application or multi-device LAN acceptance.

The network acceptance workflow runs the native client tests alongside the
existing browser acceptance gate. The protocol is shared; neither native
transport nor native packaging replaces the browser path.
