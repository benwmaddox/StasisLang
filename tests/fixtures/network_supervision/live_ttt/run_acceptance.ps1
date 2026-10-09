param(
    [string]$Toolchain,
    [string]$Supervisor,
    [string]$Peer,
    [string]$EvidenceRoot
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../../../..")).Path
$cargoTarget = Join-Path $repoRoot "build/codex-cargo-target"
$authoritySource = Join-Path $PSScriptRoot "authority"
$runRoot = Join-Path ([IO.Path]::GetTempPath()) ("stasis-live-ttt-" + [Guid]::NewGuid().ToString("N"))
$workspace = Join-Path $runRoot "workspace"

function Quote-ProcessArgument([string]$Value) {
    if ($Value.Contains('"')) {
        throw "process argument contains an unsupported quote"
    }
    return '"' + $Value + '"'
}

function Invoke-BoundedProcess(
    [string]$FilePath,
    [string[]]$ArgumentList,
    [string]$WorkingDirectory,
    [int]$TimeoutMs
) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $FilePath
    $start.Arguments = (@($ArgumentList | ForEach-Object { Quote-ProcessArgument $_ }) -join " ")
    $start.WorkingDirectory = $WorkingDirectory
    $start.UseShellExecute = $false
    $process = [Diagnostics.Process]::Start($start)
    try {
        if (!$process.WaitForExit($TimeoutMs)) {
            & taskkill.exe /PID $process.Id /T /F 2>&1 | Out-Null
            $process.WaitForExit(5000) | Out-Null
            throw "bounded process timed out: $([IO.Path]::GetFileName($FilePath))"
        }
        if ($process.ExitCode -ne 0) {
            throw "bounded process failed: $([IO.Path]::GetFileName($FilePath))"
        }
    } finally {
        $process.Dispose()
    }
}

function Invoke-Cargo([string[]]$CargoArgs) {
    $arguments = @((Join-Path $repoRoot "tools/cargo_cache.py"), "run", "--", "cargo") + $CargoArgs
    Invoke-BoundedProcess "python" $arguments $repoRoot 900000
}

function Assert-Executable([string]$Path, [string]$Label) {
    if ([string]::IsNullOrWhiteSpace($Path) -or !(Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label executable is unavailable"
    }
    return (Resolve-Path -LiteralPath $Path).Path
}

if ([string]::IsNullOrWhiteSpace($Toolchain)) {
    Invoke-Cargo @("build", "-p", "stasis")
    $Toolchain = Join-Path $cargoTarget "debug/stasis.exe"
}
if ([string]::IsNullOrWhiteSpace($Supervisor)) {
    Invoke-Cargo @(
        "build", "-p", "stasis_network",
        "--features", "supervision-cli",
        "--bin", "stasis-network-supervise"
    )
    $Supervisor = Join-Path $cargoTarget "debug/stasis-network-supervise.exe"
}
if ([string]::IsNullOrWhiteSpace($Peer)) {
    Invoke-Cargo @("build", "-p", "stasis_network", "--example", "supervision_live_ttt_peer")
    $Peer = Join-Path $cargoTarget "debug/examples/supervision_live_ttt_peer.exe"
}
$Toolchain = Assert-Executable $Toolchain "Stasis toolchain"
$Supervisor = Assert-Executable $Supervisor "network supervisor"
$Peer = Assert-Executable $Peer "live Tic-Tac-Toe peer"

New-Item -ItemType Directory -Path $runRoot | Out-Null
Copy-Item -LiteralPath $authoritySource -Destination $workspace -Recurse
$stdlib = Join-Path $workspace ".stasis_cache/toolchain/src/stdlib"
New-Item -ItemType Directory -Path (Split-Path $stdlib -Parent) -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $repoRoot "src/stdlib") -Destination $stdlib -Recurse

if ([string]::IsNullOrWhiteSpace($EvidenceRoot)) {
    $EvidenceRoot = Join-Path $runRoot "evidence"
    New-Item -ItemType Directory -Path $EvidenceRoot | Out-Null
} else {
    if (![IO.Path]::IsPathRooted($EvidenceRoot) -or !(Test-Path -LiteralPath $EvidenceRoot -PathType Container)) {
        throw "evidence root must be an existing absolute directory"
    }
    $EvidenceRoot = (Resolve-Path -LiteralPath $EvidenceRoot).Path
}
if (@(Get-ChildItem -LiteralPath $EvidenceRoot -Force).Count -ne 0) {
    throw "evidence root must be empty"
}
$tempRoot = (Resolve-Path -LiteralPath ([IO.Path]::GetTempPath())).Path.TrimEnd('\')
if (!$EvidenceRoot.StartsWith($tempRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw "evidence root must be a dedicated directory beneath TEMP"
}

$env:SDL_VIDEODRIVER = "dummy"
$env:STASIS_NETWORK_ADVERTISE_IPV4 = "127.0.0.1"
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = $Supervisor
$arguments = @(
    "--toolchain", $Toolchain,
    "--workspace", $workspace,
    "--peer", $Peer,
    "--evidence-root", $EvidenceRoot,
    "--startup-ms", "60000",
    "--action-ms", "30000",
    "--shutdown-ms", "10000"
)
$start.Arguments = (@($arguments | ForEach-Object { Quote-ProcessArgument $_ }) -join " ")
$start.WorkingDirectory = $repoRoot
$start.UseShellExecute = $false
$start.RedirectStandardInput = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$supervised = [Diagnostics.Process]::Start($start)
$script:requestId = 0
$script:captureResponses = @{}

function Stop-SupervisedProcess {
    if ($null -ne $supervised -and !$supervised.HasExited) {
        & taskkill.exe /PID $supervised.Id /T /F 2>&1 | Out-Null
        $supervised.WaitForExit(5000) | Out-Null
    }
}

function Send-Request([hashtable]$Command, [string]$ExpectedEvent, [string]$ExpectedErrorCode = "") {
    if ($script:requestId -ge 480) {
        throw "acceptance request budget exhausted before the 512-request supervisor limit"
    }
    $script:requestId += 1
    $request = [ordered]@{
        schema_version = 1
        request_id = $script:requestId
        command = $Command
    }
    $line = $request | ConvertTo-Json -Compress -Depth 8
    $supervised.StandardInput.WriteLine($line)
    $supervised.StandardInput.Flush()
    $read = $supervised.StandardOutput.ReadLineAsync()
    if (!$read.Wait(30000)) {
        throw "supervisor response timed out"
    }
    $responseLine = $read.Result
    if ([string]::IsNullOrWhiteSpace($responseLine)) {
        throw "supervisor response stream ended"
    }
    $response = $responseLine | ConvertFrom-Json
    $keys = @($response.PSObject.Properties.Name)
    $required = @("schema_version", "request_id", "tick", "ok", "event")
    foreach ($key in $required) {
        if ($keys -notcontains $key) {
            throw "supervisor response omitted $key"
        }
    }
    foreach ($key in $keys) {
        if ($key -notin @("schema_version", "request_id", "tick", "ok", "event", "capture", "error_code")) {
            throw "supervisor response exposed an unexpected field"
        }
    }
    if ($response.schema_version -ne 1 -or $response.request_id -ne $script:requestId -or $response.event -ne $ExpectedEvent) {
        throw "supervisor response did not match request"
    }
    if ($ExpectedEvent -eq "rejected") {
        if ($response.ok -or $response.error_code -ne $ExpectedErrorCode -or $keys -contains "capture") {
            throw "supervisor rejection did not match the fixed error contract"
        }
    } else {
        if (!$response.ok -or $keys -contains "error_code") {
            throw "successful supervisor response included an error"
        }
    }
    return $response
}

function Send-Step {
    $null = Send-Request ([ordered]@{ type = "step"; ticks = 1 }) "step_scheduled"
    Start-Sleep -Milliseconds 20
}

function Send-Gesture([int]$X, [int]$Y) {
    $down = [ordered]@{
        id = 0; x = $X; y = $Y
        is_down = $true; went_down = $true; went_up = $false
    }
    $up = [ordered]@{
        id = 0; x = $X; y = $Y
        is_down = $false; went_down = $false; went_up = $true
    }
    $null = Send-Request ([ordered]@{ type = "set_input_state"; pointers = @($down) }) "input_applied"
    Send-Step
    $null = Send-Request ([ordered]@{ type = "set_input_state"; pointers = @($up) }) "input_applied"
    Send-Step
}

function Read-Receipts([switch]$Strict) {
    $path = Join-Path $EvidenceRoot "peer-receipts.jsonl"
    if (!(Test-Path -LiteralPath $path -PathType Leaf)) {
        return @()
    }
    $receipts = @()
    foreach ($line in @(Get-Content -LiteralPath $path)) {
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }
        try {
            $receipt = $line | ConvertFrom-Json
            if ($null -eq $receipt.event -or $null -eq $receipt.fields) {
                throw "receipt shape"
            }
            $receipts += $receipt
        } catch {
            if ($Strict) {
                throw "peer receipt JSONL was invalid"
            }
        }
    }
    return $receipts
}

function Wait-Snapshot([int]$Revision) {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    for ($attempt = 0; $attempt -lt 32 -and [DateTime]::UtcNow -lt $deadline; $attempt += 1) {
        $found = @(Read-Receipts | Where-Object {
            $_.event -eq "snapshot" -and $_.fields.revision -eq $Revision
        })
        if ($found.Count -eq 1) {
            return
        }
        Send-Step
        Start-Sleep -Milliseconds 30
    }
    throw "timed out waiting for authoritative revision $Revision after 32 bounded step attempts"
}

function Capture-State([int]$ArtifactId) {
    $response = Send-Request ([ordered]@{ type = "capture_frame"; artifact_id = $ArtifactId }) "capture_saved"
    if ($null -eq $response.capture -or $response.capture.artifact_id -ne $ArtifactId) {
        throw "capture response omitted fixed metadata"
    }
    $script:captureResponses[$ArtifactId] = $response.capture
}

function Assert-ReceiptOracle {
    $path = Join-Path $EvidenceRoot "peer-receipts.jsonl"
    $raw = Get-Content -Raw -LiteralPath $path
    if ($raw -match '(?i)https?://|#|invite|secret|credential|\\|/[A-Za-z0-9_.-]+/') {
        throw "peer receipts exposed private or path-shaped content"
    }
    $receipts = @(Read-Receipts -Strict)
    $allowed = @("sent_action", "sent_malformed", "join_auth", "ack", "malformed_rejected", "duplicate_rejected", "snapshot", "complete")
    foreach ($receipt in $receipts) {
        if ($receipt.event -notin $allowed) {
            throw "peer receipts contained an unexpected event"
        }
        foreach ($property in $receipt.fields.PSObject.Properties) {
            if ($property.Value -isnot [ValueType]) {
                throw "peer receipt field was not numeric"
            }
        }
    }
    $expectedCounts = @{
        sent_action = 10; sent_malformed = 1; join_auth = 1; ack = 8
        malformed_rejected = 1; duplicate_rejected = 1; snapshot = 20; complete = 1
    }
    foreach ($event in $expectedCounts.Keys) {
        if (@($receipts | Where-Object event -eq $event).Count -ne $expectedCounts[$event]) {
            throw "peer receipt count mismatch for $event"
        }
    }

    $hashes = @(1019970671,1019970672,-1738785873,-1509219795,-1986119478,-1313654260,-1606102963,-1606102932,1019970889,-1738785656,1516465030,1251720965,1481287043,-297508478,374956740,2146426017,-402133277,-598615255,-598615038,-598614977)
    $sequences = @(0,0,0,1,1,2,2,2,3,3,4,4,5,5,6,6,7,7,8,8)
    $phases = @(0,0,0,0,0,0,1,1,0,0,0,0,0,0,0,0,0,2,2,2)
    $terminalPhases = @(0,0,0,0,0,0,1,2,7,7,7,7,7,7,7,7,7,1,8,10)
    $screens = @(0,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,0)
    $moves = @(0,0,1,2,3,4,5,5,0,1,2,3,4,5,6,7,8,9,9,9)
    $current = @(0,0,1,0,1,0,0,0,0,1,0,1,0,1,0,1,0,0,0,0)
    $winner = @(-1,-1,-1,-1,-1,-1,0,0,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1)
    $snapshots = @($receipts | Where-Object event -eq "snapshot" | Sort-Object { $_.fields.revision })
    for ($index = 0; $index -lt 20; $index += 1) {
        $fields = $snapshots[$index].fields
        $revision = $index + 1
        $gameId = if ($revision -eq 1 -or $revision -eq 20) { 0 } else { 1 }
        if (
            $fields.revision -ne $revision -or
            $fields.game_id -ne $gameId -or
            $fields.hash -ne $hashes[$index] -or
            $fields.sequence -ne $sequences[$index] -or
            $fields.phase -ne $phases[$index] -or
            $fields.terminal_phase -ne $terminalPhases[$index] -or
            $fields.screen -ne $screens[$index] -or
            $fields.moves -ne $moves[$index] -or
            $fields.current -ne $current[$index] -or
            $fields.winner -ne $winner[$index]
        ) {
            throw "authoritative snapshot oracle mismatch at revision $revision"
        }
    }
    $complete = @($receipts | Where-Object event -eq "complete")[0].fields
    if ($complete.revision -ne 20 -or $complete.sequence -ne 8 -or $complete.hash -ne $hashes[19]) {
        throw "peer completion marker was invalid"
    }
}

function Assert-Captures {
    foreach ($artifactId in 0..3) {
        $path = Join-Path $EvidenceRoot ("supervised-capture-{0:D2}.png" -f $artifactId)
        if (!(Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "capture artifact $artifactId was missing"
        }
        $metadata = Get-Item -LiteralPath $path
        $capture = $script:captureResponses[$artifactId]
        $sha256 = [Security.Cryptography.SHA256]::Create()
        try {
            $hashBytes = $sha256.ComputeHash([IO.File]::ReadAllBytes($path))
            $hash = [BitConverter]::ToString($hashBytes).Replace("-", "").ToLowerInvariant()
        } finally {
            $sha256.Dispose()
        }
        if ($metadata.Length -ne $capture.byte_length -or $hash -ne $capture.sha256 -or $capture.width -ne 640 -or $capture.height -ne 360) {
            throw "capture artifact $artifactId did not match its receipt"
        }
    }
}

function Get-DirectChildProcesses([int]$ParentId) {
    return @(Get-CimInstance Win32_Process | Where-Object { $_.ParentProcessId -eq $ParentId })
}

function Invoke-NonReadingStdoutFailure {
    $failureEvidence = Join-Path $runRoot "failure-nonreading-stdout"
    New-Item -ItemType Directory -Path $failureEvidence | Out-Null
    $failureStart = [Diagnostics.ProcessStartInfo]::new()
    $failureStart.FileName = $Supervisor
    $failureArguments = @(
        "--toolchain", $Toolchain,
        "--workspace", $workspace,
        "--peer", $Peer,
        "--evidence-root", $failureEvidence,
        "--startup-ms", "60000",
        "--action-ms", "1500",
        "--shutdown-ms", "3000"
    )
    $failureStart.Arguments = (@($failureArguments | ForEach-Object { Quote-ProcessArgument $_ }) -join " ")
    $failureStart.WorkingDirectory = $repoRoot
    $failureStart.UseShellExecute = $false
    $failureStart.RedirectStandardInput = $true
    $failureStart.RedirectStandardOutput = $true
    $failureStart.RedirectStandardError = $true
    $failure = [Diagnostics.Process]::Start($failureStart)
    $failureErrorTask = $failure.StandardError.ReadToEndAsync()
    $authorityPid = 0
    $peerPid = 0
    try {
        $startupDeadline = [DateTime]::UtcNow.AddSeconds(60)
        while ([DateTime]::UtcNow -lt $startupDeadline -and !$failure.HasExited) {
            foreach ($child in @(Get-DirectChildProcesses $failure.Id)) {
                if ($child.Name -eq "stasis.exe") {
                    $authorityPid = [int]$child.ProcessId
                } elseif ($child.Name -eq "supervision_live_ttt_peer.exe") {
                    $peerPid = [int]$child.ProcessId
                }
            }
            if ($authorityPid -gt 0 -and $peerPid -gt 0) {
                break
            }
            Start-Sleep -Milliseconds 20
        }
        if ($authorityPid -le 0 -or $peerPid -le 0) {
            throw "non-reading stdout case did not reach peer readiness"
        }

        $requests = [Text.StringBuilder]::new()
        foreach ($id in 1..80) {
            $line = [ordered]@{
                schema_version = 1
                request_id = $id
                command = [ordered]@{ type = "pause" }
            } | ConvertTo-Json -Compress -Depth 4
            $null = $requests.AppendLine($line)
        }
        $failure.StandardInput.AutoFlush = $true
        $writeTask = $failure.StandardInput.WriteAsync($requests.ToString())
        if (!$failure.WaitForExit(10000)) {
            & taskkill.exe /PID $failure.Id /T /F 2>&1 | Out-Null
            $failure.WaitForExit(5000) | Out-Null
            throw "non-reading stdout supervisor did not exit within 10 seconds"
        }
        if ($failure.ExitCode -eq 0) {
            throw "non-reading stdout supervisor unexpectedly succeeded"
        }
        $cleanupDeadline = [DateTime]::UtcNow.AddSeconds(3)
        while ([DateTime]::UtcNow -lt $cleanupDeadline) {
            $authorityAlive = $null -ne (Get-Process -Id $authorityPid -ErrorAction SilentlyContinue)
            $peerAlive = $null -ne (Get-Process -Id $peerPid -ErrorAction SilentlyContinue)
            if (!$authorityAlive -and !$peerAlive) {
                break
            }
            Start-Sleep -Milliseconds 20
        }
        $authorityAlive = $null -ne (Get-Process -Id $authorityPid -ErrorAction SilentlyContinue)
        $peerAlive = $null -ne (Get-Process -Id $peerPid -ErrorAction SilentlyContinue)
        if ($authorityAlive -or $peerAlive) {
            throw "non-reading stdout child cleanup was incomplete"
        }
        $failureReceipt = [ordered]@{
            schema_version = 1
            case_id = 1
            supervisor_exit_code = $failure.ExitCode
            authority_pid = $authorityPid
            peer_pid = $peerPid
            bounded_exit = 1
            authority_alive_after = 0
            peer_alive_after = 0
        }
        $failureReceipt | ConvertTo-Json -Compress | Set-Content -LiteralPath (Join-Path $EvidenceRoot "failure-nonreading-stdout.json") -Encoding UTF8
    } finally {
        if (!$failure.HasExited) {
            & taskkill.exe /PID $failure.Id /T /F 2>&1 | Out-Null
            $failure.WaitForExit(5000) | Out-Null
        }
        if ($failureErrorTask.IsCompleted) {
            $null = $failureErrorTask.Result
        }
        $failure.Dispose()
    }
}

function Invoke-ProcessFailureCase([int]$CaseId, [string]$Mode) {
    $failureEvidence = Join-Path $runRoot ("failure-case-{0:D2}" -f $CaseId)
    New-Item -ItemType Directory -Path $failureEvidence | Out-Null
    $failureStart = [Diagnostics.ProcessStartInfo]::new()
    $failureStart.FileName = $Supervisor
    $failureArguments = @(
        "--toolchain", $Toolchain,
        "--workspace", $workspace,
        "--peer", $Peer,
        "--evidence-root", $failureEvidence,
        "--startup-ms", "60000",
        "--action-ms", "1500",
        "--shutdown-ms", "3000"
    )
    $failureStart.Arguments = (@($failureArguments | ForEach-Object { Quote-ProcessArgument $_ }) -join " ")
    $failureStart.WorkingDirectory = $repoRoot
    $failureStart.UseShellExecute = $false
    $failureStart.RedirectStandardInput = $true
    $failureStart.RedirectStandardOutput = $true
    $failureStart.RedirectStandardError = $true
    $failure = [Diagnostics.Process]::Start($failureStart)
    $failureErrorTask = $failure.StandardError.ReadToEndAsync()
    $authorityPid = 0
    $peerPid = 0
    try {
        $startupDeadline = [DateTime]::UtcNow.AddSeconds(60)
        while ([DateTime]::UtcNow -lt $startupDeadline -and !$failure.HasExited) {
            foreach ($child in @(Get-DirectChildProcesses $failure.Id)) {
                if ($child.Name -eq "stasis.exe") {
                    $authorityPid = [int]$child.ProcessId
                } elseif ($child.Name -eq "supervision_live_ttt_peer.exe") {
                    $peerPid = [int]$child.ProcessId
                }
            }
            if ($authorityPid -gt 0 -and $peerPid -gt 0) {
                break
            }
            Start-Sleep -Milliseconds 20
        }
        if ($authorityPid -le 0 -or $peerPid -le 0) {
            throw "failure case $CaseId did not reach peer readiness"
        }

        if ($Mode -eq "malformed") {
            $failure.StandardInput.WriteLine("{")
            $failure.StandardInput.Flush()
        } elseif ($Mode -eq "eof") {
            $failure.StandardInput.Close()
        } elseif ($Mode -eq "timeout") {
            # Keep stdin open and send no request.
        } elseif ($Mode -eq "peer_exit") {
            Stop-Process -Id $peerPid -Force
        } elseif ($Mode -eq "authority_exit") {
            Stop-Process -Id $authorityPid -Force
        } else {
            throw "unknown process failure mode"
        }

        if (!$failure.WaitForExit(10000)) {
            & taskkill.exe /PID $failure.Id /T /F 2>&1 | Out-Null
            $failure.WaitForExit(5000) | Out-Null
            throw "failure case $CaseId supervisor did not exit within 10 seconds"
        }
        if ($failure.ExitCode -eq 0) {
            throw "failure case $CaseId supervisor unexpectedly succeeded"
        }
        $cleanupDeadline = [DateTime]::UtcNow.AddSeconds(3)
        while ([DateTime]::UtcNow -lt $cleanupDeadline) {
            $authorityAlive = $null -ne (Get-Process -Id $authorityPid -ErrorAction SilentlyContinue)
            $peerAlive = $null -ne (Get-Process -Id $peerPid -ErrorAction SilentlyContinue)
            if (!$authorityAlive -and !$peerAlive) {
                break
            }
            Start-Sleep -Milliseconds 20
        }
        $authorityAlive = $null -ne (Get-Process -Id $authorityPid -ErrorAction SilentlyContinue)
        $peerAlive = $null -ne (Get-Process -Id $peerPid -ErrorAction SilentlyContinue)
        if ($authorityAlive -or $peerAlive) {
            throw "failure case $CaseId child cleanup was incomplete"
        }
        $failureReceipt = [ordered]@{
            schema_version = 1
            case_id = $CaseId
            supervisor_exit_code = $failure.ExitCode
            authority_pid = $authorityPid
            peer_pid = $peerPid
            bounded_exit = 1
            authority_alive_after = 0
            peer_alive_after = 0
        }
        $failureReceipt | ConvertTo-Json -Compress | Set-Content -LiteralPath (Join-Path $EvidenceRoot ("failure-case-{0:D2}.json" -f $CaseId)) -Encoding UTF8
    } finally {
        if (!$failure.HasExited) {
            & taskkill.exe /PID $failure.Id /T /F 2>&1 | Out-Null
            $failure.WaitForExit(5000) | Out-Null
        }
        if ($failureErrorTask.IsCompleted) {
            $null = $failureErrorTask.Result
        }
        $failure.Dispose()
    }
}

function Assert-EvidenceBounds {
    $primaryBytes = 0L
    foreach ($file in @(Get-ChildItem -LiteralPath $EvidenceRoot -File -Force)) {
        $primaryBytes += $file.Length
    }
    if ($primaryBytes -gt (4L * 16L * 1024L * 1024L + 512L * 1024L)) {
        throw "successful acceptance evidence exceeded its fixed bound"
    }
    $aggregateBytes = $primaryBytes
    $failureRoots = @((Join-Path $runRoot "failure-nonreading-stdout"))
    foreach ($caseId in 2..6) {
        $failureRoots += Join-Path $runRoot ("failure-case-{0:D2}" -f $caseId)
    }
    foreach ($failureRoot in $failureRoots) {
        $caseBytes = 0L
        foreach ($file in @(Get-ChildItem -LiteralPath $failureRoot -File -Force)) {
            $caseBytes += $file.Length
        }
        if ($caseBytes -gt (512L * 1024L)) {
            throw "failure-case evidence exceeded its fixed bound"
        }
        $aggregateBytes += $caseBytes
    }
    if ($aggregateBytes -gt (70L * 1024L * 1024L)) {
        throw "aggregate acceptance evidence exceeded its fixed bound"
    }
}

try {
    $null = Send-Request ([ordered]@{ type = "pause" }) "paused"
    $outside = [ordered]@{
        id = 0; x = 641; y = 361
        is_down = $false; went_down = $false; went_up = $false
    }
    $null = Send-Request ([ordered]@{ type = "set_input_state"; pointers = @($outside) }) "rejected" "invalid_request"
    $inside = [ordered]@{
        id = 0; x = 0; y = 0
        is_down = $false; went_down = $false; went_up = $false
    }
    $null = Send-Request ([ordered]@{ type = "set_input_state"; pointers = @($inside) }) "input_applied"
    Wait-Snapshot 1

    Send-Gesture 480 95
    Wait-Snapshot 2
    Send-Gesture 105 105
    Wait-Snapshot 4
    Send-Gesture 195 105
    Wait-Snapshot 6
    Send-Gesture 285 105
    Wait-Snapshot 7
    Capture-State 0

    Send-Gesture 480 210
    Wait-Snapshot 9
    Capture-State 1

    Send-Gesture 105 105
    Wait-Snapshot 11
    Send-Gesture 285 105
    Wait-Snapshot 13
    Send-Gesture 105 285
    Wait-Snapshot 15
    Send-Gesture 195 285
    Wait-Snapshot 17
    Send-Gesture 285 195
    Wait-Snapshot 18
    Capture-State 2

    Wait-Snapshot 19
    Send-Gesture 480 317
    Wait-Snapshot 20
    Capture-State 3

    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (@(Read-Receipts | Where-Object event -eq "complete").Count -eq 1) {
            break
        }
        Start-Sleep -Milliseconds 20
    }
    if (@(Read-Receipts | Where-Object event -eq "complete").Count -ne 1) {
        throw "peer completion marker did not arrive"
    }

    $null = Send-Request ([ordered]@{ type = "quit" }) "quit"
    $supervised.StandardInput.Close()
    if (!$supervised.WaitForExit(15000)) {
        throw "supervisor did not exit within its shutdown bound"
    }
    if ($supervised.ExitCode -ne 0) {
        throw "supervisor exited with an error"
    }
    Assert-ReceiptOracle
    Assert-Captures
    Invoke-NonReadingStdoutFailure
    Invoke-ProcessFailureCase 2 "malformed"
    Invoke-ProcessFailureCase 3 "eof"
    Invoke-ProcessFailureCase 4 "timeout"
    Invoke-ProcessFailureCase 5 "peer_exit"
    Invoke-ProcessFailureCase 6 "authority_exit"
    $summary = [ordered]@{
        schema_version = 1
        scenario = "live_ttt_win_rematch_draw_catalog"
        final_revision = 20
        final_sequence = 8
        peer_receipt_count = 43
        nonreading_stdout_failure = 1
        failure_case_count = 6
        captures = @(0..3 | ForEach-Object {
            [ordered]@{
                artifact_id = $_
                file = "supervised-capture-{0:D2}.png" -f $_
                sha256 = $script:captureResponses[$_].sha256
            }
        })
    }
    $summary | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $EvidenceRoot "acceptance-summary.json") -Encoding UTF8
    Assert-EvidenceBounds
    [Console]::Error.WriteLine("live Tic-Tac-Toe acceptance passed; evidence retained at $EvidenceRoot")
} catch {
    Stop-SupervisedProcess
    $detail = ""
    if ($null -ne $supervised -and $supervised.HasExited) {
        $detail = $supervised.StandardError.ReadToEnd()
    }
    [Console]::Error.WriteLine("live Tic-Tac-Toe acceptance failed; evidence retained at $EvidenceRoot")
    if (![string]::IsNullOrWhiteSpace($detail)) {
        [Console]::Error.WriteLine($detail.Trim())
    }
    throw
} finally {
    if ($null -ne $supervised) {
        $supervised.Dispose()
    }
}
