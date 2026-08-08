# Stasis Asset Manifest v2

`assets/manifest.json` is the shared project contract for asset identity, integrity, and optional build-time raster preparation. Desktop, JIT, AOT, Android Workshop, and published runtimes consume this contract rather than inventing platform-specific asset IDs or accepting arbitrary filesystem paths.

Version 1 manifests remain valid and package their files unchanged. Version 2 adds optional display and sprite preparation metadata.

## Prepared sprite example

```json
{
  "schema": "stasis-assets",
  "version": 2,
  "display": {
    "logical_width": 360,
    "logical_height": 720,
    "max_physical_width": 1440,
    "max_physical_height": 3200,
    "scale_mode": "fit"
  },
  "assets": [
    {
      "id": "white_queen",
      "path": "assets/images/white_queen.png",
      "content_sha256": "<master file SHA-256>",
      "format": {
        "kind": "sprite",
        "encoding": "png",
        "width": 2048,
        "height": 2048
      },
      "prepare": {
        "max_logical_width": 42,
        "max_logical_height": 42,
        "max_render_scale": 1.15
      },
      "dependencies": []
    }
  ]
}
```

For `fit`, Stasis computes one display scale:

```text
min(max_physical_width / logical_width,
    max_physical_height / logical_height)
```

Each prepared axis is the ceiling of `maximum logical axis * display scale * max_render_scale`. The source aspect ratio is retained, the result never exceeds either bound, and Stasis never enlarges a master. `max_render_scale` defaults to `1.0` and accepts `1.0..=8.0`; use it for the greatest zoom, bounce, or other enlargement that can actually be painted.

Only PNG resizing is implemented initially. Other sprite encodings and assets without `prepare` are copied unchanged. SVG remains resolution-independent. PNG masters are resized with a Lanczos3 filter in linear-light, premultiplied-alpha space so gradients retain their brightness and transparent edge colors do not bleed into visible pixels. Opaque prepared PNGs are encoded as RGB; images with any transparency retain alpha.

Fonts use `{"kind":"font","encoding":"ttf"}` or
`{"kind":"font","encoding":"otf"}`. They are hash-validated and copied unchanged; sprite
preparation metadata is not valid for fonts.

Audio uses `{"kind":"audio","encoding":"wav","sample_rate":24000,"channels":1,"duration_frames":24000}`.
The first runtime playback slice accepts bounded little-endian PCM16 WAV files. Other manifest audio
encodings remain valid for storage and packaging but are rejected by `audio_load_music` and
`audio_load_effect` until a matching shared decoder lands; the runtime never guesses an encoding
from content that contradicts its extension.

## Generated package contract

Preparation writes only beneath the build output; project masters and the source manifest are never changed. The packaged manifest records the prepared dimensions and content hash. A resized entry also records `prepared_from_sha256`, which is the master hash. Preparation cache identity includes the master hash, algorithm version, and output dimensions, so unchanged assets can be reused deterministically.

`stasis play` prepares the same bundle beneath `.stasis_cache/play-assets` before guest startup and mirrors the source directory's position relative to the project root. Existing source-relative paths such as `../assets/images/hero.png` therefore resolve to prepared output without source rewriting. Resized cache hits do not decode the master again. Development builds stage the complete declared manifest so iterative and optional paths remain available.

Release builds and mobile packages scan the entry module's reachable import graph and stage only declared assets named by string literals that resolve beneath the project `assets/` directory, plus their transitive manifest dependencies. Paths such as `../assets/images/hero.png` resolve relative to the selected entry file's directory, including when the literal appears in an imported module; project-root paths such as `assets/images/hero.png` are also accepted. These are the same resolution rules used at runtime. The generated manifest is rewritten to the same exact subset. Runtime asset paths therefore need to be static string literals in reachable production source; dynamically constructed paths are not a supported release-packaging contract. Test-only and unreachable modules do not enlarge a release package.

Once a project has an asset manifest, every runtime-loaded asset must be declared. Source-only provenance files may remain elsewhere in the project, but undeclared files are not copied into prepared play or build output.

The package contains only the selected display envelope's output, not multiple resolution variants. A future target-profile extension can select different display envelopes without changing the per-sprite sizing contract.

## Identity and validation

- IDs are 1-128 ASCII letters, digits, `.`, `_`, or `-` and are unique within a project.
- Runtime handles are the nonzero FNV-1a 32-bit hash of `<kind>:<id>`. Load fails if two entries collide; platforms must not repair or renumber collisions independently.
- Paths use forward slashes, start with `assets/`, contain only normal path components, and must resolve to a regular file under the canonical project root.
- SHA-256 is checked against the complete bounded file before the asset is accepted.
- Declared encodings must match file extensions. Sprite dimensions, audio metadata, font encoding, display dimensions, manifest size, entry count, and individual file size are bounded by the shared resolver.
- Sprite preparation requires a version 2 manifest and top-level `display` metadata.
- Dependencies must name manifest entries, cannot repeat or reference themselves, and must be acyclic.
- Unknown fields, schemas, and future versions fail with stable diagnostic codes.

## Mobile packaging

- The AOT bundle command resolves and verifies the source manifest, then invokes the shared `stasis_assets` preparation path and packages the generated manifest and files under `assets/stasis_game/`.
- Android and iOS apply the same reachable-source asset closure as desktop release builds; platform packaging does not carry unused manifest entries or alternate prepared sizes.
- Declared fonts use the shared manifest path. As a compatibility supplement for projects that
  have not declared their path-based `load_font` assets yet, mobile packaging also scans compiled
  `.stasis` source string literals for `.ttf` and `.otf` paths and copies only referenced regular
  font files beneath the canonical project `assets/` directory.
- Android uses one GL texture per resolved sprite and batches only consecutive commands that share texture and clip state. Atlas layout remains backend-private, so atlas coordinates never enter the manifest or render-command ABI.
- Missing, corrupt, oversized, hash-mismatched, or unsupported packaged sprites render the deterministic magenta checker fallback.
