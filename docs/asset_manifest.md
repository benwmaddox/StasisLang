# Stasis Asset Manifest v1

`assets/manifest.json` is the shared project contract for sprite and audio identity. Desktop, JIT, AOT, Android Workshop, and published runtimes must consume this contract rather than inventing platform-specific asset IDs or accepting arbitrary filesystem paths.

## Shape

```json
{
  "schema": "stasis-assets",
  "version": 1,
  "assets": [
    {
      "id": "player",
      "path": "assets/images/player.png",
      "content_sha256": "<64 lowercase hexadecimal characters>",
      "format": {
        "kind": "sprite",
        "encoding": "png",
        "width": 64,
        "height": 64
      },
      "dependencies": []
    },
    {
      "id": "jump",
      "path": "assets/audio/jump.ogg",
      "content_sha256": "<64 lowercase hexadecimal characters>",
      "format": {
        "kind": "audio",
        "encoding": "ogg",
        "sample_rate": 48000,
        "channels": 2,
        "duration_frames": 24000
      },
      "dependencies": []
    }
  ]
}
```

## Identity and validation

- IDs are 1-128 ASCII letters, digits, `.`, `_`, or `-` and are unique within a project.
- Runtime handles are the nonzero FNV-1a 32-bit hash of `<kind>:<id>`. Load fails if two entries collide; platforms must not repair or renumber collisions independently.
- Paths use forward slashes, start with `assets/`, contain only normal path components, and must resolve to a regular file under the canonical project root.
- SHA-256 is checked against the complete bounded file before the asset is accepted.
- Declared encodings must match file extensions. Sprite dimensions, audio channel/sample-rate metadata, manifest size, entry count, and individual file size are bounded by the shared resolver.
- Dependencies must name manifest entries, cannot repeat or reference themselves, and must be acyclic.
- Unknown fields, schemas, and versions fail with stable diagnostic codes. New formats require a manifest version change or an explicitly backward-compatible enum addition.

The manifest establishes identity, confinement, and integrity. Decode, upload, playback, packaging, and hot-reload behavior are subsequent runtime slices.
