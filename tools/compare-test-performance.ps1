param(
  [string]$TestsDir = 'tests',
  [string]$SelfHostExe = 'build/stasis_stage10.exe',
  [string]$Stage0Exe = 'build/aot/Stasis.Cli.exe',
  [string]$Backend = 'llvm',
  [int]$Runs = 2,
  [string]$OutCsv = 'build/test_perf_compare.csv'
)

$ErrorActionPreference = 'Stop'

function Resolve-RepoPath([string]$path) {
  if ([System.IO.Path]::IsPathRooted($path)) { return $path }
  return (Join-Path (Get-Location) $path)
}

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

$repoRoot = (Get-Location).Path
if (!$repoRoot.EndsWith('\')) { $repoRoot = $repoRoot + '\' }

$stage0 = Resolve-RepoPath $Stage0Exe
$selfhost = Resolve-RepoPath $SelfHostExe
$testsRoot = Resolve-RepoPath $TestsDir
$outCsvPath = Resolve-RepoPath $OutCsv

if (!(Test-Path $stage0)) { throw "Stage0 CLI not found: $stage0" }
if (!(Test-Path $selfhost)) { throw "Self-host exe not found: $selfhost" }
if (!(Test-Path $testsRoot)) { throw "Tests dir not found: $testsRoot" }

$files = Get-ChildItem -Recurse $testsRoot -Filter *.stasis | Sort-Object FullName
if ($files.Count -eq 0) { throw "No .stasis files found under: $testsRoot" }

$rows = @()
foreach ($file in $files) {
  $rel = $file.FullName
  if ($rel.StartsWith($repoRoot, [StringComparison]::OrdinalIgnoreCase)) {
    $rel = $rel.Substring($repoRoot.Length)
  }

  $stage0Runs = @()
  $selfRuns = @()
  $stage0Exit = 0
  $selfExit = 0

  for ($i = 0; $i -lt $Runs; $i++) {
    $r0 = Invoke-Timed $stage0 test --backend $Backend $rel
    $stage0Runs += $r0.Ms
    if ($r0.ExitCode -ne 0) { $stage0Exit = $r0.ExitCode }

    $r1 = Invoke-Timed $selfhost test --backend $Backend $rel
    $selfRuns += $r1.Ms
    if ($r1.ExitCode -ne 0) { $selfExit = $r1.ExitCode }
  }

  $row = [ordered]@{
    file = $rel
    stage0_exit = $stage0Exit
    selfhost_exit = $selfExit
  }
  for ($i = 0; $i -lt $Runs; $i++) {
    $row["stage0_ms_run$($i+1)"] = $stage0Runs[$i]
    $row["selfhost_ms_run$($i+1)"] = $selfRuns[$i]
  }
  $rows += [pscustomobject]$row
}

$rows | Export-Csv -NoTypeInformation -Encoding ascii -Path $outCsvPath

$failStage0 = @($rows | Where-Object { $_.stage0_exit -ne 0 }).Count
$failSelf = @($rows | Where-Object { $_.selfhost_exit -ne 0 }).Count

Write-Host "wrote: $outCsvPath"
Write-Host ("files={0} stage0_failed={1} selfhost_failed={2} runs={3} backend={4}" -f $rows.Count, $failStage0, $failSelf, $Runs, $Backend)
