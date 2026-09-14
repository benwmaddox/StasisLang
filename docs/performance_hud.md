# Stasis performance HUD contract

The development performance HUD uses the same phase order on Web, desktop,
Android, and iOS:

1. **tick** — guest simulation/update code.
2. **guest render** — guest code building the render command buffer.
3. **host replay** — host validation, decoding, and interpretation of that
   command buffer. A backend may include inseparable host-side draw work here.
4. **render prep** — CPU batching, vertex/instance assembly, and buffer upload
   when the backend can measure that boundary independently.
5. **GPU submit** — CPU time issuing graphics API calls when measurable.
6. **GPU execution** — nonblocking hardware timestamp timing. This is
   unavailable
   until a backend can read delayed queries without a per-frame fence.
7. **frame work** — active CPU work: tick plus the available render phases.
8. **present wait** — swap, vsync, compositor, or presentation duration;
   it is never included in frame work or the 60 FPS budget verdict.

Unavailable or unreliable measurements retain an explicit unavailable
sentinel in the snapshot contract and are omitted from the rendered HUD,
never substituted with zero. SDL commonly omits render prep and GPU submit;
WebGL2 reports instance, batch, draw-submission, texture-bind, atlas-transition,
and uploaded-byte counts for its single visible path. Its composite count is
always zero. Native sprite counts are sprites, not GPU
instances, unless a backend actually performs instanced drawing.

The budget verdict compares current active frame work with 16.67 ms. The
displayed worst value is bounded to the most recent five seconds, so it can
recover after a transient spike. Workload details (commands, lines,
rectangles, sprites, text, instances, batches, draws, and uploaded bytes when
available) explain why a phase is expensive.

The Web development HUD is hidden by default. Press **F3** to toggle it, or add
`?stasis-hud=1` to the package URL to start it visible for automation and
mobile-browser testing. The toggle changes only the diagnostic overlay:
performance timing and body-dataset telemetry continue to update while it is
hidden. Release packages do not include the HUD. Desktop uses **F3**, Android
keeps its three-finger toggle, and iOS uses the same three-finger gesture in the
shared SDL event path.
