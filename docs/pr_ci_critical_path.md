# PR CI critical path

PR CI keeps the required `test` check as a small aggregator. The four ordinary
Linux lanes it requires run independently so a slow Cargo lane does not wait
behind unrelated setup or another Cargo target:

| Lane | Ownership | Bound |
| --- | --- | ---: |
| `pr-ci-preflight` | source, runtime, web, native, and architecture checks | hosted job |
| `pr-ci-cargo-workspace` | workspace tests excluding `stasis` | 15 min |
| `pr-ci-cargo-stasis-library` | Stasis executable build and library tests | 15 min per command |
| `pr-ci-cargo-stasis-main` | all Stasis main-binary tests except the isolated provenance test | 15 min |
| `pr-ci-cargo-stasis-provenance` | the slow substituted-renderer provenance test | 15 min |
| `pr-ci-cargo-stasis-integration` | every file in `apps/stasis/tests/*.rs` | 15 min |

The aggregator fails unless every required lane reports `success`. It retains
the historical `test` job name for branch protection and runs with
`always()` so a failed lane cannot leave the required check pending. Draft pull
requests continue to skip the ordinary lanes.

The Stasis main-binary suite is split by the measured slow-test boundary. The
main lane skips exactly one fully-qualified test, while the provenance lane
runs that test with `--exact`; the two lanes therefore cover the original set
without overlap. The library lane retains the explicit Stasis executable build
because its runtime-launcher tests require that sibling binary.

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

Visual evidence: not applicable; this change is CI workflow timing and coverage
configuration.
