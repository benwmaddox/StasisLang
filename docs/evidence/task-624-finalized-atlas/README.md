# Task 624 finalized atlas evidence

The native SDL lifecycle fixture rendered the same cropped sprite before and
after sealing its page from 512x512 to 512x256. Both PNGs are 200x120, 96,193
bytes, and have SHA-256
`9605624f0007114c0e119e6102c07c7824b0ee95759622ef0b29906d95bc03f5`:

- [before trim](atlas-seal-before.png)
- [after trim](atlas-seal-after.png)

The byte identity proves that the shorter texture, recalculated normalized V
coordinates, source crop, linear filtering gutter, tint, alpha, and presentation
produce the same visible frame for this deterministic fixture. The image shows
the cropped pale sprite at the upper-left on an otherwise black 200x120 frame.

The same executable also asserted these query-reported RGBA8 transitions:

- 512x512 (1,048,576 bytes) to 512x256 (524,288 bytes).
- 2048x2048 (16,777,216 bytes) to 2048x128 (1,048,576 bytes).

It covered staged-allocation and first-upload rollback, stale tokens, staged-plan
conflicts, stable handles and x/y coordinates, same-allocation reload, larger
reload migration, release behavior, a full-height power-of-two no-op, a
non-power-of-two no-resize seal, and renderer reset.

Reproduction from the repository root:

```powershell
cmake --build build-atlas-624-release --config Release --target stasis_sprite_atlas_lifecycle_test
ctest --test-dir build-atlas-624-release -C Release -R '^stasis_sprite_atlas_(lifecycle|staging)_contract$' --output-on-failure --no-tests=error
```

`build-atlas-624-release` was configured fresh for this task with native graphics
tests enabled. The PNGs are renderer output, not the source-reconstructed atlas
preview. No GPU timing or driver-private allocation measurement is claimed.
