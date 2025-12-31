# Data Hot-Reload Plan

Fast data reloading (<50ms) for game development iteration.

## Status (implemented)

Data hot-reload is implemented today for the Cranelift runner workflow:

- The CLI emits a per-module `struct-meta.json` describing the flattened `global state` field layout and JSON paths.
- The native runner (`runtime/stasis_runner.c`) loads a JSON file and applies values into the running module by looking up exported global symbols (DLL exports).
- During tick-hosted runs, the runner polls the JSON file mtime once per tick and re-applies changes between ticks (no recompilation).

See:
- `runtime/stasis_data.c` (JSON -> globals binder)
- `samples/data_hotreload_smoke.stasis` + `data/data_hotreload_smoke/balance.json`
- `samples/data_hotreload_latency.stasis` + `data/data_hotreload_latency/balance.json`

## Problem

Current hot reload timings:

| Change Type | Time | Bottleneck |
|-------------|------|------------|
| Code (.stasis) | 50-250ms | Cranelift AOT + linking |
| SVG assets | ~5-10ms | `gfx_poll_reload()` |
| Data files | <50ms typical | file read + JSON parse + apply |

Code changes require compilation. Data changes (level layouts, entity definitions, balance parameters) don't need compilation - they should reload in <50ms.

## Goals

1. Define a data format for game configuration
2. Load data on init (`main()`)
3. Hot-reload data during dev mode when files change (<50ms)
4. No recompilation required for data changes
5. Integrate cleanly with existing `tick()` workflow

## What Is "Data"?

Examples of data that should hot-reload fast:

- Level layouts (tile grids, entity spawn positions)
- Entity definitions (brick types, enemy stats, powerup effects)
- Balance parameters (speeds, cooldowns, damage values)
- Progression curves (score thresholds, difficulty scaling)
- UI layout (positions, sizes, colors)
- Animation parameters (durations, easing curves)

Not data (handled by existing systems):

- Code logic (hot-swap workflow)
- SVG sprites (`gfx_poll_reload()`)
- Audio assets (future work)

## Design Options

### Option A: Runtime Facilities (Recommended)

Add built-in functions to the runtime that handle data loading and watching.

```stasis
// In main():
data_watch("data/levels.json");
data_watch("data/balance.json");

// In tick():
if (data_changed("data/levels.json")) {
    reload_levels();
}
```

**Pros:**
- No language changes required
- Fast C implementation in runtime
- Matches existing `gfx_poll_reload()` pattern
- Clear separation: code is compiled, data is interpreted

**Cons:**
- Requires parsing in C (or linking a JSON library)
- Data access API needs design

### Option B: CLI-Mediated Data Push

CLI watches data files and pushes changes to the runner via IPC.

```
CLI detects change → parses data → writes to shared memory → signals runner
```

**Pros:**
- Parsing in C# (easier JSON/TOML handling)
- Can validate data against schema before pushing
- Decoupled from runtime

**Cons:**
- Adds IPC complexity
- CLI must stay running
- Latency from process boundary

### Option C: Memory-Mapped Data Files

Runner memory-maps data files directly. File changes are visible immediately.

**Pros:**
- Near-instant reload (OS handles caching)
- Zero parsing if using binary format

**Cons:**
- Binary format is developer-hostile
- Complex error handling for corrupted files
- Platform differences in mmap semantics

## Current Approach (runner-based binding)

The current implementation is closest to Option A, but it is host/runner-mediated rather than exposed as Stasis built-ins:

- The Stasis program keeps a single `global state` struct for gameplay state.
- The compiler/CLI emits `struct-meta.json` for that `state` layout (field symbol names + JSON paths + types).
- The runner reads a JSON file and writes values into exported `state__...` globals in the loaded DLL.

This keeps edits to `.json` files fast (no compilation) and works with the existing `tick()` hot-swap loop.

## Data Format

### Primary Format: JSON

- Human-readable and editable
- Good editor support
- Fast enough for <50ms target (typical game data is <100KB)
- Uses cJSON library (embedded in runtime)

### JSON Format (current)

Use nested JSON matching the `jsonPath` emitted in `struct-meta.json` (dot-separated, without the `state__` prefix).
Example for `state.balance.health`:

```json
{ "balance": { "health": 123 } }
```

The binder also accepts the flattened symbol name as a fallback key (e.g. `state__balance__health`) but nested JSON is preferred.

### Binary Format (Future Optimization)

If JSON parsing becomes a bottleneck:
- Bake JSON → binary during build
- Memory-map binary in release mode
- Keep JSON for dev mode

## Current Workflow (today)

- Define your configuration under `global state` (for example `state.balance.health`).
- Put JSON next to your source (preferred):
  - `samples/my_game.stasis`
  - `samples/my_game/data/config.json`
- Run in the tick-hosted dev loop (Cranelift):

```bat
.\stasis.bat run .\samples\data_hotreload_latency.stasis --backend cranelift
```

While running, edit the JSON file and save. The runner re-applies changes between ticks.

## API Design

Note: The sections below describe an older proposed Stasis-level `data_watch()` API. The current implementation uses runner-based binding (see "Status" + "Current Workflow" above).

### Core Functions

```c
// Initialize data watching (call once per file in main())
// Returns: handle (>0 on success, 0 on failure)
i32 data_watch(path: string);

// Check if data file changed since last poll
// Returns: 1 if changed, 0 if not
i32 data_changed(handle: i32);

// Get raw file contents as string (for custom parsing)
// Returns: pointer to null-terminated string (valid until next poll)
string data_get_raw(handle: i32);

// Get file size in bytes
i32 data_get_size(handle: i32);
```

### Typed Accessors (Convenience Layer)

```c
// JSON path queries
i32 data_get_i32(handle: i32, path: string, default: i32);
f32 data_get_f32(handle: i32, path: string, default: f32);
string data_get_string(handle: i32, path: string);

// Array access
i32 data_array_len(handle: i32, path: string);
i32 data_array_get_i32(handle: i32, path: string, index: i32, default: i32);
```

### Example Usage

```stasis
struct GameData {
    levels_handle: i32;
    balance_handle: i32;
}

global data: GameData;

function main(): i32 {
    data.levels_handle = data_watch("data/levels.json");
    data.balance_handle = data_watch("data/balance.json");

    if (data.levels_handle == 0) {
        print_string("Failed to load levels.json\n");
        return 1;
    }

    load_levels();
    load_balance();
    return 0;
}

function tick(): i32 {
    // Check for data changes (fast path: just checks a flag)
    if (data_changed(data.levels_handle)) {
        load_levels();
        print_string("Levels reloaded!\n");
    }

    if (data_changed(data.balance_handle)) {
        load_balance();
    }

    // ... game logic using current data ...
    return 0;
}

function load_balance(): void {
    state.player_speed = data_get_f32(data.balance_handle, "player.speed", 200.0);
    state.bullet_damage = data_get_i32(data.balance_handle, "bullet.damage", 10);
    state.enemy_spawn_rate = data_get_f32(data.balance_handle, "enemy.spawn_rate", 0.5);
}
```

## Implementation Plan

### Phase 1: File Watching Infrastructure

Add to `stasis_runner.c`:

1. **Data file registry**
   - Array of watched file paths + handles
   - Last-modified timestamps
   - File content buffers

2. **`data_watch()` implementation**
   - Open file, read contents, record mtime
   - Allocate handle slot
   - Return handle

3. **`data_changed()` implementation**
   - Check file mtime against recorded value
   - If changed: re-read file, update buffer, return 1
   - If unchanged: return 0
   - Target: <1ms for the check, <10ms for reload

4. **`data_get_raw()` implementation**
   - Return pointer to current content buffer

### Phase 2: JSON Parsing

1. **Integrate lightweight JSON parser**
   - Options: cJSON (small, MIT), yyjson (fast, MIT)
   - Parse on file change, cache parsed tree

2. **Implement typed accessors**
   - Path parsing (e.g., "player.speed" → navigate JSON tree)
   - Type coercion with defaults

### Phase 3: CLI Integration

1. **Watch data files alongside source**
   - Detect `.json` files in project
   - Print reload notifications

2. **Data validation (optional)**
   - Schema checking before runner sees data
   - Better error messages

### Phase 4: Optimization (If Needed)

1. **Binary format for release**
2. **Incremental parsing (only changed subtrees)**
3. **Memory-mapped files for large data**

## File Watching Strategy

### Approach: Polling with Smart Timing

Same strategy as SVG hot-reload:

```c
// In data_changed():
time_t current_mtime = get_file_mtime(path);
if (current_mtime != recorded_mtime) {
    recorded_mtime = current_mtime;
    reload_file_contents();
    reparse_json();
    return 1;
}
return 0;
```

**Timing:**
- `stat()` call: <0.1ms
- File read (100KB): ~1ms
- JSON parse (100KB): ~1-5ms
- Total: <10ms typical

### Debouncing

The game already runs at 60 FPS (16.6ms per tick). Checking once per tick is natural debouncing:

- Maximum reload frequency: 60/second
- File saves that span multiple writes: handled by mtime settling

## Error Handling

### File Not Found

- `data_watch()` returns 0
- Game code checks handle validity
- Missing data is a game bug, not runtime's problem

### Parse Errors

- Log error to console with line/column
- Keep previous valid data (don't corrupt state)
- Set a "parse error" flag game can query

```c
i32 data_has_error(handle: i32);
string data_get_error(handle: i32);
```

### File Access Errors

- Editor has file locked during save
- Retry on next poll (mtime will still differ)
- Transient errors resolve naturally

## Directory Structure

Recommended project layout:

```
samples/
  my_game.stasis       # Code (hot-swap on change)

data/
  my_game/
    levels.json        # Data (hot-reload on change)
    balance.json
    enemies.json

assets_src/
  my_game/
    player.svg         # Art (gfx_poll_reload on change)
    enemy.svg
```

## Performance Budget

Target: <50ms from file save to visible change

| Step | Budget | Notes |
|------|--------|-------|
| File watch check | <1ms | stat() syscall |
| File read | <5ms | Typical game data <100KB |
| JSON parse | <10ms | yyjson handles MB/s |
| Game reload logic | <10ms | Copy values to state |
| **Total** | <26ms | Well under 50ms target |

Buffer remaining for:
- Slow disks
- Large data files
- Complex reload logic

## Comparison to Code Hot-Reload

| Aspect | Code Hot-Reload | Data Hot-Reload |
|--------|-----------------|-----------------|
| Trigger | .stasis file change | .json file change |
| Processing | Compile → AOT → Link → Load DLL | Read → Parse |
| Time | 50-250ms | <50ms |
| State | Preserved via DLL symbol copy | Game re-reads values |
| Failure | Compilation error | Parse error + keep old |

## Open Questions

1. **Should data handles be i32 or opaque pointers?**
   - i32 matches sprite handles
   - Pointers would be more direct but break state serialization

2. **Global data registry vs. explicit handles?**
   - Current design: explicit handles stored in state
   - Alternative: `data_get("path", "key")` with implicit registry

3. **Hot-reload in release builds?**
   - Option: compile-time flag to disable watching
   - Option: always enable (small overhead)

4. **Nested file includes?**
   - Should `levels.json` be able to `#include "enemies.json"`?
   - Start simple: no includes, game code handles composition

## Next Steps

1. Add multi-file binding (levels + balance) and a simple merge/override convention.
2. Add better on-reload diagnostics (per-file: parse time + applied field count + errors).
3. Consider exposing a Stasis-level API (`data_watch`/`data_changed`) once there is a stable runtime import surface for it.
