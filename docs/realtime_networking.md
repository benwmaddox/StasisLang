# Realtime networking contract

Realtime games use the Rust `stasis_network::realtime` contract. Turn-based
games continue to use their existing command messages and are not routed
through this module.

## Rates and the transport boundary

Each session declares independently bounded, positive simulation, presentation,
and raw-control sampling rates plus a positive input delay in simulation ticks.
A typical contract is 60 Hz simulation, 120 Hz presentation, 20 Hz control
sampling, and a three tick authoritative delay; 60/30/120 is also valid.
Presentation may interpolate between completed simulation states, but it never
mutates simulation state and it cannot turn a late frame into a gameplay tick.

`ControlEnvelope` is a versioned, bounded `RTC1` payload. It contains scheduled
transitions with `(seat, epoch, sequence, apply_tick, state)` identity and can
be sent as bytes through the existing production network message envelope.
The transport remains responsible for delivery; the realtime session is
responsible for validation, deduplication, ordering, and tick application.
No new socket or game-specific transport is introduced. Stable native
`stasis_realtime_*` entrypoints and `realtime_controls.stasis` let JIT and AOT
guests build and submit RTC1 payloads, advance/read controls, apply corrections,
attach hashes, inspect resync state, and drive lifecycle transitions. Snapshot
tokens and replay callbacks remain Rust-host responsibilities.

The guest ABI uses signed 32-bit integers. Guest-driven sessions therefore stop
without mutation at tick `i32::MAX`, and guest-visible epochs are capped at
`i32::MAX`. RTC1 bytes cross the guest ABI as bounded `i32` array elements in
the range 0-255 so JIT and native AOT use an identical collection layout. JIT
adapters reject undersized registered arrays before reading or writing them;
native pointer callers must provide the declared capacity. Authoritative hashes
use unsigned low/high `i32` lanes to preserve all 64 bits.

## Tick and admission rules

The caller submits controls separately from `advance_tick`. A transition must
be at or after `earliest_apply_tick` and no later than `latest_apply_tick` (the
bounded future horizon); its state is held until another accepted transition
for that seat is applied. A neutral transition is an ordinary, first-class
release and is applied at its exact scheduled tick.

`advance_tick` increments exactly one simulation tick or reports tick-space
exhaustion without mutation. It does not wait for a packet, a presentation
frame, or another seat. Due transitions are applied in stable
`(seat, epoch, sequence, apply_tick)` order. Missing controls keep their last
state. Admission outcomes are deterministic: accepted, accepted
reordered, duplicate, stale, conflict, late, too-far, malformed, or full.
For a multi-transition guest payload, every transition is processed and the
return code reports the highest-precedence outcome (resync, conflict, full,
malformed, inactive, stale, late, too-far, duplicate, reordered, accepted).
Sequence identity is retained through bounded recent history so retransmission
is safe without making the queue unbounded. If two conflicting variants for a
pending `(seat, epoch, sequence)` arrive, the session quarantines that identity and
removes the pending transition; both arrival orders therefore converge to no
control at that tick. A conflict discovered after application is reported as
late/stale and never rewrites simulation history.

## Lifecycle and authority

Disconnect and reconnect neutralize that seat, advance its epoch, and discard
its queued controls; disconnected seats reject input until reconnect. Pause
and focus loss neutralize every seat, advance every epoch, and discard the
pending queue. Rematch does the same and starts the simulation tick counter at
zero. Epoch increments are preflighted before any lifecycle mutation. Sequence
floors, recent identities, and quarantines reset with the new epoch; packets
from the previous epoch remain stale regardless of their sequence. An
authoritative snapshot replaces the tick, persistent controls,
per-seat epochs/activity, and sequence floors, then discards pending, recent,
and quarantined transitions. Monotonic snapshot revisions prevent an older
snapshot from rewinding accepted authority.

In host-authoritative mode, the host applies the contract and sends snapshots
or corrections; clients may display a prediction but must converge to the
authoritative snapshot. In deterministic-peer mode, every peer must use the
same validated rates, delay, transition ordering, integer state rules, and
lifecycle decisions. A lost transition must be recovered before its due tick;
after due, deterministic peers cannot retroactively repair history and must
restart/correct from authority. Host-authoritative mode uses a snapshot
correction after due. A hash mismatch is a correction signal, not permission
to stall the tick loop.

Snapshots are accepted only when their tick leaves room for the future horizon,
their revision and per-seat epochs do not regress or reach exhausted `u32::MAX`,
and all inactive seats are neutral. Sequence floors are adopted exactly, so
old packets remain stale after recovery.

## Replay and rendering

The bounded replay log records accepted scheduled transitions, quarantine
decisions, every simulation tick, and caller-supplied post-tick authoritative
hashes. Call `record_authoritative_hash(tick, hash)` exactly once immediately
after the corresponding `advance_tick`; wrong-tick, duplicate, and
cross-lifecycle attachment is rejected. `ReplayLog::replay` requires one hash
for every tick and calls a game-owned hash function against each exact
post-tick control snapshot. Overflow marks a log incomplete while simulation
continues; incomplete logs cannot replay. Sessions use a bounded default replay
capacity and may select an explicit capacity up to `MAX_REPLAY_RECORDS`.
Lifecycle events and authoritative snapshot payloads are replayed through a
game-owned callback so non-control world state can be restored deterministically.
Rendering/interpolation is outside the log and outside state mutation; a render
pass must only project the latest completed simulation state.
