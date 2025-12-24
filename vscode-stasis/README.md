# Stasis VSCode Extension

This extension provides:

- Syntax highlighting for `*.stasis`
- Language Server Protocol features (diagnostics, hover, completion)

## Development

1. Build the language server:
   - `dotnet build Stasis.LanguageServer/Stasis.LanguageServer.csproj`
2. Publish the server into `vscode-stasis/server/` (platform-specific):
   - `dotnet publish Stasis.LanguageServer/Stasis.LanguageServer.csproj -c Release -r win-x64 -o vscode-stasis/server/`
3. Build the extension:
   - `cd vscode-stasis && npm install && npm run build`

