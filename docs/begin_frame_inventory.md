# BeginFrame tracked-source inventory

Companion to [the implemented design](begin_frame_design.md). Audited 2026-09-10.
This is the immutable pre-migration inventory used to scope Task 562, not a list
of current call sites. Line numbers identify that source snapshot; function names
remain useful after lines move. Includes comments, fixture strings and
UI names so that similarly named operations are not silently conflated.
Third-party ThorVG animation segment documentation is excluded: its "begin frame"
is an animation range, not a Stasis rendering lifecycle call.

Reproduce from the repository root:

```text
git grep -n -i -E "begin_?frame|begin frame|gfx_cmd_begin" -- ":!runtime/third_party" ":!AGENTS.md" ":!docs/begin_frame_design.md" ":!docs/begin_frame_inventory.md"
```

Classification: canonical graphics definitions and their vendor copies reset
command construction; native C definitions prepare submission; Web import and
private batcher definitions are separate legacy-list reset and clear operations.
UI definitions reset layout/input scratch state only. Remaining application and
fixture occurrences call those APIs; documentation and source-string CI checks
are migration consumers, not runtime operations. The AOT audio test definition
is a test stub. No compiler-specific BeginFrame lowering was found.

## [README.md](../README.md)

- Line 96: `begin_frame();`

## [apps/stasis/src/toolchain_cli.rs](../apps/stasis/src/toolchain_cli.rs)

- Line 174: `begin_frame();`

## [apps/stasis/src/toolchain_cli/gauntlet.rs](../apps/stasis/src/toolchain_cli/gauntlet.rs)

- Line 799: `begin_frame();`

## [apps/stasis/tests/desktop_screenshot_capture.rs](../apps/stasis/tests/desktop_screenshot_capture.rs)

- Line 66: `"function render(): i32 { begin_frame(); clear(0.03, 0.06, 0.12, 1.0); ",`

## [crates/stasis_android_bridge/src/lib.rs](../crates/stasis_android_bridge/src/lib.rs)

- Line 4142: `begin_frame();`
- Line 4262: `begin_frame();`
- Line 6928: `begin_frame();`

## [crates/stasis_compiler/src/backend/aot.rs](../crates/stasis_compiler/src/backend/aot.rs)

- Line 3180: `function begin_frame(): void { return; }`

## [crates/stasis_compiler/tests/sealed_display_list.rs](../crates/stasis_compiler/tests/sealed_display_list.rs)

- Line 21: `begin_frame();`
- Line 27: `begin_frame();`

## [docs/gpu_instancing_report.md](../docs/gpu_instancing_report.md)

- Line 398: `begin_frame();`
- Line 471: `begin_frame();`

## [docs/knowledge/examples/src/loading_screen.stasis](../docs/knowledge/examples/src/loading_screen.stasis)

- Line 123: `begin_frame();`

## [docs/knowledge/loading-screens.md](../docs/knowledge/loading-screens.md)

- Line 21: `'begin_frame' and 'end_frame' build a command buffer; 'end_frame' marks it for`
- Line 207: `begin_frame();`

## [docs/project_architecture.md](../docs/project_architecture.md)

- Line 289: `begin_frame();`

## [docs/public_graphics_api.md](../docs/public_graphics_api.md)

- Line 5: `1. 'begin_frame()' and 'clear(...)'.`

## [mobile/android/app/src/main/assets/exploration_sample/src/host_game.stasis](../mobile/android/app/src/main/assets/exploration_sample/src/host_game.stasis)

- Line 20: `exploration_host_begin_frame();`

## [mobile/android/app/src/main/assets/exploration_sample/src/host_runtime.stasis](../mobile/android/app/src/main/assets/exploration_sample/src/host_runtime.stasis)

- Line 28: `function exploration_host_begin_frame(): void {`
- Line 29: `begin_frame();`

## [mobile/android/app/src/main/assets/workshop_sample/src/preview_adapter.stasis](../mobile/android/app/src/main/assets/workshop_sample/src/preview_adapter.stasis)

- Line 37: `begin_frame();`

## [runtime/README.md](../runtime/README.md)

- Line 136: `| 'stasis_begin_frame()' | Start a new frame |`

## [runtime/stasis_graphics.c](../runtime/stasis_graphics.c)

- Line 3413: `STASIS_EXPORT void stasis_begin_frame(void) {`
- Line 4061: `stasis_begin_frame();`

## [runtime/stasis_graphics.def](../runtime/stasis_graphics.def)

- Line 23: `stasis_begin_frame`

## [runtime/tests/stasis_mobile_runtime_test.c](../runtime/tests/stasis_mobile_runtime_test.c)

- Line 33: `static int begin_frame_calls;`
- Line 186: `void stasis_begin_frame(void) {`
- Line 187: `begin_frame_calls += 1;`
- Line 225: `stasis_begin_frame();`
- Line 424: `begin_frame_calls = 0;`
- Line 502: `assert(begin_frame_calls == 1);`
- Line 644: `assert(begin_frame_calls == 0 && end_frame_calls == 0 && gfx_submit_calls == 0);`
- Line 655: `assert(begin_frame_calls == 0 && end_frame_calls == 0 && gfx_submit_calls == 0);`

## [runtime/web/game.js](../runtime/web/game.js)

- Line 1948: `web_begin_frame: (r, g, b) => { commands.length = 0; commands.push([0, r, g, b]); },`
- Line 2541: `beginFrame: (red, green, blue, alpha = 1) => {`
- Line 2708: `getGpuBatcher()?.beginFrame((command[1] & 255) / 255,`
- Line 2761: `batcher.beginFrame(f32[GFX_F_CLEAR_BASE], f32[GFX_F_CLEAR_BASE + 1],`

## [runtime/web/tests/render_pipeline.test.mjs](../runtime/web/tests/render_pipeline.test.mjs)

- Line 1176: `runtime.env.web_begin_frame(0, 0, 0);`

## [samples/android_aot_seam/main.stasis](../samples/android_aot_seam/main.stasis)

- Line 20: `begin_frame();`

## [samples/android_lifecycle_failure_seam/render/main.stasis](../samples/android_lifecycle_failure_seam/render/main.stasis)

- Line 19: `begin_frame();`

## [samples/android_orientation_seam/main.stasis](../samples/android_orientation_seam/main.stasis)

- Line 106: `begin_frame();`

## [samples/android_packaged_assets_seam/main.stasis](../samples/android_packaged_assets_seam/main.stasis)

- Line 86: `begin_frame();`

## [samples/android_resource_restore_seam/main.stasis](../samples/android_resource_restore_seam/main.stasis)

- Line 43: `begin_frame();`

## [samples/android_resource_restore_seam/tests/android_resource_restore.test.stasis](../samples/android_resource_restore_seam/tests/android_resource_restore.test.stasis)

- Line 17: `begin_frame();`

## [samples/android_storage_seam/main.stasis](../samples/android_storage_seam/main.stasis)

- Line 48: `begin_frame();`

## [samples/android_touch_seam/main.stasis](../samples/android_touch_seam/main.stasis)

- Line 113: `begin_frame();`

## [samples/asset_breakout/src/main.stasis](../samples/asset_breakout/src/main.stasis)

- Line 275: `begin_frame();`

## [samples/asset_breakout/vendor/stasis/stdlib/graphics.stasis](../samples/asset_breakout/vendor/stasis/stdlib/graphics.stasis)

- Line 254: `// not cross begin_frame/code-swap; reserve and finalize/cancel in one frame.`
- Line 541: `function begin_frame(): void {`
- Line 542: `gfx_cmd_begin();`

## [samples/asset_breakout/vendor/stasis/stdlib/internal/gfx_cmd.stasis](../samples/asset_breakout/vendor/stasis/stdlib/internal/gfx_cmd.stasis)

- Line 124: `function @inline gfx_cmd_begin(): void {`

## [samples/asset_breakout/vendor/stasis/stdlib/ui_single_pass.stasis](../samples/asset_breakout/vendor/stasis/stdlib/ui_single_pass.stasis)

- Line 3: `// Geometry is deliberately ephemeral: 'ui_begin_frame' resets the current`
- Line 143: `function ui_begin_frame(x: f32, y: f32, w: f32, h: f32): void {`

## [samples/audio_asset_playback/audio_asset_playback.stasis](../samples/audio_asset_playback/audio_asset_playback.stasis)

- Line 61: `begin_frame();`

## [samples/brickout_revenge/brickout_revenge.stasis](../samples/brickout_revenge/brickout_revenge.stasis)

- Line 134: `begin_frame();`

## [samples/brickout_revenge/brickout_revenge_v1.stasis](../samples/brickout_revenge/brickout_revenge_v1.stasis)

- Line 28: `// - begin_frame(), end_frame(): bracket GPU work`
- Line 3025: `begin_frame();`
- Line 3096: `begin_frame();`

## [samples/brickout_revenge/brickout_revenge_v1_cmd.stasis](../samples/brickout_revenge/brickout_revenge_v1_cmd.stasis)

- Line 18: `// - begin_frame(), end_frame(): bracket GPU work`
- Line 2838: `begin_frame();`

## [samples/bucket_catcher.stasis](../samples/bucket_catcher.stasis)

- Line 351: `begin_frame();`

## [samples/color_switch/main.stasis](../samples/color_switch/main.stasis)

- Line 34: `begin_frame();`

## [samples/immediate_axis_layout/main.stasis](../samples/immediate_axis_layout/main.stasis)

- Line 77: `begin_frame();`

## [samples/maximized_portrait/main.stasis](../samples/maximized_portrait/main.stasis)

- Line 30: `begin_frame();`

## [samples/pointer_pong/main.stasis](../samples/pointer_pong/main.stasis)

- Line 173: `begin_frame();`

## [samples/pong_web_minimal/src/main.stasis](../samples/pong_web_minimal/src/main.stasis)

- Line 16: `function @extern("web_begin_frame") web_begin_frame(red: i32, green: i32, blue: i32): void;`
- Line 82: `web_begin_frame(5, 12, 24);`

## [samples/render_parity/frame.stasis](../samples/render_parity/frame.stasis)

- Line 30: `begin_frame();`

## [samples/sprite_sheet_animation/main.stasis](../samples/sprite_sheet_animation/main.stasis)

- Line 22: `begin_frame();`

## [samples/swarm_field/src/main.stasis](../samples/swarm_field/src/main.stasis)

- Line 157: `begin_frame();`

## [samples/swarm_field/vendor/stasis/stdlib/graphics.stasis](../samples/swarm_field/vendor/stasis/stdlib/graphics.stasis)

- Line 254: `// not cross begin_frame/code-swap; reserve and finalize/cancel in one frame.`
- Line 541: `function begin_frame(): void {`
- Line 542: `gfx_cmd_begin();`

## [samples/swarm_field/vendor/stasis/stdlib/internal/gfx_cmd.stasis](../samples/swarm_field/vendor/stasis/stdlib/internal/gfx_cmd.stasis)

- Line 124: `function @inline gfx_cmd_begin(): void {`

## [samples/swarm_field/vendor/stasis/stdlib/ui_single_pass.stasis](../samples/swarm_field/vendor/stasis/stdlib/ui_single_pass.stasis)

- Line 3: `// Geometry is deliberately ephemeral: 'ui_begin_frame' resets the current`
- Line 143: `function ui_begin_frame(x: f32, y: f32, w: f32, h: f32): void {`

## [samples/tap_target/main.stasis](../samples/tap_target/main.stasis)

- Line 56: `begin_frame();`

## [samples/typed_drawable_visual/typed.stasis](../samples/typed_drawable_visual/typed.stasis)

- Line 37: `begin_frame();`

## [samples/typed_sprite/main.stasis](../samples/typed_sprite/main.stasis)

- Line 47: `begin_frame();`

## [samples/typed_sprite/tests/typed_sprite.test.stasis](../samples/typed_sprite/tests/typed_sprite.test.stasis)

- Line 38: `begin_frame();`
- Line 49: `begin_frame();`

## [samples/ui_gallery/gallery_common.stasis](../samples/ui_gallery/gallery_common.stasis)

- Line 64: `ui_begin_frame(x, y, w, h);`

## [samples/ui_gallery/main.stasis](../samples/ui_gallery/main.stasis)

- Line 63: `begin_frame();`

## [samples/ui_gallery/tests/ui_gallery.test.stasis](../samples/ui_gallery/tests/ui_gallery.test.stasis)

- Line 5: `ui_begin_frame(10.0, 20.0, 100.0, 100.0);`
- Line 14: `ui_begin_frame(0.0, 0.0, 100.0, 100.0);`
- Line 24: `ui_begin_frame(0.0, 0.0, 200.0, 100.0);`
- Line 32: `ui_begin_frame(0.0, 0.0, 200.0, 100.0);`
- Line 38: `ui_begin_frame(0.0, 0.0, 360.0, 240.0);`
- Line 47: `ui_begin_frame(0.0, 0.0, 360.0, 240.0);`
- Line 53: `ui_begin_frame(0.0, 0.0, 100.0, 60.0);`
- Line 56: `ui_begin_frame(0.0, 0.0, 100.0, 60.0);`

## [samples/ui_gallery/ui_single_pass.stasis](../samples/ui_gallery/ui_single_pass.stasis)

- Line 3: `// Geometry is deliberately ephemeral: 'ui_begin_frame' resets the current`
- Line 143: `function ui_begin_frame(x: f32, y: f32, w: f32, h: f32): void {`

## [samples/ui_gallery/vendor/stasis/stdlib/graphics.stasis](../samples/ui_gallery/vendor/stasis/stdlib/graphics.stasis)

- Line 254: `// not cross begin_frame/code-swap; reserve and finalize/cancel in one frame.`
- Line 541: `function begin_frame(): void {`
- Line 542: `gfx_cmd_begin();`

## [samples/ui_gallery/vendor/stasis/stdlib/internal/gfx_cmd.stasis](../samples/ui_gallery/vendor/stasis/stdlib/internal/gfx_cmd.stasis)

- Line 124: `function @inline gfx_cmd_begin(): void {`

## [samples/ui_gallery/vendor/stasis/stdlib/ui_single_pass.stasis](../samples/ui_gallery/vendor/stasis/stdlib/ui_single_pass.stasis)

- Line 3: `// Geometry is deliberately ephemeral: 'ui_begin_frame' resets the current`
- Line 143: `function ui_begin_frame(x: f32, y: f32, w: f32, h: f32): void {`

## [samples/web_export_smoke/src/main.stasis](../samples/web_export_smoke/src/main.stasis)

- Line 22: `function @extern("web_begin_frame") web_begin_frame(red: i32, green: i32, blue: i32): void;`
- Line 97: `web_begin_frame(7, 17, 31);`

## [samples/windows_launch_smoke/main.stasis](../samples/windows_launch_smoke/main.stasis)

- Line 61: `begin_frame();`

## [samples/world_camera_viewport/src/main.stasis](../samples/world_camera_viewport/src/main.stasis)

- Line 191: `begin_frame();`

## [src/stdlib/graphics.stasis](../src/stdlib/graphics.stasis)

- Line 254: `// not cross begin_frame/code-swap; reserve and finalize/cancel in one frame.`
- Line 548: `function begin_frame(): void {`
- Line 549: `gfx_cmd_begin();`

## [src/stdlib/internal/gfx_cmd.stasis](../src/stdlib/internal/gfx_cmd.stasis)

- Line 124: `function @inline gfx_cmd_begin(): void {`

## [src/stdlib/ui_single_pass.stasis](../src/stdlib/ui_single_pass.stasis)

- Line 3: `// Geometry is deliberately ephemeral: 'ui_begin_frame' resets the current`
- Line 143: `function ui_begin_frame(x: f32, y: f32, w: f32, h: f32): void {`

## [tests/stasis/seams/asset_extern_abi_probe.stasis](../tests/stasis/seams/asset_extern_abi_probe.stasis)

- Line 18: `begin_frame();`

## [tests/stasis/seams/desktop_display_metrics_probe.stasis](../tests/stasis/seams/desktop_display_metrics_probe.stasis)

- Line 92: `gfx_cmd_begin();`

## [tests/stasis/seams/desktop_hot_swap_generation_invalid.stasis](../tests/stasis/seams/desktop_hot_swap_generation_invalid.stasis)

- Line 15: `begin_frame();`

## [tests/stasis/seams/desktop_hot_swap_generation_reject.stasis](../tests/stasis/seams/desktop_hot_swap_generation_reject.stasis)

- Line 16: `begin_frame();`

## [tests/stasis/seams/desktop_hot_swap_generation_v1.stasis](../tests/stasis/seams/desktop_hot_swap_generation_v1.stasis)

- Line 16: `begin_frame();`

## [tests/stasis/seams/desktop_hot_swap_generation_v2.stasis](../tests/stasis/seams/desktop_hot_swap_generation_v2.stasis)

- Line 16: `begin_frame();`

## [tests/stasis/seams/desktop_input_frame_probe.stasis](../tests/stasis/seams/desktop_input_frame_probe.stasis)

- Line 54: `gfx_cmd_begin();`

## [tests/stasis/seams/desktop_manifest_assets_probe.stasis](../tests/stasis/seams/desktop_manifest_assets_probe.stasis)

- Line 44: `gfx_cmd_begin();`

## [tests/stasis/seams/generated_mobile_aot_probe.stasis.fixture](../tests/stasis/seams/generated_mobile_aot_probe.stasis.fixture)

- Line 47: `begin_frame();`

## [tests/stasis/seams/gfx_cmd_capacity_probe.stasis](../tests/stasis/seams/gfx_cmd_capacity_probe.stasis)

- Line 79: `gfx_cmd_begin();`
- Line 85: `gfx_cmd_begin();`
- Line 92: `gfx_cmd_begin();`
- Line 99: `gfx_cmd_begin();`
- Line 105: `gfx_cmd_begin();`
- Line 117: `gfx_cmd_begin();`
- Line 128: `gfx_cmd_begin();`
- Line 135: `gfx_cmd_begin();`
- Line 142: `gfx_cmd_begin();`
- Line 149: `gfx_cmd_begin();`

## [tests/stasis/seams/jit_aot_host_replay_probe.stasis](../tests/stasis/seams/jit_aot_host_replay_probe.stasis)

- Line 154: `gfx_cmd_begin();`

## [tests/stasis/seams/mobile_packaged_assets_probe.stasis](../tests/stasis/seams/mobile_packaged_assets_probe.stasis)

- Line 78: `begin_frame();`

## [tests/stasis/seams/sprite_run_writer_public_probe.stasis](../tests/stasis/seams/sprite_run_writer_public_probe.stasis)

- Line 19: `begin_frame();`
- Line 37: `begin_frame();`
- Line 42: `begin_frame();`
- Line 62: `begin_frame();`
- Line 108: `begin_frame();`

## [tests/stasis/seams/world_camera_viewport_probe.stasis](../tests/stasis/seams/world_camera_viewport_probe.stasis)

- Line 13: `gfx_cmd_begin();`

## [tests/stasis/sprite_sheet_animation.test.stasis](../tests/stasis/sprite_sheet_animation.test.stasis)

- Line 56: `gfx_cmd_begin();`
- Line 67: `gfx_cmd_begin();`
- Line 81: `gfx_cmd_begin();`
- Line 95: `gfx_cmd_begin();`

## [tests/stasis/ui_single_pass.test.stasis](../tests/stasis/ui_single_pass.test.stasis)

- Line 4: `ui_begin_frame(10.0, 20.0, 100.0, 100.0);`
- Line 23: `ui_begin_frame(0.0, 0.0, 100.0, 20.0);`
- Line 42: `ui_begin_frame(0.0, 0.0, 40.0, 40.0);`
- Line 55: `ui_begin_frame(0.0, 0.0, 100.0, 80.0);`
- Line 69: `ui_begin_frame(0.0, 0.0, 100.0, 100.0);`
- Line 86: `ui_begin_frame(0.0, 0.0, 100.0, 100.0);`
- Line 91: `ui_begin_frame(0.0, 0.0, 100.0, 100.0);`
- Line 98: `ui_begin_frame(0.0, 0.0, 100.0, 100.0);`
- Line 106: `ui_begin_frame(0.0, 0.0, 200.0, 100.0);`
- Line 114: `ui_begin_frame(0.0, 0.0, 200.0, 100.0);`
- Line 120: `ui_begin_frame(0.0, 0.0, 120.0, 60.0);`
- Line 126: `ui_begin_frame(0.0, 0.0, 120.0, 60.0);`
- Line 137: `ui_begin_frame(20.0, 10.0, 300.0, 200.0);`
- Line 143: `ui_begin_frame(40.0, 30.0, 600.0, 400.0);`
- Line 150: `gfx_cmd_begin();`
- Line 151: `ui_begin_frame(0.0, 0.0, 100.0, 60.0);`

## [tests/stasis/world_camera.test.stasis](../tests/stasis/world_camera.test.stasis)

- Line 80: `gfx_cmd_begin();`

## [tools/ci/check_android_shell.py](../tools/ci/check_android_shell.py)

- Line 1671: `assert "begin_frame();" in preview_adapter`

## [tools/ci/check_runtime_abi_contract.py](../tools/ci/check_runtime_abi_contract.py)

- Line 828: `r"\bbegin_frame\(\)\s*;",`
- Line 847: `VSCODE_RENDER_FIXTURE: ("begin_frame();", "draw_line("),`
- Line 849: `"begin_frame();",`
- Line 856: `"begin_frame();",`
- Line 862: `"begin_frame();",`
- Line 873: `"begin_frame();",`

## [tools/ci/test_android_emulator_seams.py](../tools/ci/test_android_emulator_seams.py)

- Line 413: `self.assertIn("begin_frame();", render)`

## [tools/ci/test_runtime_abi_contract.py](../tools/ci/test_runtime_abi_contract.py)

- Line 112: `contract.WORKSHOP_PREVIEW_ADAPTER, "begin_frame();", "legacy_begin();", "public_graphics_path",`
- Line 234: `(contract.HOT_SWAP_V2_FIXTURE, "begin_frame();", "legacy_begin();"),`

## [vscode-stasis/test/fixture/src/main.stasis](../vscode-stasis/test/fixture/src/main.stasis)

- Line 40: `begin_frame();`
