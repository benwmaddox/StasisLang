# Brickout Defense

Faithful landscape Stasis port of the recovered 2016 Brickout Defense game. The playfield remains in the original 660 x 550 coordinate space, with the original campaign, challenge, tower economy, ball behaviors, input, audio, and animation timing.

The sample uses the pinned Stasis standard library and typed `ImageAsset`, `Sprite`, `SpriteRef`, and `TextRun` APIs. Its image catalog loads one image request at a time; all six typed audio requests start during boot. Gameplay remains gated until every catalog image, all three font sizes, and all six audio assets are ready and the background music starts looping successfully. Any failed asset request or music start displays a failure state and does not enter gameplay.

Controls: tap/click menu and playfield items; keys 1–6 choose shop slots, P pauses, N spawns the next group, U upgrades, S sells, and Escape backs out or exits from the main menu.

The original raster and vector assets are retained under `assets/original/`. `assets/original/asset_lineage.json` records hashes for every recovered source file. `assets/asset_catalog.json` records each loaded image, its dimensions, source hash, and all 597 unchanged animation frame registrations. Run `python tools/build_asset_catalog.py` to verify the asset lineage and generated Stasis modules.

## Validation scope

The migration was checked with the pinned `nightly-20260929-369` CLI: `fmt --check`, `check`, and `test` pass, with 14 deterministic tests across three files. The desktop package and Android arm64 package each pass a 629-entry asset/hash audit (622 images, one font path used at three sizes, and six audio files). A Windows desktop menu capture and deterministic 60 fps interaction recording exercise the challenge, tower shop, placement, and a launched wave with moving balls.

The Android evidence covers package generation, Gradle APK build, compiled manifest/resources, asset hashes, and the existing local debug signer. It does not claim an Android device launch: no Android device was connected and no earlier landscape APK was available for an in-place update check. Physical-device runtime and update continuity remain unverified.
