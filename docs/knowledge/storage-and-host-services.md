# Storage and host services

<!-- tags: storage, clipboard, platform, preferences -->

Use host services for small durable preferences and deliberate external
actions. Keep ordinary gameplay state in Stasis globals and advance it from
`tick()`. The [compiled example](examples/src/feature_reference.stasis) shows
the storage call shape; its test exercises the pure bounded-data path without
depending on a host preference directory.

## Small durable values

Import `/vendor/stasis/stdlib/storage.stasis`. `storage_load_i32(scope, key,
fallback)` returns the fallback for a missing, invalid, corrupt, or unavailable
value. `storage_save_i32` returns `true` only when the complete value was
published. Use a stable scope for the game and a stable key for each value.
Both components must be 1–63 ASCII letters, digits, underscores, or hyphens.

```stasis
function save_best_score(value: i32): bool {
    return storage_save_i32("knowledge_demo", "best_score", value);
}
```

For a small share code or draft, `storage_load_ascii` writes into a
caller-owned `ascii[N]` and returns a byte count, or `-1` on failure. A failed
load clears its logical length. `storage_save_ascii` accepts printable ASCII
bytes 32–126. A buffer's capacity is the maximum accepted value; neither API
is a general file or serialization service.

## Clipboard and external actions

`clipboard_load_ascii` and `clipboard_save_ascii` use the same bounded ASCII
idea. Clipboard access depends on the graphical host; an unavailable or invalid
read fails without claiming success. `open_external_url` requests a host
action, subject to that platform's support and policy. Treat its result as a
request outcome rather than evidence that a browser finished navigation.

Storage, clipboard, URLs, network responses, and asynchronous readiness are
host observations. Compact input-only [record/replay](record-and-replay.md)
cannot reproduce those observations unless a profile explicitly captures
them. Keep this boundary in mind when selecting a replay test.
