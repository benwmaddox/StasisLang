# Generation record

All three raster references were generated with the built-in ImageGen path. The model sheet was generated from text; the following sheets were reference-based edits carrying the approved identity forward.

## Canonical model sheet

```text
Create an original man-at-arms for a charming turn-based medieval tactics game.
Use premium hand-authored 2D animation construction with colored outlines, matte
flat fills, subtle gouache grain, restrained two-tone cel shading, softly
irregular curves, and an original warm storybook-medieval identity.

Show the same character from the front, three-quarter, profile, and back, plus
neutral, cheerful, determined, and worried head studies. Lock a broad rounded
torso, short strong legs, dark moustache, russet coif, asymmetric steel kettle
helmet with one brass rivet, teal brigandine, cream quilted sleeves, oxblood
leather, walnut tower shield, spear, and sword. Use no words or copied elements.
```

## Action sheet edit

```text
Treat the canonical model sheet as the exact identity and costume reference.
Show idle guard, brisk march, spear thrust, shield brace, victory, and tired
low-health poses. Preserve face, moustache, proportions, equipment construction,
palette, outlines, texture, and shading. Change only pose, expression, and
equipment angle. Use no labels or additional characters.
```

## Asset-family edit

```text
Treat the canonical and action sheets as identity references. Show a large
three-quarter command portrait, compact elevated map unit, circular face
medallion, tower shield emblem, and one shield-brace battle vignette on a small
grass-and-stone map tile. Preserve every character and rendering invariant.
Arrange the results as separate art specimens rather than a literal UI.
```

Future generations should attach `man_at_arms_model_sheet.png` first. Never rely on the abbreviated record above without also enumerating the current invariants from the package README.

## Exact 128 px spritesheet prompt

The spritesheet was generated through the built-in ImageGen path with the canonical model sheet as Image 1 and the approved action sheet as Image 2.

```text
Use case: identity-preserve
Asset type: final raster spritesheet for a turn-based tactics game
Input images: Image 1 is the canonical identity and costume model sheet. Image
2 is approved pose language only. Preserve Image 1 exactly and use Image 2
only to understand movement.

Draw one production-ready spritesheet containing exactly six isolated sprites
of the same man-at-arms. The finished sheet is a 3-column by 2-row grid of
equal square cells. Each cell will be reduced to exactly 128 by 128 pixels, so
compose every sprite specifically for that final size.

Cell order, left to right:
Top row: 1 idle guard, 2 purposeful walk, 3 spear thrust.
Bottom row: 4 shield brace, 5 hurt recoil, 6 cheerful victory.

Camera and scale: identical elevated three-quarter camera in every cell,
suitable for a tactical map. Identical character scale in all six cells. Feet
share the same baseline within each row. Center each silhouette with generous
safe padding. Keep all weapons inside their own cells; angle the short spear
diagonally when needed. No sprite may overlap another cell.

Identity invariants: exact same warm face, swept dark moustache, compact
3.65-head body, broad rounded torso, short strong legs, large readable hands,
russet padded coif, asymmetric steel kettle helmet with one brass rivet,
deep-teal brigandine, cream quilted sleeves, oxblood belt and boots, round
walnut shield with the same cream tower mark, short spear, sword and scabbard.
Do not redesign, recolor, mirror the shield emblem, add armor, or vary
proportions.

Small-sprite art direction: premium authored 2D television-animation finish
adapted to a 128-pixel game unit. Use beautifully controlled rounded
silhouettes, expressive posing, selective asymmetry, clean colored outlines,
matte flat color, two-tone cel shading, and warm appealing facial acting.
Simplify aggressively for small size: broad color masses, large readable eyes
and moustache, only a few oversized armor rivets, only a few large quilting
diamonds, no tiny seams, no painterly noise, no microtexture, no gradients. At
thumbnail size the helmet, moustache, teal torso, cream sleeves, tower shield,
boots, spear, and action must be instantly readable.

Backdrop for extraction: perfectly flat solid #ff00ff chroma-key across the
entire canvas. The background must be one uniform color with no grid lines,
borders, shadows, gradients, texture, floor plane, reflections, labels, or
lighting variation. Do not use #ff00ff anywhere in the character. No cast
shadows or contact shadows. Crisp antialiased edges and generous separation
between sprites.

Constraints: exactly six sprites and exactly one character per cell. No text,
numbers, captions, UI, extra objects, extra people, logos, trademarks,
watermark, gore, photorealism, anime, pixel art, glossy 3D, decorative frame,
checkerboard, or copied franchise elements.
```
