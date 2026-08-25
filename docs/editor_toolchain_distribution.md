# Atomic Editor Toolchain Distribution

The VS Code extension, compiler/LSP/DAP executable, and graphics runtime are one release unit. They
must not be selected or upgraded independently.

## Invariant

Every editor operation uses the executable in the immutable toolchain directory bundled inside the
platform VSIX. The graphics runtime is loaded from beside that executable. Both binaries expose the
same `release_id`, and activation fails before starting any editor service when their identities,
protocols, or packaged hashes differ.

The release pipeline stamps one identity into Rust with `STASIS_RELEASE_ID` and into the native
runtime with the `STASIS_RELEASE_ID` CMake setting. `stasis --json editor-info` validates the sibling
runtime ABI and identity and reports hashes for both files. VSIX packaging records that response and
the hashes in `dist/toolchain-manifest.json`.

At activation the extension:

1. resolves only its bundled toolchain unless an explicit development override is configured;
2. rejects absolute or escaping manifest paths;
3. hashes the executable and graphics runtime;
4. invokes `editor-info` from the executable's directory;
5. checks the release identity and LSP, DAP, live, and graphics protocol versions;
6. starts LSP, DAP, tests, and Live Workshop only after every check succeeds.

There is deliberately no implicit `PATH` fallback. Reinstalling or rolling back a platform VSIX
therefore installs or rolls back the complete editor toolchain.

## Development override

Source-tree development can set `stasis.developer.executablePath` to an absolute executable path.
The override remains subject to the same `editor-info` handshake, including a compatible sibling
graphics runtime. Local Rust and CMake builds use the release identity `development` unless an
explicit identity is supplied. Reload VS Code after changing the override.

## Release construction

Nightly release jobs build Rust and the native runtime from the same commit and release identity.
The tested platform archive is then embedded unchanged in the corresponding `linux-x64`,
`win32-x64`, or `darwin-arm64` VSIX. VSIX packaging never reconstructs or searches for individual
runtime files. Each downloadable OS release is one archive containing the standalone toolchain
archive, the matching VSIX, and `stasis-editor-release.json` with hashes for both.

Authenticode signing protects Windows provenance and reputation but is not the compatibility
mechanism. Signing should occur before archive and VSIX manifests are generated; the shared release
identity and post-signing hashes are what prove the files belong together.

## Extension packaging

Local packaging requires the root of a complete toolchain bundle:

```powershell
$env:STASIS_TOOLCHAIN_DIR = "C:\path\to\stasis-nightly-win-x64"
$env:STASIS_TOOLCHAIN_EXECUTABLE = "stasis.exe"
npm --prefix vscode-stasis run package -- --target win32-x64
```

Unix bundles use `bin/stasis` as the default executable path.

On Windows, `scripts/build_local_editor_release.ps1` builds the same release-directory shape under
`dist/stasis-editor-release-win32-x64`. It runs Rust and extension tests by default; pass
`-RunVsCodeE2E` to additionally install the packaged VSIX in the VS Code test host and exercise the
editor workflow. `scripts/install_vscode_stasis.ps1` consumes that directory instead of packaging a
separate extension/toolchain pair. Environments governed by Windows App Control can either pass
`-SigningCertificate` and `-SigningPassword` or configure the repository's existing
`STASIS_AOT_SIGN_TOOL`; signing occurs before hashes and the VSIX are produced.
