# PR CI critical path

PR CI has three declared jobs. The `test` job keeps its historical name for
branch protection and fails closed if either required lane did not succeed.

| Lane | Ownership | Bound |
| --- | --- | ---: |
| `pr-ci-preflight` | source, runtime, web, native, action-pin, and CI-policy checks | hosted job |
| `pr-ci-core-cargo` | Rust formatting, workspace tests excluding `stasis`, Stasis library/main tests, and fast architecture characterization | 15 min per command |
| `test` | fail-closed summary of the two required lanes | hosted job |

The core job builds the Stasis CLI once before the library/main tests and runs
the fast architecture gate afterward so its Cargo artifacts are reused. The
manifest check remains in preflight; the default-gate behavior tests run in the
core job. Node 24 and the VS Code test dependencies are installed in the core
job because the characterization command runs the VS Code protocol tests.

## Full validation and publication

`nightly-validation.yml` is a separate reusable workflow with no pull-request
trigger. It retains the former full CI suite: the complete Rust shards,
isolated provenance test harness, integration and web-package tests, browser
compiler/Chromium acceptance, Linux/Windows generics package matrix, Windows
platform seams, VS Code E2E, and Android package-link acceptance. Its final
summary requires every lane, including the platform and editor jobs, to report
success. Contributors can run it manually; `nightly-release.yml` calls it and
waits for success before publication.

Performance benchmarks and the full network/browser acceptance workflow are
also manual or nightly-only. Nightly release gates on both reusable workflows.
The package-time unit contract remains in preflight, so the benchmark workflow
does not run a duplicate contract job.

Hosted timing for this three-job layout has not yet been measured. The earlier
sharded-lane measurements describe the previous required-check topology and
should not be treated as a current PR runtime baseline.

Visual evidence: not applicable; this change is CI workflow timing and coverage
configuration.
