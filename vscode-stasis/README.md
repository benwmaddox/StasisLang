# Stasis VSCode Extension

This extension provides:

- Syntax highlighting for `*.stasis`
- Language Server Protocol features (diagnostics, hover, completion)

## Quick syntax highlighting (no build)

If you only want syntax highlighting (no LSP), use the lightweight extension in `vscode-stasis-syntax/`:

- `powershell -ExecutionPolicy Bypass -File .\\scripts\\install_vscode_stasis_syntax.ps1`

## Install full extension (syntax + LSP)

This builds/publishes the language server, packages the VSIX, and installs it via the VS Code CLI:

- `powershell -ExecutionPolicy Bypass -File .\\scripts\\install_vscode_stasis_lsp.ps1 -Force`

Notes:

- If you're using VS Code Remote - WSL, install the extension into the WSL environment (not Windows). In WSL, use `./scripts/install_vscode_stasis_lsp_wsl.sh --force`.
- If you pass `-Runtime linux-x64` you may pull in large native artifacts; prefer omitting `-Runtime` unless you specifically need a RID-specific publish.

## Development

1. Build the language server:
   - `dotnet build Stasis.LanguageServer/Stasis.LanguageServer.csproj`
2. Publish the server into `vscode-stasis/server/` (platform-specific):
   - `dotnet publish Stasis.LanguageServer/Stasis.LanguageServer.csproj -c Release -r win-x64 -o vscode-stasis/server/`
3. Build the extension:
   - `cd vscode-stasis && npm install && npm run build`
