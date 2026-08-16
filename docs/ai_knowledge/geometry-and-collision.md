# Geometry and collision

<!-- tags: 2d, geometry, collision, rectangles, equality, bounds, sprites -->

## Coordinate contract

Declare the coordinate system once: origin, positive axes, units, and whether
rectangles use a top-left plus width/height or two edges. Convert all objects to
that representation before collision tests.

For an axis-aligned rectangle with left `x`, top `y`, width `w`, height `h`:

```text
left   = x
right  = x + w
top    = y
bottom = y + h
```

This is **pseudocode**, not a Stasis excerpt. Source:
`docs/renderer_resource_lifecycle.md`, `samples/brickout_revenge/`,
`samples/typed_sprite/`.

## Overlap and contact

Choose equality deliberately. For solid rectangles where touching counts as a
collision, use inclusive contact; for strict area overlap, use strict
inequalities. Do not mix the two policies across walls, paddle, ball, and
bricks.

The standard-library AABB helper uses inclusive contact. Some game-specific
checks in the repository use strict overlap, so call-site policy still matters.

**Pseudocode, inclusive contact:**

```text
overlap = left_a <= right_b and right_a >= left_b and
          top_a  <= bottom_b and bottom_a >= top_b
```

**Pseudocode, strict area overlap:**

```text
overlap = left_a < right_b and right_a > left_b and
          top_a  < bottom_b and bottom_a > top_b
```

## Recommended resolution strategy

1. Detect candidate pairs using current geometry.
2. Determine the collision normal/side from the smallest penetration or known
   gameplay rule.
3. If the gameplay rule requires separation, move the object out of penetration.
4. Reflect, constrain, or otherwise update velocity on the resolved axis.
5. Apply score/damage/removal exactly once.

This is a strategy, not a required Stasis sequence. Repository games vary: a
paddle collision may reposition before reflecting velocity, while a brick hit
may only flip velocity and update lifecycle state. If an entity can be removed,
defer or safely apply removal so the iteration does not skip the next entity.
Keep collision resolution in update; rendering only shows the result.

Source: `samples/brickout_revenge/`, `samples/headless_scenario/`,
`samples/bounded_performance/`, `src/stdlib/collision.stasis`.

## Edge cases

| Case | Required decision |
| --- | --- |
| Touch at one edge | Collision or no collision? |
| Zero width/height | Invalid data or point geometry? |
| Multiple contacts in one tick | Stable ordering or accumulated normals? |
| Fast object crosses a thin target | Swept test, smaller tick, or accepted limit |
| Corner contact | Which axis resolves first? |
| Object at world bound | Clamp position and prevent outward velocity |

## Test matrix

Test separated, touching, slight overlap, full containment, corner contact,
negative velocity, and world-bound contact. Test both collision outcome and
post-resolution position/velocity. State inspection should expose enough values
to distinguish a geometry bug from a render-coordinate bug.
