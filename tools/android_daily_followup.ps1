param(
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

$repoRoot = "D:\code\StasisLang"
$codex = "C:\Users\Ben\AppData\Local\OpenAI\Codex\bin\codex.exe"
$prompt = @"
Continue the StasisLang Android Workshop branch work.

Daily workflow:
1. Review current state on the android branch.
2. Determine the next smallest useful Android Workshop slice.
3. Implement it with tests or structural verification.
4. Build and, when wireless debugging is available, install/run on the Android phone.
5. Commit only the relevant slice.
6. Push origin android.
7. Summarize changes and any blockers.

Leave unrelated local dirty files alone unless explicitly instructed otherwise.
"@

if (!(Test-Path -LiteralPath $repoRoot)) {
    throw "Repo root not found: $repoRoot"
}

if (!(Test-Path -LiteralPath $codex)) {
    throw "Codex CLI not found: $codex"
}

$args = @("--cd", $repoRoot, $prompt)

if ($DryRun) {
    Write-Output "Codex: $codex"
    Write-Output "WorkingDirectory: $repoRoot"
    Write-Output "Arguments: $($args -join ' ')"
    return
}

Start-Process -FilePath $codex -ArgumentList $args -WorkingDirectory $repoRoot
