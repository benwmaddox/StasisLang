# Stasis cross-host contracts

`host_runtime.json` is the versioned owner for values and tags shared by host
implementations. Hosts remain handwritten; the CI checker validates their
Stasis, C, Rust, Java, and JavaScript copies against this registry.

Platform-specific event loops, renderers, resource implementations, and UI
presentation do not belong here. Explicit `platform_extensions` record real
differences without pretending that every host implements the same machinery.

Contract version 1 preserves HostFrame v3, render command versions 2 through
6, graphics runtime ABI 3, and mobile runtime ABI 1. Unknown registry versions
are rejected. Production diagnostic DTO migration and asset-package identity
publication are completed in later #384 checkpoints.
