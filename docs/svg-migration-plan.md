# SVG Pipeline (Current)

Stasis now ships SVG-only sprites. Legacy `.stv` has been removed. This note captures the remaining work to finish the pipeline polish.

## Current Flow

```
svg source -> bake_svg_to_rgba() -> RGBA pixels -> GPU atlas (mipmapped)
                                \-> hot reload via mtime check
```

## Remaining Tasks

1. Add an on-disk PNG cache for faster cold starts (optional; gitignored).
2. Ensure hot-reload clears any cached PNG before rebuilding.
3. Remove any straggling references to `.stv` in tooling/tests if they show up in the future.

## Guidance for Authors

- Author sprites as plain SVG with explicit `width`/`height` (or a `viewBox` that matches).
- Keep shapes simple (rect, line, circle, path) for predictable baking; gradients/opacity are OK.
- Avoid SVG filters and SMIL animation tags; the runtime bakes a single frame. For animation, split into layers and animate transforms/alpha in Stasis code.
- Keep files ASCII to match repo guidelines.
