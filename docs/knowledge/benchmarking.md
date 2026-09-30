# Benchmarking ticks and functions

<!-- tags: benchmarking, tick, functions, profiling -->

In desktop development play, press **F3** to see `tick` time in the performance
HUD. Annotate `tick` with `@tick_budget_us(1000)` for a run summary of average
microseconds, p99, and overruns. Average covers the whole run; p99 uses the last
4096 samples.

To measure `tick` and selected functions, use the built-in profiler:

```text
stasis play --ticks 600 --profile-functions tick,update_enemies --profile-warmup 60 --profile-output build/profile.json
```

Replace `update_enemies` with a reachable function in your game. The run skips
the first 60 ticks of measurements, then reports completed calls, total
inclusive/exclusive time, average inclusive time, and maximum inclusive call time.
Inclusive time includes callees; exclusive time subtracts nested calls to
other profiled functions. JSON times are nanoseconds; the console labels its
units. Missing rows warn about spelling, reachability, warmup, or inlining.

Repeat the same workload on the same machine and toolchain. Profiling adds
overhead; compare like-for-like runs. CLI elapsed time also includes compilation
and startup, so use the profiler or HUD for tick timing.
