# Benchmarking Stasis projects

<!-- tags: benchmarking, performance, inspect, hud, profiling -->

First choose the cost you want to measure. Compilation, simulation, rendering,
asset loading, recording/encoding, and package size have different paths.
Keep the project revision, vendored toolchain pin, target, capacity settings,
assets, machine, and run command with every result. Run `stasis vendor status`
and `stasis env` before a comparison so the input identity is reviewable.

## Establish correctness and storage cost

```text
stasis fmt --check
stasis check
stasis test
stasis inspect
stasis inspect --capacity state.enemies=512
```

`inspect` reports static state layout and projected capacity changes. It is
not a timing benchmark. Read totals and largest pools before tuning an
allocation policy. Compare the same named state paths across revisions.

## Measure the intended runtime path

For a headless simulation comparison, use a fixed tick count and identical
initial state and inputs:

```text
stasis run --headless --ticks 10000 --fast-forward
```

This command runs `main` and 10,000 ticks without `render` or graphics. A
wall-clock measurement of the whole CLI invocation includes startup and JIT
compilation. Report that as end-to-end time, not isolated tick time. Use an
external timer and repeat several runs; compare the median and the spread,
not a single fastest run. Separate a cold invocation from warmed filesystem
and OS cache runs. Do not change code, assets, capacity, or toolchain between
the two sides of a timing comparison.

For example, time `stasis check` with `Measure-Command { stasis check }` in
PowerShell or `/usr/bin/time -p stasis check` on Linux/macOS. Check the command's
exit status for every trial. Time the headless run the same way, but report it
as end-to-end compile-plus-simulation time.

For graphics and asset cost, measure the graphical `play` path on the same
hardware and toggle the development performance HUD with **F3** on desktop
or Web (three fingers on mobile). Compare `tick`, `guest render`, `host
replay`, and `frame work`; available backends also show render preparation,
GPU submission, and workload counts. `present wait` is separate from active
frame work. An unavailable phase is not a measured zero, and the HUD's worst
value covers only the recent five seconds. Release packages omit the HUD. A
`stasis record` run is useful for reproducible rendered output, but its elapsed
time includes capture, PNG work, and possibly FFmpeg encoding; do not label
it game frame time. A replay can fix the input sequence for a rendering
comparison only if that game stays within replay's supported observations.

## Make the result explainable

Record the exact command, release ID, source revision, target, tick/frame
count, first-run versus warmed-run status, machine, and median/range of
measurements. Save correctness results beside the timing data. When the
question is a regression, measure the same scenario before and after and
state the percent change. A faster `check` can coexist with slower runtime;
keep the metrics separate. For visible changes, attach and inspect a PNG or
MP4 from the same scenario.
