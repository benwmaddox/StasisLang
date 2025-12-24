param(
    [ValidateSet('cranelift', 'llvm')]
    [string]$Backend = 'cranelift',

    [bool]$IncludeExamples = $true
)

$ErrorActionPreference = 'Continue'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $repoRoot

try {
    $dotnetBaseArgs = @('run', '--project', 'Stasis.Cli/Stasis.Cli.csproj', '-c', 'Release', '--')

    $files = @()
    $files += Get-ChildItem samples -Recurse -Filter *.stasis | ForEach-Object { $_.FullName }
    if ($IncludeExamples) {
        $files += Get-ChildItem examples -Recurse -Filter *.stasis -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName }
    }
    $files = $files | Sort-Object

    $results = New-Object System.Collections.Generic.List[object]

    foreach ($file in $files) {
        $rel = Resolve-Path $file -Relative
        $text = Get-Content $file -Raw

        $hasTest = $text -match '(?m)^\s*test\s+'
        $hasMain = $text -match '(?m)^\s*(export\s+)?function\s+main\s*\('

        if (-not $hasTest -and -not $hasMain) {
            $results.Add([pscustomobject]@{
                    file = $rel
                    action = 'skip'
                    backend = $Backend
                    ok = $true
                    seconds = 0.0
                    exit_code = 0
                    note = 'no main/tests (module/library)'
                })
            continue
        }

        $actionArgs = if ($hasTest) { @('test', $rel) } else { @('build', $rel) }
        $action = if ($hasTest) { 'test' } else { 'build' }

        if ($Backend -ne 'cranelift') {
            $actionArgs += @('--backend', $Backend)
        }

        Write-Host ("=== {0} ({1}) {2}" -f $action, $Backend, $rel)
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $outObj = & dotnet @dotnetBaseArgs @actionArgs 2>&1
        $exitCode = $LASTEXITCODE
        $sw.Stop()

        $outText = ($outObj | ForEach-Object { $_.ToString() }) -join "`n"
        $ok = ($exitCode -eq 0)

        $results.Add([pscustomobject]@{
                file = $rel
                action = $action
                backend = $Backend
                ok = $ok
                seconds = [math]::Round($sw.Elapsed.TotalSeconds, 2)
                exit_code = $exitCode
                note = if ($ok) { '' } else { $outText }
            })
    }

    $ran = @($results | Where-Object { $_.action -ne 'skip' })
    $skipped = @($results | Where-Object { $_.action -eq 'skip' })
    $failed = @($results | Where-Object { -not $_.ok })

    Write-Host ''
    Write-Host ("Ran: {0}  Skipped: {1}  Failed: {2}" -f $ran.Count, $skipped.Count, $failed.Count)
    if ($failed.Count -gt 0) {
        Write-Host ''
        Write-Host 'Failures:'
        $failed | Select-Object file, action, backend, seconds, exit_code | Format-Table -AutoSize
        exit 1
    }
}
finally {
    Pop-Location
}
