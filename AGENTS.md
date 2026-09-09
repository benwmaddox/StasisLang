# Repository Guidelines

## Project map

- `docs/spec.md` is the language specification; `docs/live-compilation-prd.md` is the product and architecture contract; `docs/build_checklist.md` is execution context.
- `crates/stasis_compiler` owns Rust source indexing, parsing into structured artifacts, semantic checks, and HIR construction/lowering.
- `crates/stasis_jit` owns Cranelift JIT/AOT integration, function pointer tables, and executable memory management.
- `crates/stasis_runner` owns the tick loop, swap sequencing, and commit orchestration. `apps/stasis` is the in process graphical runner.
- `.stasis` files own user code, the standard library, and samples. Rust owns compiler, host, runtime, and platform boundaries; use C only when a platform binding requires it.

## Commands

- Use Rust/Cargo for implementation. Common checks are `cargo build`, `cargo test`, and `cargo run -p stasis --release -- --ticks 300 --watch-dir samples/brickout_revenge`.
- Codex and automation must run Cargo through `python tools/cargo_cache.py run -- cargo ...`; human interactive Cargo commands may use their normal target and incremental settings. Use `python tools/cargo_cache.py measure` to inspect cache ownership; cleanup is dry run unless `--apply` is explicit.
- Use `rg` for search and keep commands deterministic and scriptable. The repository validation entrypoint is `tools/validate_repo.sh`.
- Android Workshop uses the `Stasis_API_35` AVD. From the repository root, run `powershell -NoProfile -ExecutionPolicy Bypass -File mobile/android/test_emulator.ps1 -Headless`; reuse an installed build with `-Headless -SkipBuild`. For a truly headless run, stop an existing GUI AVD with `C:\Android\Sdk\platform-tools\adb.exe -s emulator-5554 emu kill` and wait for it to disappear from `adb devices`. `mobile/android/validate_device.ps1 -Serial emulator-5554` selects that emulator explicitly.

## Source and validation rules

- Keep touched files ASCII where practical, use short snake case names, and keep comments brief. Arithmetic, comparison, and assignment are infix; receiver form is preferred for calls. `from_*` helpers mutate a target, while `to_*` helpers are pure conversions.
- Keep Rust tests runnable by default; `tools/validate_repo.sh` rejects `#[ignore]` under product and test roots. Put checks requiring credentials or unavailable tools in explicit examples.
- Ship behavior changes with deterministic tests and cover the relevant parser, semantic, lowering, JIT, AOT, or hot swap boundary. A focused Cargo test must name its owning target (`--lib`, `--bin`, or `--test`); `running 0 tests` means the selection is wrong.
- Keep every command within 900 seconds. Check for lingering test or compiler processes only after a timeout, cancellation, or suspected leak, and inspect or clean only processes attributable to this task. Keep unsafe Rust in audited platform boundary crates; repository validation rejects unsafe blocks in orchestration and product crates.
- For graphical behavior, inspect a PNG for a representative still state and an MP4 when the claim depends on motion, timing, input, animation, or a multi step interaction. Every AI work summary includes `Visual evidence:` with the inspected paths or `not applicable`; if relevant media cannot be captured, state the limitation and record the validation gap.
- Incremental compilation checks must cover file invalidation, per function gating, and unchanged function cache reuse. Hot swap checks must cover all or nothing commit, rejection preserving old code and data, and `on_code_swap` failure aborting the commit.

## Runtime invariants

- The runtime is one OS process with in process compilation. File level semantic analysis is authoritative; per function semantic hashes gate backend work only.
- Compilation produces a pending patch in the background and commits it between ticks through stable `FnId -> code_ptr` indirection. Reject layout or signature incompatibility, `on_code_swap` failure, and any partial commit. Preserve deterministic tick based semantics; Stasis gameplay must not use `dt` to advance progression.

## Compiler implementation

- Keep explicit precedence handling and shared parser matchers. The current flow indexes files, parses function bodies into stored statement artifacts, and then `Compiler::lower_function_to_hir` consumes those artifacts to build HIR. Extend that handoff instead of adding ad hoc token offsets, parser shape detectors, or duplicate parser pipelines.
- Reachability pruning is the primary dead code mechanism. Roots are `main`, `tick`, `on_code_swap` when present, and host required exported entries; maintain simple call and type reference graphs and lower only reachable functions and struct metadata.
- Do not emit fake semantics or temporary fallback paths. If a feature is not implemented, return a deterministic diagnostic. Keep lowering state compact and validate value stack, block depth, and pending jump invariants at statement and function boundaries; use bounded jump list backpatching with deterministic overflow diagnostics.
- Keep diagnostic and instrumented behavior on the same pipeline. Keep compiler slices narrow and prove compiler behavior changes with at least one representative program that reaches Cranelift IR, builds an executable, runs, and has asserted behavior in the applicable JIT/AOT path. A slice without that executable proof is incomplete.
- Earlier compiler lessons are archived in [`docs/compiler_process_history.md`](docs/compiler_process_history.md) for context only; they are not active requirements.

## Work selection and review

- For ordinary work, the user's request defines the scope. Repository plans and bug lists provide context only.
- For Night Shift or inbox automation, the selected GitHub issue, PR, review, or review comment is the sole work source. The central runner owns branch setup, fetch or fast forward, and executor launch.
- Before a nontrivial edit, inspect status and relevant code, resolve architectural questions, and state a concise plan. Select review personas from `docs/review_personas.md` by risk; they are optional, not a fixed gate.
- Review the final diff, remove incidental changes, simplify touched code, and run applicable final checks once. Update durable docs when a discovery changes an invariant. Commit, append a Night Shift report, or reply on GitHub only when the user or the authorized automation task explicitly includes that action.
