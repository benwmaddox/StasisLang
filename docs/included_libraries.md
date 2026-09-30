# Target-aware included libraries

Manifest v3 separates authenticated code inclusion from permission grants. `libraries` selects
Stasis-owned source, native, or Web artifacts for one exact target. `capabilities` grants the
behavior those artifacts may perform. A library is never downloaded and projects cannot provide
replacement native binaries.

## Manifest contract

Selections use stable catalog IDs. A target entry replaces the complete base request for that ID;
target families do not inherit from one another.

```json
{
  "manifest_version": 3,
  "capabilities": { "network": true, "network_client": true },
  "libraries": {
    "selections": {
      "stasis.network": { "enabled": true, "features": ["host", "client"] }
    },
    "targets": {
      "web": { "stasis.network": { "features": ["host"] } },
      "android-x86_64": { "stasis.network": { "enabled": false } }
    }
  }
}
```

Canonical targets are `web`, `windows-x86_64`, `windows-arm64`, `linux-x86_64`,
`linux-arm64`, `macos-x86_64`, `macos-arm64`, `android-arm64`, `android-x86_64`,
`ios-arm64`, and `ios-simulator-arm64`. Unknown IDs, features, or targets; duplicate features;
missing capability grants; target gaps; catalog conflicts; and unauthenticated inputs fail before
compiler or package output. `host` requires `capabilities.network`; `client` requires
`capabilities.network_client`. Selecting both is the explicit dual-role contract.
The machine-readable fragment is
[`included_libraries.schema.json`](included_libraries.schema.json); runtime validation additionally
enforces capability grants and authenticated catalog closure.

The compiler snapshot exposes target constants:

- `project_library_stasis_network_available()`
- `project_library_stasis_network_feature_host()`
- `project_library_stasis_network_feature_client()`

Application imports remain explicit. Importing `network_client.stasis` when the exact target disables
`stasis.network` fails with the target and importing source path. The resolved set and SHA-256 digest
are identical in JIT, AOT, Wasm, incremental candidate snapshots, child-build receipts, cache
identity, and `stasis_provenance.json`.

Inspect a selection without building a package:

```text
stasis inspect libraries --target android-arm64 --json
```

[`samples/target_libraries`](../samples/target_libraries) is a complete dual-role example with a
host-only Web override and an offline Android-emulator override.

The result reports explicit or legacy-implicit selection, features, required capabilities,
authenticated source/header and notice hashes, target artifact kind/path, ABI/minimum-OS/toolchain
constraints, dependencies, load policy, exclusions, reasons, catalog digest, and library-set digest.

## Networking migration

Manifest v1/v2 behavior is unchanged: `capabilities.network` implicitly selects `host`,
`capabilities.network_client` implicitly selects `client`, the pair remains mutually exclusive, and
existing layouts and receipt fields remain compatible. To migrate, change to v3, retain the old
capability as a permission grant, and add the matching explicit library feature. A dual-role project
grants both capabilities and selects both features.

The one stable `stasis_network.h` ABI exports both host and client entry points. Trusted manifest
features decide which policy/lifecycle surface is active; launch arguments cannot elevate the role.
Offline packages omit network code, permissions, guest bundles, aliases, and runtime hooks.

| Target | Catalog artifact and loader | Signing/order contract |
| --- | --- | --- |
| Windows | optional `stasis_network.dll` plus import library beside reusable payload | authenticate, stage, Authenticode-sign DLL, verify, then sign enclosing package |
| Android | `libstasis_network.so` for the selected ABI, normal ELF dependency (API 26, NDK r27) | NDK/API identity, SONAME/NEEDED, stripped ABI and 16 KiB alignment before APK signing |
| Linux | `libstasis_network.so`, package-local `$ORIGIN` | authenticate/hash before package publication |
| macOS | nested `@rpath` dylib with the required architecture slice | sign dylib first, then app, then notarize |
| iOS | authenticated static archive for the selected device or simulator target | link before final app signing; no downloaded executable code |
| Web | existing reachable JS/Wasm/guest bundle pieces | hash with Web package provenance; disabled output strips all pieces |

Source checkouts may build authenticated release inputs while developing. Published toolchains must
consume the release catalog/provenance copy so game packages do not rebuild the shared library.

## Catalog authoring and release

The catalog is Stasis-owned and compiled into the toolchain. Each entry fixes logical ID/version,
features, owned modules/headers, target artifacts and kinds, stable ABI, minimum OS/SDK/NDK and
toolchain identity, dependencies/conflicts, required capabilities, load policy, and license notice.
Its canonical JSON digest authenticates selection metadata. Source/header/notices carry direct
SHA-256 values; native artifact authentication points at its exact
`stasis_release_provenance.json` key. Release builders must hash inputs before signing, preserve
nested-signing order, and publish the catalog and artifacts atomically.

Auditors reject missing, extra, stale, wrong-target, wrong-ABI, modified, or unlicensed files. Errors
identify library ID, target, artifact, and expected/actual identity without logging credentials or
signing material. Adding a feature must preserve the stable ABI or increment the catalog library
version and compatibility contract. Transitive additions require deterministic dependency order and
license closure; dependency cycles are forbidden.

## Troubleshooting

- `disabled for <target>`: enable the library in the exact target override, or remove the import.
- `requires capabilities...`: grant the corresponding permission; inclusion alone is not authority.
- `unknown library/feature/target`: use only IDs and canonical targets shipped by this toolchain.
- ABI/toolchain/minimum-OS mismatch: restore the matching release artifact; do not substitute a local binary.
- Hash/license/catalog failure: restore the complete selected Stasis release and rerun inspection.
- Native load failure: inspect platform dependency metadata (`dumpbin`, `readelf`, `otool`, APK
  `lib/<abi>`), package-local loader paths, slices, signing, and minimum OS before launch.

Size/time measurements must compare clean offline, host, client, and dual packages on the same commit,
target, machine, cache state, and signing mode. Desktop provenance and mobile package receipts record
the selected artifact SHA-256, byte size, source, and library build duration. Record whole-package
wall build time and final package bytes in CI evidence. The
offline result is the regression baseline; a release is not acceptable if ordinary offline builds
compile/link network code or if each game rebuilds the release library.
