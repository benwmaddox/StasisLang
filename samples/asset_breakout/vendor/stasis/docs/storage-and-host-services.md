# Storage and host services

<!-- tags: storage, clipboard, platform, preferences -->

Import `/vendor/stasis/stdlib/storage.stasis` for small durable preferences.
See the compiled [call example](examples/src/feature_reference.stasis):

```stasis
function save_best_score(value: i32): bool {
    return storage_save_i32("knowledge_demo", "best_score", value);
}
```

`storage_load_i32(scope, key, fallback)` returns the fallback on failure.
`storage_save_i32` returns `true` after complete publication. Scope and key
must be 1-63 ASCII letters, digits, underscores, or hyphens.

`storage_load_ascii` fills caller-owned `ascii[N]`, returning its byte count
or `-1`; failure clears its logical length. `storage_save_ascii` accepts
printable ASCII bytes 32-126. Capacity bounds the accepted value.

`clipboard_load_ascii` and `clipboard_save_ascii` provide bounded clipboard
access where the graphical host supports it. `open_external_url` requests a
host action; its result does not prove completed browser navigation.

Storage, clipboard, URLs, and network responses are host observations. Check
[replay limits](record-and-replay.md) before relying on them in a recorded run.
