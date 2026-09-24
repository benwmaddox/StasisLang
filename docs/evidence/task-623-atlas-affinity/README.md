# Atlas affinity evidence

All named-game recording attempts used the fingerprint-matched Stasis CLI/runtime with a hidden SDL software renderer at 2000×900 and 60 fps. No GPU timing is available on this host. Full per-game outcomes and exact page inventory are in [representative-captures.json](representative-captures.json). Screenshots are losslessly re-encoded from their recorded frames; pixel hashes are included in the JSON.

## SheepHerder runtime capture

A delayed click (frame 24 down, frame 25 release at 500,430) started gameplay; the 129-frame recording produced 21 resident sprites, six exact pages, and five bounded runtime pair rows with total weight 6,336. The page extents were 2010×908, 2048×2048, 1578×1576, 512×512, 512×512, and 2010×123 (37,111,320 allocation bytes). Every page has `group_id=0` and is excluded by native plan eligibility, so the production planner correctly returns `empty_input`; baseline and candidate optimizer metrics are null. The two observed 512×512 pages are cold/protected pages, not eligible default affinity pages. The native query does not report page-run/submission counts, so those remain null.

The v4 AOT summary is incomplete with exact cause `unbounded or dynamic for loop`; the preview uses runtime histogram provenance, but none of its five rows maps to the planner-eligible resident subset. This is an observed safe fallback, not an app speedup. See the [runtime report](sheep-herder-runtime/SheepHerder-report.json), [native query](sheep-herder-runtime/atlas-affinity-native-query-v1.json), [v4 manifest](sheep-herder-runtime/engine-bundle-manifest-v4.json), and [gameplay frame](sheep-herder-runtime/gameplay-frame-000129.png).

## RootbeerMaze3 capture

The 129-frame 2000×900 record command completed, but this portrait-only game selected its unsupported-layout screen. It emitted no valid native pair snapshot; page inventory, cut metrics, and optimizer metrics are null. The [recorded frame](rootbeer-maze3-runtime/unsupported-layout-frame-000129.png) documents the fallback state.

## HamsterHavenTycoon capture

The 2000×900 record command stopped before frame 1 because guest `main()` returned 1 after `gameplay_config_valid()` failed. No native snapshot or frame was produced, so page inventory and all planner metrics are null. No app or game source was changed for this attempt.

## Native fixture

The separate 200×120 [native ABI fixture evidence](native-fixture/README.md) verifies real page/resident query and byte-identical native before/after output. The Rust planner preview of this fixture is useful optimizer-path evidence, but it is not a named packaged game's performance measurement.

## Capture commands

The capture runs used the CLI and matching native DLL/runner from the same staged directory. The workspaces were disposable copies under `build`; the original game projects were not modified. The `record_invocation` objects in [representative-captures.json](representative-captures.json) contain the exact executable, runtime paths, snapshot destination, and argument arrays. For example, the SheepHerder setup was:

```powershell
$env:STASIS_RUNTIME_DLL_PATH = 'D:\code\StasisLang\build\task623-sheepherder-run-bin\stasis_graphics.dll'
$env:STASIS_RUNTIME_RUNNER_PATH = 'D:\code\StasisLang\build\task623-sheepherder-run-bin\stasis_runner.exe'
$env:STASIS_ATLAS_SNAPSHOT_OUT = 'D:\code\StasisLang\build\task623-sheepherder-gameplay129-native.json'
& 'D:\code\StasisLang\build\task623-sheepherder-run-bin\stasis.exe' record --workspace 'D:\code\StasisLang\build\task623-sheepherder-runtime-copy' --input-script 'D:\code\StasisLang\build\task623-sheepherder-runtime-copy\task623-start-play-frame24-input.json' --output 'D:\code\StasisLang\build\task623-sheepherder-gameplay129-frames' --width 2000 --height 900 --fps 60 --frames 129 --json
```

RootbeerMaze3 and HamsterHavenTycoon used the same executable/runtime pair with their respective disposable workspace and output paths. The full attempt arguments and outcomes are in the JSON summary.
