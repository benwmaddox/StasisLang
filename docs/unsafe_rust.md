# Unsafe Rust boundaries

Stasis permits unsafe Rust only where Rust must cross a boundary it cannot express in the type
system:

- `crates/stasis_dynload/src/` owns dynamic-library handles, foreign function pointers, and the
  JIT/AOT guest-memory ABI.
- `crates/stasis_android_bridge/src/` owns JNI/C pointer conversion at the Android boundary.
- `mobile/android/codex_native/src/` owns the Codex Android JNI/C string boundary.

All compiler, runner, language-service, editor, and application orchestration code must remain safe
Rust. `tools/validate_repo.sh` enforces this file-level boundary.

## Ownership rules

1. Executable JIT pointers may be published only while their `JitArena` owner is retained.
2. Runtime-owned scalar and collection storage may be snapshotted and restored through owned Rust
   values.
3. Host-registered pointers are borrowed FFI memory. A registration promises that its allocation is
   stable for the guest execution window, but it does not transfer ownership to Stasis.
4. Generic snapshots never dereference, copy, restore, or rebind borrowed FFI memory. A feature that
   needs rollback must use generation metadata and typed state accessors, or introduce an explicit
   owner/lease whose lifetime covers the operation.
5. Storage rebinding and generation publication happen only between guest execution windows.

Unsafe blocks should be small and adjacent to the boundary operation. Each must state the ownership,
validity, alignment, and synchronization fact that makes the operation valid. A safe wrapper is
appropriate only when its signature or owning type preserves those facts for every caller.
