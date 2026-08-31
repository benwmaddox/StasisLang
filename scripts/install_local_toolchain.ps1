param(
  [string]$ReleaseId = "",
  [string]$BinRoot = "",
  [switch]$SkipBuild,
  [switch]$TestInjectPromotionFailure,
  [switch]$TestInjectValidationFailure,
  [switch]$TestPromotionOnly,
  [string]$TestPromotionRoot = ""
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
if (-not $BinRoot) { $BinRoot = Join-Path $repoRoot "bin" }
$binRoot = [IO.Path]::GetFullPath($BinRoot)
$repoPrefix = $repoRoot.TrimEnd('\') + [IO.Path]::DirectorySeparatorChar
if (-not $binRoot.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
  throw "BinRoot must stay inside the Stasis repository."
}

$runtimeSourceFiles = @(
  "CMakeLists.txt",
  "MINIMP3-LICENSE.txt",
  "minimp3.h",
  "minimp3_ex.h",
  "stasis_svg.cpp",
  "stasis_svg.h",
  "stasis_asset_path.h",
  "stasis_display_scale.h",
  "stasis_render_contract.h",
  "stasis_renderer_lifecycle.h",
  "stasis_performance_metrics.h",
  "stasis_audio_assets.c",
  "stasis_audio_assets.h",
  "stasis_graphics.c",
  "stasis_sprite_atlas_policy.h",
  "stasis_image_writer.c",
  "stasis_image_writer.h",
  "stasis_runner.manifest",
  "stasis_runner_macos.plist.in",
  "stasis_mobile_aot_runtime.c",
  "stasis_mobile_aot_runtime.h",
  "stasis_mobile_runtime.c",
  "stasis_mobile_runtime.h",
  "stasis_platform_storage.c",
  "stasis_platform_storage.h",
  "stb_truetype.h"
)
$runtimeSourceDirectories = @("third_party/thorvg")

function Invoke-Bounded {
  param(
    [Parameter(Mandatory)] [string]$FilePath,
    [string[]]$Arguments = @(),
    [string]$WorkingDirectory = $repoRoot,
    [int]$TimeoutSeconds = 900
  )
  $startInfo = [Diagnostics.ProcessStartInfo]::new()
  $startInfo.FileName = $FilePath
  $startInfo.WorkingDirectory = $WorkingDirectory
  $startInfo.UseShellExecute = $false
  $startInfo.CreateNoWindow = $true
  $startInfo.RedirectStandardOutput = $true
  $startInfo.RedirectStandardError = $true
  foreach ($argument in $Arguments) {
    if ($startInfo.ArgumentList) {
      [void]$startInfo.ArgumentList.Add($argument)
    } else {
      $escaped = $argument -replace '(\\*)"', '$1$1\"'
      $escaped = $escaped -replace '(\\+)$', '$1$1'
      $startInfo.Arguments += '"' + $escaped + '" '
    }
  }
  $process = [Diagnostics.Process]::new()
  $process.StartInfo = $startInfo
  if (-not $process.Start()) { throw "failed to start $FilePath" }
  $stdoutTask = $process.StandardOutput.ReadToEndAsync()
  $stderrTask = $process.StandardError.ReadToEndAsync()
  if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    throw "$FilePath exceeded $TimeoutSeconds seconds"
  }
  $process.WaitForExit()
  $stdout = $stdoutTask.Result
  $stderr = $stderrTask.Result
  if ($stdout) { Write-Host $stdout }
  if ($stderr) { Write-Host $stderr }
  if ($process.ExitCode -ne 0) {
    throw "$FilePath failed with exit code $($process.ExitCode)"
  }
  return $stdout
}

function Promote-ToolchainDirectory {
  param(
    [Parameter(Mandatory)] [string]$Staging,
    [Parameter(Mandatory)] [string]$Destination,
    [Parameter(Mandatory)] [scriptblock]$PostActivationValidation,
    [switch]$InjectBackupCleanupFailure,
    [switch]$InjectFailure
  )
  $backup = "$Destination.backup-$PID-$([guid]::NewGuid().ToString('N'))"
  $hadPrevious = Test-Path -LiteralPath $Destination
  try {
    if ($hadPrevious) { Move-Item -LiteralPath $Destination -Destination $backup }
    if ($InjectFailure) { throw "test-injected promotion failure" }
    Move-Item -LiteralPath $Staging -Destination $Destination
    & $PostActivationValidation
  } catch {
    if (Test-Path -LiteralPath $Destination) {
      Remove-Item -LiteralPath $Destination -Recurse -Force -ErrorAction SilentlyContinue
    }
    if ($hadPrevious -and (Test-Path -LiteralPath $backup)) {
      Move-Item -LiteralPath $backup -Destination $Destination -Force
    }
    if (Test-Path -LiteralPath $Staging) {
      Remove-Item -LiteralPath $Staging -Recurse -Force -ErrorAction SilentlyContinue
    }
    throw "toolchain promotion failed; previous bin was restored: $($_.Exception.Message)"
  }
  if (Test-Path -LiteralPath $backup) {
    try {
      if ($InjectBackupCleanupFailure) { throw "test-injected backup cleanup failure" }
      Remove-Item -LiteralPath $backup -Recurse -Force -ErrorAction Stop
    } catch {
      Write-Warning "installed toolchain, but could not remove backup ${backup}: $($_.Exception.Message)"
    }
  }
}

function Copy-RuntimeSources {
  param(
    [Parameter(Mandatory)] [string]$SourceRoot,
    [Parameter(Mandatory)] [string]$Destination
  )
  $runtimeDestination = Join-Path $Destination "runtime"
  New-Item -ItemType Directory -Force -Path $runtimeDestination | Out-Null
  foreach ($relative in $runtimeSourceFiles) {
    $source = Join-Path (Join-Path $SourceRoot "runtime") $relative
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
      throw "runtime source input is missing: $relative"
    }
    Copy-Item -LiteralPath $source -Destination (Join-Path $runtimeDestination $relative) -Force
  }
  foreach ($relative in $runtimeSourceDirectories) {
    $source = Join-Path (Join-Path $SourceRoot "runtime") $relative
    if (-not (Test-Path -LiteralPath $source -PathType Container)) {
      throw "runtime source directory is missing: $relative"
    }
    $destinationPath = Join-Path $runtimeDestination $relative
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destinationPath) | Out-Null
    Copy-Item -LiteralPath $source -Destination $destinationPath -Recurse -Force
  }
}

function Assert-CompleteToolchainStaging {
  param([Parameter(Mandatory)] [string]$Root)
  $required = @(
    "stasis.exe", "stasis_dynload.dll", "stasis_dynload.dll.lib",
    "stasis_graphics.dll", "stasis_runner.exe", "src", "runtime", "mobile",
    "tools/windows"
  )
  foreach ($relative in $required) {
    if (-not (Test-Path -LiteralPath (Join-Path $Root $relative))) {
      throw "incomplete toolchain staging: missing $relative"
    }
  }
}

if ($TestPromotionOnly) {
  if ($env:STASIS_TEST_MODE -ne "1") {
    throw "TestPromotionOnly requires STASIS_TEST_MODE=1"
  }
  if (-not $TestPromotionRoot) { throw "TestPromotionRoot is required for TestPromotionOnly" }
  $testRoot = [IO.Path]::GetFullPath($TestPromotionRoot)
  $testDestination = Join-Path $testRoot "bin"
  $testStaging = Join-Path $testRoot "staging"
  New-Item -ItemType Directory -Force -Path $testDestination, $testStaging | Out-Null
  Set-Content -LiteralPath (Join-Path $testDestination "marker.txt") -Value "previous" -NoNewline
  Set-Content -LiteralPath (Join-Path $testStaging "marker.txt") -Value "new" -NoNewline
  $incompleteRejected = $false
  try { Assert-CompleteToolchainStaging -Root $testStaging } catch { $incompleteRejected = $true }
  if (-not $incompleteRejected) { throw "incomplete staging unexpectedly passed validation" }
  $marker = Get-Content -LiteralPath (Join-Path $testDestination "marker.txt") -Raw
  if ($marker -ne "previous") { throw "incomplete staging changed the prior bin" }

  $successRoot = Join-Path $testRoot "success"
  $successDestination = Join-Path $successRoot "bin"
  $successStaging = Join-Path $successRoot "staging"
  New-Item -ItemType Directory -Force -Path $successDestination, $successStaging | Out-Null
  Set-Content -LiteralPath (Join-Path $successDestination "marker.txt") -Value "previous" -NoNewline
  Set-Content -LiteralPath (Join-Path $successStaging "marker.txt") -Value "new" -NoNewline
  Promote-ToolchainDirectory -Staging $successStaging -Destination $successDestination -PostActivationValidation {
    $candidateMarker = Get-Content -LiteralPath (Join-Path $successDestination "marker.txt") -Raw
    if ($candidateMarker -ne "new") { throw "post-activation validation did not see the candidate" }
    Set-Content -LiteralPath (Join-Path $successDestination "validation-ran.txt") -Value "yes" -NoNewline
  }
  $marker = Get-Content -LiteralPath (Join-Path $successDestination "marker.txt") -Raw
  if ($marker -ne "new") { throw "successful promotion did not activate the candidate" }
  if (-not (Test-Path -LiteralPath (Join-Path $successDestination "validation-ran.txt"))) {
    throw "post-activation validation did not run"
  }
  if (Test-Path -LiteralPath $successStaging) { throw "successful promotion left staging behind" }
  if (Get-ChildItem -LiteralPath $successRoot -Filter "bin.backup-*" -ErrorAction SilentlyContinue) {
    throw "successful promotion left a backup behind"
  }

  $cleanupRoot = Join-Path $testRoot "cleanup-failure"
  $cleanupDestination = Join-Path $cleanupRoot "bin"
  $cleanupStaging = Join-Path $cleanupRoot "staging"
  New-Item -ItemType Directory -Force -Path $cleanupDestination, $cleanupStaging | Out-Null
  Set-Content -LiteralPath (Join-Path $cleanupDestination "marker.txt") -Value "previous" -NoNewline
  Set-Content -LiteralPath (Join-Path $cleanupDestination "prior-only.txt") -Value "keep" -NoNewline
  Set-Content -LiteralPath (Join-Path $cleanupStaging "marker.txt") -Value "new" -NoNewline
  $cleanupWarnings = @(
    Promote-ToolchainDirectory -Staging $cleanupStaging -Destination $cleanupDestination -PostActivationValidation {
      $candidateMarker = Get-Content -LiteralPath (Join-Path $cleanupDestination "marker.txt") -Raw
      if ($candidateMarker -ne "new") { throw "post-activation validation did not see the candidate" }
      Set-Content -LiteralPath (Join-Path $cleanupDestination "validation-ran.txt") -Value "yes" -NoNewline
    } -InjectBackupCleanupFailure 3>&1 | ForEach-Object { $_.ToString() }
  )
  $marker = Get-Content -LiteralPath (Join-Path $cleanupDestination "marker.txt") -Raw
  if ($marker -ne "new") { throw "backup cleanup failure restored the prior bin" }
  if (-not (Test-Path -LiteralPath (Join-Path $cleanupDestination "validation-ran.txt"))) {
    throw "backup cleanup failure skipped post-activation validation"
  }
  if (Test-Path -LiteralPath (Join-Path $cleanupDestination "prior-only.txt")) {
    throw "backup cleanup failure restored prior-only contents"
  }
  if (Test-Path -LiteralPath $cleanupStaging) { throw "backup cleanup failure left staging behind" }
  if (-not (Get-ChildItem -LiteralPath $cleanupRoot -Filter "bin.backup-*" -ErrorAction SilentlyContinue)) {
    throw "backup cleanup failure did not leave the backup for manual cleanup"
  }
  if (-not ($cleanupWarnings -match "could not remove backup")) {
    throw "backup cleanup failure did not report a warning"
  }

  $rollbackRoot = Join-Path $testRoot "rollback"
  $rollbackDestination = Join-Path $rollbackRoot "bin"
  $rollbackStaging = Join-Path $rollbackRoot "staging"
  New-Item -ItemType Directory -Force -Path $rollbackDestination, $rollbackStaging | Out-Null
  Set-Content -LiteralPath (Join-Path $rollbackDestination "marker.txt") -Value "previous" -NoNewline
  Set-Content -LiteralPath (Join-Path $rollbackDestination "prior-only.txt") -Value "keep" -NoNewline
  Set-Content -LiteralPath (Join-Path $rollbackStaging "marker.txt") -Value "new" -NoNewline
  try {
    Promote-ToolchainDirectory -Staging $rollbackStaging -Destination $rollbackDestination -PostActivationValidation {
      $candidateMarker = Get-Content -LiteralPath (Join-Path $rollbackDestination "marker.txt") -Raw
      if ($candidateMarker -ne "new") { throw "post-activation validation did not see the candidate" }
      Set-Content -LiteralPath (Join-Path $rollbackDestination "validation-ran.txt") -Value "yes" -NoNewline
      throw "test-injected post-activation validation failure"
    }
    throw "post-activation validation failure injection unexpectedly succeeded"
  } catch {
    if (-not (Test-Path -LiteralPath (Join-Path $rollbackDestination "prior-only.txt"))) {
      throw "validation rollback did not restore the prior bin"
    }
    $marker = Get-Content -LiteralPath (Join-Path $rollbackDestination "marker.txt") -Raw
    if ($marker -ne "previous") { throw "validation rollback restored the wrong bin contents" }
    if (Test-Path -LiteralPath (Join-Path $rollbackDestination "validation-ran.txt")) {
      throw "validation rollback left candidate contents behind"
    }
    if (Test-Path -LiteralPath $rollbackStaging) { throw "validation rollback left staging behind" }
    if (Get-ChildItem -LiteralPath $rollbackRoot -Filter "bin.backup-*" -ErrorAction SilentlyContinue) {
      throw "validation rollback left a backup behind"
    }
  }

  try {
    Promote-ToolchainDirectory -Staging $testStaging -Destination $testDestination -PostActivationValidation { } -InjectFailure
    throw "promotion failure injection unexpectedly succeeded"
  } catch {
    if (-not (Test-Path -LiteralPath (Join-Path $testDestination "marker.txt"))) {
      throw "promotion rollback did not restore the prior bin"
    }
    $marker = Get-Content -LiteralPath (Join-Path $testDestination "marker.txt") -Raw
    if ($marker -ne "previous") { throw "promotion rollback restored the wrong bin contents" }
    if (Test-Path -LiteralPath $testStaging) { throw "promotion rollback left staging behind" }
    if (Get-ChildItem -LiteralPath $testRoot -Filter "bin.backup-*" -ErrorAction SilentlyContinue) {
      throw "promotion rollback left a backup behind"
    }
  }
  Write-Host "Toolchain promotion transaction tests passed"
  exit 0
}

$sourceCommit = (Invoke-Bounded -FilePath "git" -Arguments @("-C", $repoRoot, "rev-parse", "HEAD")).Trim()
$commonGitDir = (Invoke-Bounded -FilePath "git" -Arguments @(
  "-C", $repoRoot, "rev-parse", "--path-format=absolute", "--git-common-dir"
)).Trim()
$cargoTarget = Join-Path (Split-Path -Parent $commonGitDir) "build/codex-cargo-target"
$status = (Invoke-Bounded -FilePath "git" -Arguments @("-C", $repoRoot, "status", "--porcelain"))
if ($status.Trim()) { throw "local toolchain build requires a clean source revision" }
if (-not $ReleaseId) { $ReleaseId = "local-$($sourceCommit.Substring(0, 12))" }
$python = (Get-Command python -ErrorAction Stop).Source
$fingerprint = (Invoke-Bounded -FilePath $python -Arguments @(
  "tools/compute_toolchain_fingerprint.py", "--source-commit", $sourceCommit,
  "--release-id", $ReleaseId
)).Trim()
$env:STASIS_RELEASE_ID = $ReleaseId
$env:STASIS_SOURCE_COMMIT = $sourceCommit
$env:STASIS_BUILD_TARGET = "x86_64-pc-windows-msvc"
$env:STASIS_BUILD_FINGERPRINT = $fingerprint

$runtimeBuild = Join-Path $repoRoot "target/local-editor-runtime"
$staging = "$binRoot.staging-$PID-$([guid]::NewGuid().ToString('N'))"
if (Test-Path -LiteralPath $staging) { throw "fresh staging directory already exists: $staging" }

try {
  if (-not $SkipBuild) {
    Invoke-Bounded -FilePath $python -Arguments @(
      "tools/cargo_cache.py", "run", "--", "cargo", "build",
      "--manifest-path", (Join-Path $repoRoot "Cargo.toml"),
      "-p", "stasis", "-p", "stasis_dynload", "--release"
    ) | Out-Null

    $vcpkgRoot = $env:VCPKG_INSTALLATION_ROOT
    if (-not $vcpkgRoot) { $vcpkgRoot = "C:\vcpkg" }
    if (-not (Test-Path -LiteralPath (Join-Path $vcpkgRoot "vcpkg.exe"))) {
      throw "vcpkg.exe was not found. Set VCPKG_INSTALLATION_ROOT."
    }
    $generatorScript = Join-Path $repoRoot "tools/windows/select-cmake-vs-generator.ps1"
    $generator = (& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $generatorScript).Trim()
    if ($LASTEXITCODE -ne 0) { throw "Visual Studio generator detection failed." }
    Invoke-Bounded -FilePath "cmake" -Arguments @(
      "-S", (Join-Path $repoRoot "runtime"), "-B", $runtimeBuild, "-G", $generator, "-A", "x64",
      "-DCMAKE_TOOLCHAIN_FILE=$(Join-Path $vcpkgRoot 'scripts/buildsystems/vcpkg.cmake')",
      "-DVCPKG_TARGET_TRIPLET=x64-windows-static", "-DSTASIS_GRAPHICS_BUILD_SHARED=ON",
      "-DSTASIS_GRAPHICS_BUILD_STATIC=OFF", "-DSTASIS_GRAPHICS_BUNDLE_SDL=ON",
      "-DSTASIS_GRAPHICS_SDL_ONLY=ON", "-DSTASIS_BUILD_RUNNER=ON", "-DSTASIS_BUILD_SYS=OFF",
      "-DSTASIS_RELEASE_ID=$ReleaseId", "-DSTASIS_BUILD_FINGERPRINT=$fingerprint"
    ) | Out-Null
    Invoke-Bounded -FilePath "cmake" -Arguments @("--build", $runtimeBuild, "--config", "Release", "--target", "stasis_graphics", "stasis_runner") | Out-Null
  }

  New-Item -ItemType Directory -Force -Path $staging | Out-Null
  $cli = Join-Path $cargoTarget "release/stasis.exe"
  $dynload = Join-Path $cargoTarget "release/stasis_dynload.dll"
  $dynloadImport = Join-Path $cargoTarget "release/stasis_dynload.dll.lib"
  $runtime = Join-Path $runtimeBuild "bin/Release/stasis_graphics.dll"
  $runner = Join-Path $runtimeBuild "bin/Release/stasis_runner.exe"
  foreach ($required in @($cli, $dynload, $dynloadImport, $runtime, $runner)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) { throw "required build output is missing: $required" }
  }
  Copy-Item -LiteralPath $cli -Destination (Join-Path $staging "stasis.exe")
  Copy-Item -LiteralPath $dynload -Destination (Join-Path $staging "stasis_dynload.dll")
  Copy-Item -LiteralPath $dynloadImport -Destination (Join-Path $staging "stasis_dynload.dll.lib")
  Copy-Item -LiteralPath $runtime -Destination (Join-Path $staging "stasis_graphics.dll")
  Get-ChildItem -LiteralPath (Split-Path -Parent $runtime) -Filter "*.dll" -File |
    Copy-Item -Destination $staging -Force
  Copy-Item -LiteralPath $runner -Destination (Join-Path $staging "stasis_runner.exe")
  Copy-Item -LiteralPath (Join-Path $repoRoot "src") -Destination (Join-Path $staging "src") -Recurse
  Copy-RuntimeSources -SourceRoot $repoRoot -Destination $staging
  Copy-Item -LiteralPath (Join-Path $repoRoot "mobile") -Destination (Join-Path $staging "mobile") -Recurse
  Copy-Item -LiteralPath (Join-Path $repoRoot "tools/windows") -Destination (Join-Path $staging "tools/windows") -Recurse
  Copy-Item -LiteralPath (Join-Path $repoRoot "README.md") -Destination (Join-Path $staging "README.md")
  Copy-Item -LiteralPath (Join-Path $repoRoot "LICENSE") -Destination (Join-Path $staging "LICENSE")
  if (Test-Path -LiteralPath (Join-Path $repoRoot "docs/knowledge")) {
    New-Item -ItemType Directory -Force -Path (Join-Path $staging "docs") | Out-Null
    Copy-Item -LiteralPath (Join-Path $repoRoot "docs/knowledge") -Destination (Join-Path $staging "docs/knowledge") -Recurse
  }

  $signingArtifacts = @(
    (Join-Path $staging "stasis.exe"),
    (Join-Path $staging "stasis_dynload.dll"),
    (Join-Path $staging "stasis_graphics.dll"),
    (Join-Path $staging "stasis_runner.exe")
  )
  if ($env:STASIS_AOT_SIGN_TOOL) {
    $signTool = Get-Command $env:STASIS_AOT_SIGN_TOOL -CommandType Application -ErrorAction SilentlyContinue
    if (-not $signTool) {
      throw "configured signing tool was not found: $env:STASIS_AOT_SIGN_TOOL"
    }
    foreach ($artifact in $signingArtifacts) {
      & $signTool.Source $artifact
      if ($LASTEXITCODE -ne 0) { throw "configured local signer failed for $artifact" }
    }
  } elseif ($env:STASIS_REQUIRE_SIGNED_EXECUTION -eq "1") {
    throw "STASIS_REQUIRE_SIGNED_EXECUTION=1 but STASIS_AOT_SIGN_TOOL is not set"
  }
  Assert-CompleteToolchainStaging -Root $staging

  $smokeOutput = Join-Path $repoRoot "target/local-editor-record-smoke"
  $postActivationValidation = {
    $installedExecutable = Join-Path $binRoot "stasis.exe"
    $editorInfoText = Invoke-Bounded -FilePath $installedExecutable -Arguments @("--json", "editor-info") -WorkingDirectory $binRoot
    $editorInfo = $editorInfoText | ConvertFrom-Json
    if ($editorInfo.result.build_fingerprint -ne $fingerprint -or $editorInfo.result.graphics_runtime.build_fingerprint -ne $fingerprint) {
      throw "activated editor-info fingerprint does not match source revision $fingerprint"
    }

    if (Test-Path -LiteralPath $smokeOutput) { Remove-Item -LiteralPath $smokeOutput -Recurse -Force }
    Invoke-Bounded -FilePath $installedExecutable -Arguments @(
      "--workspace", (Join-Path $repoRoot "samples/windows_launch_smoke"), "record",
      "--output", $smokeOutput, "--width", "320", "--height", "180", "--fps", "30", "--frames", "1"
    ) -WorkingDirectory $binRoot | Out-Null
    if (-not (Get-ChildItem -LiteralPath $smokeOutput -Filter "*.png" -File -ErrorAction SilentlyContinue)) {
      throw "bounded record smoke did not produce a PNG frame"
    }
    if ($TestInjectValidationFailure -and $env:STASIS_TEST_MODE -eq "1") {
      throw "test-injected post-activation validation failure"
    }
  }

  Promote-ToolchainDirectory -Staging $staging -Destination $binRoot -PostActivationValidation $postActivationValidation -InjectFailure:($TestInjectPromotionFailure -and $env:STASIS_TEST_MODE -eq "1")
  Write-Host "Installed verified Stasis toolchain in $binRoot (fingerprint $fingerprint)"
} catch {
  if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue }
  throw
}
