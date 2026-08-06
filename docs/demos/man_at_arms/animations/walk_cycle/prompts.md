# Walk-cycle generation record

Both operations used the built-in ImageGen path.

## Base generation

```text
Create exactly six sequential keyframes of the same man-at-arms completing one
seamless purposeful walk cycle. Arrange them in a 3-column by 2-row grid, read
left-to-right across each row.

Phases: left contact, left down, right leg passing, right contact, right down,
left leg passing. Use a fixed elevated front-right three-quarter tactical-map
camera, identical scale and body volume, one shared foot baseline, and generous
safe padding. Keep the spear upright in the same rear hand with subtle
counter-swing. Keep the shield forward in the same arm with its tower upright,
unchanged, and never mirrored.

Preserve the canonical face, moustache, compact proportions, coif, helmet,
teal brigandine, cream sleeves, oxblood leather, shield, spear, sword, palette,
colored outlines, and two-tone cel shading. Head, torso, equipment, and costume
must not morph between frames. Frame 6 must return naturally to frame 1.

Use a perfectly uniform #ff00ff chroma background with no shadows, borders,
grid, labels, effects, dust, or motion blur. Exactly six frames and one
character per cell.
```

The actual API prompt also enumerated each gait phase, every identity invariant, the small-size rendering contract, and all hard-failure conditions.

## Corrective edit

```text
Preserve the entire top row and all character, equipment, style, camera, scale,
and background properties. Correct only the bottom-row lower-body gait so it
uses the opposite leading leg:

4. right heel forward, left toe behind;
5. right foot planted under the weight, left heel lifting, body slightly lower;
6. left leg passing the planted right leg, body rising toward frame 1.

Frames 4-6 must visibly use the opposite leading leg from frames 1-3. Preserve
two anatomically plausible legs and boots. Only legs, boot angles, and subtle
vertical body bounce may change. Do not change arms, equipment, face, costume,
palette, camera, or the uniform chroma background.
```
