# Stasis cross-host contracts

`host_runtime.json` is the versioned owner for values and tags shared by host
implementations. Hosts remain handwritten; the CI checker validates their
Stasis, C, Rust, Java, and JavaScript copies against this registry.

Platform-specific event loops, renderers, resource implementations, and UI
presentation do not belong here. Explicit `platform_extensions` record real
differences without pretending that every host implements the same machinery.

Contract version 1 preserves HostFrame v4, graphics runtime ABI 3, and mobile
runtime ABI 1. The downstream render-command contract accepts only the current
version 7; generated artifacts and hosts must be rebuilt together as described
in [`docs/runtime_compatibility.md`](../../docs/runtime_compatibility.md).
Unknown registry versions are rejected. Runner diagnostics retain their stable
compiler codes, and packaged asset trees publish the versioned identity
declared by the registry.
