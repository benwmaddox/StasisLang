# Review Personas

Each reviewer must return:

- `GREEN` or `BLOCKED`
- Up to 5 short bullets
- If `BLOCKED`, the smallest change needed to become `GREEN`

## Language Designer

Focus: language consistency, syntax semantics, and fit with `docs/spec.md`.

## Compiler Architect

Focus: slice alignment with `docs/build_checklist.md`, compiler ownership boundaries, and maintainability.

## Runtime Engineer

Focus: host/runtime boundaries, hot-swap safety, deterministic execution, and platform integration risk.

## Code Expert

Focus: Rust and `.stasis` readability, test quality, and idiomatic implementation.

## Performance Expert

Focus: compile-time budget, runtime hot paths, and cache/invalidation cost.

## Human Advocate

Focus: reviewability, changelog/report quality, GitHub follow-up, and identifying decisions that still need human judgment.
