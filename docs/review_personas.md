# Review Personas

Review personas are optional. Select only those that match the change's risk; there is no fixed set of review gates.

Each selected reviewer returns `GREEN` or `BLOCKED`, up to five short bullets, and, when blocked, the smallest change needed to proceed.

- **Language Designer:** syntax, semantics, and fit with `docs/spec.md`.
- **Compiler Architect:** compiler ownership, reachability, lowering, and maintainability.
- **Runtime Engineer:** host boundaries, hot swap safety, deterministic execution, and platform integration.
- **Code Expert:** Rust and Stasis readability, implementation shape, and meaningful tests.
- **Performance Expert:** compile time, runtime hot paths, and cache or invalidation cost.
- **Human Advocate:** reviewability, user impact, follow up, and decisions needing human judgment.
