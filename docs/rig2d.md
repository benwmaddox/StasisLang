# Bounded rigid 2D rigging

`src/stdlib/rig2d.stasis` provides a small renderer-independent hierarchy for
rigid cutout characters and articulated props. A rig owns at most 24 bones,
stores one rest pose and one mutable local pose, and resolves those locals into
world positions and clockwise screen-degree angles in one bounded forward pass.

The module does not load assets, choose draw order, play clips, advance time, or
submit graphics commands. Those choices remain in the game. This makes the same
rig usable with raster sprite sheets, cached SVG sprites, debug lines, tests, or
no renderer at all.

## Import and storage

Use the path that matches the project's standard-library mode:

```stasis
// Checked-in vendor snapshot (the default for generated projects).
import "/vendor/stasis/stdlib/rig2d.stasis";

// Toolchain-following project.
import "/.stasis_cache/toolchain/src/stdlib/rig2d.stasis";
```

Inside the StasisLang repository, samples and tests use a relative import such
as `../../src/stdlib/rig2d.stasis`.

Each `Rig2D` directly owns `RigBone2D[RIG2D_BONE_CAPACITY]`; there is no shared
module arena and no slot allocator. Put long-lived rigs in explicit global
state, just like other persistent Stasis data:

```stasis
global courier_rig: Rig2D;
```

The fixed capacity is 24 bones per rig. A rig occupies its full bounded layout
even when it has fewer bones. Adding the `Rig2D` field or changing its position
changes application state layout and can require restarting a live session;
ordinary pose edits do not change layout.

## Construct a parent-first hierarchy

Call `clear()` before rebuilding an existing rig, then add every parent before
its children. `-1` denotes a root. Multiple roots are supported. `add_bone`
returns the new zero-based index, or `-1` without changing `count` when the
parent is invalid or capacity is exhausted.

```stasis
function setup_courier_rig(): bool {
    courier_rig.clear();
    let board: i32 = courier_rig.add_bone(-1, 0.0, 0.0, 0.0);
    let body: i32 = courier_rig.add_bone(board, 0.0, -40.0, 0.0);
    let arm: i32 = courier_rig.add_bone(body, 18.0, -24.0, 12.0);
    return board == 0 && body == 1 && arm == 2;
}
```

The position and angle passed to `add_bone` become both the rest transform and
the initial local transform. Local translations are measured in the parent's
coordinate system. All angles are clockwise screen degrees and normalize to
the half-open range `[-180, 180)`.

The caller must supply finite coordinates and angles within plus or minus
360000 degrees. Version 1 does not add per-write finite-number checks; violating
that precondition can poison later world transforms.

## Pose during deterministic ticks

`set_local` replaces one bone's current parent-relative transform.
`blend_local` moves the current transform toward a target: position is linear,
angle takes the shortest arc, weight is clamped to `[0, 1]`, and an exact
180-degree tie takes the negative arc. Both return `false` for an invalid index.

```stasis
function tick_courier(bank: f32): bool {
    // Exact root-relative placement.
    if (!courier_rig.set_local(1, 0.0, -40.0, bank * 0.25)) {
        return false;
    }

    // Incremental smoothing toward a target local pose.
    if (!courier_rig.blend_local(2, 18.0, -24.0, 12.0 + bank, 0.35)) {
        return false;
    }

    return courier_rig.solve(220.0, 640.0, bank);
}
```

`blend_local` is incremental smoothing, not a stateless mix of two complete
poses. Repeating it produces exponential convergence, so the result depends on
how many times it is called. Call it from the deterministic tick path when pose
state must replay consistently; do not call it from a render function whose
cadence can vary.

`reset_pose()` copies every rest transform back to its local transform. It does
not recalculate world values. Call `solve()` afterward before rendering.

## Solve, then render attachments

`solve(root_x, root_y, root_angle)` validates the complete parent topology
before writing any world values. Invalid topology or an invalid count returns
`false` and preserves the last solved world pose. An empty rig solves
successfully. For each valid bone in parent-first order, solving applies:

```text
world_position = parent_position + rotate(local_position, parent_angle)
world_angle    = wrap(parent_angle + local_angle)
```

For root bones, the supplied root position and root angle act as the parent
transform. The rig has no scale or shear. Mirroring is therefore an attachment
rendering choice, or must be authored explicitly in local positions.

After a successful solve, read `world_x`, `world_y`, and `world_angle` and map
them into the graphics API. Attachment source rectangles, sizes, pivots, alpha,
layering, and sprite ownership are intentionally caller data:

```stasis
function draw_arm(arm: i32): void {
    let x: f32 = courier_rig.world_x(arm);
    let y: f32 = courier_rig.world_y(arm);
    let angle: f32 = courier_rig.world_angle(arm);

    // A real consumer passes x/y/angle to SpriteRunWriter together with its
    // own sprite handle, source rectangle, dimensions, and pivot.
}
```

Invalid world indexes return `0.0`; invalid parent indexes return `-1`.
Because a root also has parent `-1`, callers should retain successful bone
indexes from construction and check `solve()` instead of using sentinels as
the only validity signal.

## Lifecycle and ownership rules

The intended frame lifecycle is:

1. Initialize or rebuild topology with `clear()` and `add_bone()`.
2. During each deterministic tick, update local transforms with `set_local`,
   `blend_local`, or `reset_pose`.
3. Call `solve()` once after the last local mutation.
4. During rendering, read solved world transforms without mutating the rig.

Rig transforms are normally presentation state. Gameplay collision, movement,
pickup results, and networking should continue to use their authoritative game
state unless a project deliberately specifies and tests a different contract.

Although `bones` is structurally visible in the current language, application
code should treat it as module-owned storage. Direct parent mutation can violate
the parent-first invariant; direct local mutation bypasses index validation and
angle normalization.

## Deliberate version 1 limits

- 24 rigid bones per `Rig2D`, with bounded `O(count)` solve work.
- Translation and rotation only; no scale, shear, weighted mesh, or skinning.
- No inverse kinematics, constraints, animation clips, queues, or layers.
- No JSON/atlas importer, editor format, attachment type, or draw-order model.
- Polynomial sine/cosine helpers keep the module host-independent. The module
  is intended for visual transforms, not authoritative cross-architecture
  fixed-point simulation.

These limits preserve the boundary proven by Afterlight: the generic hierarchy
did not change when that game extended its consumer from a simpler courier to a
14-bone head, scarf, arm, and two-link leg rig. Future clip playback, IK, or
attachment helpers can build on the solved-transform API without coupling the
core to a renderer or asset format.

## Migrating the Afterlight proof

The upstream API intentionally removes the prototype's global 96-bone arena and
manual four-slot binding. An Afterlight-style consumer migrates as follows:

| Prototype | Upstream module |
| --- | --- |
| `rig.bind(slot)` | `rig.clear()` before rebuilding topology |
| `rig2d_bones[slot * 24 + index]` | validated methods, or `rig.bones[index]` for diagnostics |
| four globally coordinated rig slots | any explicit `Rig2D` values allowed by application state budget |
| copied handles alias one slot | every `Rig2D` owns its fixed bone array |

Bone indexes, `add_bone`, `set_local`, `blend_local`, `reset_pose`, `solve`,
`parent`, and world accessors otherwise retain the proof's behavior. Courier
attachment rendering and pose tuning do not move upstream.

## Validation

`tests/stasis/rig2d.test.stasis` covers capacity and parent validation, rotated
chains, independently owned rigs, shortest-arc blending, endpoint clamping,
rest-pose reset, repeated solve stability, atomic failure, trigonometric wrapping,
empty and multiple-root rigs, sentinels, and exact angle boundaries. The backend
seam executes a representative owned-array hierarchy through JIT and linked AOT
paths.
