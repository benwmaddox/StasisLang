# Linux high-DPI scaling acceptance

Date: 2026-09-01

## Mapping

Stasis keeps game layout, simulation, and pointer coordinates in the logical
canvas requested by `init_window`. SDL owns native window coordinates and the
complete renderer backing. Stasis derives the fitted content viewport from
those extents and prepares sprites and fonts at the smallest bounded physical
extent that can cover their maximum logical footprint.

On X11, `SDL_VIDEO_X11_SCALING_FACTOR` is the official SDL test control for a
process-wide scale. Acceptance runs set it before SDL video initialization and
use `SDL_VIDEODRIVER=x11` under Xvfb. Values 1.0, 1.25, 1.5, and 2.0 therefore
exercise the same runtime path used by a configured X11 desktop rather than a
Stasis-only detector or alternate renderer. Wayland scale remains compositor
negotiated and is not simulated by the X11 variable.

SDL's X11 model reports this as content scale while keeping window coordinates
and renderer pixels one-to-one. Stasis therefore multiplies requested windowed
physical extent by the effective SDL scale, then retains the original logical
canvas for rendering and input. It reapplies that policy on SDL's display-scale
event. Maximized and fullscreen windows already own the physical display
backing and are not multiplied beyond it.

## Rationale

`SDL_GetRenderOutputSize` names the full renderer backing. The fitted output
returned after logical presentation is a different extent and can lag during
a canvas transition. Full backing drives framebuffer accounting and the exact
integer-rational ceiling used for sprite/font preparation; fitted extent drives
ordinary screenshot ownership. This separation preserves a 720 x 360 logical
game and pointer mapping while preventing stale, oversized, or float-rounded
resource tiers.

The nearest tempting alternative is multiplying logical coordinates by the X11
scale in game or host input code. That duplicates SDL ownership, changes
gameplay geometry, and makes fractional pointer behavior nondeterministic.

## Deterministic evidence

The display-scale C contract covers 1x, 1.25x, 1.5x, 2x, downscale, odd
fractional backing, letterbox pointer round trips, exact rational ceiling, and
hard extent/atlas bounds. The desktop seam runs on Windows and Linux and checks
display and density generations, duplicate-event stability, density replacement
of one live sprite, and reclamation when returning from 2x to 1.25x.

Linux framebuffer acceptance belongs in a real X11/Xvfb job. Each capture must
record immutable toolchain/source provenance, logical/native/full-drawable and
fitted/captured dimensions, density generation, current sprite/font receipts,
and framebuffer/source/decode/texture byte totals with hard caps. Include a
2560 x 1440 display and a small-window display. A local Windows run can validate
the deterministic contracts and harness policy but is not Linux framebuffer
evidence.

## Extension

A future Wayland acceptance job should supply compositor-scale configuration
and consume the same receipt schema. It should not add a Wayland-specific
logical canvas or resource cache. A runtime resize or display migration remains
one metrics transition: display generation changes for any extent change, while
density generation changes only when the exact preparation ratio changes.

## Theory gained

Linux desktop scale is an SDL/platform input to one shared display contract,
not a game setting. Because full backing, fitted capture, and prepared resources
are joined by density generation without sharing ownership, the same invariants
predict correct behavior for X11 scale changes, window resize, and later Wayland
compositor transitions.

Visual evidence: pending the Gambit Guard Linux X11/Xvfb acceptance workflow;
this repository slice changes runtime contracts, tests, and documentation only.
