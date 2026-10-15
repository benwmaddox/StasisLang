# Action-only receiver API migration

This revision removes the redundant receiver-family prefixes from the public
standard-library APIs listed below. It is a source-breaking rename: update each
call to the canonical name, in either receiver form (`value.action(args)`) or
function form (`action(value, args)`). The old public spellings are not retained
as compatibility aliases. This changes Stasis source names only; host extern
symbols and their ABI contracts remain unchanged.

The ASCII and UTF-8 conversions already use the overloaded public names
`from_i32` and `from_ascii` from task #838. The remaining `ascii_from_i32`,
`utf8_from_i32`, and `utf8_from_ascii` declarations are internal implementation
helpers, not supported migration aliases. The `WorldTileRange` operation is
named `calculate_for_camera` because the camera is its input, while the tile
range is the receiver.

| Previous public name | Receiver type | New name | Maintained caller coverage | Same-name overloads/collisions |
| --- | --- | --- | --- | --- |
| `ascii_max_length` | `ascii[]` | `max_length` | ASCII buffer tests, numeric-text JIT/AOT/Wasm suite, Android numeric seam | `utf8[].max_length` |
| `ascii_scan_limit` | `ascii[]` | `scan_limit` | ASCII buffer tests and compiler/runtime callers | - |
| `ascii_set_len` | `ascii[]` | `set_len` | JIT capacity tests, numeric-text tests and samples | - |
| `ascii_is_empty` | `ascii[]` | `is_empty` | ASCII buffer tests and samples | - |
| `ascii_get` | `ascii[]` | `get` | ASCII buffer tests and samples | - |
| `ascii_set` | `ascii[]` | `set` | ASCII buffer tests and samples | - |
| `ascii_recount` | `ascii[]` | `recount` | JIT capacity tests and ASCII buffer tests | - |
| `ascii_clear` | `ascii[]` | `clear` | Numeric-text tests, browser playground, and compiler coverage analysis | `clear(f32, f32, f32, f32)`, `Rig2D.clear` |
| `ascii_copy` | `ascii[]` | `copy` | Print fixture, numeric-text tests, Android seam and samples | - |
| `ascii_append` | `ascii[]` | `append` | ASCII buffer tests and samples | `LineBatch.append` |
| `ascii_push` | `ascii[]` | `push` | ASCII buffer tests and samples | `AudioStream.push` |
| `ascii_write_i32` | `ascii[]` | `write_i32` | Numeric-text tests and compiler backend coverage | - |
| `ascii_append_i32` | `ascii[]` | `append_i32` | Numeric-text tests, Android seam and samples | - |
| `ascii_push_i32` | `ascii[]` | `push_i32` | Sprite-run seam, text coverage proof and samples | - |
| `ascii_write_f32_fixed` | `ascii[]` | `write_f32_fixed` | Numeric-text tests and compiler backend coverage | - |
| `ascii_from_f32_fixed` | `ascii[]` | `from_f32_fixed` | Numeric-text JIT/AOT/Wasm suite and samples | - |
| `ascii_append_f32_fixed` | `ascii[]` | `append_f32_fixed` | Numeric-text tests and compiler backend coverage | - |
| `ascii_write_f64_short` | `ascii[]` | `write_f64_short` | Numeric-text tests and compiler backend coverage | - |
| `ascii_from_f64_short` | `ascii[]` | `from_f64_short` | Numeric-text tests and samples | - |
| `ascii_append_f64_short` | `ascii[]` | `append_f64_short` | Numeric-text tests and samples | - |
| `ascii_pad_left` | `ascii[]` | `pad_left` | Numeric-text tests and JIT/AOT/Wasm suite | - |
| `ascii_cmp` | `ascii[]` | `cmp` | ASCII buffer and compiler JIT tests | - |
| `ascii_eq` | `ascii[]` | `eq` | ASCII buffer tests and samples | - |
| `ascii_find_byte` | `ascii[]` | `find_byte` | ASCII buffer tests and compiler JIT tests | - |
| `ascii_find_last_byte` | `ascii[]` | `find_last_byte` | ASCII buffer tests and compiler JIT tests | - |
| `ascii_starts_with` | `ascii[]` | `starts_with` | ASCII buffer tests and compiler JIT tests | - |
| `ascii_ends_with` | `ascii[]` | `ends_with` | ASCII buffer tests and samples | - |
| `ascii_find` | `ascii[]` | `find` | ASCII buffer tests and compiler JIT tests | - |
| `ascii_contains` | `ascii[]` | `contains` | ASCII buffer tests and samples | - |
| `utf8_max_length` | `utf8[]` | `max_length` | UTF-8 buffer tests and numeric-text JIT/AOT/Wasm suite | `ascii[].max_length` |
| `utf8_set_byte_len` | `utf8[]` | `set_byte_len` | UTF-8 buffer tests and compiler JIT tests | - |
| `utf8_set_char_len` | `utf8[]` | `set_char_len` | UTF-8 buffer tests and compiler JIT tests | - |
| `utf8_set_len_ascii` | `utf8[]` | `set_len_ascii` | UTF-8 buffer tests, graphics capacity seam and samples | - |
| `utf8_clear_ascii` | `utf8[]` | `clear_ascii` | UTF-8 buffer tests and compiler backend coverage | - |
| `load_image` | `ImageAsset` | `load` | Asset-breakout sample and asset loader/package tests | `AudioAsset.load` |
| `load_audio` | `AudioAsset` | `load` | Audio playback sample, Android resource seams and Android shell checks | `ImageAsset.load` |
| `load_sprite_from` | `Sprite` | `load_from` | Browser playground/Pong, asset probes, Android bridge and snapshot tests | `SpriteSheet.load_from` |
| `load_sprite_sheet_from` | `SpriteSheet` | `load_from` | Sprite-sheet sample, asset snapshots and editor completion tests | `Sprite.load_from` |
| `reset_presentation` | `PresentationList` | `reset` | Sealed display-list tests and program snapshot fixtures | `LineBatch`, `WorldCameraHistory`, `LoadingProgress`, `AdTask` receivers |
| `reset_lines` | `LineBatch` | `reset` | Sprite-run writer seam and public graphics tests | `PresentationList`, `WorldCameraHistory`, `LoadingProgress`, `AdTask` receivers |
| `world_camera_configure` | `WorldCamera` | `configure` | World-camera tests, viewport seam and sample | - |
| `world_camera_follow` | `WorldCamera` | `follow` | World-camera tests, viewport seam and sample | - |
| `world_camera_clip_begin` | `WorldCamera` | `clip_begin` | World-camera tests and viewport seam | - |
| `world_camera_clip_end` | `WorldCamera` | `clip_end` | World-camera tests and viewport seam | - |
| `world_camera_history_reset` | `WorldCameraHistory` | `reset` | World-camera tests, viewport seam and sample | `PresentationList`, `LineBatch`, `LoadingProgress`, `AdTask` receivers |
| `world_camera_history_advance` | `WorldCameraHistory` | `advance` | World-camera tests, viewport seam and sample | - |
| `world_camera_history_advance_or_teleport` | `WorldCameraHistory` | `advance_or_teleport` | World-camera tests and sample | - |
| `world_camera_tile_range_calculate` | `WorldTileRange` | `calculate_for_camera` | World-camera tests, viewport seam and sample | - |

Representative migrated callers include
[`tests/stasis/stdlib_numeric_text.test.stasis`](../tests/stasis/stdlib_numeric_text.test.stasis),
[`crates/stasis_compiler/tests/numeric_text_jit_aot_wasm.rs`](../crates/stasis_compiler/tests/numeric_text_jit_aot_wasm.rs),
[`tests/stasis/world_camera.test.stasis`](../tests/stasis/world_camera.test.stasis),
[`samples/world_camera_viewport/src/main.stasis`](../samples/world_camera_viewport/src/main.stasis),
[`crates/stasis_browser_compiler/web/pong_example.mjs`](../crates/stasis_browser_compiler/web/pong_example.mjs),
and [`tests/stasis/seams/mobile_packaged_assets_probe.stasis`](../tests/stasis/seams/mobile_packaged_assets_probe.stasis).

The compiler resolves these calls using the type of the first argument, so
same-name APIs on distinct receiver types remain distinct in method and
function form. Asset loader annotations stay attached to the path parameter;
the sprite host extern remains `stasis_jit_sprite_load_from`.
