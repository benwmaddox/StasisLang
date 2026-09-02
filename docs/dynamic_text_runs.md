# Dynamic text runs

`TextRun.replace_text_from(font, text)` is the cold-path API for text that changes during a
session. The caller owns and may reuse the `utf8[]` buffer. The runtime consumes the buffer during
the call and retains its own bounded copy and prepared layout; later `TextRun.draw` calls read only
the cached handle and prepared geometry. Rendering therefore performs no Stasis string copy and no
host call.

Call replacement during initialization, `tick`, or another graphics-effect cold path, never from
`render`. The function has `@effects(graphics)` and returns `true` only after the new font, UTF-8,
metrics, retained bytes, and layout have all been accepted. Failure leaves the receiver fields and
its previously drawable entry unchanged. An empty receiver remains empty after failure.

`load_text_from` remains the immutable API. Immutable runs deduplicate by `(font, text)`. Replacing
an empty receiver or an immutable receiver creates a distinct replaceable entry; subsequent
successful replacements reuse that entry's stable handle. This prevents one caller from changing
another caller's immutable cached text.

## Bounds and lifetime

- Native SDL (desktop and the SDL-backed macOS/iOS-supported paths) supports 16 active replaceable
  runs. Each owns at most 1023 retained UTF-8 bytes and 1024 prepared glyph quads. These fixed
  reservations bound churn independently of the immutable 1024-entry, 256 KiB byte, and 65536-quad
  cache. The font atlas is shared with immutable runs.
- Android Workshop's embedded catalog supports 4096 total runs, 256 KiB total retained text, and
  4096 bytes per replaceable run. Replacement recycles the catalog entry and preserves it across a
  compatible catalog refresh.
- Web supports 4096 total runs, 256 KiB total retained UTF-8, and 4096 bytes per replaceable run.
  The import reads the generated collection's current `.length` metadata, not its backing capacity
  or a NUL scan, so shortening a reused buffer cannot retain stale suffix bytes.
  Pending font calibration carries the run generation; stale readiness work cannot overwrite a
  newer replacement's metrics.

All targets reject malformed UTF-8 before publishing receiver state. Dynamic and immutable text use
the same target-specific measurement, glyph atlas, and fallback behavior. The native renderer's
current baked atlas covers the same limited character range for both kinds; valid UTF-8 outside
that range is retained but unsupported glyphs are skipped, as they are for immutable runs.

Compatible code swaps retain `TextRun` fields, so stable handles continue to draw. A rejected swap
does not alter the running state. Renderer or density restoration rebuilds prepared geometry from
retained text and the existing font identity. A full runtime/session reset clears host catalogs;
callers must initialize or replace their runs again after such a reset.

Pointer Pong demonstrates the intended pattern: two caller-owned UTF-8 buffers and two replaceable
runs are updated only when a score changes, preserve the original two-digit `00` through `99`
display, and let `render` emit cached-handle draw commands only.

## Characterization

The deterministic Android test `embedded_dynamic_text_churn_is_bounded_and_transactional` performs
5000 replacements, checks one stable dynamic handle and two total entries (one immutable and one
dynamic), changes fonts, uses a localized multibyte value, and verifies three failure modes preserve
the prior entry. Run it with:

```text
python tools/cargo_cache.py run -- cargo test -p stasis_android_bridge dynamic_text_churn -- --nocapture
```

Pointer Pong's before/after construction, steady-frame command count, and native retained-capacity
characterization compares baseline revision `14f6d57fbb69459302a552c905f199473322cec8` with the
working tree and is reproducible with Node by running:

```text
node tools/measure_dynamic_text_runs.mjs
```

On Windows x64 with Node v24.12.0, the migration changes construction from 10 immutable handles to
2 dynamic handles and a steady score frame from 4 cached draw commands to 2. The old digit payloads
occupied 20 arena bytes including terminators. Two active dynamic score capacities reserve 67584
bytes; the full 16-slot native pool reserves 540672 bytes, independent of replacement count. The
script also verifies that the hot draw uses only the cached handle and that replacement is absent
from `render`; measured hot-path string copies and host calls are both zero.
