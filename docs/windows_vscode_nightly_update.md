# Windows VS Code nightly updates

The repository contains a deterministic updater for the Windows Stasis VS Code extension:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools\windows\update-vscode-stasis-nightly.ps1
```

The updater selects the newest published `nightly-*` GitHub release, requires the
`stasis-editor-release-win32-x64.zip` asset, verifies its SHA-256 digest and the VSIX hash in
`stasis-editor-release.json`, and installs the matching VSIX with the VS Code CLI. It compares the
installed extension's bundled toolchain `release_id` with the release tag, so an unchanged nightly
returns without downloading or installing anything. State and logs are kept under the user's local
application-data directory, not in the repository.

Register or replace the daily Task Scheduler entry with:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools\windows\register-vscode-stasis-nightly-task.ps1 -Time 09:00
```

The task runs as the current interactive user at 09:00 local time. Use `-WhatIf` with the
registration script to inspect the action without changing Task Scheduler, and use `-CheckOnly` with
the updater to check release freshness without downloading or installing.
