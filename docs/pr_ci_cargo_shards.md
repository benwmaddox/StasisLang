# PR CI Cargo shards

The required PR CI `test` job keeps the existing check name and runs three
bounded Cargo test steps. Each step has the repository's 15-minute timeout and
uses one test thread so the process-global JIT dispatch tables remain
deterministic.

| Shard | Cargo ownership |
| --- | --- |
| Workspace | `--workspace --exclude stasis --all-targets` |
| Stasis unit | `-p stasis --lib --bins` |
| Stasis integration | Every top-level `apps/stasis/tests/*.rs`, sorted and passed as `--test` targets |

The workspace shard excludes only `stasis` because the next two shards run that
package explicitly. The integration shard discovers the test files from the
checkout and fails if the list is empty, so adding a new integration target
does not silently remove it from PR coverage. No test filters, ignored-test
flags, or reduced test modes are used.

The three steps remain inside the historical `test` job, so existing branch
protection continues to require all of them through the same check.
