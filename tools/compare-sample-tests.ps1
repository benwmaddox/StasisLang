param(
  [string[]]$Roots = @('samples', 'examples'),
  [string]$SelfHostExe = 'build/stasis_release.exe',
  [string]$Stage0Exe = 'build/aot/Stasis.Cli.exe',
  [string]$Backend = 'llvm',
  [string]$OutCsv = 'build/sample_test_compare.csv'
)

$ErrorActionPreference = 'Stop'

function Resolve-RepoPath([string]$path) {
  if ([System.IO.Path]::IsPathRooted($path)) { return $path }
  return (Join-Path (Get-Location) $path)
}

function Find-LatestSelfHostExe {
  $candidates = @(Get-ChildItem -ErrorAction SilentlyContinue build -Filter 'stasis_stage*.exe')
  if ($candidates.Count -eq 0) { return $null }
  return ($candidates | Sort-Object LastWriteTime | Select-Object -Last 1).FullName
}

$releaseCandidate = Resolve-RepoPath 'build/stasis_release.exe'

function Invoke-Timed {
  param(
    [string]$exe,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$args
  )
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $old = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  & $exe @args 1>$null 2>$null 3>$null 4>$null 5>$null 6>$null
  $ErrorActionPreference = $old
  $exitCode = $LASTEXITCODE
  $sw.Stop()
  [pscustomobject]@{
    ExitCode = $exitCode
    Ms = [int]$sw.Elapsed.TotalMilliseconds
  }
}

function Has-Tests([string]$path) {
  $text = Get-Content $path -Raw
  return ($text -match '(?m)^\s*test\s+')
}

function Uses-Graphics([string]$path) {
  $text = Get-Content $path -Raw
  return ($text -match 'graphics\.stasis' -or $text -match '\bgfx_' -or $text -match 'stasis_graphics')
}

$repoRoot = (Get-Location).Path
if (!$repoRoot.EndsWith('\')) { $repoRoot = $repoRoot + '\' }

$stage0 = Resolve-RepoPath $Stage0Exe
$selfhost = Resolve-RepoPath $SelfHostExe
$outCsvPath = Resolve-RepoPath $OutCsv

if (!(Test-Path $stage0)) { throw "Stage0 CLI not found: $stage0" }

if (!$PSBoundParameters.ContainsKey('SelfHostExe')) {
  if (Test-Path $releaseCandidate) {
    $selfhost = $releaseCandidate
  } else {
    $latest = Find-LatestSelfHostExe
    if ($null -ne $latest) { $selfhost = $latest }
  }
}

if (!(Test-Path $selfhost)) {
  $latest = Find-LatestSelfHostExe
  if ($null -eq $latest) { throw "Self-host exe not found: $selfhost" }
  $selfhost = $latest
}

$files = @()
foreach ($root in $Roots) {
  if (!(Test-Path $root)) { continue }
  $files += Get-ChildItem -Recurse $root -Filter *.stasis
}

$files = @($files | Sort-Object FullName)
if ($files.Count -eq 0) { throw "No .stasis files found under: $($Roots -join ', ')" }

$rows = @()
foreach ($file in $files) {
  $path = $file.FullName
  if (!(Has-Tests $path)) { continue }

  $rel = $path
  if ($rel.StartsWith($repoRoot, [StringComparison]::OrdinalIgnoreCase)) {
    $rel = $rel.Substring($repoRoot.Length)
  }

  $graphics = Uses-Graphics $path
  $row = [ordered]@{
    file = $rel
    graphics = $graphics
    stage0_exit = ''
    stage0_ms = ''
    selfhost_exit = ''
    selfhost_ms = ''
    note = ''
  }

  if ($graphics) {
    $row.note = 'skipped (graphics/interactive)'
    $rows += [pscustomobject]$row
    continue
  }

  $r0 = Invoke-Timed $stage0 test --backend $Backend $rel
  $r1 = Invoke-Timed $selfhost test --backend $Backend $rel

  $row.stage0_exit = $r0.ExitCode
  $row.stage0_ms = $r0.Ms
  $row.selfhost_exit = $r1.ExitCode
  $row.selfhost_ms = $r1.Ms
  $rows += [pscustomobject]$row
}

$rows | Export-Csv -NoTypeInformation -Encoding ascii -Path $outCsvPath

$failed0 = @($rows | Where-Object { $_.stage0_exit -ne '' -and $_.stage0_exit -ne '0' }).Count
$failed1 = @($rows | Where-Object { $_.selfhost_exit -ne '' -and $_.selfhost_exit -ne '0' }).Count
$skipped = @($rows | Where-Object { $_.note -ne '' }).Count

Write-Host "wrote: $outCsvPath"
Write-Host ("files_with_tests={0} skipped={1} stage0_failed={2} selfhost_failed={3} backend={4}" -f $rows.Count, $skipped, $failed0, $failed1, $Backend)
