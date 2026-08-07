# ImageGen walk-cycle experiment

This package tests an ImageGen-first animation workflow for the Hearthguard man-at-arms. It contains six generated keyframes, deterministic registration, transparent 192 x 192 runtime frames, and two animated review formats.

## Review

- [Transparent animated WebP](walk_cycle_192.webp)
- [Parchment-background review GIF](walk_cycle_192_review.gif)
- [Six-frame contact sheet](walk_cycle_sheet_192.png)
- [Individual transparent frames](frames_192/)
- [Generation and corrective prompts](prompts.md)
- [Animation manifest](animation_manifest.json)

The sequence runs at 120 ms per frame, approximately 8.33 frames per second, for a 720 ms loop.

## Result

Identity, palette, equipment handedness, shield emblem, camera, scale, and bottom-center registration remain coherent at runtime size. Deterministic alignment removes the placement jump between the two generated rows.

The gait is a usable stylized march candidate, not a production-approved classical walk cycle. ImageGen initially repeated the same leading-leg configuration. A targeted second pass improved the opposing stride, but the passing poses still have less leg separation and weight transfer than a hand-authored animation. Keep this asset as evidence for the workflow; approve it for gameplay only after seeing it move on the actual map.

## Pipeline

1. Generate one six-frame 3 x 2 keyframe sheet from immutable identity, sprite-scale, and pose references.
2. Reject semantic gait errors even when the still sheet looks attractive.
3. Apply one narrowly scoped edit to the failed half-cycle.
4. Remove the uniform chroma background with soft matte and despill.
5. Split the 1536 x 1024 master into six 512 x 512 source cells.
6. Apply recorded translation offsets to share a bottom-center gameplay anchor.
7. Reduce each frame directly to 192 x 192 with Lanczos filtering.
8. Assemble transparent WebP and review GIF outputs deterministically.
