# External URL host action

Import `src/stdlib/external_url.stasis` and call
`open_external_url(url: string): i32` from a pointer `went_down` or non-repeat
keyboard activation. The first consumer can pass `https://www.maddoxlabs.com/`.
The API has the `platform` effect; games never invoke shells or platform code.

| Result | Meaning |
| --- | --- |
| `EXTERNAL_URL_INVALID` (-1) | Invalid handle, encoding, URL, or byte length. No dispatch. |
| `EXTERNAL_URL_IGNORED` (0) | No input authority, deterministic run, unavailable host, or blocked/refused dispatch. |
| `EXTERNAL_URL_OPENED` (1) | The platform accepted the browser dispatch. Page loading is outside the contract. |

Do not branch authoritative gameplay on the result. Browser availability and
user settings are external state. A recording/headless run validates the same
request but returns ignored without opening a browser. Tests may inject an
opener to observe requests without launching anything.

## Validation and action lifetime

The payload is nonempty UTF-8, at most 2048 bytes, checked before copying guest
text. Both literal and mutable string representations use that bound. Only
literal lowercase `http://` and `https://` prefixes are supported. The host
rejects C0/C1 controls, DEL, ASCII whitespace, backslashes, malformed percent
escapes, credentials, empty/malformed hosts, and invalid ports. Hostnames use
ASCII DNS spelling (use punycode for international names), IPv4, or bracketed
IPv6. Paths, queries, and fragments can contain valid UTF-8.

One real input edge provides one attempt in the current frame. A valid attempt
consumes that authority even when dispatch is blocked; held input does not
renew it. Frame completion, focus loss, and lifecycle transitions discard
unused authority. Invalid requests do not consume an edge. The game remains
responsible for hit-testing its link and consuming the UI input before gameplay
handlers. The host does not infer which on-screen element was activated.

## Platforms

| Host | Dispatch |
| --- | --- |
| Desktop | SDL URL opener, passed a bounded terminated string directly without shell interpolation. Requires a real pointer/key edge and a live window. |
| Android release shell | `ACTION_VIEW` browser intent marshalled to the Activity UI thread. Lifecycle checks prevent opening from an unavailable Activity; missing handlers and security exceptions are handled. Acceptance means queued dispatch. |
| Android Workshop JIT | A real preview touch arms the embedded host callback across the same bounded guest boundary and native browser adapter. Synthetic acceptance/AI input does not arm it. The existing Workshop tick input is touch-based. |
| iOS | `UIApplication` URL opening on the main thread with foreground checks and completion/error handling. Acceptance means queued dispatch. |
| Web | Requires active browser user activation and one pointer/key edge. A blank window is opened only when a valid URL is requested, its opener is immediately cleared, then a `noopener noreferrer`/`no-referrer` link navigates it. A refused popup returns ignored; requests are never retried automatically. |
| Headless/recording | Valid requests return ignored. No browser dispatch. |

The web host checks transient activation at the actual request. A browser may
expire activation before a delayed tick; that request returns ignored. Games
must request the link on the activation frame rather than from a timer or
background callback.

## Compiler and lifecycle contracts

The guest extern is `stasis_jit_open_external_url(i32 string_handle): i32`.
JIT resolves it through `stasis_dynload`; desktop AOT links the same runtime
export; mobile AOT resolves its literal/collection storage before invoking the
registered host adapter. Web packages retain the import and any dynamic text
memory it needs. Unused functions remain subject to normal reachability pruning.
No HostFrame or render ABI offsets change.

URL calls from JIT `on_code_swap` execution return ignored, so a rejected swap
cannot open an external page. Input authority is host state and is not restored
from guest snapshots or replayed by hot-swap state restoration.

The deterministic pointer fixture is
`tests/stasis/seams/external_url_edge_probe.test.stasis`: one down edge followed
by held frames makes one request. Focused Rust, C, and JavaScript tests cover
the guest mapping, bounded validation, unavailable dispatch, and one-shot
consumption. Platform-device and visual validation limits are recorded in the
task handoff; unit tests do not establish that a device has a browser installed.
