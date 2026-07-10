# Stasis Workshop for Android PRD

## Purpose

Stasis Workshop for Android is a sideload-first developer app for editing, compiling, previewing, and committing Stasis projects from an Android device.

The first version is a personal/dev workflow. The long-term direction is a full Stasis Android app/workshop.

## Product Decisions

- Distribution starts as a sideload-first developer app.
- The app should evolve toward a full Stasis Android workshop.
- Preview rendering is not hard-coded yet.
- Git v1 uses the GitHub API for commit, push, and PR creation.
- Full local git/libgit2 support can come later.
- Normal `.stasis` files remain the persisted source of truth.
- The user edits symbols, and the app maps those symbols back to `.stasis` file spans.
- Android uses the shared Stasis parser, semantic analysis, lowering, JIT, and AOT pipelines. Platform differences must be expressed as target capabilities or bridge configuration, not as Android-specific compiler forks.
- Android-only implementation stays at platform boundaries such as ABI/JNI, filesystem and lifecycle integration, target toolchain selection, and APK packaging.

## Android Preview

The Android preview may use either:

- Rust-owned rendering surface, if the existing Stasis renderer/runtime can be reused cleanly.
- Android-native preview surface, if that improves UI integration, touch handling, compositing, or long-term app polish.

Selection criterion:

- Choose the path that gets stable preview plus hot reload working with the least architectural friction.

Recommended first prototype:

- Start with the path closest to the existing Stasis runtime.
- Allow replacement with Android-native preview if the bridge gets awkward.

## Workshop Surface Priorities

The game preview remains the default surface. The workshop overlay is a pull-down workspace whose primary job is to accept intent and execute commands, not to expose every developer setting at once.

Priority order:

1. Chat and commands are the first content in the pull-down workspace. They should be immediately available for short requests, command entry, progress, diagnostics, and recent outcomes.
2. AI API key/model configuration is a collapsed secondary settings area. It is normally set once and should not compete with chat/command controls.
3. Manual source and symbol browsing remains available in the pull-down workspace, but is secondary to chat/commands because real code is inspected less frequently.
4. GitHub backup/code-sync activity is frequent but mostly background work. The UI should show compact sync state and errors without making commit/push controls the foreground workflow; detailed review remains available when needed.
5. A top-of-game voice shortcut will start a voice-change request. It sits directly below the pull-down Workshop button so the persistent debug details remain visible rather than being covered by the voice control. Its active state must expose explicit `Cancel` and `Run` actions before any edit is applied.
6. Starting an AI run must provide a clear, persistent status message that identifies the active phase (for example, preparing, running commands, validating tests, applying, completed, or failed) and shows the latest actionable result.

The detailed editor, settings, GitHub review, and voice surfaces should preserve the current app-private `.stasis` project as the source of truth and must not bypass local compile/test validation.

Workshop completion also requires:

- Durable per-project command history, cancellation/retry, and explicit AI token/cost budgets.
- A durable per-project AI work queue shared by typed and voice requests. Every submission becomes a visible item with `pending`, `in progress`, and terminal state; pending items can be cancelled before execution, while the active item uses the existing safe cancellation boundary.
- Budget accounting retains full precision, while user-facing dollar totals and limits round to the nearest cent.
- Audio import/recording and lightweight editing alongside image assets.
- Autosave, process-death recovery, offline/background behavior, and battery/network-aware long-running work.
- First-run onboarding, templates, and a complete manual workflow that does not require AI.
- Accessible, adaptive phone/tablet/foldable layouts and screen-reader/keyboard support.
- Permission minimization, explicit external-upload consent, credential revocation, and user-controlled deletion of projects, traces, caches, and history.
- Versioned project metadata with rollback-safe migrations across Workshop/compiler upgrades.
- Redacted crash recovery/support bundles that exclude secrets and unapproved source or media.

## Bundled Games and Build Identity

- The Workshop build is a general game workshop, never a game-specific product. It may preload an exploration tutorial, Pong, and additional templates, and users can create/import/switch arbitrary projects.
- The default bundled project will become a touch-first exploration tutorial: tapping chooses a destination, a character walks toward it deterministically, and nearby collectible items enter an inventory.
- The exploration tutorial uses a data-oriented learning architecture: stable entity IDs; bounded structure-of-arrays component storage; explicit input, movement, collection, inventory, and render-extraction systems; deterministic tick progression; no hidden object graph; and small files introduced in a teachable order.
- The detailed architecture, lesson order, deterministic schedule, and acceptance matrix are canonicalized in `docs/android_exploration_sample_design.md`.
- Pong remains bundled as a compact mechanics/hot-reload example and must remain selectable after the exploration tutorial becomes the default.
- Every release build is game-specific. Its package, AOT roots, assets, display name, and acceptance tests identify exactly one game and contain no Workshop editor/JIT surface. The current reference release build is Pong.

## Source Organization

Example project layout:

```text
src/
  main.stasis
  root.stasis
  game_state.stasis
  player.stasis
  enemy.stasis
  projectile.stasis
  input.stasis
  assets.stasis

  systems/
    collision.stasis
    combat.stasis
    spawning.stasis
```

## Stasis Syntax

Generated source and examples must use Stasis syntax:

```stasis
function name(param: Type): ReturnType {
    // ...
}
```

Do not generate Rust-style declarations with mutable-reference parameters or arrow return types in Stasis examples.

Struct and element parameters use Stasis view/reference passing semantics. Examples should use `self: Type`, not Rust reference syntax.

## Function Placement Rules

- Struct definitions go in their own file.
- Receiver-style functions go in the file for the receiver type.
- A function whose first parameter is a struct view usually goes in the file for that first struct type.
- A function that returns or creates a specific struct goes in that struct's file.
- Lifecycle functions go in `src/main.stasis`.
- No-owner utility functions go in `src/root.stasis`.
- Cross-struct/system behavior goes in `src/systems/<system>.stasis`.

Lifecycle functions:

- `main()`
- `init()`
- `tick()`
- `render()`
- `on_code_swap()`

Reachability roots:

- `main`
- `tick`
- `on_code_swap`
- host-required exported entry symbols

Hot swap only occurs between ticks.

## Example Files

### `src/player.stasis`

```stasis
struct Player {
    x: f32;
    y: f32;
    velocity_y: f32;
    jump_cooldown_ticks: i32;
}

function update(self: Player, input: InputState): void {
    if (self.jump_cooldown_ticks > 0) {
        self.jump_cooldown_ticks -= 1;
    }

    if (input.jump_pressed && self.jump_cooldown_ticks == 0) {
        self.jump();
    }

    self.velocity_y += 0.35;
    self.y += self.velocity_y;
}

function jump(self: Player): void {
    self.velocity_y = -8.5;
    self.jump_cooldown_ticks = 12;
}

function create_default_player(): Player {
    return GameState.player;
}
```

Notes:

- Player functions live with `Player`.
- Receiver form is preferred.
- Function names can be receiver-scoped.
- Struct/view parameters are Stasis views, not Rust references.

### `src/enemy.stasis`

```stasis
struct Enemy {
    x: f32;
    y: f32;
    hp: i32;
    active: bool;
}

function update(self: Enemy): void {
    if (!self.active) {
        return;
    }

    self.x -= 1.0;
}

function damage(self: Enemy, amount: i32): void {
    self.hp -= amount;

    if (self.hp <= 0) {
        self.active = false;
    }
}
```

### `src/game_state.stasis`

```stasis
global GameState {
    player: Player;
    enemies: Enemy[32];
    score: i32;
    tick_count: i32;
}
```

Persistent data lives in global memory, matching the current Stasis model.

### `src/main.stasis`

```stasis
import "game_state.stasis";
import "player.stasis";
import "enemy.stasis";
import "input.stasis";
import "systems/collision.stasis";

function main(): void {
    init();
}

function init(): void {
    GameState.score = 0;
    GameState.tick_count = 0;
}

function tick(): void {
    GameState.tick_count += 1;

    let input = read_input();

    GameState.player.update(input);

    foreach (let enemy in GameState.enemies) {
        enemy.update();
    }

    collision_update();
}

function on_code_swap(): void {
    // Optional hook after a successful hot swap.
}
```

### `src/systems/collision.stasis`

```stasis
import "../game_state.stasis";
import "../player.stasis";
import "../enemy.stasis";

function collision_update(): void {
    foreach (let enemy in GameState.enemies) {
        if (!enemy.active) {
            continue;
        }

        if (player_overlaps_enemy(GameState.player, enemy)) {
            enemy.damage(1);
        }
    }
}

function player_overlaps_enemy(player: Player, enemy: Enemy): bool {
    let dx = player.x - enemy.x;
    let dy = player.y - enemy.y;

    return dx * dx + dy * dy < 64.0;
}
```

Cross-struct behavior belongs in system files when no single struct clearly owns the behavior.

## Call Style

Preferred:

```stasis
player.update(input);
player.jump();
enemy.damage(5);
```

Supported fallback:

```stasis
update(player, input);
jump(player);
damage(enemy, 5);
```

The Android editor and AI patch format should prefer receiver-style calls because that matches the current language direction.

## Symbol Tree UX

The app should present symbols like this:

```text
Main
  main()
  init()
  tick()
  on_code_swap()

Structs
  Player
    struct Player
    update(self: Player, input: InputState): void
    jump(self: Player): void
    create_default_player(): Player

  Enemy
    struct Enemy
    update(self: Enemy): void
    damage(self: Enemy, amount: i32): void

Systems
  Collision
    collision_update(): void
    player_overlaps_enemy(player: Player, enemy: Enemy): bool

Root
  get_starting_level_index(): i32
```

## AI Code Request Format

```json
{
  "user_prompt": "Make the player jump higher but prevent repeated jumps.",
  "selected_symbols": [
    {
      "kind": "struct",
      "name": "Player",
      "file": "src/player.stasis",
      "source": "struct Player { ... }"
    },
    {
      "kind": "function",
      "name": "jump",
      "owner": "Player",
      "file": "src/player.stasis",
      "source": "function jump(self: Player): void { ... }"
    },
    {
      "kind": "function",
      "name": "update",
      "owner": "Player",
      "file": "src/player.stasis",
      "source": "function update(self: Player, input: InputState): void { ... }"
    }
  ],
  "stasis_style_rules": {
    "use_function_keyword": true,
    "use_receiver_style_when_possible": true,
    "do_not_use_rust_references": true,
    "struct_functions_live_with_struct": true,
    "lifecycle_functions_live_in_main": true,
    "no_owner_functions_live_in_root": true
  }
}
```

## AI Code Response Format

```json
{
  "summary": "Increased jump strength and added a short cooldown.",
  "edits": [
    {
      "kind": "replace_function",
      "owner": "Player",
      "name": "jump",
      "file": "src/player.stasis",
      "new_source": "function jump(self: Player): void {\n    self.velocity_y = -10.0;\n    self.jump_cooldown_ticks = 12;\n}"
    },
    {
      "kind": "replace_function",
      "owner": "Player",
      "name": "update",
      "file": "src/player.stasis",
      "new_source": "function update(self: Player, input: InputState): void {\n    if (self.jump_cooldown_ticks > 0) {\n        self.jump_cooldown_ticks -= 1;\n    }\n\n    if (input.jump_pressed && self.jump_cooldown_ticks == 0) {\n        self.jump();\n    }\n\n    self.velocity_y += 0.35;\n    self.y += self.velocity_y;\n}"
    }
  ],
  "expected_reload": "FastReload",
  "reason": "Only function bodies changed. Player layout did not change."
}
```

## Reload Classification

Function body change:

- Compiler detects function body hash changed.
- Function signature is unchanged.
- Struct/global layout hash is unchanged.
- Classification: `FastReload`.
- Runtime compiles changed functions with Cranelift, installs new code pointers, swaps at a tick boundary, and preserves global memory.

Struct layout change:

```stasis
struct Player {
    x: f32;
    y: f32;
    velocity_y: f32;
    jump_cooldown_ticks: i32;
    dash_cooldown_ticks: i32;
}
```

If `dash_cooldown_ticks` is added:

- Classification: `ResetRequired`.
- Reason: `Player` layout changed; global memory layout may need to be rebuilt, so current runtime state cannot be blindly preserved.

## Git UX

V1 uses the GitHub API commit, push, and PR flow.

The Android UX should summarize changes by symbol first:

```text
Changed symbols:
  Player
    modified jump(self: Player): void
    modified update(self: Player, input: InputState): void

Changed files:
  src/player.stasis
```

Raw file diffs should be available as an advanced/review option.

## Visual Asset and Multimodal AI Workflow

- The Workshop must import images from Android storage/photo picker into the active project as normal binary assets with stable project-relative paths.
- A small touch-first paint editor must support creating and editing simple game images, including pencil/brush, eraser, color selection, undo/redo, clear, crop/canvas sizing, and explicit save/cancel.
- Imported or painted images must be previewable and selectable as attachments to chat, voice, and command requests.
- AI requests must use real multimodal image inputs for selected attachments rather than describing image bytes as text.
- The Workshop must capture the actual game preview framebuffer as an image and optionally attach it to AI requests. The existing logical render snapshot remains useful structured context, but is not a pixel screenshot.
- AI responses that create or revise image assets must write normal project assets, preserve originals until accepted, and participate in review, undo, GitHub backup, and project export.
- Image handling must impose bounded dimensions/file sizes and deterministic conversion rules so mobile memory and API costs remain visible and controlled.

## Rule Summary

- Use Stasis syntax in all generated source.
- Use `function`, not `fn`.
- Use receiver-style functions when possible.
- Use `self: Type`, not Rust reference syntax.
- Keep structs in their own files.
- Keep struct-owned functions with their struct.
- Keep lifecycle functions in `main.stasis`.
- Keep no-owner helpers in `root.stasis`.
- Keep cross-struct behavior in `systems/*.stasis`.
- Use symbol-based editing on Android.
- Persist normal `.stasis` files on disk.
- Compile changed symbols locally.
- Hot reload changed functions with Cranelift.
- Use the GitHub API for v1 commit/push.
- Support project image import, lightweight paint editing, and multimodal AI attachments.
- Distinguish real pixel screenshots from logical render snapshots.
- Start sideload-first.
- Choose preview renderer based on least friction.
