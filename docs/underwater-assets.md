# Underwater Automation Asset Pipeline

This pipeline keeps the underwater automation sample deterministic while allowing a fullscreen water/caustics pass and lightweight instanced geometry for lanes, modules, and units.

## Directory Layout
- `docs/assets/underwater/caustics.glsl`: fullscreen quad fragment shader for subtle sunlight-through-water ripples. Vertex shader can reuse the existing passthrough quad in the renderer.
- `docs/assets/underwater/palette.txt`: palette swatches (hex) for background gradient, bioluminescent highlights, and UI accents.
- `docs/assets/underwater/icons.json`: small metadata file describing module icon indices (if we add a texture atlas later).

## Build/Load Flow
1) Copy the GLSL file into the renderer’s shader pack at build time (e.g., embed as a resource in `Stasis.Cli`).
2) On renderer init, compile/link the caustics shader alongside the existing line shader and keep a uniform block: `time`, `depth_scale`, `intensity`, `surface_jitter`, `biolume_color`.
3) After drawing the scene, bind the caustics shader, set uniforms from Stasis globals (`postfx_strength`, `postfx_phase`, `postfx_speed`) via the `set_postfx(strength, phase, speed, r, g, b)` builtin, and draw a fullscreen triangle/quad.
4) Instance data for lanes/modules/units is built on the Stasis side (SoA arrays → transient instance buffer) and uploaded once per frame.

## Determinism & Testing
- Keep shaders versioned in `docs/assets/underwater/`; no runtime fetches. Runtime falls back to an embedded shader if the file is missing.
- Expose a headless toggle that bypasses GL and logs postfx uniform values for tests.
- Add a small host test that loads `caustics.glsl`, sets uniforms, and renders a 1x1 target to ensure the pass links correctly.

## Future Assets
- If we add textured icons, place the atlas PNG in `docs/assets/underwater/` and document UVs in `icons.json`. Keep the default sample line/quad-only to avoid blocking on texture upload.
