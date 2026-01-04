# Brickout Revenge: tower selection, drag placement, and SFX

This note is a practical rundown of what to touch in `samples/brickout_revenge/brickout_revenge.stasis` and `samples/brickout_revenge/data/config.json` to add:
- tower selection UI
- drag-to-place towers
- sound effects

It also lists prerequisites that are not wired yet.

Design reference:
- `docs/brickout-revenge-brainstorm.md` for the overall game goals and progression model that this input flow supports.
## Current anchors in the sample
- `GameState` has `cursor`, `layout`, and `tap_pulses`, but no selection/drag state.
- UI scaffolding exists in `draw_menu_panel`, `draw_tech_panel`, and `draw_ui_panels`, but the slots are not interactive.
- Pointer input is already exposed via `input_pointer_*` and used only for `record_tap_pulses()`.
- Tower stats live in config (`tower_basic`, `tower_armored`, `tower_reflector`) and are already consumed when towers shoot.
- Grid placement helpers only go one way: `grid_to_world_x/y` exist; there is no world-to-grid conversion yet.
- There is no audio/SFX logic in the sample.

## Tower selection (UI + state)

### State to add
Add explicit selection state to `GameState`:
- `selected_tower: BrickType` (default `BrickType.Basic`)
- `ui_hover_slot: i32` (optional, for hover highlight)
- `ui_pressed_slot: i32` (optional, for click/tap down state)

You can keep `cursor.brick_type` as the active selection, but make it explicit which field is the source of truth.

### UI hit testing
Add a small helper that maps screen pixels into UI-space:
- Use the existing `state.layout.ui_x/ui_y/ui_w/ui_h` rectangle.
- Create `screen_to_ui_x/y` using the same viewport offsets as `play_to_screen_x/y`.

Then define the menu slot rects in `draw_menu_panel` and reuse that same layout data in a new `ui_pick_menu_slot(x, y) -> i32` helper. The slots are already sized by `slot_w/slot_h/slot_gap`.

### Selection flow
- On `pointer_went_down`, if pointer is inside the menu panel, select the corresponding slot.
- Update `selected_tower` (and/or `cursor.brick_type`) immediately.
- Draw a highlight around the selected slot in `draw_menu_panel`.

### Config dependency
Selection should map 1:1 to `BrickType.{Basic,Armored,Reflector}` so the existing tower stats in `config` continue to work.

## Drag-to-place (interaction + placement rules)

### Prereq helpers to add
These are missing today and should be added before drag placement:
- `screen_to_play_x/y(screen_px: f32) -> f32`
- `world_to_grid_x/y(world: f32) -> i32` with clamping to grid bounds
- `grid_is_occupied(gx, gy) -> bool` (based on active bricks in `state.bricks`)

### State to add
Add drag state to `GameState`:
- `drag_active: i32`
- `drag_pointer_id: i32`
- `drag_world_x: f32`
- `drag_world_y: f32`
- `drag_grid_x: i32`
- `drag_grid_y: i32`
- `drag_valid: i32` (0/1)

### Drag loop
- On `pointer_went_down` in the play area, start drag:
  - store pointer id
  - convert screen px to play/world coords
  - snap to grid via `world_to_grid_*`
  - compute `drag_valid` (inside grid and not occupied)
- While dragging (`pointer_is_down`), update `drag_world_*` and re-snap.
- On `pointer_went_up` for the same id:
  - if `drag_valid`, place the brick (see below)
  - clear `drag_active`

### Placement
Placement can reuse existing brick initialization logic used in `setup_default_bricks()`:
- Find a free brick slot
- Fill in `brick_type`, `x/y` via `grid_to_world_x/y`, `width/height`, `hp`, `active = true`
- Optionally set `tower_*` fields (range, cooldown, etc.) if you add those per-brick

### Visual feedback
During drag:
- draw a ghosted brick at the snapped grid position
- use green for valid, red for invalid

## Sound effects (SFX)

### Prereqs that are not wired yet
The runtime has low-level audio functions, but the sample has no event/mixer layer:
- `stasis_audio_*` exists in the runtime, but there is no Brickout audio event queue.
- There is no asset pipeline for WAV/OGG in the sample.

To add SFX, you need one of the two approaches below.

### Option A: procedural mixer (fastest to wire)
- Add a small `SoundEvent` ring buffer to `GameState` (e.g., fixed array + head/tail indices).
- Each frame, emit events for actions (select, start drag, place, invalid, shot, hit, break, lose).
- Write `game_get_sound_samples(out, frames)` that synthesizes short waveforms per event (simple sin/noise envelopes).
- Push samples with `audio_push_f32_interleaved`.

### Option B: sample playback (better quality, more assets)
- Define a simple `SoundAsset` struct with PCM data in memory.
- Load samples from files once (requires a loader; not present yet).
- Mixer combines active voices into the output buffer each frame.

### Suggested event map
- UI: `select`, `drag_start`, `drag_place`, `drag_invalid`
- Combat: `tower_shot`, `ball_bounce`, `brick_break`, `wave_start`, `lose`

## Recommended implementation order
1. Add missing helper functions (screen->play, world->grid, occupancy).
2. Add selection state + menu slot hit testing.
3. Add drag state + ghost preview.
4. Add placement + occupancy update.
5. Add SFX event queue + procedural mixer (Option A).

## Files to touch
- `samples/brickout_revenge/brickout_revenge.stasis`
- `samples/brickout_revenge/data/config.json` (if you add new tuning values)
- `runtime/` only if you extend audio beyond current `stasis_audio_*` API
