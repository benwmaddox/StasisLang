# Stasis for Visual Studio Code

The Stasis extension keeps the editor on the same compiler and runtime contracts as the command-line toolchain. It provides:

- Stasis syntax highlighting and editor indentation;
- continuous compiler diagnostics through a standard Language Server Protocol client;
- canonical document formatting through `stasis format --stdin`;
- compiler-backed LSP completion with signatures, documentation, snippets, typed local/member
  ranking, and safe import edits;
- compiler-aware hover and signature help;
- compiler-backed LSP **Go to Definition**, **Find All References**, Outline, breadcrumbs, and
  workspace symbol search;
- `.test.stasis` discovery and file-level execution in VS Code's Test Explorer;
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

## Navigation and tests

**Go to Definition** and **Find All References** call the installed compiler's
`symbol references` command and translate its UTF-8 source spans into VS Code locations. Functions,
structs, tests, and typed struct fields have definition locations. Indexed receivers such as
`state.enemies[0].speed` resolve to the declaring field and expose their reads and writes.

While a play session is active, completion uses the persistent live compiler and runtime layout.
It resolves locals, members, and concrete indexed state paths such as
`state.enemies[0].{hp,speed}` without guessing types in the extension.

The Test Explorer discovers `.test.stasis` files under the manifest's `tests` directory. Each file
is one isolated Test Explorer item and runs through `stasis --json test <file>`, so editor and CLI
test behavior stay identical.

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

`npm run test:e2e` packages the extension, installs that VSIX into a clean VS Code profile, and
drives formatting, completion, graphical play, pause/step/resume, live values, and framebuffer
capture. The test decodes the captured PNG and verifies its physical dimensions, clear color, and
a command-buffer line, so a platform only passes after it produces the expected rendered pixels.
It requires a built `stasis` executable and graphics runtime. Set
`STASIS_E2E_EXECUTABLE` and `STASIS_RUNTIME_LIBRARY_PATH` when they are not in their standard
development locations. Linux runs need a display such as `xvfb-run`; GitHub CI supplies one.

The extension starts one persistent `stasis lsp --stdio` server per Stasis workspace. Language and
runtime semantics remain in Stasis, where the terminal UI, editor, tests, and packaged games can
share them. Formatting still uses its migration adapter until its LSP operation lands in a later
slice. Diagnostics, hover, signature help, completion, navigation, symbols, and rename use the
persistent language server. The same server launches and controls the Live Workshop through bounded
custom LSP requests, owns runtime observations, and composes compatible live values and indexed
collection fields into standard hover and completion responses. The extension does not spawn or
parse a parallel live JSON process.
