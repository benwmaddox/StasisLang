# Repository Guidelines

## Project Structure & Module Organization
- `docs/spec.md` is the canonical language spec.
- `docs/live-compilation-prd.md` is the canonical product/architecture requirements document.
- `docs/build_checklist.md` is the execution plan; keep slice ordering and temporary migration details there.
- `crates/stasis_compiler` hosts Rust compiler substrate/bindings called by Stasis orchestration.
- `crates/stasis_jit` hosts Cranelift integration for JIT (dev) and AOT (prod), function pointer table integration, and code generation memory management.
- `crates/stasis_runner` hosts tick loop, swap sequencing, and commit orchestration.
- `apps/stasis` is the single in-process graphical runner app.
- `src/stdlib/` contains Stasis standard library modules.
- `samples/brickout_revenge/` is the primary end-to-end sample target.
- `tests/rust/` contains host-side Rust tests. Add deterministic `.stasis` fixtures under `tests/` when needed.

## Build, Test, and Development Commands
- Primary implementation toolchain is Rust/Cargo.
- Use:
- `cargo build`
- `cargo test`
- `cargo run -p stasis --release -- --ticks 300 --watch-dir samples/brickout_revenge`
- Use `rg` for search (`rg pattern path`, `rg --files`).
- Keep commands deterministic and scriptable.
- Validation entrypoint:
- `tools/validate_repo.sh`

## Coding Style & Naming Conventions
- Keep files ASCII unless a file already uses non-ASCII and there is a clear reason.
- Prefer short, lowercase, snake_case file/module names.
- Keep comments brief and only for non-obvious behavior.
- Follow spec syntax and semantics:
- Arithmetic/comparison are infix only (`+ - * / %`, `< <= > >= == !=`).
- Assignment is infix (`=`, `+=`, `-=`, `*=`, `/=`, `%=`).
- Method-style arithmetic/comparison forms are removed.
- Receiver-form call style is preferred (`enemy.damage(5)`), function-form remains supported (`damage(enemy, 5)`).
- Conversion helpers:
- `from_*` are mutating target operations (statement-style side effects).
- `to_*` are pure conversions (expression-safe).

## Testing Guidelines
- Ship work in feature slices from `docs/build_checklist.md` and include tests in the same PR.
- Only implement changes that map to active items in `docs/build_checklist.md`, or to inbox-synced PR review feedback in `docs/bugs.md`; if a proposed change is outside those sources, pause and ask before changing requirements or code.
- Prefer deterministic, isolated tests with explicit expected output/state.
- If test can reasonably be written in stasis for stasis code, do so. It can be in a .test.stasis file next to the .stasis file.
- Cover parser/semantics/lowering/JIT boundaries and hot-swap safety behavior.
- Keep each test command bounded to 5 minutes max (300 seconds); split/shard runs when needed, and treat overruns as stability regressions.
- After each edit/test step, check for lingering test processes (for example `target/debug/deps/*.exe`) and clean them up before the next step.
- For incremental compilation:
- Validate file-level invalidation correctness.
- Validate per-function gating behavior.
- Validate unchanged-function cache reuse.
- For hot swap:
- Validate all-or-nothing commit.
- Validate rejection paths preserve old code/data.
- Validate `on_code_swap` failure abort behavior.

## Commit & Pull Request Guidelines
- Use short imperative subjects; Conventional Commits are preferred (`feat:`, `fix:`, `docs:`, `test:`).
- Reference affected spec/PRD/checklist sections when relevant.
- Keep PRs scoped to one slice group where possible (exact grouping/sequencing lives in `docs/build_checklist.md`).
- Each PR should include:
- behavioral summary
- tests added/updated
- docs updates
- explicit removal of obsolete paths introduced during the slice

## Architecture & Design Notes
- Single OS process runtime with in-process compiler and Cranelift JIT for development.
- Production build target uses Cranelift AOT output.
- File-level correctness is primary; semantic analysis runs for full changed file.
- Per-function semantic hashes gate backend work only.
- Hot swap is a two-phase model:
- background compilation to produce pending patch
- commit between ticks on main thread
- Dispatch boundary is stable indirection: `FnId -> code_ptr`.
- Swap safety rules:
- reject on layout/signature incompatibility
- reject on `on_code_swap` failure
- no partial commits
- preserve deterministic tick-based semantics; avoid `dt`-driven gameplay progression in Stasis logic.

## Language Ownership Rules
- Rust owns compiler implementation, host/runtime boundary, platform integration, Cranelift embedding, pointer-table commit mechanics, and process/watch plumbing.
- `.stasis` owns user code, stdlib, and samples.
- Use C only when unavoidable for platform-level bindings.

## Compiler Slice Process (Active)
- Keep the frontend parser hardcoded with explicit precedence handling and shared matcher helpers; avoid adding new ad-hoc token offset chains.
- Prefer one-pass compiler flow by default (`parse/check/lower` in one forward path per function); only allow explicit exceptions for required pre-scan metadata and jump backpatch resolution.
- Treat function/struct reachability pruning as the primary dead-code mechanism for this phase.
- Reachability roots are `main`, `tick`, and `on_code_swap` when present, plus host-required exported entry symbols.
- Build and maintain a simple call graph and type-reference graph; lower only reachable functions and reachable struct metadata.
- Do not add new parser-shape fallback detectors; replace/delete detector-driven paths instead of expanding them.
- Do not add temporary compiler fallbacks that fake behavior (for example hash-stub returns, hardcoded placeholder values, or "temporary" alternate compile paths).
- If a path is not yet truly implemented, fail with a deterministic diagnostic instead of emitting fake semantics.
- Size compiler slices small enough that every newly claimed feature path is real, end-to-end testable, and verified in JIT/AOT as applicable.
- Keep lowering state compact and validated: assert invariants at statement/function boundaries (`value stack`, `block depth`, `pending jumps`) and fail deterministically on violations.
- Use explicit jump-list backpatching with bounded limits and overflow diagnostics for control-flow emission.
- Add only a tiny local post-emit cleanup pass before Cranelift handoff (no broad optimizer track in this phase).
- Deduplicate constants with a simple semantic cache and keep scoped symbol lookup in hashed stacks; do not introduce packed type/state encodings at this stage.
- Keep diagnostic/instrumented behavior on the same pipeline (extra checks/tracing only), not a second compilation path.
- Keep commits narrow and slice-scoped: avoid mixing reachability/lowering changes with unrelated backend/runtime work in the same commit.
- End each slice with a cruft pass on touched files and aggressively remove code paths that no longer conform to the active reachability-first approach.
- Keep test runs bounded and deterministic: each command must stay within 5 minutes (300 seconds), and lingering test/compiler processes must be checked/cleaned after each step.
- Compiler feature-slice completion gate: each slice must include at least one representative sample program that goes end-to-end through the compiler pipeline to Cranelift IR, is built into an executable, is run, and has its behavior verified by test assertions.
- If a slice cannot yet pass that end-to-end executable verification path, the slice is not complete.
- After each code change, run a quick simplicity review on the touched code and simplify again if a more direct version is possible.

## Contributor Workflow
- Workflow contract: `docs/contributor_workflow.md`
- Reviewer personas: `docs/review_personas.md`
- Bug queue: `docs/bugs.md`
- Validation entrypoint: `tools/validate_repo.sh`
- If an inbox process syncs PR review feedback into `docs/bugs.md`, treat that as the highest-priority bug work.
- If the inbox sync changes tracked docs, commit that sync before continuing so the run starts from a clean tree.
- If the task came from PR review feedback, reply on GitHub when appropriate after fixing or clarifying the issue.

## Self-Reflection Loop (Required)
- At the end of each compiler slice, record one `Good`, one `Bad`, and one `Adjustment` entry in the work summary, then update this file if a process rule should change.
- Current reflection (2026-02-23):
- Good: narrow slice commits plus bounded targeted tests kept changes stable and debuggable.
- Bad: detector-heavy metadata extraction grew faster than its maintainability payoff and slowed direct progress to simple Cranelift lowering.
- Adjustment: prioritize reachability-first pruning and delete detector/fallback branches as soon as equivalent lowered behavior is covered.
- Current reflection (2026-02-23, cleanup slice):
- Good: deleting detector blocks immediately reduced compiler complexity and made ownership boundaries clearer.
- Bad: temporary compatibility channels (`simple_*` metrics) still exist and can hide stale host expectations.
- Adjustment: remove compatibility metric channels quickly after reachability contracts are wired to avoid long-lived dead interfaces.
- Current reflection (2026-02-23, simple-pass restart slice):
- Good: replacing the copied orchestration file with a fresh single-pass parser immediately clarified scope and ownership.
- Bad: initial rewrite used unsupported control-flow keywords (`break`/`continue`), causing avoidable early fixture failures.
- Adjustment: after the first parser chunk, run one small representative fixture immediately to validate language-surface assumptions before adding more code.
- Current reflection (2026-02-23, struct-layout reachability slice):
- Good: wiring struct/global pruning directly in `.stasis` kept the change small and testable while preserving host-glue boundaries.
- Bad: large end-to-end fixture execution exceeded the 5-minute command budget and is too slow for routine slice verification.
- Adjustment: default slice verification to bounded Rust-side harness tests for fast feedback, and run larger end-to-end fixture commands only as explicitly budgeted checks.
- Current reflection (2026-02-24, host-required root wiring slice):
- Good: adding host-required roots as explicit hashes injected into `.stasis` kept ownership clear and avoided parser/keyword surface expansion.
- Bad: host compiler API had no compile-options channel, so root wiring currently rides through harness generation rather than a structured config object.
- Adjustment: introduce a small explicit compile-config object in Rust host next, so required roots and future compile flags are passed through one typed path.
