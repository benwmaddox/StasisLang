# Build server plan for brickout revenge (Windows + Android)

## Goals
- Produce optimized Windows and Android builds for brickout revenge.
- Keep build steps reproducible, deterministic, and documented.
- Make Android device installs easy for local testing.
- Support future extensibility for multiple apps and targets.

## Build server setup

### Required toolchains
- Windows
  - .NET SDK (matching `global.json` if present).
  - LLVMSharp toolchain (LLVM binaries and LLVMSharp runtime dependencies).
  - WASM tooling if the pipeline emits WebAssembly artifacts (align with compiler output).
- Android
  - Android SDK + NDK (pinned versions).
  - `adb` for device detection and installs.
  - Java (JDK) and Gradle if the Android packaging step uses them.

### Environment pinning strategy
- Pin versions using a build image manifest (for example, a container image tag or VM image ID).
- Record checksums for SDK/NDK downloads and LLVM binaries.
- Store a `build-tools.lock.json` (or similar) containing exact versions, download URLs, and SHA256 sums.
- Ensure the build image is refreshed only when lock entries change.

### Caching strategy
- Cache NuGet packages (`~/.nuget/packages`) keyed by `packages.lock.json` or the solution hash.
- Cache Android Gradle and NDK directories keyed by `gradle.lockfile` and NDK version.
- Cache intermediate build outputs (compiler IR, object files) per commit to speed rebuilds.

## Build pipeline steps

### Common steps
1. Checkout repository at the target Git SHA.
2. Restore dependencies.
3. Build optimized artifacts.
4. Package outputs.

### Windows pipeline
- Restore: `dotnet restore Stasis.sln`
- Build (optimized): `dotnet build Stasis.sln -c Release -p:ContinuousIntegrationBuild=true`
- Package
  - Collect compiler, runtime, and application output artifacts.
  - Bundle `brickout revenge` binary and required runtime assets.
  - Emit a zip archive for distribution.

### Android pipeline
- Restore: `dotnet restore Stasis.sln`
- Build (optimized): `dotnet build Stasis.sln -c Release -p:ContinuousIntegrationBuild=true`
- Package
  - Run the Android packaging step (Gradle task or custom script).
  - Use NDK configuration aligned with LLVM output.
  - Emit an APK or AAB artifact.

## Artifact naming and versioning
- Format: `{app}-{platform}-{build-type}-{git-sha}-{timestamp}`
- Example: `brickout-revenge-windows-release-a1b2c3d-20240521T153000Z.zip`
- Include a manifest file describing toolchain versions and checksums.

## Android device loading

### Build server install flow
1. Detect device: `adb devices`
2. Optional clean install: `adb uninstall com.stasis.brickoutrevenge`
3. Install: `adb install -r out/android/brickout-revenge.apk`
4. Optional asset push: `adb push assets/ /sdcard/Android/data/com.stasis.brickoutrevenge/files/`

### Manual local testing
- Detect device: `adb devices`
- Install: `adb install -r out/android/brickout-revenge.apk`
- Uninstall (clean): `adb uninstall com.stasis.brickoutrevenge`

## Multi-app extensibility

### Config example (YAML)
```yaml
apps:
  - name: brickout-revenge
    entryPoint: examples/brickout/main.stasis
    targets:
      - platform: windows
        buildType: release
        packaging: zip
      - platform: android
        buildType: release
        packaging: apk
```

### Pipeline behavior
- Load config from `build/apps.yaml`.
- Loop over each app and target to run restore, build, and packaging steps.
- Surface app-specific packaging outputs under `out/{app}/{platform}`.

## Failure handling and reporting
- On failure, collect logs from build and packaging steps.
- Retain logs and partial artifacts for debugging.
- Emit user-friendly errors with command output and suggested fixes.

## Spec and documentation alignment
- Build outputs must align with `docs/spec.md` and the deterministic compilation model.
- The compilation pipeline should follow `docs/compilation.md` for parser and lowering behavior.
- Ensure LLVM/WASM outputs are consistent with the compiler requirements in those specs.

## Standard commands for maintainers
- `dotnet restore Stasis.sln`
- `dotnet build Stasis.sln -c Release -p:ContinuousIntegrationBuild=true`
- Android packaging command (example): `./build/android/package.sh`
- Windows packaging command (example): `./build/windows/package.ps1`
