# Brickout Defense / Brickout Revenge original reference

This port treats the 2016 published Flash build and the original `Graphics/TD`
artwork as authoritative. The earlier Stasis sample is implementation scaffolding,
not a gameplay specification.

## Recovered sources

- Published build: `RootbeerGames/games/brickoutRevenge/TowerDefense.swf`
- Published stage size: 660 x 550 at 60 FPS
- Original artwork: `C:/Users/Ben/OneDrive/Graphics/TD`
- Decompiled game code: 183 ActionScript classes, including all game-specific
  levels, balls, towers, projectiles, UI states, embedded fonts, and sounds

## Original audio

The published SWF contains the original background music plus Flixel/menu, life-loss,
curve-ball collision, and rocket explosion effects. The checked-in recovery tool preserves
the original compact MP3 music and exports the short effects as PCM WAV files under
`assets/original/audio/`; the port loops the original music and plays the effects at their
corresponding gameplay events.

Recovery output is intentionally kept outside this repository. Only the ported
Stasis implementation and redistributable game assets are committed here.

## Playfield and flow

- Playfield bounds are x=9..441 and y=15..463.
- The computer-controlled 133 x 19 paddle protects the top edge.
- Towers occupy 32 x 32 cells on a 16-pixel placement grid.
- Balls spawn in timed groups, bounce through the tower field, and cost a life
  when they reach the bottom. Fire balls do not cost a life.
- Towers acquire the first living ball in range and fire homing projectiles.
- A level is won after all spawn groups and living balls are exhausted; it is
  lost at zero lives.
- Kills award gold and score. Towers can be selected, upgraded through level 5,
  or sold for 80 percent of invested value.

## Original tower families

| Tower | Level-0 behavior |
| --- | --- |
| Wall | Cost 10, 25 HP, blocks balls, no projectile |
| Standard | Cost 5, 10 HP, 125 range, 10 damage, 3 second reload |
| Rapid | Cost 15, 10 HP, 125 range, 1 damage, 2.1 second reload |
| Healer | Cost 50, 10 HP, heals nearby towers every 15 seconds |
| Sniper | Cost 50, 10 HP, 200 range, 100 damage, 30 second reload |
| Rocket | Cost 30, 10 HP, 125 range, area damage, 10 second reload |
| Gold miner | Cost 50, 20 HP, produces gold while consuming its own HP |

## Original ball families

All base health and damage scale by `level / 15`.

| Ball | Base behavior |
| --- | --- |
| Basic | 80 HP, 2 damage, speed 50..60 |
| Fast | 80 HP, 1 damage, speed 80..90 |
| Slow | 80 HP, 3 damage, speed 30..40 |
| Curve | Basic speed with gradual steering |
| Swarm | Steers toward another living ball |
| Splitter | Creates two children at +/-15 degrees, up to three generations |
| Fire | Passes through towers, damaging each at most once per five seconds |
| Mover | Pushes a tower one half-cell before applying normal collision damage |

## Original playable content

- Campaign: Marathon; Split & Swarm
- Challenge: Test Level (gold miner variant and broad enemy showcase)
- Recovered but not exposed in the published campaign menu: Earthquake
- Main menu entries for former challenges and upgrades were placeholders.

## Port policy

- Preserve original coordinate system, economy, tower/ball statistics, level
  schedules, names, and special behaviors.
- Preserve deterministic tick behavior in Stasis; express original elapsed-time
  values as fixed 60 Hz tick counts.
- Use pointer/touch controls in addition to the original keyboard shortcuts so
  the same game is playable on desktop and Android.
- Use the original art and embedded fonts where the Stasis runtime supports them;
  provide vector/runtime-safe equivalents only where extraction is unsuitable.
