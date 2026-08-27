param(
  [string]$ReleaseId = "local-development",
  [string]$OutputRoot = "",
  [switch]$SkipBuild,
  [switch]$RunVsCodeE2E,
  [string]$SigningCertificate = "",
  [string]$SigningPassword = ""
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutputRoot) {
  $OutputRoot = Join-Path $repoRoot "dist/stasis-editor-release-win32-x64"
}
$OutputRoot = [IO.Path]::GetFullPath($OutputRoot)
$repoPrefix = [IO.Path]::GetFullPath($repoRoot) + [IO.Path]::DirectorySeparatorChar
if (-not $OutputRoot.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
  throw "OutputRoot must stay inside the Stasis repository."
}

$runtimeBuild = Join-Path $repoRoot "target/local-editor-runtime"
$commonGitDir = (git -C $repoRoot rev-parse --path-format=absolute --git-common-dir).Trim()
if ($LASTEXITCODE -ne 0) { throw "Failed to resolve the shared Cargo target." }
$cargoTarget = Join-Path (Split-Path -Parent $commonGitDir) "build/codex-cargo-target"
$toolchainRoot = Join-Path $repoRoot "target/local-editor-toolchain-win32-x64"
$toolchainArchive = Join-Path $repoRoot "target/stasis-local-toolchain-win32-x64.zip"
$extensionRoot = Join-Path $repoRoot "vscode-stasis"
$vsix = Join-Path $extensionRoot ".vsix/stasislang.stasis.vsix"

if (-not $SkipBuild) {
  $env:STASIS_RELEASE_ID = $ReleaseId
  $env:STASIS_SOURCE_COMMIT = (git -C $repoRoot rev-parse HEAD).Trim()
  $env:STASIS_BUILD_TARGET = "x86_64-pc-windows-msvc"
  $env:STASIS_BUILD_FINGERPRINT = (python (Join-Path $repoRoot "tools/compute_toolchain_fingerprint.py") --source-commit $env:STASIS_SOURCE_COMMIT --release-id $ReleaseId).Trim()
  python (Join-Path $repoRoot "tools/cargo_cache.py") run -- cargo build --manifest-path (Join-Path $repoRoot "Cargo.toml") -p stasis --release
  if ($LASTEXITCODE -ne 0) { throw "Stasis release build failed." }
  python (Join-Path $repoRoot "tools/cargo_cache.py") run -- cargo build --manifest-path (Join-Path $repoRoot "Cargo.toml") -p stasis_dynload --release
  if ($LASTEXITCODE -ne 0) { throw "Stasis dynamic runtime build failed." }

  $vcpkgRoot = $env:VCPKG_INSTALLATION_ROOT
  if (-not $vcpkgRoot -or -not (Test-Path (Join-Path $vcpkgRoot "vcpkg.exe"))) {
    $vcpkgRoot = "C:\vcpkg"
  }
  if (-not (Test-Path (Join-Path $vcpkgRoot "vcpkg.exe"))) {
    throw "vcpkg.exe was not found. Set VCPKG_INSTALLATION_ROOT."
  }
  $generatorScript = Join-Path $repoRoot "tools/windows/select-cmake-vs-generator.ps1"
  $generator = (& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $generatorScript).Trim()
  if ($LASTEXITCODE -ne 0) { throw "Visual Studio generator detection failed." }
  # Some automation hosts inject both Path and PATH. MSBuild treats them as a
  # duplicate dictionary key when it launches cl.exe, so retain one canonical entry.
  $processPath = [Environment]::GetEnvironmentVariable("Path", "Process")
  [Environment]::SetEnvironmentVariable("PATH", $null, "Process")
  [Environment]::SetEnvironmentVariable("Path", $null, "Process")
  [Environment]::SetEnvironmentVariable("Path", $processPath, "Process")
  $cmakeCache = Join-Path $runtimeBuild "CMakeCache.txt"
  if ((Test-Path $cmakeCache) -and -not (Select-String -Path $cmakeCache -SimpleMatch "CMAKE_GENERATOR:INTERNAL=$generator" -Quiet)) {
    Remove-Item -LiteralPath $runtimeBuild -Recurse -Force
  }
  cmake -S (Join-Path $repoRoot "runtime") -B $runtimeBuild -G $generator -A x64 `
    -DCMAKE_TOOLCHAIN_FILE="$vcpkgRoot/scripts/buildsystems/vcpkg.cmake" `
    -DVCPKG_TARGET_TRIPLET=x64-windows-static `
    -DSTASIS_GRAPHICS_BUILD_SHARED=ON `
    -DSTASIS_GRAPHICS_BUILD_STATIC=OFF `
    -DSTASIS_GRAPHICS_BUNDLE_SDL=ON `
    -DSTASIS_GRAPHICS_SDL_ONLY=ON `
    -DSTASIS_BUILD_RUNNER=OFF `
    -DSTASIS_BUILD_SYS=OFF `
    -DSTASIS_RELEASE_ID="$ReleaseId" `
    -DSTASIS_BUILD_FINGERPRINT="$($env:STASIS_BUILD_FINGERPRINT)"
  if ($LASTEXITCODE -ne 0) { throw "Graphics runtime configuration failed." }
  cmake --build $runtimeBuild --config Release --target stasis_graphics
  if ($LASTEXITCODE -ne 0) { throw "Graphics runtime build failed." }

  if (Test-Path $toolchainRoot) { Remove-Item -LiteralPath $toolchainRoot -Recurse -Force }
  New-Item -ItemType Directory -Force -Path $toolchainRoot | Out-Null
  Copy-Item (Join-Path $cargoTarget "release/stasis.exe") $toolchainRoot -Force
  Copy-Item (Join-Path $cargoTarget "release/stasis_dynload.dll") $toolchainRoot -Force
  Copy-Item (Join-Path $runtimeBuild "bin/Release/*.dll") $toolchainRoot -Force
  Copy-Item (Join-Path $repoRoot "src") $toolchainRoot -Recurse -Force

  if ($SigningCertificate) {
    $signtool = (Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Recurse -Filter signtool.exe | Sort-Object FullName -Descending | Select-Object -First 1).FullName
    if (-not $signtool) { throw "signtool.exe was not found." }
    @("stasis.exe", "stasis_graphics.dll") | ForEach-Object {
      $signedFile = Join-Path $toolchainRoot $_
      & $signtool sign /fd SHA256 /f $SigningCertificate /p $SigningPassword /tr http://timestamp.digicert.com /td SHA256 $signedFile
      if ($LASTEXITCODE -ne 0) { throw "Signing failed for $signedFile." }
    }
  } elseif ($env:STASIS_AOT_SIGN_TOOL) {
    $signTool = Get-Command $env:STASIS_AOT_SIGN_TOOL -CommandType Application -ErrorAction SilentlyContinue
    if (-not $signTool) {
      if ($env:STASIS_REQUIRE_SIGNED_EXECUTION -eq "1") {
        throw "Configured signing tool was not found: $env:STASIS_AOT_SIGN_TOOL"
      }
      Write-Warning "Ignoring unavailable optional signing tool: $env:STASIS_AOT_SIGN_TOOL"
    } else {
      @("stasis.exe", "stasis_graphics.dll") | ForEach-Object {
        $signedFile = Join-Path $toolchainRoot $_
        & $signTool.Source $signedFile
        if ($LASTEXITCODE -ne 0) { throw "Configured local signer failed for $signedFile." }
      }
    }
  } elseif ($env:STASIS_REQUIRE_SIGNED_EXECUTION -eq "1") {
    throw "STASIS_REQUIRE_SIGNED_EXECUTION=1 but STASIS_AOT_SIGN_TOOL is not set."
  }

  & (Join-Path $toolchainRoot "stasis.exe") --json editor-info
  if ($LASTEXITCODE -ne 0) { throw "Local editor toolchain identity validation failed." }
  if (Test-Path $toolchainArchive) { Remove-Item -LiteralPath $toolchainArchive -Force }
  Compress-Archive -Path "$toolchainRoot/*" -DestinationPath $toolchainArchive -Force
}

if (-not (Test-Path $toolchainArchive) -or -not (Test-Path (Join-Path $toolchainRoot "stasis.exe"))) {
  throw "Local editor toolchain outputs are missing; rerun without -SkipBuild."
}

$npm = (Get-Command npm.cmd -ErrorAction Stop).Source
$env:STASIS_TOOLCHAIN_DIR = $toolchainRoot
$env:STASIS_TOOLCHAIN_EXECUTABLE = "stasis.exe"
$env:STASIS_E2E_EXECUTABLE = Join-Path $toolchainRoot "stasis.exe"
Push-Location $extensionRoot
try {
  & $npm ci
  if ($LASTEXITCODE -ne 0) { throw "npm ci failed." }
  & $npm test
  if ($LASTEXITCODE -ne 0) { throw "Extension tests failed." }
  if ($RunVsCodeE2E) {
    & $npm run test:e2e
    if ($LASTEXITCODE -ne 0) { throw "Installed VSIX E2E failed." }
  }
  & $npm run package -- --target win32-x64
  if ($LASTEXITCODE -ne 0) { throw "VSIX packaging failed." }
} finally {
  Pop-Location
}

if (Test-Path $OutputRoot) { Remove-Item -LiteralPath $OutputRoot -Recurse -Force }
node (Join-Path $repoRoot "tools/assemble_editor_release.mjs") `
  --toolchain-archive $toolchainArchive `
  --vsix $vsix `
  --out $OutputRoot `
  --release-id $ReleaseId `
  --platform win32-x64
if ($LASTEXITCODE -ne 0) { throw "Editor release assembly failed." }
Write-Host "Local editor release: $OutputRoot"
