# Android Workshop Codex Harness

## Outcome

The Workshop should support two AI providers without changing its role as the
authoritative UI and runtime harness:

1. **Direct API mode** sends Responses API requests from the Android device. It
   enforces one device-wide monthly USD limit. There is no separate per-action
   or per-run limit.
2. **Codex subscription mode** pairs the Android Workshop with a trusted desktop
   companion running `codex app-server`. Codex signs in with ChatGPT on the
   desktop, while the Workshop supplies the Stasis-specific tools and displays
   progress, approvals, and results.

This follows the supported Codex authentication split: ChatGPT sign-in uses
subscription entitlements, while API-key sign-in uses usage-based Platform
billing. See [Codex authentication](https://learn.chatgpt.com/docs/auth).

## Recommended boundary

```text
Android Workshop
  voice, prompt, source/runtime state, screenshots, approvals
        |
        | outbound authenticated TLS pairing
        v
Desktop companion
  secure session relay + Workshop MCP proxy
        |
        | local stdio JSON-RPC
        v
codex app-server
  ChatGPT login, threads, turns, events, rate limits
```

`codex app-server` is the supported integration surface for embedding Codex in
another product. It provides account login, thread/turn lifecycle, streaming
events, approvals, and rate-limit reads. Its WebSocket transport is currently
experimental, so the companion should use the stable stdio transport and own
the authenticated network connection to the phone. See the
[Codex app-server documentation](https://learn.chatgpt.com/docs/app-server).

The app-server must not be exposed directly to the public network. The phone
initiates pairing to a companion with TLS, a high-entropy one-time pairing
secret, explicit user approval, and a pinned device identity. This is consistent
with the security guidance for [Codex remote connections](https://learn.chatgpt.com/docs/remote-connections).

## Workshop as the harness

The phone remains authoritative for the running Stasis project. The companion
exposes a local MCP server to Codex and relays its tool calls to the paired
Workshop. The first tool set should cover the existing Workshop operations:

- list and read symbols
- update source or a symbol
- compile and run tests
- capture the rendered game
- inspect runtime diagnostics
- request an explicit approval for consequential changes

The app-server owns generic agent behavior: conversation history, turn state,
streaming progress, tool-call routing, cancellation, and approvals. This avoids
reimplementing the Codex agent loop on Android while preserving the Workshop's
specialized Stasis context.

## Authentication and limits

For an individual ChatGPT account, authentication occurs interactively on the
trusted desktop using browser or device-code login. ChatGPT credentials and
tokens are never copied to or extracted by the Android application. Codex access
tokens are an additional automation option for supported Business and Enterprise
workspaces; see [Codex access tokens](https://learn.chatgpt.com/docs/enterprise/access-tokens).

Budget presentation depends on provider:

- **Direct API:** show estimated API cost and enforce the device-wide monthly
  USD limit after every response or image generation.
- **Codex subscription:** do not estimate dollars. Read and display the Codex
  account limits from `account/rateLimits/read` and subscribe to
  `account/rateLimits/updated`.

## Implementation slices

1. Remove the direct API per-run limit and retain the device monthly cap.
2. Build a desktop companion prototype with local app-server process management
   and authenticated phone pairing.
3. Implement app-server initialization, account status, thread resume/start,
   turn streaming, cancellation, and approvals.
4. Relay the Workshop tool surface through an MCP proxy and test a complete
   edit-compile-run-screenshot loop.
5. Add an API/Codex provider selector, subscription rate-limit display, session
   recovery, and an explicit API fallback.
