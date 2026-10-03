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
the first 60 ticks of measurements, then reports separate `tick` and `render`
roots. Function calls and inclusive/exclusive totals come only from committed
post-warmup frame samples; inclusive time includes callees, while exclusive
time subtracts nested calls to other profiled functions. JSON schema 2 also
includes per-frame inclusive/exclusive cost distributions (median, p95, max,
and total), whose samples are whole-frame function costs rather than individual
call latencies. A function not called in a committed frame contributes zero to
that frame's distribution. Times are nanoseconds; runtime phase times are
microseconds.

On desktop, `host_frames` contains only frames whose native performance-metrics
snapshot was published for the current submission. Guest tick/render rows share
those frame IDs. Headless runs can report committed guest tick/render samples
without host frame IDs; unsupported phases remain `null` with a zero available
sample count, never fabricated zero timings. `present_wait` is separate from
`frame_work`. The 1,200-sample cap applies to the shared function and frame
window. Missing rows warn about spelling, reachability, warmup, or inlining.

Repeat the same workload on the same machine and toolchain. Profiling adds
overhead: each selected helper invocation records a monotonic start/end and
updates per-frame counters, so very small hot helpers are perturbed most. The
report avoids per-call logging and computes distributions from bounded
per-frame aggregates. Compare like-for-like runs. CLI elapsed time also includes
compilation and startup, so use the profiler or HUD for tick timing.

`schema_version` identifies the current pre-1.0 export shape. Consumers should
update with producer changes; historical readers and migration compatibility
are not promised.

When `--screenshot` is enabled, the native runtime captures the requested frame
before the host-replay timer ends. Screenshot readback is therefore included in
that frame's `host_replay` and `frame_work` values and remains in the reported
window and distributions. Use matching screenshot settings when comparing
runs.
