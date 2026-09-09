# Portable LAN foundation

`stasis_network::lan` owns native address selection and the permission/discovery
adapter contract. The existing HTTP/WebSocket protocol, native mailbox ABI,
session credentials, QR content and manual URL fragment format are unchanged.
Discovery is an optional future convenience, not a prerequisite for sharing a
private invite. This slice does not run an mDNS browser or publish a service.

## Selection and failure policy

For an IPv4 wildcard listener, an explicit Rust advertised-address argument wins
over `STASIS_NETWORK_ADVERTISE_IPV4`; either override bypasses enumeration.
Overrides change only the invite address, not the listener or firewall. They
are operator assertions: the adapter cannot prove that a remote peer can reach
them. A concrete bind uses its own address. Loopback remains available for
explicit same-machine tests.

Automatic selection enumerates local interfaces using the native OS adapter.
It requires neither Internet access nor a default route, so an isolated Wi-Fi
network with a usable assigned IPv4 address works. It excludes loopback,
unspecified, link-local, multicast, broadcast and reserved address ranges.
Duplicate addresses do not create ambiguity. A single usable address is chosen;
multiple distinct addresses require an override, independently of enumeration
order. Interface names are not a reliable portable way to distinguish a VPN
from Wi-Fi, so selection never guesses from names or prefers a default VPN route.
No usable IPv4 address, ambiguous selection and enumeration failure are explicit
failures, never an apparently successful localhost invite. IPv6-only automatic
advertisement is outside this slice.

The typed Rust errors distinguish selection failures; the existing C ABI keeps
its existing error contract. Diagnostics must not include environment values,
interface names, SSIDs, session secrets, resume credentials or private URLs.
Only the explicit copy/QR action should materialize a private invite. An enumerated address is an OS-assigned candidate; adapters with retained
addresses can still be disconnected. A chosen address or successful bind proves
neither peer reachability nor permission.
After an adapter change, restart the host and share the new session invite.

## Permission and discovery shell

Native shells report local-network access separately from discovery status.
Unknown permission is not a claim of access; denial requires user action in
platform settings. Disabled, unavailable or denied discovery leaves manual
copy/QR joining available when local-network access is allowed. A denial of
local-network access affects direct IP links too. Do not infer permission from
Internet connectivity or trigger a prompt at application startup without a
user-initiated LAN action. Platform callbacks can populate the portable states
when each production shell implements discovery. `UnsupportedLanShell` is the
honest default: permission is unknown, discovery operations return a typed
unsupported error, and repeated stop calls succeed. Shell errors carry no
platform error strings or user data.

| Platform | Onboarding and integration boundary |
| --- | --- |
| Windows | On the user's host action, explain inbound access for the packaged executable on the intended trusted Private network. Use Windows Security > Firewall & network protection > Allow an app through firewall if the initial prompt was denied. Managed rules can override local settings. Never disable the firewall, elevate automatically, or add broad public-network rules. The existing read-only `tools/diagnose_desktop_network.ps1 -Port <port>` reports policy without credentials. |
| Android | The current Android shell targets SDK 35. Keep `INTERNET` for sockets; NSD uses `NsdManager` behind the optional discovery adapter. Android 16 offers local-network restriction testing; Android 17 apps targeting SDK 37 require `ACCESS_LOCAL_NETWORK` for broad local access or eligible system-mediated device selection. Handle denial/revocation separately from NSD failures. A future target upgrade must add the appropriate manifest/runtime flow and device tests together, not merely declare a permission. |
| iOS/iPadOS | Add a meaningful `NSLocalNetworkUsageDescription` to the containing app. If browsing/registering Bonjour services, declare their actual types in `NSBonjourServices`. Check multicast entitlement requirements for the actual APIs used; do not add a blanket entitlement just for manual HTTP links. Test on physical devices with allow, deny and Settings recovery. |
| macOS | Apply Apple's Local Network privacy requirements and a stable signed application identity; Bonjour service declarations belong to the app bundle. Account for the application firewall and sandbox network client/server entitlements where sandboxing is used. Permission state is per user. |
| Linux | No universal desktop LAN permission dialog exists. Report unavailable permission introspection honestly. Respect the distribution firewall (for example firewalld/nftables), sandbox network policy, and optional Avahi availability. Missing Avahi must not prevent direct IP invites. Never modify firewall rules automatically. |

Platform references, checked September 2026:

- [Android local network permission and NSD evolution](https://developer.android.com/privacy-and-security/local-network-permission)
- [Apple Local Network privacy, Bonjour and signing requirements](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)
- [Windows application firewall rules and profiles](https://learn.microsoft.com/en-us/windows/security/operating-system-security/network-security/windows-firewall/rules)

## Bounded acceptance

Run `python tools/cargo_cache.py run -- cargo test -p stasis_network --lib --tests`
with a 15-minute limit. `network-browser-acceptance.yml` runs this on Windows,
Linux and macOS; its existing Android native-client job retains device ABI and
transport coverage. Deterministic adapter tests cover offline selection,
order/duplicate handling, multi-NIC ambiguity, invalid candidates, enumeration
failure and independent discovery/local-network states. Native enumeration is
also exercised on the actual CI operating system. These checks do not prove
firewall traversal or physical Wi-Fi reachability.

Before shipping a platform shell, perform this representative two-device gate:

1. Connect host and peer to the same isolated Wi-Fi with Internet disconnected.
   Start hosting; copy/scan the invite and verify semantic join, message and
   reconnect behavior. Keep the URL fragment out of captured evidence.
2. Enable a second NIC or VPN. Expect explicit selection failure; set the host's
   Wi-Fi override and repeat the join. Disable the adapter and verify failure
   is reported without silently switching an existing invite.
3. Deny inbound/local-network access, verify a useful settings recovery path,
   then allow access and repeat. On Windows test Private versus Public profile;
   on Apple test the signed app; on Android test current and upgraded target
   permission behavior; on Linux test the deployment firewall/sandbox.
4. Disable or deny discovery while permitting local access. Manual and QR links
   must still work. If local access itself is denied, neither path may claim
   successful connection. Repeat with client isolation enabled to ensure a
   listener alone is never reported as proof of remote reachability.

Record device/OS, permission choice, selected address source (not private URL),
result and secret-free evidence in release acceptance. Hardware permission and
multi-device checks are manual gates, not claims made by the desktop CI matrix.
