# Android Workshop Codex Harness

## Outcome

The primary Workshop AI provider is Codex running entirely on the Android
device. Direct OpenAI API access remains an explicit fallback with one
device-wide monthly USD limit. Desktop pairing is optional and is not required
for normal Workshop use.

This follows the Codex authentication split: ChatGPT sign-in uses subscription
entitlements, while API-key sign-in uses usage-based Platform billing. See
[Codex authentication](https://learn.chatgpt.com/docs/auth).

## Primary boundary

```text
Android Workshop UI
  voice, prompts, approvals, progress
        |
        | JNI / in-process calls
        v
Phone-native Codex Rust components
  ChatGPT device login, token refresh, account limits, agent turns
        |
        | controlled Workshop tool bridge
        v
Stasis project and runtime on the same phone
  symbols, compile, tests, frames, screenshots, diagnostics
```

The phone owns the source, runtime state, credentials, conversations, and tool
execution. It does not expose an app-server listener or copy credentials to
another machine.

## Upstream compatibility audit

The Android build is pinned to official Codex revision
`5c19155cbd93bfa099016e7487259f61669823ff`. The official `codex-login` crate
successfully compiles for `aarch64-linux-android` after disabling desktop
native-TLS defaults in favor of the existing Rustls stack. The repeatable build
is `mobile/android/build_codex_native.ps1`; it fetches that exact revision,
applies the checked-in TLS patch, builds a native library, and packages it into
the Workshop flavor.

The full `codex-app-server` crate does not yet build unchanged for Android. Its
Code Mode dependency requests a prebuilt Android Rusty V8 archive that is not
published for the pinned V8 release. The phone-native integration therefore
starts with the official login/account layer and will add the agent client and
Workshop tool loop without V8. Code Mode should be gated out for Android rather
than replaced with a fake implementation.

## Authentication

The Workshop calls Codex's official device-code flow from its native Rust
library. It displays the verification URL and one-time code, opens the Android
browser, copies the code to the clipboard before navigation, polls in the
native layer, and stores the resulting `auth.json` under
the application's private files directory. The credentials never leave the
phone. The selectable in-app code and completion status remain visible when
the user returns from the browser; the matching clipboard entry is cleared
after successful sign-in. The app-server protocol documents this frontend-owned flow as
`chatgptDeviceCode`; see the
[Codex app-server documentation](https://learn.chatgpt.com/docs/app-server).

The current first slice provides:

- a phone-native Codex provider presented as the primary path; existing API-key
  installations retain their fallback selection until the turn bridge is ready
- real ChatGPT device-code login using upstream Codex code
- persistent account detection and ChatGPT plan display
- native Codex primary/secondary rate-limit reads using the official
  `usedPercent`, `windowDurationMins`, and `resetsAt` contract
- an explicit OpenAI API-key fallback
- a deterministic pinned-source Android build

## Workshop tool harness

The agent turn layer should expose only controlled Workshop operations, not a
general Android shell:

- list and read symbols
- update source or a symbol
- compile and run tests
- capture the rendered game
- inspect runtime diagnostics
- request explicit approval for consequential changes

The existing Workshop Responses API harness already implements this operation
set and its deterministic edit/compile/test loop. The phone-native Codex client
should adapt those handlers at the tool-call boundary instead of duplicating
their behavior.

## Limits

- **Codex subscription:** do not estimate dollars. Display Codex account
  rate-limit data as remaining percentage for the five-hour and weekly windows.
  Refresh after a Codex action with a 30-minute attempt debounce and retain the
  last successful snapshot if a refresh fails.
- **Direct API fallback:** show estimated API cost and enforce the device-wide
  monthly USD limit after every response or image generation.

## Remaining slices

1. Add the minimal upstream Codex model client and thread/turn stream without
   the V8-backed Code Mode component.
2. Adapt the existing Workshop tool handlers to Codex tool calls and verify a
   complete edit-compile-test-screenshot turn on the phone.
3. Add cancellation, approvals, and persisted thread resume.
4. Encrypt or wrap the private Codex credential file with Android Keystore
   protection and add logout/account-switch controls.
5. Retain paired desktop execution only as an optional provider for large
   desktop repositories.
