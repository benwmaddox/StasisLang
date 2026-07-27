# Runtime integer storage

Import `src/stdlib/storage.stasis` when a game needs a small durable integer such as an unlocked level, high score, or settings version.

```stasis
import "../src/stdlib/storage.stasis";

let unlocked: i32 = storage_load_i32("my_game", "unlocked_level", 1);
if (unlocked < 4) {
    storage_save_i32("my_game", "unlocked_level", 4);
}
```

The graphical desktop host stores values under SDL's app-private preference directory. Android configures the same scoped storage contract under the application's private files directory. Headless Linux and macOS JIT hosts use the platform user-data directory; `STASIS_STORAGE_DIR` can override that root for controlled environments.

Every host uses the explicit game scope. Scope and key are restricted to 1-63 ASCII letters, digits, underscores, or hyphens; invalid names fail closed. A missing, invalid, or corrupt value returns the supplied fallback. Save returns `true` only after the complete integer value has been written and atomically published.

This API is intentionally narrow. It does not expose arbitrary filesystem paths, enumerate other games' values, serialize game memory, or provide a general document store. A future typed preference should extend the same scoped host boundary rather than exposing platform paths to Stasis code.
