# Supervised live Tic-Tac-Toe fixture

This fixture exercises the ordinary live input and authenticated network paths. The
authority imports the four byte-identical Maddox & Friends modules recorded in
`provenance.json`; `main.stasis` only adapts host input, transport envelopes,
snapshots, and rendering around those rules.

The external peer consumes the private invite through the existing
`stasis-network-supervision-v1` stdin preamble. Its stdout is bounded JSONL and
contains no invite, path, resume credential, or free-form diagnostic. Receipts cover
the authenticated join, every command, admitted ACKs, a malformed checksum
rejection, a duplicate sequence rejection with unchanged revision/hash, and every
authoritative snapshot. A final `complete` receipt is emitted only after the peer's
independent 20-revision projection oracle has accepted the catalog snapshot.
The successful scenario retains exactly 43 canonical receipt lines across the eight
fixed event classes.

## Deterministic scenario

The control harness sends pointer down/up pairs through supervised live
`set_input_state`, with one `step` after each edge:

1. Select Tic-Tac-Toe at `(480, 95)`.
2. Win: host cells `0, 1, 2` at `(105,105)`, `(195,105)`, `(285,105)`;
   the peer interleaves guest cells `3, 4` and verifies their ACKs.
3. Request a rematch at `(480, 210)`; the peer accepts through command `19`.
4. Draw: host cells `0, 2, 6, 7, 5`; the peer interleaves `1, 3, 4, 8`.
5. The peer requests catalog return through command `20`; the host confirms at
   `(480, 317)`.

Every snapshot is one upstream envelope with 16 payload words: the exact 14-word
Tic-Tac-Toe snapshot, terminal-flow phase, and screen (`0` catalog, `1` game).

## Integration validation

After registering the example as `supervision_live_ttt_peer`, build it through the
repository cache wrapper:

```powershell
python tools/cargo_cache.py run -- cargo build -p stasis_network --example supervision_live_ttt_peer
```

The repository-owned integration script must stage a fresh authority copy and
stdlib, build the shared `network_guest.bundle` with the matching fresh Stasis CLI,
and start that source through the supervised live JIT launcher. It then drives the
pointer sequence above, asserts the JSONL receipts and safe control responses,
captures the terminal/catalog frames, and verifies bounded cleanup. Desktop AOT
authority packaging is not part of this live acceptance path. Each command remains
capped at 900 seconds.

The bounded end-to-end entrypoint performs that staging, control, receipt, and
capture validation. Omit executable parameters to build one fresh matching set
through `tools/cargo_cache.py`:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tests/fixtures/network_supervision/live_ttt/run_acceptance.ps1
```

Pass `-Toolchain`, `-Supervisor`, and `-Peer` together to validate an already-built
matching set without invoking Cargo. `-EvidenceRoot` may select an existing empty,
dedicated directory beneath `TEMP`; the run retains its PNGs, canonical peer
receipts, and `acceptance-summary.json` for review.

After the successful game, the entrypoint runs six bounded process cases with fresh
evidence directories: non-reading supervisor stdout, malformed harness JSONL, stdin
EOF, idle action timeout, forced peer exit, and forced authority exit. Every case
requires a nonzero supervisor exit within ten seconds and verifies that its recorded
authority and peer PIDs are gone. The retained case receipts contain numeric status
and PIDs only.
