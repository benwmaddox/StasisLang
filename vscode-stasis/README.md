# Stasis for Visual Studio Code

The Stasis extension keeps the editor on the same compiler and runtime contracts as the command-line toolchain. It provides:

- Stasis syntax highlighting and editor indentation;
- canonical document formatting through `stasis format --stdin`;
- compiler-backed project completion, with richer local/member completion while a play session is active;
- a graphical hot-swap play session using the manifest entry;
- pause, resume, and single-tick controls;
- typed live inspection and watches in the **Stasis > Live Values** sidebar.

## Requirements

Install a current `stasis` executable on `PATH`, or set `stasis.executablePath` to its absolute path. Open a folder containing `stasis.json`.

Projects created by `stasis new` recommend this extension and enable format-on-save only for the `stasis` language. For an existing project, use:

```json
{
  "[stasis]": {
    "editor.defaultFormatter": "stasislang.stasis",
    "editor.formatOnSave": true
  }
}
```

## Live play and values

Run **Stasis: Start Play Session**. The extension saves open files, starts the normal graphical hot-swap runtime, and leaves game code on disk as the source of truth. Use the Live Values title actions or command palette to pause, resume, step, inspect a path once, or add a watch.

Examples of accepted live queries include:

```text
state.player.health
enemies[0].hp
enemies[?hp <= 0]
```

Watch updates are emitted between deterministic ticks. The extension never evaluates game state independently; displayed values come from the running Stasis runtime.

Set `stasis.live.entry` only when a project needs an entry other than the one in `stasis.json`.

## Development

On Windows, build, test, package, and install the local extension with:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\install_vscode_stasis.ps1 -Force
```

Pass `-SkipInstall` to validate and create `.vsix/stasislang.stasis.vsix` without changing the
installed VS Code extension. The underlying commands are:

```powershell
cd vscode-stasis
npm install
npm test
npm run build
npm run package
```

The extension is intentionally a thin CLI client. Language or runtime semantics belong in Stasis, where the terminal UI, editor, tests, and packaged games can share them.
