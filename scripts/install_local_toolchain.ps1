param(
  [string]$ReleaseId = "",
  [string]$BinRoot = "",
  [switch]$SkipBuild,
  [switch]$TestInjectPromotionFailure,
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
    [switch]$InjectFailure
  )
  $backup = "$Destination.backup-$PID-$([guid]::NewGuid().ToString('N'))"
  $hadPrevious = Test-Path -LiteralPath $Destination
  try {
    if ($hadPrevious) { Move-Item -LiteralPath $Destination -Destination $backup }
    if ($InjectFailure) { throw "test-injected promotion failure" }
    Move-Item -LiteralPath $Staging -Destination $Destination
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
      Remove-Item -LiteralPath $backup -Recurse -Force
    } catch {
      Write-Warning "installed toolchain, but could not remove backup ${backup}: $($_.Exception.Message)"
    }
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
  try {
    Promote-ToolchainDirectory -Staging $testStaging -Destination $testDestination -InjectFailure
    throw "promotion failure injection unexpectedly succeeded"
  } catch {
    if (-not (Test-Path -LiteralPath (Join-Path $testDestination "marker.txt"))) {
      throw "promotion rollback did not restore the prior bin"
    }
    $marker = Get-Content -LiteralPath (Join-Path $testDestination "marker.txt") -Raw
    if ($marker -ne "previous") { throw "promotion rollback restored the wrong bin contents" }
  }
  Write-Host "Toolchain promotion rollback test passed"
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
    Invoke-Bounded -FilePath $python -Arguments @("tools/cargo_cache.py", "run", "--", "cargo", "build", "--manifest-path", (Join-Path $repoRoot "Cargo.toml"), "-p", "stasis", "--release") | Out-Null
    Invoke-Bounded -FilePath $python -Arguments @("tools/cargo_cache.py", "run", "--", "cargo", "build", "--manifest-path", (Join-Path $repoRoot "Cargo.toml"), "-p", "stasis_dynload", "--release") | Out-Null

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
  Copy-Item -LiteralPath (Join-Path $repoRoot "runtime") -Destination (Join-Path $staging "runtime") -Recurse
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

  $editorInfoText = Invoke-Bounded -FilePath (Join-Path $staging "stasis.exe") -Arguments @("--json", "editor-info") -WorkingDirectory $staging
  $editorInfo = $editorInfoText | ConvertFrom-Json
  if ($editorInfo.result.build_fingerprint -ne $fingerprint -or $editorInfo.result.graphics_runtime.build_fingerprint -ne $fingerprint) {
    throw "staged editor-info fingerprint does not match source revision $fingerprint"
  }

  $smokeOutput = Join-Path $repoRoot "target/local-editor-record-smoke"
  if (Test-Path -LiteralPath $smokeOutput) { Remove-Item -LiteralPath $smokeOutput -Recurse -Force }
  Invoke-Bounded -FilePath (Join-Path $staging "stasis.exe") -Arguments @(
    "--workspace", (Join-Path $repoRoot "samples/windows_launch_smoke"), "record",
    "--output", $smokeOutput, "--width", "320", "--height", "180", "--fps", "30", "--frames", "1"
  ) -WorkingDirectory (Join-Path $repoRoot "samples/windows_launch_smoke") | Out-Null
  if (-not (Get-ChildItem -LiteralPath $smokeOutput -Filter "*.png" -File -ErrorAction SilentlyContinue)) {
    throw "bounded record smoke did not produce a PNG frame"
  }

  Promote-ToolchainDirectory -Staging $staging -Destination $binRoot -InjectFailure:($TestInjectPromotionFailure -and $env:STASIS_TEST_MODE -eq "1")
  Write-Host "Installed verified Stasis toolchain in $binRoot (fingerprint $fingerprint)"
} catch {
  if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue }
  throw
}
