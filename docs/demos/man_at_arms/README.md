# Hearthguard man-at-arms visual system

This package turns one original character concept into a reusable visual identity for a warm medieval tactics game. The design aims for premium 2D-animation clarity and personality without copying an existing game's characters, interface, or iconography.

## Review order

1. [Canonical model sheet](man_at_arms_model_sheet.png) - identity, proportions, costume, palette, and expressions.
2. [Action sheet](man_at_arms_action_sheet.png) - the same identity across six gameplay states.
3. [Asset family](man_at_arms_asset_family.png) - portrait, map unit, medallion, emblem, and battle presentation.
4. [Layered vector rig](man_at_arms_rig.svg) - deterministic named components for production.
5. [Tower emblem](tower_emblem.svg) - reusable faction/unit symbol.
6. [Style tokens](style_tokens.json) - palette, strokes, proportions, sizes, and rendering rules.
7. [Art-direction manifest](art_direction_manifest.json) - immutable reference roles, hashes, derivation chain, and locked traits.

Rendered checks from the vector master are included at [review size](man_at_arms_rig_review.png), [96 px](man_at_arms_rig_96.png), and [64 px](man_at_arms_rig_64.png). The emblem also has a [192 px review render](tower_emblem_192.png).

## Character identity lock

The man-at-arms is defined by five primary reads:

- broad, kind face with a dark swept moustache;
- low asymmetric steel kettle helmet with one brass rivet;
- deep-teal brigandine over cream diamond quilting;
- round walnut shield with a cream tower mark;
- compact 3.65-head body, large hands, short boots, and upright spear.

If three or more reads change, treat the result as a different character. Pose, expression, viewing angle, and equipment angle may change freely.

## Shape language

Use broad arcs and softened trapezoids for the body. Steel uses slightly sharper facets, wood uses bowed verticals, and cloth uses uneven quilt diamonds. Avoid perfect bilateral symmetry: the helmet highlight, rivet, belt hang, shield grain, and stance should keep a handmade offset.

At 64 px, retain only the helmet brim, face/moustache block, teal torso, cream sleeves, shield tower, boots, and spear. Rivets and quilting may collapse into controlled color clusters.

## Rendering contract

- Use the exact palette in `style_tokens.json` unless creating an intentional faction recolor.
- Use colored outlines; never pure black.
- Use two value steps per matte material and at most three for steel.
- Keep the face warmer and lighter than adjacent russet and teal shapes.
- Apply subtle grain once during raster export. Do not generate unique noisy paths.
- Inspect silhouettes at 48, 64, and 96 px before approving a pose.
- Keep vector masters; export PNG runtime assets at 4x and downsample through the Stasis asset pipeline.

## Pose production

Start from `man_at_arms_rig.svg`. Preserve the named groups and transform the large masses first: torso, head, limbs, shield, and spear. Correct the silhouette before adjusting facial features or secondary details. For a new AI pose reference, attach the canonical model sheet and repeat every identity invariant in the prompt.

Recommended core states are idle, march, attack anticipation, spear thrust, shield brace, damage, victory, and low health. Use the action sheet as direction, not geometry to auto-trace.

## Faction variants

Create variants by replacing only `teal`, `teal_shadow`, the shield field, and the small brass accent. Face, equipment construction, cream quilting, oxblood leather, tower proportions, and outline colors remain fixed unless the variant is a genuinely different unit class.

## Current boundary

The raster sheets are the high-quality art anchors, not sliced runtime sprites. The SVG rig is deliberately deterministic and editable, but it is an initial production rig rather than the final painted finish or a frame-complete animation set. A final artist pass should refine hand anatomy, facial curves, material texture, and pose-specific overlap while preserving the contract above. Do not lower the raster references to match the vector rig; raise future rig revisions toward the approved sheets.

The checked-in PNG renders were produced from the SVG masters with Inkscape 1.4.4. Re-render them after changing vector geometry and compare all three character sizes before accepting the edit.
