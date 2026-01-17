# Brickout Revenge - Game Design Document

## Overview
A hybrid auto-battler/tower defense + breakout game where the player builds defensive brick formations and the computer controls the paddle and balls trying to break through.

**Core Concept**: Set up your defenses, then watch the battle unfold. No runtime control - pure strategy.

## Game Flow
1. **Setup Phase**: Player places bricks/towers within budget
2. **Battle Phase**: Computer-controlled paddle and balls attack
3. **Resolution**: Win if bricks survive, lose if all destroyed
4. **Progression**: Earn currency, unlock upgrades, face harder waves

## Technical Constraints
- `MAX_BRICKS = 50` - Maximum bricks on field
- `MAX_BALLS = 30` - Maximum balls in play
- `MAX_EFFECTS = 100` - Visual effects pool
- Seeded randomization for replays
- Fixed timestep physics
- Virtual resolution: 360x720; bottom 96 units reserved for a slide-up UI menu (gameplay uses y in [0, 624]). Window size is configurable and the game scales to fit while preserving aspect ratio.
- The v1 sample attempts to resize the window to best fit the desktop (either full height or full width) while preserving the virtual aspect ratio and staying >= the virtual size.
- Brick grid: 6x6; placement requires bricks fully inside the gameplay area.

## Data Files (v1 sample)
- `samples/brickout_revenge/data/config.json` is used for data binding (hot-reload) in the Cranelift runner.
- Brickout v1 level scripts can be provided via JSON by setting:
  - `brickout_levels_magic` to a nonzero value (otherwise the sample falls back to compiled-in defaults).
  - `brickout_level_name_0`/`brickout_level_name_1`/`brickout_level_name_2` (UTF-8 strings).
  - `brickout_level_initial_scraps` / `brickout_level_initial_power_cap` (arrays of 3).
  - `brickout_level_event_tick` / `brickout_level_event_preset` (flat arrays of 192 with fixed partitions: offsets 0, 64, 128).
  - Preset encoding: 0=Normal, 1=Heavy, 2=Splitter.

## Core Systems

### Physics
- Ball movement with velocity vectors
- Brick collision with bounce angles
- Paddle AI tracks balls
- Edge bouncing (top, left, right walls)

### Brick Types (Tower Defense)

#### Starter Branches (3)
1. **Basic Bricks** - Standard durability, cheap
2. **Armored Bricks** - High HP, slow to destroy
3. **Reflector Bricks** - Redirect ball angles

#### Unlockable Branches (2)
4. **Explosive Bricks** - Damage nearby balls on destruction
5. **Regenerating Bricks** - Slowly heal over time

### Ball Types (Computer Side)

#### Starter Branches (3)
1. **Normal Ball** - Standard speed and damage
2. **Heavy Ball** - Slow but high damage
3. **Splitter Ball** - Splits into 2 on brick hit

#### Unlockable Branches (2)
4. **Piercing Ball** - Goes through bricks (limited)
5. **Homing Ball** - Slight attraction to bricks

### Paddle AI
- Tracks nearest ball
- Speed scales with difficulty
- Can miss intentionally at low difficulty

## Progression System

### Currency
- Earned per wave survived
- Bonus for bricks remaining
- Spent on upgrades and new brick types

### Difficulty Scaling
- More balls per wave
- Faster balls
- Smarter paddle AI
- Special ball types appear

### Seasonal Buffs
Environmental modifiers that change each "season" (every N waves):
- **Spring**: Balls move slower
- **Summer**: Bricks take more damage
- **Autumn**: Random ball direction changes
- **Winter**: Paddle moves slower

## Visual Design
- Clean line-based graphics (fits Stasis graphics system)
- Bricks as rectangles with type-specific colors
- Balls as circles
- Paddle as thick line at bottom
- Effects for hits, explosions, power-ups

### Color Scheme
- Basic Brick: White
- Armored Brick: Blue
- Reflector Brick: Yellow
- Explosive Brick: Red
- Regenerating Brick: Green
- Balls: Magenta/Pink
- Paddle: Cyan

## Data Structures

```
struct Vec2 { x: f32, y: f32 }

struct Ball {
    pos: Vec2
    vel: Vec2
    radius: f32
    damage: i32
    ball_type: i32
    active: bool
}

struct Brick {
    pos: Vec2
    width: f32
    height: f32
    hp: i32
    max_hp: i32
    brick_type: i32
    active: bool
}

struct Paddle {
    x: f32
    y: f32
    width: f32
    height: f32
    speed: f32
}

struct GameState {
    bricks: Brick[50]
    balls: Ball[30]
    paddle: Paddle
    score: i32
    wave: i32
    phase: i32  // 0=setup, 1=battle, 2=win, 3=lose
}
```

## Implementation Phases

### Phase 1: Core Mechanics
- Ball movement and bouncing
- Brick collision detection
- Paddle AI movement
- Basic game loop

### Phase 2: Game Flow
- Setup phase (place bricks)
- Battle phase (auto-play)
- Win/lose conditions
- Wave progression

### Phase 3: Brick Variety
- Implement all 5 brick types
- Visual differentiation
- Special abilities

### Phase 4: Ball Variety
- Implement all 5 ball types
- Ball spawning logic
- Difficulty scaling

### Phase 5: Polish
- Particle effects
- Score display
- Seasonal system
- Sound (if available)
