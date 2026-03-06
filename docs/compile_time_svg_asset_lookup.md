# Compile-Time SVG Asset Lookup Research

## Goal

Support compile-time discovery and metadata baking for SVG assets so Stasis code can use:

- deterministic asset identity (name -> stable lookup key)
- natural SVG dimensions (width, height)
- optional compile-time defaults for load size
- generated fields/constants per asset

Target user outcome: avoid hand-writing repeated `gfx_load_sprite("assets/foo.svg", w, h)` calls and manually duplicating dimensions.

## Current Baseline (Repo Findings)

- Asset loading is runtime-only today through `gfx_load_sprite(path: string, max_w: i32, max_h: i32)`.
- Runtime rasterization parses SVG width/height using NanoSVG and bakes to requested max dimensions.
- AOT manifest already carries compile-time metadata for:
  - `string_literals`
  - `collection_max_lengths`
- AOT load path already seeds runtime tables from manifest metadata.
- String host bridges currently rely on literal IDs; dynamic string lookup for host calls is intentionally limited.

This means the project already has a "compile metadata -> manifest -> runtime seed" pattern that can be extended for SVG catalogs.

## Requirements

1. Deterministic output across machines and runs.
2. Name-based lookup support for handles.
3. Natural SVG dimensions available without runtime reparsing.
4. Fits both JIT dev and AOT prod flows.
5. Clear diagnostics for malformed/missing assets.

## Design Options

## Option A: Manifest-Only Asset Catalog

Compile step discovers SVG files and writes metadata into engine bundle manifest:

- normalized asset path
- asset name (derived stem or explicit alias)
- name hash
- natural width/height
- optional default max width/height

Pros:

- Lowest implementation risk.
- Reuses existing manifest pipeline and validation patterns.

Cons:

- Stasis code still needs host/runtime helpers to use the metadata.
- No "fields/constants per SVG" directly in language yet.

## Option B: Manifest Catalog + Runtime Registry (Recommended)

Extend Option A with runtime seeding APIs so handles can be resolved by stable keys.

Flow:

1. Compiler extracts SVG catalog at compile time.
2. Manifest includes `svg_assets` section.
3. Backend reads manifest and seeds dynload/runtime asset table before game code runs.
4. New externs resolve by ID/hash (not dynamic string requirement):
   - `gfx_asset_handle_by_name_hash(name_hash: i32) -> i32`
   - `gfx_asset_natural_width_by_name_hash(name_hash: i32) -> i32`
   - `gfx_asset_natural_height_by_name_hash(name_hash: i32) -> i32`

Pros:

- Deterministic and fast lookups.
- Avoids current string-bridge limitation by using `i32` hash APIs.
- Works for both JIT and AOT.

Cons:

- Requires small runtime API additions.
- Needs careful collision policy for name hashes.

## Option C: Generated Stasis Fields/Constants File

At compile time, generate a Stasis source file (or synthetic compile unit) with constants per asset:

- `const ASSET_BALL_NAME_HASH: i32 = ...`
- `const ASSET_BALL_NATURAL_W: i32 = 32`
- `const ASSET_BALL_NATURAL_H: i32 = 32`

Pros:

- Direct "fields and values for each svg" in language.
- Very convenient in game code.

Cons:

- Requires codegen plumbing and import strategy.
- Alone does not solve handle lookup unless paired with Option B.

## Recommended Approach

Implement **B + C** in phases:

1. Add manifest catalog schema and extraction (B foundation).
2. Add runtime seeded registry + hash-based extern lookups (B completion).
3. Add generated Stasis constants for ergonomics (C).

This satisfies the requested compile-time baking and keeps runtime deterministic.

## Proposed Manifest Schema

```json
{
  "svg_assets": [
    {
      "asset_key": "assets/ball.svg",
      "name": "ball",
      "name_hash": 123456789,
      "natural_width": 32,
      "natural_height": 32,
      "default_max_width": 32,
      "default_max_height": 32
    }
  ]
}
```

Notes:

- `asset_key` should be normalized forward-slash relative path from watched root.
- `name` default = file stem, with deterministic diagnostics on collisions.

## Compile-Time Extraction Rules

1. Asset discovery
   - Primary: explicit assets folder policy (for example `assets/**/*.svg` under watched root/import closure root).
   - Optional: additionally scan `gfx_load_sprite("...")` calls and ensure referenced files are cataloged.
2. Parse natural size
   - Prefer `<svg width height>`.
   - Fallback to `viewBox` width/height.
   - Reject unsupported percentage-only dimensions unless fallback is available.
3. Deterministic ordering
   - Sort by normalized `asset_key`.
4. Diagnostics
   - Missing file, malformed SVG, invalid dimensions, duplicate names, hash collision.

## Runtime Registry Model

Registry row:

- `name_hash: i32`
- `asset_key: string` (or string-literal ID + lookup)
- `natural_w: i32`
- `natural_h: i32`
- `default_max_w: i32`
- `default_max_h: i32`
- `handle: i32` (0 until loaded)

Policy:

- lazy load on first handle request, or eager preload mode via config
- handle remains stable until process restart
- hot reload updates metadata/handles only on successful swap

## Generated Fields/Values (Language Surface)

Generate constants per asset:

```stasis
const ASSET_BALL_NAME_HASH: i32 = 123456789;
const ASSET_BALL_NATURAL_W: i32 = 32;
const ASSET_BALL_NATURAL_H: i32 = 32;
```

Helper wrapper shape:

```stasis
function asset_ball_handle(): i32 { return gfx_asset_handle_by_name_hash(ASSET_BALL_NAME_HASH); }
```

This gives compile-time fields/values and deterministic handle lookup without requiring dynamic strings.

## Example from Current Brickout Assets

- `ball.svg` -> natural `32x32`
- `paddle.svg` -> natural `128x24`
- `brick_basic.svg` -> natural `160x64`

These can be emitted as generated constants and used as canonical sprite dimensions.

## Testing Plan

1. Compiler extraction tests
   - valid width/height
   - viewBox fallback
   - malformed svg failure
   - duplicate asset name failure
2. Manifest tests
   - `svg_assets` ordering is deterministic
   - values serialized as expected
3. Backend/runtime tests
   - manifest read + registry seed
   - handle lookup by hash returns non-zero after load
   - natural size lookup returns catalog values
4. Integration tests
   - Brickout sample compiles and resolves cataloged assets
   - hot swap preserves deterministic behavior

## Migration Plan

1. Phase 1: manifest-only `svg_assets` generation (no runtime behavior change).
2. Phase 2: runtime registry + hash-based lookup externs.
3. Phase 3: generated constants file for ergonomic Stasis usage.
4. Phase 4: optional deprecation path for direct string path calls in performance-sensitive samples.

## Risks and Mitigations

- Name collisions across folders:
  - Mitigate with deterministic diagnostic and explicit alias support.
- Hash collisions:
  - Detect at compile time; fail with actionable diagnostics.
- SVG dimension ambiguity:
  - Use strict parser rules and explicit fallback order.
- Drift between metadata and runtime loaded asset:
  - Include file mtime/hash in catalog rows for optional validation.

## Open Questions

1. Should name lookup key be stem-only (`ball`) or scoped path (`assets/ball`)?
2. Should handles be eagerly preloaded or lazily loaded by default?
3. Should generated constants live in a checked-in file or transient compile artifact?
4. Should raster (png/webp) assets share the same catalog path in the first version?
