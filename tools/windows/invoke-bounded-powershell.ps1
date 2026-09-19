[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('status', 'provision', 'sign', 'verify')]
    [string] $Command,
    [Parameter(Mandatory = $true)]
    [string] $ScriptPath,
    [string[]] $ScriptArguments,
    [ValidateRange(1, 600)]
    [int] $TimeoutSeconds = 120
)

$ErrorActionPreference = 'Stop'

function Get-ProcessStreamText(
    [System.Threading.Tasks.Task[string]] $Task,
    [string] $StreamName
) {
    # A descendant that keeps an inherited pipe open must not turn cleanup into a
    # second unbounded wait after the supervised process has been terminated.
    if (-not $Task.Wait(5000)) {
        return "[$StreamName output did not close within 5 seconds]"
    }
    return [string]$Task.Result
}

function Write-ProcessOutput([string] $Text, [switch] $ErrorStream) {
    if ([string]::IsNullOrEmpty($Text)) { return }
    if ($ErrorStream) {
        [Console]::Error.Write($Text)
    } else {
        # Keep stdout in the PowerShell pipeline so callers can capture the
        # status JSON; writing directly to Console.Out would bypass that pipe.
        Write-Output $Text
    }
}

function Stop-ProcessTree([Diagnostics.Process] $Process) {
    if ($Process.HasExited) { return }

    $treeStopped = $false
    try {
        # Process.Kill(bool) is available on modern .NET and is the least
        # surprising way to terminate the PowerShell child and its descendants.
        $Process.Kill($true)
        $treeStopped = $true
    } catch {
        # Windows PowerShell 5.1 exposes only Kill(), so use taskkill's tree
        # option when the newer overload is unavailable.
        $taskKill = Get-Command taskkill.exe -CommandType Application -ErrorAction SilentlyContinue
        if ($taskKill) {
            & $taskKill.Source /PID $Process.Id /T /F 2>&1 | Out-Null
            $treeStopped = ($LASTEXITCODE -eq 0)
        }
    }

    if (-not $Process.HasExited) {
        try { $Process.Kill() } catch { }
    }
    if (-not $Process.WaitForExit(5000)) {
        throw "bounded PowerShell command process did not terminate within 5 seconds (treeStopped=$treeStopped)"
    }
}

$process = $null
$exitCode = 1
try {
    $resolvedScript = (Resolve-Path -LiteralPath $ScriptPath -ErrorAction Stop).Path
    $powerShell = (Get-Command pwsh.exe -CommandType Application -ErrorAction Stop).Source

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $powerShell
    $startInfo.WorkingDirectory = (Get-Location).Path
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    if ($null -eq $startInfo.ArgumentList) {
        throw 'bounded PowerShell invocation requires PowerShell 7 or newer'
    }
    foreach ($argument in @(
        '-NoProfile',
        '-NonInteractive',
        '-ExecutionPolicy',
        'Bypass',
        '-File',
        $resolvedScript,
        $Command
    )) {
        $null = $startInfo.ArgumentList.Add($argument)
    }
    if ($ScriptArguments) {
        foreach ($argument in $ScriptArguments) {
            $null = $startInfo.ArgumentList.Add($argument)
        }
    }

    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw 'bounded PowerShell command did not start' }

    $standardOutput = $process.StandardOutput.ReadToEndAsync()
    $standardError = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        Stop-ProcessTree $process
        $stdoutText = Get-ProcessStreamText $standardOutput 'stdout'
        $stderrText = Get-ProcessStreamText $standardError 'stderr'
        Write-ProcessOutput $stdoutText
        Write-ProcessOutput $stderrText -ErrorStream
        throw "bounded PowerShell command '$Command' timed out after $TimeoutSeconds seconds"
    }

    $stdoutText = Get-ProcessStreamText $standardOutput 'stdout'
    $stderrText = Get-ProcessStreamText $standardError 'stderr'
    Write-ProcessOutput $stdoutText
    Write-ProcessOutput $stderrText -ErrorStream
    $exitCode = $process.ExitCode
    if ($exitCode -ne 0) {
        throw "bounded PowerShell command '$Command' failed with exit code $exitCode"
    }
    $exitCode = 0
} catch {
    [Console]::Error.WriteLine("$($_.Exception.Message)")
} finally {
    if ($process) { $process.Dispose() }
}

exit $exitCode
