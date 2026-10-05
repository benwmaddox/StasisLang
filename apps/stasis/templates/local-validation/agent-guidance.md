
## Local validation

Install PowerShell 7 (`pwsh`) and run `pwsh -NoProfile -File tools/local-validation.ps1` from the
directory containing `stasis.json` before pushing. This resolves and verifies the exact pinned
Stasis release, runs format, check, tests, configured deterministic helpers, and a fresh Web
package, then records a receipt under `build/local-validation`. The pre-push hook requires a clean
tree and runs the same gate for pushes that point exactly at `HEAD`. These hooks are local checks
and can be bypassed with `git commit --no-verify` or `git push --no-verify`; remote-side rules, if
configured, decide whether a push is accepted.

The pre-commit hook may use a `development` or `local-*` Stasis build for formatting when its
vendor status proves the project's exact current hash-version-2 vendor snapshot. This format-only
identity does not satisfy the full local gate or pre-push hook; those still require the exact
official nightly release recorded in `stasis.json`.

After cloning, activate the checked-in hooks once with `git config core.hooksPath .githooks`.
Project creation and default gate resolution do not download a toolchain; pass `-RestoreToolchain`
only when you explicitly want the generated release restore helper to run.

Add repository-specific deterministic tests or package checks in `tools/local-validation.json`;
keep their commands repeatable and scoped to this project.
