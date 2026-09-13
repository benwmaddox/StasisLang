# PR CI critical path

PR CI keeps the required `test` check as a small aggregator. The seven ordinary
Linux lanes it requires run independently so a slow Cargo lane does not wait
behind unrelated setup or another Cargo target:

| Lane | Ownership | Bound |
| --- | --- | ---: |
| `pr-ci-preflight` | source, runtime, web, native, and architecture checks | hosted job |
| `pr-ci-cargo-workspace` | workspace tests excluding `stasis` | 15 min |
| `pr-ci-cargo-stasis-library` | Stasis executable build and library tests | 15 min per command |
| `pr-ci-cargo-stasis-main` | Stasis executable build plus all main-binary tests except the isolated provenance test | 15 min per command |
| `pr-ci-cargo-stasis-test-harness` | compile and publish the immutable Stasis main test harness | 15 min |
| `pr-ci-cargo-stasis-provenance` | run the slow substituted-renderer provenance test from the published harness | 15 min |
| `pr-ci-cargo-stasis-integration` | every file in `apps/stasis/tests/*.rs` | 15 min |

The aggregator fails unless every required lane reports `success`. It retains
the historical `test` job name for branch protection and runs with
`always()` so a failed lane cannot leave the required check pending. Draft pull
requests continue to skip the ordinary lanes.

The Stasis main-binary suite is split by the measured slow-test boundary. The
main lane skips exactly one fully-qualified test, while the provenance lane
runs that test with `--exact`; the two lanes therefore cover the original set
without overlap. The library and main lanes each retain an explicit Stasis
executable build because their desktop-editor runtime-launcher tests require
that sibling binary. The builds are intentionally local to their lanes because
hosted jobs do not share Cargo targets.

The provenance lane depends only on the independent test-harness lane. That
lane compiles the full main test target once and publishes the resulting test
executable; provenance downloads and runs that immutable executable directly.
This removes the roughly three-minute clean compile from the 15-minute test
step while still executing the exact slow test once and leaving the remaining
main-binary tests in the main lane.

## Baseline

These are five recent successful hosted runs from the serial workflow. The
elapsed value is the run `createdAt` to `updatedAt` interval, including the
ordinary `test` job and the skipped optional jobs.

| Run | Head | Elapsed |
| ---: | --- | ---: |
| [34728407382](https://github.com/benwmaddox/StasisLang/actions/runs/34728407382) | `f82b1fcb` | 26:21 |
| [34728336434](https://github.com/benwmaddox/StasisLang/actions/runs/34728336434) | `600e4d95` | 26:23 |
| [34727481566](https://github.com/benwmaddox/StasisLang/actions/runs/34727481566) | `26540865` | 18:00 |
| [34726710407](https://github.com/benwmaddox/StasisLang/actions/runs/34726710407) | `6f3e27dd` | 26:49 |
| [34726181390](https://github.com/benwmaddox/StasisLang/actions/runs/34726181390) | `74ffa387` | 26:36 |

Sorted baseline: 18:00, 26:21, 26:23, 26:36, 26:49. Median: 26:23.
The conservative five-run p95 bound is 26:49.

The slowest sampled run spent about 12 minutes in Stasis unit Cargo tests and
about 7 minutes in Stasis integration Cargo tests after the preceding checks.
Those two independent lanes define the expected post-change critical path;
hosted follow-up measurements are recorded here after the split stabilizes.

## Post-change measurements

These are five successful hosted repetitions after the split and compiled
provenance-harness repair. GitHub keeps reruns under the same workflow run URL;
the rows below are attempts 1 through 5 of
[34733590591](https://github.com/benwmaddox/StasisLang/actions/runs/34733590591).
Elapsed time is the workflow start to the required `test` aggregator finish.

| Attempt | Workflow start (UTC) | Aggregator finish (UTC) | Elapsed |
| ---: | --- | --- | ---: |
| 1 | 2026-09-13 02:39:40 | 2026-09-13 02:52:54 | 13:14 |
| 2 | 2026-09-13 02:53:36 | 2026-09-13 03:06:04 | 12:28 |
| 3 | 2026-09-13 03:07:27 | 2026-09-13 03:20:29 | 13:02 |
| 4 | 2026-09-13 03:21:17 | 2026-09-13 03:34:18 | 13:01 |
| 5 | 2026-09-13 03:35:35 | 2026-09-13 03:50:55 | 15:20 |

Sorted post-change samples: 12:28, 13:01, 13:02, 13:14, 15:20. Median:
13:02. The conservative five-run p95 bound is 15:20, satisfying the required
median <=15 minutes and p95 <=20 minutes. Every required lane and the
fail-closed aggregator passed in all five repetitions. Coverage remains
complete: the main-binary suite still runs in full across the main lane and
the exact isolated provenance test, with the latter executed from the
published test harness rather than compiled a second time.

Visual evidence: not applicable; this change is CI workflow timing and coverage
configuration.
