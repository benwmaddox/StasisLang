param(
    [Parameter(Mandatory = $true)][string]$Toolchain,
    [Parameter(Mandatory = $true)][string]$Workspace,
    [Parameter(Mandatory = $true)][string]$GameWindowTitle,
    [int]$TimeoutSeconds = 30
)

$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class StasisVisibleWindows {
    private delegate bool EnumWindowsProc(IntPtr window, IntPtr state);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr state);
    [DllImport("user32.dll")]
    private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr window, StringBuilder text, int count);

    public static string[] Titles(uint expectedProcessId) {
        var titles = new List<string>();
        EnumWindows(delegate(IntPtr window, IntPtr state) {
            uint processId;
            GetWindowThreadProcessId(window, out processId);
            if (processId != expectedProcessId || !IsWindowVisible(window)) return true;
            var text = new StringBuilder(512);
            if (GetWindowText(window, text, text.Capacity) > 0) titles.Add(text.ToString());
            return true;
        }, IntPtr.Zero);
        return titles.ToArray();
    }
}
'@

$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = (Resolve-Path -LiteralPath $Toolchain).Path
$start.UseShellExecute = $false
$workspacePath = (Resolve-Path -LiteralPath $Workspace).Path
$start.Arguments = "--workspace `"$workspacePath`" editor"
$process = [Diagnostics.Process]::Start($start)
try {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        if ($process.HasExited) {
            throw "stasis editor exited before publishing both windows (exit $($process.ExitCode))"
        }
        $titles = [StasisVisibleWindows]::Titles([uint32]$process.Id)
        if ($titles -contains "Stasis Editor" -and $titles -contains $GameWindowTitle) {
            Write-Output "verified independent Stasis Editor and $GameWindowTitle windows"
            exit 0
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "stasis editor did not publish both expected windows within ${TimeoutSeconds}s"
} finally {
    if (-not $process.HasExited) {
        $process.Kill()
        $process.WaitForExit()
    }
    $process.Dispose()
}
