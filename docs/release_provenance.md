# Release and package provenance

Official Stasis archives contain `stasis_release_provenance.json`. The schema-v1
manifest identifies the release tag and source commit, records the clean build
state, hashes the exact compiler executable, every packaged renderer/mobile
runtime source, and every mobile shell/template file, identifies `gfx_cmd` v1
and enabled backends/features, and records the full Cargo package/version/source
set, `Cargo.lock` SHA-256, Rust compiler version, and resolved SDL versions when
SDL is linked into the release.

`stasis package` and `stasis package-mobile` verify those hashes before compiling
or copying anything. A missing manifest or a compiler/runtime mismatch stops the
package with the expected and actual SHA-256 values. This prevents a release
binary from silently packaging renderer sources copied from another checkout.

Every generated desktop, Android, and iOS package contains
`stasis_provenance.json`. Mobile packages also place the manifest in the game
asset root and compile a small generated header into the lifecycle adapter. At
startup, the adapter logs the build label, release tag, source commit, and
`gfx_cmd_v1` renderer contract. The desktop graphics runtime logs the bounded
sidecar manifest from the executable directory during initialization.
`stasis_mobile_package.json` points to the
embedded manifest and exposes the release/development classification to build
audit tools.

## Development builds

A source checkout has no authority to claim an official release. Local proof
packages therefore require the explicit `--development-build` flag. Their
manifest sets `development_build` and `dirty_state` to `true`, uses a null
release tag, and logs `non-release development build` at runtime. This flag
permits local iteration; it never disables asset or compiler validation.
Manually dispatched bootstrap artifacts use the same development classification;
only a published `v*` release or generated `nightly-*` release is official.

## Proof and repinning policy

- A source-proof build demonstrates behavior from a named commit but is not a
  distributable release.
- A local proof package uses `--development-build` and is visibly non-release.
- An official artifact comes from the release workflow and must pass manifest
  hash verification when it regenerates its Android and iOS smoke packages.

Games such as Chess TD may repin only to an official tag. Record the tag,
`source_commit`, compiler SHA-256, and `runtime/stasis_graphics.c` SHA-256 from
the release manifest in the game's dependency documentation. Rebuild the
minimal package from the extracted archive without `--development-build`; a
successful package audit is the proof that the consumed renderer matches the
release. Never label a source-proof or local proof directory as the pinned
official artifact.

## SDL dependency policy

Desktop release graphics runtimes build SDL2 and SDL2_image from SHA-256-pinned
upstream source archives and link them into `stasis_graphics`. The pinned
versions match the versions recorded by the existing official Windows release
provenance (SDL2 2.32.10 and SDL2_image 2.8.12); Unix package-manager aliases
must not select a different implementation during a release build.

Changing either SDL version is an explicit dependency upgrade. It requires the
cross-platform renderer and installed-VSIX acceptance gates, updated provenance,
and a review of runtime and packaging compatibility. Routine release builds do
not float to newer SDL packages.
