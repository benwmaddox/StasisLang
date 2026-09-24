param(
    [int]$Samples = 10,
    [int]$Warmups = 2,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
if ($Samples -lt 1 -or $Warmups -lt 0) { throw 'Samples must be positive and Warmups nonnegative.' }
$root = $PSScriptRoot
$names = @('fib', 'mandelbrot', 'matmul', 'nqueens', 'sieve', 'partial_sums', 'life')
$expected = @{
    fib = '39088169'; mandelbrot = '306045'; matmul = '296801'; nqueens = '14200'
    sieve = '9592'; partial_sums = '1'; life = '838'
}
$artifactDir = Join-Path $root 'artifacts'
New-Item -ItemType Directory -Force -Path $artifactDir | Out-Null
$rustLlvmExe = Join-Path $artifactDir 'rust_llvm.exe'
$rustCraneliftExe = Join-Path $artifactDir 'rust_cranelift.exe'
$rustSource = Join-Path $root 'rust_baseline.rs'
$stasisExe = (Get-Command stasis -ErrorAction Stop).Source

function Invoke-Timed([string]$Exe, [string[]]$Arguments) {
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Exe
    $startInfo.Arguments = $Arguments -join ' '
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $process = [System.Diagnostics.Process]::Start($startInfo)
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    $timer.Stop()
    if ($process.ExitCode -ne 0) { throw "$Exe exited $($process.ExitCode): $stderr" }
    return @{ output = $stdout.Trim(); ms = $timer.Elapsed.TotalMilliseconds }
}

function Get-Median([double[]]$Values) {
    $sorted = @($Values | Sort-Object)
    $middle = [int][Math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2 -eq 1) { return [double]$sorted[$middle] }
    return ([double]$sorted[$middle - 1] + [double]$sorted[$middle]) / 2.0
}

if (-not $SkipBuild) {
    foreach ($name in $names) {
        $project = Join-Path $root "stasis/$name"
        & $stasisExe build --workspace $project --mode release --out "build/$name.exe" --signing optional
        if ($LASTEXITCODE -ne 0) { throw "Stasis build failed: $name" }
    }
    $rustFlags = @('--edition=2021', '-C', 'opt-level=3', '-C', 'target-cpu=native', '-C', 'panic=abort')
    & rustc +nightly @rustFlags -Zcodegen-backend=llvm $rustSource -o $rustLlvmExe
    if ($LASTEXITCODE -ne 0) { throw 'Rust LLVM build failed.' }
    & rustc +nightly @rustFlags -Zcodegen-backend=cranelift $rustSource -o $rustCraneliftExe
    if ($LASTEXITCODE -ne 0) { throw 'Rust Cranelift build failed. Install nightly with rustc-codegen-cranelift-preview.' }
}

$rows = @()
$baseOrder = @('aot', 'jit', 'rust_llvm', 'rust_cranelift')
foreach ($name in $names) {
    $project = Join-Path $root "stasis/$name"
    $aotExe = Join-Path $project "build/$name.exe"
    $paths = @{ aot = $aotExe; jit = $stasisExe; rust_llvm = $rustLlvmExe; rust_cranelift = $rustCraneliftExe }
    $arguments = @{ aot = @(); jit = @('run', '--workspace', $project, '--json'); rust_llvm = @($name); rust_cranelift = @($name) }
    $times = @{ aot = @(); jit = @(); rust_llvm = @(); rust_cranelift = @() }
    for ($round = 0; $round -lt $Warmups + $Samples; $round++) {
        for ($offset = 0; $offset -lt $baseOrder.Count; $offset++) {
            $language = $baseOrder[($round + $offset) % $baseOrder.Count]
            $result = Invoke-Timed $paths[$language] $arguments[$language]
            $actual = $result.output
            if ($language -eq 'jit') {
                $jsonStart = $actual.IndexOf('{')
                if ($jsonStart -lt 0) { throw "$name JIT result did not contain JSON: $actual" }
                $jitResult = $actual.Substring($jsonStart) | ConvertFrom-Json
                if (-not $jitResult.ok -or $jitResult.result.backend -ne 'jit' -or $jitResult.result.exit_code -ne 0) {
                    throw "$name JIT result was unsuccessful: $actual"
                }
                $actual = $actual.Substring(0, $jsonStart)
            }
            if ($actual -ne $expected[$name]) {
                throw "$name $language output '$($result.output)', expected '$($expected[$name])'"
            }
            if ($round -ge $Warmups) { $times[$language] += [double]$result.ms }
        }
    }
    $aotMedian = Get-Median $times.aot
    $jitMedian = Get-Median $times.jit
    $llvmMedian = Get-Median $times.rust_llvm
    $craneliftMedian = Get-Median $times.rust_cranelift
    $row = [ordered]@{
        name = $name; expected = $expected[$name]
        aot_ms = [Math]::Round($aotMedian, 3)
        jit_ms = [Math]::Round($jitMedian, 3)
        rust_llvm_ms = [Math]::Round($llvmMedian, 3)
        rust_cranelift_ms = [Math]::Round($craneliftMedian, 3)
        rust_cranelift_over_llvm = [Math]::Round($craneliftMedian / $llvmMedian, 3)
        aot_samples_ms = $times.aot
        jit_samples_ms = $times.jit
        rust_llvm_samples_ms = $times.rust_llvm
        rust_cranelift_samples_ms = $times.rust_cranelift
    }
    $rows += $row
    Write-Output ("{0,-14} Stasis AOT {1,8:N2}  JIT {2,8:N2}  Rust LLVM {3,8:N2}  Cranelift {4,8:N2} ms" -f $name, $aotMedian, $jitMedian, $llvmMedian, $craneliftMedian)
}

$report = [ordered]@{
    timestamp_utc = [DateTime]::UtcNow.ToString('o')
    computer = $env:COMPUTERNAME
    cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)
    os = [System.Environment]::OSVersion.VersionString
    stasis_version = (& $stasisExe version | Out-String).Trim()
    rust_version = (& rustc +nightly --version | Out-String).Trim()
    rust_flags = '--edition=2021 -C opt-level=3 -C target-cpu=native -C panic=abort; -Zcodegen-backend=llvm|cranelift'
    samples = $Samples; warmups = $Warmups
    method = 'Whole-process elapsed time, rotating order: Stasis AOT and both Rust builds exclude compilation; each Stasis JIT launch includes compilation'
    results = $rows
}
$reportPath = Join-Path $artifactDir 'latest.json'
$report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $reportPath
Write-Output "Report: $reportPath"
