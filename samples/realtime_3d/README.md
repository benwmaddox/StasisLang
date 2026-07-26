# Real-time 3D sample

This sample queues Blender-exported chess and creep meshes into the experimental
desktop OpenGL pass, then draws the normal Stasis line/text HUD over the result.
It uses a 720x960 logical viewport to exercise the portrait phone composition.

Run from the repository root after building the OpenGL runtime:

```powershell
cargo run -p stasis -- check --workspace samples/realtime_3d
cargo run -p stasis -- play samples/realtime_3d/main.stasis --ticks 300
```

Capture a deterministic review frame with:

```powershell
cargo run -p stasis -- play samples/realtime_3d/main.stasis --ticks 300 --screenshot target/realtime-3d/frame.png --screenshot-frame 300 --exit-after-screenshot
```

The OBJ loader is intentionally a narrow proof. Production assets should use GLB
with PBR materials, textures, hierarchy, skins, and animation.

The current desktop AOT builder does not automatically stage these loose OBJ/font
files. A release-run test must copy `assets/` beside the generated runner first;
production 3D assets should instead participate in Stasis's shared manifest and
packaging contract.

Prototype provenance: the chess/creep meshes were exported from ChessTD's
`chesstd-model-library.blend`; the bundled Nunito Bold instance is Copyright 2014
The Nunito Project Authors and is distributed under the SIL Open Font License 1.1
in `assets/OFL-Nunito.txt`. Replace the project-specific model exports with a
neutral licensed fixture before proposing this sample upstream.

Theory gained: batching the checkerboard into one light and one dark mesh removes
62 submissions without changing its authored appearance, predicting that instance
batches should be the default command shape for repeated tactical-board objects.
