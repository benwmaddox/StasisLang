# Compiler Process History

These dated notes preserve lessons from earlier compiler slices. They are historical context and are not operating instructions. Current process rules live in `AGENTS.md` and `docs/contributor_workflow.md`; no slice is required to produce `Good`, `Bad`, `Adjustment`, or `Theory gained` sections.

## 2026-02-23

- Narrow slice commits and bounded targeted tests kept changes stable and debuggable. Detector heavy metadata extraction grew faster than its maintenance value, so later work prioritized reachability pruning and deletion of detector or fallback branches once equivalent lowering existed.
- Deleting detector blocks reduced compiler complexity and clarified ownership. Temporary `simple_*` compatibility metrics could hide stale host expectations, so compatibility channels should be removed promptly after reachability contracts are wired.
- Replacing copied orchestration with a fresh parser clarified scope, but an early rewrite used unsupported `break` and `continue` keywords. A small representative fixture after the first parser chunk would have exposed that language surface mismatch sooner.
- Struct and global reachability wiring in Stasis kept the change small and preserved host glue boundaries. A large end to end fixture exceeded the routine time budget, which led to using bounded Rust harness checks for fast feedback and reserving larger executable runs for explicitly budgeted validation.

## 2026-02-24

Host required roots were added as explicit hashes injected by the Stasis harness, preserving ownership without expanding parser keywords. The missing typed compile options channel suggested a future explicit compile config object for required roots and other flags.

## 2026-07-16

A single Rust parser owned semantic edit plan gave CLI and Android the same identity, import, validation, hashing, and rollback behavior. Early coverage missed non textual reachability roots, same line declaration boundaries, import only lifecycle roots, cross surface owner translation, and failure after source mutation but before receipt publication; future semantic edit coverage should include each boundary.
