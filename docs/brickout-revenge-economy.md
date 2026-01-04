# Brickout Revenge: Economic System Design

## Overview

This document explores adding an Age of Empires 2-inspired economic layer to Brickout Revenge. The core tension: **you can't just build defenses**. You must balance resource generation, defensive structures, and offensive capabilities to survive escalating waves.

Design reference:
- `docs/brickout-revenge-brainstorm.md` for the broader game concept, layout targets, and monetization assumptions.

---

## Current State

- 50 bricks spawn automatically in a grid
- 3 brick types: Basic (balanced), Armored (slow/strong), Reflector (fast/disruptive)
- No resource management - everything is free
- Waves scale in ball HP, damage, and count
- Win condition: survive (currently undefined)

---

## Design Philosophy

### The AoE2 Triangle

```
        ECONOMY
          /\
         /  \
        /    \
       /      \
    DEFENSE--OFFENSE
```

In AoE2:
- **Economy** (villagers, farms, trade) enables everything
- **Defense** (walls, towers, castles) protects economy
- **Offense** (military) threatens enemy economy and ends games

### Translated to Brickout Revenge

| AoE2 Concept | Brickout Equivalent |
|--------------|---------------------|
| Villagers gathering | Harvester bricks extracting from resource balls |
| Farms/mines | Resource-generating brick types |
| Walls/towers | Defensive bricks (current implementation) |
| Military units | Special attack bricks, ball modifiers |
| Trade | Risk/reward mechanics with special bricks |

---

## Resource System

### Option A: Single Currency (Simpler)

**Energy** - Universal resource from destroyed balls and harvester bricks.

Pros:
- Easy to understand
- Single economy to balance
- Faster decision-making

Cons:
- Less strategic depth
- No meaningful trade-offs between resource types

### Option B: Dual Currency (Recommended)

**Scrap** - Raw materials from ball destruction
- Gained when any brick destroys a ball
- Base amount: 1 scrap per ball kill
- Bonus: +1 if overkill (ball HP was low)

**Power** - Energy from special sources
- Generated passively by Generator bricks
- Gained from Power Orb balls (special spawn)
- Required for advanced brick abilities

**The Balance:**
- Basic structures cost only Scrap
- Advanced structures cost Scrap + Power
- Upgrades require accumulated Power
- Power generation takes brick slots away from combat

### Option C: Triple Resource (Complex)

**Scrap** (materials), **Power** (energy), **Tech** (research points)

Only recommended if adding a tech tree with unlockable brick types.

---

## New Brick Types for Economy

### Tier 1: Economic Foundation

#### Harvester Brick
```
Cost: 50 Scrap
HP: 15 (low)
Range: 100 (short)
Ability: Extracts 3 Scrap per ball hit (doesn't damage balls)
Weakness: Cannot attack - purely economic
```
**Strategic Role:** Early game economy. Vulnerable, must be protected.

#### Generator Brick
```
Cost: 80 Scrap
HP: 20
Range: 0 (no attack)
Ability: +1 Power per 5 seconds passively
Weakness: Takes a slot, produces nothing in combat
```
**Strategic Role:** Late game scaling. Invest now, benefit later.

### Tier 2: Hybrid Units

#### Salvager Brick
```
Cost: 100 Scrap, 20 Power
HP: 25
Range: 130
Ability: Normal attack + 50% bonus Scrap from kills
```
**Strategic Role:** Bridges economy and defense.

#### Reactor Brick
```
Cost: 120 Scrap, 40 Power
HP: 30
Range: 150
Ability: Attacks deal splash damage; killed balls explode for +2 Scrap each
Weakness: Slow cooldown (400ms)
```
**Strategic Role:** Area control with economic bonus.

### Tier 3: Specialists

#### Trade Post Brick
```
Cost: 150 Scrap
HP: 35
Ability: Convert 10 Scrap → 3 Power (manual activation during setup)
```
**Strategic Role:** Resource conversion for late game pivots.

#### Vault Brick
```
Cost: 200 Scrap, 30 Power
HP: 50 (very high)
Range: 0
Ability: Stores up to 100 bonus Scrap; if destroyed, lose stored resources
```
**Strategic Role:** Risk/reward banking. Protects surplus but vulnerable.

---

## New Ball Types (Resource-Related)

### Resource Balls (Spawn Naturally)

| Ball Type | Appearance | Behavior | Reward |
|-----------|-----------|----------|--------|
| Standard | White | Normal | 1 Scrap |
| Rich | Gold shimmer | Slower, more HP | 5 Scrap |
| Power Orb | Blue glow | Fast, low HP | 3 Power |
| Volatile | Red pulse | Explodes on death (damages adjacent bricks) | 8 Scrap |
| Armored | Steel texture | High HP, low damage | 2 Scrap |

### Threat Balls (Punish Greed)

| Ball Type | Appearance | Behavior | Threat |
|-----------|-----------|----------|--------|
| Thief | Dark purple | Steals 5 Scrap if hits paddle | -5 Scrap |
| Drainer | Cyan | Damages Generator bricks 2x | Anti-economy |
| Swarm Leader | Glowing core | Spawns 2 mini-balls on bounce | Overwhelm |

---

## Wave Economy Scaling

### Early Game (Waves 1-5)
- Mostly Standard balls
- Occasional Rich ball (10% chance)
- Player establishes economy
- Recommended: 2-3 Harvesters, rest Basic bricks

### Mid Game (Waves 6-15)
- Power Orbs begin spawning (15% chance)
- Thief balls introduced (5% chance)
- Volatile balls appear (10% chance)
- Pressure to build advanced structures

### Late Game (Waves 16+)
- Swarm Leaders appear
- Drainers target economy
- Rich balls more common but so are threats
- Economy must sustain constant rebuilding

---

## Setup Phase Economy

### Between-Wave Budget

After each wave:
1. **Income Phase:** Collect accumulated resources
2. **Damage Report:** See which bricks were destroyed
3. **Build Phase:** Place new bricks, upgrade existing
4. **Ready Check:** Confirm to start next wave

### Building Costs (Recommended Starting Values)

| Brick | Scrap Cost | Power Cost | Notes |
|-------|------------|------------|-------|
| Basic | 30 | 0 | Starter unit |
| Armored | 60 | 0 | Tank |
| Reflector | 45 | 10 | Utility |
| Harvester | 50 | 0 | Economy |
| Generator | 80 | 0 | Investment |
| Salvager | 100 | 20 | Hybrid |
| Reactor | 120 | 40 | AOE |
| Trade Post | 150 | 0 | Conversion |
| Vault | 200 | 30 | Banking |

### Upgrade System

Each brick can be upgraded once (level 2):
- +50% HP
- +25% range
- -20% cooldown
- Upgrade cost: 50% of original Scrap cost + 10 Power

---

## Balance Considerations

### The "Turtle Problem"

**Risk:** Players build only economy + defense, never progress.

**Solutions:**
1. Wave timer - limited time before balls auto-spawn
2. Escalating ball HP outpaces pure defense
3. Generator cap (max 5) prevents infinite scaling
4. Threat balls specifically counter passive play

### The "All-In Problem"

**Risk:** Players spend everything immediately, lose to attrition.

**Solutions:**
1. Starting resources are limited (100 Scrap, 0 Power)
2. Brick repair costs resources (can't just rebuild for free)
3. Vault mechanic rewards saving
4. Power gates advanced content

### Economic Breakpoints

Target resource curves per wave:

| Wave | Expected Income | Recommended Spend | Reserve |
|------|-----------------|-------------------|---------|
| 1-3 | 50-80 Scrap | 40-60 | 20+ |
| 4-6 | 100-150 | 80-120 | 30+ |
| 7-10 | 150-250 | 120-200 | 50+ |
| 11-15 | 250-400 | 200-350 | 80+ |
| 16+ | 400+ | Variable | 100+ |

### The AoE2 "Boom" vs "Rush" Decision

Create two viable strategies:

**Economic Boom:**
- Heavy Harvester/Generator investment early
- Minimal combat bricks
- Survive waves 1-5 with less defense
- Dominant mid-game economy
- Risk: Lose to early aggression (tough waves)

**Combat Rush:**
- All Basic/Armored bricks
- No economic structures
- Strong early defense
- Struggles late game without income
- Risk: Lose to attrition

**Balanced:**
- 60% combat, 40% economy
- Steady progression
- Never dominant but never weak
- The "safe" option

---

## Fun Factor Analysis

### What Makes AoE2 Economy Fun?

1. **Meaningful choices** - Every villager placed is a unit not trained
2. **Visible accumulation** - Watch numbers grow
3. **Risk of loss** - Raids threaten economy
4. **Comeback potential** - Strong eco can recover from losses
5. **Multiple viable strategies** - Boom, rush, timing attacks

### Applying to Brickout Revenge

1. **Every brick slot matters** - 50 max means real trade-offs
2. **Resource counters on screen** - Satisfying to see Scrap pile up
3. **Threat balls can steal resources** - Creates tension
4. **Harvesters can rebuild after losses** - Economy enables comebacks
5. **Rush vs Boom paths** - Multiple ways to win

### "Juice" Suggestions

- **Scrap collection VFX:** Particles fly to counter when balls die
- **Power pulse:** Generator bricks glow when producing
- **Income summary:** End-of-wave screen shows resource breakdown
- **Upgrade fanfare:** Visual/audio feedback for leveling bricks
- **Low resource warning:** Screen edge flashes when below 20 Scrap

---

## Implementation Phases

### Phase 1: Core Economy
- Add Scrap resource
- Bricks cost Scrap to place
- Balls drop Scrap on death
- Setup phase with simple UI

### Phase 2: Economic Bricks
- Harvester brick
- Generator brick
- Power resource
- Upgrade system (level 2 only)

### Phase 3: Risk/Reward
- Threat balls (Thief, Drainer)
- Volatile balls
- Rich balls and Power Orbs
- Vault brick

### Phase 4: Polish
- Visual feedback for all economy actions
- Balance tuning based on playtest
- Additional brick types if needed
- Win condition (survive wave X, reach Y resources)

---

## Open Questions

1. **Brick placement grid:** Keep fixed grid or allow free placement?
2. **Repair mechanic:** Can damaged bricks be healed, or only replaced?
3. **Carry-over:** Do resources persist between games (meta-progression)?
4. **Difficulty modes:** Scale starting resources or wave intensity?
5. **Multiplayer potential:** Competitive economy racing?

---

## Summary

The economic layer transforms Brickout Revenge from a pure defense game into a strategic resource management experience. By forcing players to balance between income generation (Harvesters/Generators), defense (combat bricks), and investment (upgrades/Vaults), we create meaningful decisions every wave.

The key insight from AoE2: **economy is interesting because it competes for the same resources as military**. In Brickout Revenge, brick slots are the limiting factor. You can't have 50 Generators AND 50 combat bricks. That tension is where strategy lives.

**Recommended starting point:** Implement Phase 1 with just Scrap, test the setup phase flow, then layer in complexity.
