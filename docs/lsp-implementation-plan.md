# Stasis Language Server Protocol (LSP) Implementation Plan

## Overview

This document outlines the implementation plan for adding Language Server Protocol support to Stasis, including a VSCode extension with embedded LSP server.

## Goals

- Enable IDE features for Stasis: code completion, hover information, and live diagnostics
- Provide seamless developer experience through VSCode extension
- Build foundation for future advanced features (go-to-definition, find references, rename)

## Architecture Decisions

Based on user preferences and codebase analysis:

1. **LSP Library**: OmniSharp.Extensions.LanguageServer (v0.19+)
   - Full-featured, strongly-typed LSP implementation
   - Excellent .NET 9 support
   - Active maintenance and community

2. **Distribution Model**: Embedded LSP server in VSCode extension
   - Single installation for users
   - Extension bundles compiled LSP server executable
   - Automatic startup and lifecycle management

3. **Parsing Strategy**: Full reparse per change (Phase 1)
   - Simpler implementation
   - Acceptable performance for typical Stasis files
   - Incremental parsing deferred to Phase 2

4. **Feature Scope**: Core features only (Phase 1)
   - Code completion after dot notation
   - Hover type information
   - Real-time diagnostics
   - Defer: go-to-definition, find references, rename, formatting

## Project Structure

```
StasisLang/
├── Stasis.LanguageServer/          # New LSP server project
│   ├── Stasis.LanguageServer.csproj
│   ├── Program.cs                   # Entry point
│   ├── StasisLanguageServer.cs     # Main LSP server
│   ├── Handlers/
│   │   ├── CompletionHandler.cs
│   │   ├── HoverHandler.cs
│   │   └── DiagnosticsHandler.cs
│   ├── Services/
│   │   ├── DocumentManager.cs      # Track open documents
│   │   ├── SymbolIndex.cs          # Symbol database for completion
│   │   └── TypeResolver.cs         # Type information for hover
│   └── Models/
│       ├── DocumentState.cs        # Per-document parsed state
│       └── SymbolInfo.cs           # Symbol metadata
│
├── Stasis.LanguageServer.Tests/    # New test project
│   └── ...
│
├── vscode-stasis/                   # New VSCode extension
│   ├── package.json
│   ├── src/
│   │   └── extension.ts            # Extension entry point
│   ├── syntaxes/
│   │   └── stasis.tmLanguage.json  # Syntax highlighting
│   └── server/                      # Bundled LSP server
│       └── (published LSP binaries)
│
└── Stasis.sln                       # Updated solution file
```

## Implementation Phases

### Phase 1: Foundation (Core LSP Infrastructure)

#### 1.1 Create Stasis.LanguageServer Project

**File**: `Stasis.LanguageServer/Stasis.LanguageServer.csproj`

```xml
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net9.0</TargetFramework>
    <Nullable>enable</Nullable>
    <PublishSingleFile>true</PublishSingleFile>
    <PublishReadyToRun>true</PublishReadyToRun>
  </PropertyGroup>

  <ItemGroup>
    <PackageReference Include="OmniSharp.Extensions.LanguageServer" Version="0.19.9" />
    <ProjectReference Include="..\Stasis.Compiler\Stasis.Compiler.csproj" />
  </ItemGroup>
</Project>
```

#### 1.2 Implement LSP Server Entry Point

**File**: `Stasis.LanguageServer/Program.cs`

Create server initialization:
- Set up OmniSharp language server
- Configure stdio communication
- Register handlers
- Start server loop

**File**: `Stasis.LanguageServer/StasisLanguageServer.cs`

Configure server capabilities:
- Text document sync (full reparse)
- Completion provider
- Hover provider
- Diagnostic provider

#### 1.3 Document Management Service

**File**: `Stasis.LanguageServer/Services/DocumentManager.cs`

Track document state:
- `Dictionary<Uri, DocumentState>` for open documents
- Parse document on open/change
- Cache AST, semantic result, and symbol table
- Invalidate on content change

**File**: `Stasis.LanguageServer/Models/DocumentState.cs`

```csharp
public class DocumentState
{
    public string Content { get; set; }
    public int Version { get; set; }
    public ParseResult ParseResult { get; set; }
    public SemanticResult SemanticResult { get; set; }
    public SymbolIndex SymbolIndex { get; set; }
    public IReadOnlyList<Diagnostic> AllDiagnostics { get; set; }
}
```

### Phase 2: Diagnostics Handler

**File**: `Stasis.LanguageServer/Handlers/DiagnosticsHandler.cs`

Implement real-time error reporting:
- On document open/change, trigger full compilation
- Collect diagnostics from lexer, parser, semantic analyzer
- Convert Stasis `Diagnostic` → LSP `Diagnostic`
- Map `SourceSpan` → LSP `Range` (line/column conversion)
- Publish diagnostics to client

**Critical**: Line/column conversion
```csharp
private static (int line, int character) OffsetToPosition(string text, int offset)
{
    int line = 0, character = 0;
    for (int i = 0; i < offset && i < text.Length; i++)
    {
        if (text[i] == '\n') { line++; character = 0; }
        else character++;
    }
    return (line, character);
}
```

### Phase 3: Hover Handler (Type on Hover)

**File**: `Stasis.LanguageServer/Handlers/HoverHandler.cs`

Provide type information on hover:

1. **Find symbol at cursor position**:
   - Convert LSP Position → offset in document
   - Walk AST to find node containing offset
   - Extract identifier/expression at position

2. **Resolve type**:
   - For identifiers: lookup in symbol table
   - For member access: resolve base type, then member type
   - For literals: infer type

3. **Format hover content**:
   ```
   (variable) state: State
   ```
   or
   ```
   (enum member) State.Idle: State
   Value: 0
   ```

**File**: `Stasis.LanguageServer/Services/TypeResolver.cs`

Helper to resolve types for expressions:
- Reuse semantic analyzer's type resolution logic
- Handle member access, array access, function calls
- Format type names nicely

### Phase 4: Completion Handler (Dot Notation)

**File**: `Stasis.LanguageServer/Handlers/CompletionHandler.cs`

Provide completions after dot:

1. **Detect completion trigger**:
   - Triggered on `.` character
   - Parse text before cursor to get receiver expression

2. **Resolve receiver type**:
   - Parse partial document up to cursor
   - Find expression before `.`
   - Resolve type of receiver

3. **Generate completions**:

   **For enum types**:
   ```csharp
   enum State { Idle, Jump, Run }
   State.|  → Complete: Idle, Jump, Run
   ```
   - Filter symbol table for `"EnumName.*"`
   - Return all enum members

   **For struct types**:
   ```csharp
   struct Player { hp: u8; posX: f32; }
   player.|  → Complete: hp, posX
   ```
   - Look up struct definition
   - Return all field names with types

   **For array types**:
   ```csharp
   array.|  → Complete: length
   ```
   - Return built-in array properties

4. **Completion format**:
   ```json
   {
     "label": "Idle",
     "kind": CompletionItemKind.EnumMember,
     "detail": "State.Idle: State",
     "documentation": "Enum member with value 0"
   }
   ```

**File**: `Stasis.LanguageServer/Services/SymbolIndex.cs`

Build symbol index for completions:
```csharp
public class SymbolIndex
{
    // Global symbols
    public Dictionary<string, SymbolInfo> Globals { get; }

    // Enum members by enum name
    public Dictionary<string, List<EnumMember>> EnumMembers { get; }

    // Struct fields by struct name
    public Dictionary<string, List<StructField>> StructFields { get; }

    public void BuildFromSemanticResult(SemanticResult result);
}
```

### Phase 5: VSCode Extension

**Directory**: `vscode-stasis/`

#### 5.1 Extension Structure

**File**: `vscode-stasis/package.json`

```json
{
  "name": "stasis-language",
  "displayName": "Stasis Language Support",
  "description": "Language support for Stasis programming language",
  "version": "0.1.0",
  "engines": { "vscode": "^1.75.0" },
  "categories": ["Programming Languages"],
  "activationEvents": ["onLanguage:stasis"],
  "main": "./out/extension.js",
  "contributes": {
    "languages": [{
      "id": "stasis",
      "extensions": [".stasis"],
      "configuration": "./language-configuration.json"
    }],
    "grammars": [{
      "language": "stasis",
      "scopeName": "source.stasis",
      "path": "./syntaxes/stasis.tmLanguage.json"
    }]
  },
  "scripts": {
    "compile": "tsc -p ./",
    "watch": "tsc -watch -p ./",
    "package": "vsce package"
  },
  "dependencies": {
    "vscode-languageclient": "^9.0.0"
  },
  "devDependencies": {
    "@types/node": "^20.0.0",
    "@types/vscode": "^1.75.0",
    "typescript": "^5.0.0",
    "@vscode/vsce": "^2.22.0"
  }
}
```

#### 5.2 Extension Entry Point

**File**: `vscode-stasis/src/extension.ts`

```typescript
import * as path from 'path';
import * as vscode from 'vscode';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;

export function activate(context: vscode.ExtensionContext) {
  // Path to bundled LSP server executable
  const serverPath = context.asAbsolutePath(
    path.join('server', 'Stasis.LanguageServer')
  );

  const serverOptions: ServerOptions = {
    run: { command: serverPath, transport: TransportKind.stdio },
    debug: { command: serverPath, transport: TransportKind.stdio }
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: 'file', language: 'stasis' }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher('**/*.stasis')
    }
  };

  client = new LanguageClient(
    'stasisLanguageServer',
    'Stasis Language Server',
    serverOptions,
    clientOptions
  );

  client.start();
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
```

#### 5.3 Syntax Highlighting

**File**: `vscode-stasis/syntaxes/stasis.tmLanguage.json`

Basic TextMate grammar:
- Keywords: `enum`, `struct`, `function`, `let`, `if`, `for`, `return`, etc.
- Types: `i32`, `u8`, `f32`, `bool`, `void`
- Comments: `//` line comments
- Strings: `"..."` literals
- Numbers: integer and float literals

#### 5.4 Extension Packaging

Build script to:
1. Publish Stasis.LanguageServer as self-contained executable
2. Copy to `vscode-stasis/server/`
3. Compile TypeScript extension
4. Package with `vsce package`

## Enhanced Compiler Components

### Extend SemanticAnalyzer

**File**: `Stasis.Compiler/Semantic/SemanticAnalyzer.cs`

Add LSP-specific metadata collection:

```csharp
public sealed class SemanticResult
{
    public IReadOnlyList<Diagnostic> Diagnostics { get; init; }
    public IReadOnlyDictionary<string, Symbol> Symbols { get; init; }

    // NEW: Symbol locations for go-to-definition (Phase 2)
    public IReadOnlyDictionary<string, SourceSpan> SymbolLocations { get; init; }

    // NEW: Reference tracking (Phase 2)
    public IReadOnlyList<(SourceSpan Location, string SymbolName)> References { get; init; }
}
```

**Enhancement**: During semantic analysis, track:
- Where each symbol is declared (`SourceSpan`)
- Where each symbol is referenced

This requires minimal changes to existing code - just add location tracking during symbol table building.

### Add Struct Field Lookup

**File**: `Stasis.Compiler/Semantic/SemanticAnalyzer.cs`

Currently, struct definitions are stored but field lookup is limited. Enhance:

```csharp
private Dictionary<string, StructDeclarationSyntax> _structs;

public StructDeclarationSyntax? GetStructDefinition(string structName)
{
    return _structs.TryGetValue(structName, out var decl) ? decl : null;
}
```

Make this accessible to LSP server for field completions.

## Testing Strategy

### Unit Tests

**Project**: `Stasis.LanguageServer.Tests/`

Test each handler independently:

1. **CompletionHandler Tests**:
   - Enum member completion
   - Struct field completion
   - Array property completion
   - No completions for non-dot contexts

2. **HoverHandler Tests**:
   - Variable type hover
   - Enum member hover
   - Function signature hover
   - No hover over whitespace

3. **DiagnosticsHandler Tests**:
   - Syntax errors reported
   - Type errors reported
   - Diagnostic position accuracy

### Integration Tests

Test full LSP protocol:
- Initialize server
- Open document
- Trigger completions
- Request hover
- Verify diagnostics

Use OmniSharp.Extensions.LanguageServer.Testing utilities.

### Manual Testing

1. Install extension in VSCode
2. Open `.stasis` file
3. Test:
   - Red squiggles appear for errors
   - Hover shows type information
   - Dot completion works for enums/structs
   - Performance acceptable (<100ms for typical files)

## Implementation Order

### Week 1: LSP Server Foundation
1. Create `Stasis.LanguageServer` project
2. Add OmniSharp dependencies
3. Implement `Program.cs` and server initialization
4. Implement `DocumentManager` service
5. Test: Server starts and accepts connections

### Week 2: Diagnostics
6. Implement `DiagnosticsHandler`
7. Add line/column conversion utilities
8. Test: Diagnostics appear in client

### Week 3: Hover
9. Implement `HoverHandler`
10. Implement `TypeResolver` service
11. Add AST node finding by position
12. Test: Hover shows types

### Week 4: Completion
13. Implement `SymbolIndex` builder
14. Implement `CompletionHandler`
15. Add enum member completion
16. Add struct field completion
17. Test: Completion works after dot

### Week 5: VSCode Extension
18. Create `vscode-stasis` extension
19. Implement `extension.ts`
20. Create syntax highlighting
21. Set up build/packaging
22. Test: Extension installs and activates

### Week 6: Polish & Testing
23. Write unit tests
24. Write integration tests
25. Performance profiling
26. Bug fixes and refinements
27. Documentation

## Critical Files to Modify

### New Files
- `Stasis.LanguageServer/Program.cs`
- `Stasis.LanguageServer/StasisLanguageServer.cs`
- `Stasis.LanguageServer/Handlers/CompletionHandler.cs`
- `Stasis.LanguageServer/Handlers/HoverHandler.cs`
- `Stasis.LanguageServer/Handlers/DiagnosticsHandler.cs`
- `Stasis.LanguageServer/Services/DocumentManager.cs`
- `Stasis.LanguageServer/Services/SymbolIndex.cs`
- `Stasis.LanguageServer/Services/TypeResolver.cs`
- `vscode-stasis/src/extension.ts`
- `vscode-stasis/syntaxes/stasis.tmLanguage.json`

### Modified Files
- `Stasis.sln` (add new projects)
- `Stasis.Compiler/Semantic/SemanticAnalyzer.cs` (minor enhancements)
- `Stasis.Compiler/Semantic/SemanticResult.cs` (add location tracking)

## Future Enhancements (Phase 2+)

Not included in initial implementation:

1. **Go-to-Definition**: Jump to symbol declaration
2. **Find References**: Show all usages of symbol
3. **Rename**: Rename symbol across all files
4. **Code Actions**: Quick fixes for common errors
5. **Formatting**: Auto-format Stasis code
6. **Incremental Parsing**: Faster reparsing for large files
7. **Multi-file Support**: Cross-file symbol resolution
8. **Signature Help**: Parameter hints in function calls
9. **Document Symbols**: Outline view
10. **Semantic Highlighting**: Advanced syntax coloring

## Success Criteria

Phase 1 is successful when:

✅ VSCode extension installs with single click
✅ Syntax highlighting works for `.stasis` files
✅ Diagnostics appear in real-time as user types
✅ Hover shows type information for variables, enums, structs
✅ Dot completion provides enum members
✅ Dot completion provides struct fields
✅ Performance <100ms for typical files
✅ No crashes or hangs
✅ All tests pass

## Risk Mitigation

**Risk**: OmniSharp.Extensions breaking changes
- Mitigation: Pin to specific version, test thoroughly

**Risk**: Performance issues with full reparse
- Mitigation: Profile early, optimize hot paths, add timeouts

**Risk**: LSP protocol complexity
- Mitigation: Start with minimal feature set, incremental additions

**Risk**: Cross-platform executable bundling
- Mitigation: Test on Windows/Linux/macOS, use self-contained publish

## Conclusion

This plan provides a clear, phased approach to adding LSP support to Stasis. By focusing on core features first (completion, hover, diagnostics) and using proven libraries (OmniSharp.Extensions), we minimize risk while delivering immediate value to developers.

The architecture builds naturally on Stasis's existing compiler infrastructure, requiring minimal modifications to the core compiler while enabling powerful IDE features through the LSP server layer.
