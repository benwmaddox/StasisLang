# Spec Implementation Status (Rust Compiler)

Last updated: 2026-03-02

This document tracks how much of `docs/spec.md` is implemented in the Rust compiler/runtime pipeline.
It is intended to be concrete and release-oriented (JIT + AOT), and it explicitly excludes the experimental self-host `.stasis` compiler track under `compiler/`.

Status legend:
- **Implemented**: supported end-to-end in the Rust compiler/runtime.
- **Partial**: implemented for some shapes/modes (often JIT-first) or with known gaps.
- **Missing**: specified, but not implemented in the Rust compiler/runtime yet.
- **Deferred (Out of Scope)**: intentionally not being built for the current release approach.
- **Paused**: intentionally not being worked right now.

## Spec Section -> Rust Implementation Status

| Spec section | Status | Notes (JIT vs AOT, gaps, links) |
| --- | --- | --- |
| 1. Overview | Partial | Direction matches current approach (Rust compiler, JIT dev + AOT prod). Specific implementation details vary and are tracked per-section. |
| 2. Core Principles | Partial | Static global memory + deterministic tick model are core. "No hidden allocation/copies" is a goal; some lowering paths still enforce this by restrictions rather than rich analysis. |
| 3. Lexical Structure | Partial | Identifiers/keywords/integer literals are stable. Float literals are supported for the current float type (`f32`). Backtick literals exist for tests. |
| 4. Types (overall) | Partial | `i32`, `f32`, `bool`, `void` are implemented as true builtins. Other "primitive" names currently behave as named scalar types with `i32` ABI compatibility rather than true narrow storage types. |
| 4.1 Primitive Types | Partial | Implemented: `i32`, `f32`, `bool`, `void`. Missing: `f64` (planned soon). Paused: true `u16`/`u32` narrow-int semantics. `u8` is used as a named scalar type for byte buffers (ABI-compatible with `i32`). |
| 4.2 Composite Types | Implemented | Fixed arrays `Type[N]`, views `Type[]`, structs, enums, and string buffer forms (`ascii[N]`, `ascii[]`, `utf8[N]`, `utf8[]`) are implemented enough to run samples (including Brickout Revenge). |
| 4.3 Numeric Conversion Semantics | Partial | `from_*`/`to_*` conversions exist for the current numeric set. `f64` conversions are missing until `f64` exists. |
| 4.4 Local Type Inference | Implemented | Local inference for `let name = <expr>` is supported; typed `let name: Type = ...` is supported. |
| 5. Operators and Expressions | Partial | Implemented for `i32`/`f32`/`bool` forms required by current samples/tests. Missing: `f64` operator coverage until `f64` exists. |
| 6. Declarations and Statements | Partial | `let`, `global`, assignment, `if/else`, `for`, `foreach`, `return`, `continue` are supported in the Rust pipeline. Some shapes are still covered by "stub fallback" in AOT mode until parity hardening lands. |
| 6.5.1 `for` loop (required init/condition/step) | Implemented | Omitting any of the three `for` header segments is rejected (init is required, condition is required, step is required). |
| 6.5.5 `continue` | Implemented | Supported and enforced as loop-only. |
| 7. Functions and Calls | Partial | Function declarations and calls are stable; receiver-form call resolution is supported. Some AOT paths still rely on "stub fallback" for not-yet-supported lowering shapes. |
| 8. Enums | Implemented | Enums are used by samples and supported by Rust lowering. |
| 9. Modules and Imports | Implemented | `import` and project-local module resolution are implemented. |
| 10. Testing Construct | Partial | `.test.stasis` discovery/execution exists in JIT dev/test workflows. AOT test execution parity is not a current priority. |
| 11. Memory Model | Partial | Static globals and deterministic layout are central. Remaining gaps are mostly around richer compile-time enforcement and diagnostics, not basic execution. |
| 12. Runtime Boundary and Extern | Partial | Host-set profile/registry plumbing exists. Required-host extraction/diagnostics are tracked separately (not complete). |
| 12.2 Optional Plugin Libraries | Deferred (Out of Scope) | Plugin libraries are explicitly out of scope for the current release approach; do not plan features around them right now. |
| 13. Tick Policy | Implemented | Tick-based semantics are supported and used by the runtime loop. |
| 14. Incremental Compilation and Hot Swap | Partial | Development hot-swap is implemented (JIT). Production AOT is the release approach; it should run the same games as JIT, but "AOT parity" is still being hardened and is explicitly tracked as a release goal. |
| 15. Swap Hook | Implemented | `on_code_swap()` exists and is invoked on successful commits. |
| 16. Diagnostics | Partial | Diagnostics exist but need continued hardening for determinism and coverage. |
| 17. Development Target | Partial | JIT dev is solid. AOT prod exists and is the release path, but still needs parity hardening and explicit speed/size optimization work for shippable builds. |
| 18. Status Note | Implemented | Current direction is accurately represented. |

## Release-Oriented Notes (JIT vs AOT)

- **Release approach**: Cranelift AOT is the production/release backend; JIT is for development/watch/hot-swap.
- **AOT parity requirement**: AOT must run the same sample games as JIT (notably `samples/brickout_revenge/brickout_revenge_v1.stasis`).
- **Quality gate**: AOT should reject shipping builds that still rely on "stub fallback" lowering in emitted artifacts.
- **Not in scope**: optional plugin libraries and anything that requires a self-host `.stasis` compiler in the release pipeline.

