# Release and package provenance

Official Stasis archives contain `stasis_release_provenance.json`. The schema-v1
manifest identifies the release tag and source commit, records the clean build
state, hashes the exact compiler executable, every packaged renderer/mobile
runtime source, every mobile shell/template file, and any shipped desktop
network static library and public header, identifies the stable
`gfx_cmd` command-buffer family with current schema v7 and enabled
backends/features, and records the full Cargo package/version/source
set, `Cargo.lock` SHA-256, Rust compiler version, and resolved SDL versions when
SDL is linked into the release.

`stasis package` and `stasis package-mobile` verify those hashes before compiling
or copying anything. A missing manifest or a compiler/runtime mismatch stops the
package with the expected and actual SHA-256 values. This prevents a release
binary from silently packaging renderer sources copied from another checkout.
Network-enabled desktop packaging additionally requires the selected prebuilt
library and header to be the exact archive-relative artifacts authenticated by
`desktop_network_artifacts`. Missing, substituted, partially recorded, or
unrecorded network artifacts stop packaging, including when
`--development-build` selects development package behavior.

Every generated desktop, Android, and iOS package contains
`stasis_provenance.json`; Windows desktop packages keep it in the relative
`app/` payload beside the runtime. Mobile packages also place the manifest in the game
asset root and compile a small generated header into the lifecycle adapter. At
startup, the adapter logs the build label, release tag, source commit, and
`gfx_cmd` renderer family with schema 7. Package validation accepts only the
current command-buffer schema; generated artifacts must be rebuilt with the matching toolchain.
The desktop graphics runtime logs the bounded
sidecar manifest from the resolved runtime payload directory during initialization.

Web package provenance additionally contains `web_package.asset_metadata_audit`
and `web_package.size_metrics`. The audit table is the complete metadata derived
from the prepared `assets/manifest.json`; release runtime JavaScript receives only
the browser-required projection. Size metrics define `raw_bytes` as exact UTF-8
length and `gzip_bytes` as RFC 1952 gzip at level 9 with mtime zero, and record
before/after values for both linked JavaScript and asset metadata. These values
are deterministic package evidence, not estimates from a deployment server.
`stasis_mobile_package.json` points to the
embedded manifest and exposes the release/development classification to build
audit tools.

Windows release/bootstrap archives also ship `tools/windows/stasis-signing.ps1`. Any release
signing step runs before provenance or hash manifests are finalized, so recorded hashes describe
the signed bytes. The archive entrypoint accepts explicit `status`, `sign`, and `verify` operations;
production credentials remain CI-supplied and are never generated or persisted by the toolchain.

## Local release and development builds

A source checkout has no authority to claim an official release. Local proof
packages therefore use `build_class: "local_release"` when the installed
toolchain has no `stasis_release_provenance.json`. They still use optimized release behavior and
set `development_build` to `false`, while recording the real source commit, dirty state, compiler
hash, and runtime/template hashes. Their release tag remains null and mobile runtimes label them
`local release`, so they cannot be confused with a verified official archive.

`--development-build` explicitly selects development behavior and provenance. Those packages use
`build_class: "development"`, set `development_build` to `true`, and log
`non-release development build` at runtime. The flag never disables asset or compiler validation.
Manually dispatched bootstrap artifacts use the same development classification;
only a published `v*` release or generated `nightly-*` release is official.

## Proof and repinning policy

- A source-proof build demonstrates behavior from a named commit but is not a
  distributable release.
- A local release package is optimized release output with content-addressed local provenance.
- A local development package uses `--development-build` and is visibly non-release.
- An official artifact comes from the release workflow and must pass manifest
  hash verification when it regenerates its Android and iOS smoke packages.

Games such as Chess TD may repin only to an official tag. Record the tag,
`source_commit`, compiler SHA-256, Windows runner manifest SHA-256, macOS
runner plist SHA-256, and
`runtime/stasis_graphics.c` SHA-256 from
the release manifest in the game's dependency documentation. Rebuild the
minimal package from the extracted archive without `--development-build`; a
successful package audit is the proof that the consumed renderer matches the
release. Never label a source-proof or local proof directory as the pinned
official artifact.

## SDL dependency policy

Desktop release graphics runtimes build SDL3 and SDL3_image from SHA-256-pinned
upstream source archives and link them into `stasis_graphics`. The pinned
versions match the versions recorded by the existing official Windows release
provenance (SDL3 3.4.10 and SDL3_image 3.4.4); Unix package-manager aliases
must not select a different implementation during a release build.

Changing either SDL version is an explicit dependency upgrade. It requires the
cross-platform renderer and installed-VSIX acceptance gates, updated provenance,
and a review of runtime and packaging compatibility. Routine release builds do
not float to newer SDL packages.
