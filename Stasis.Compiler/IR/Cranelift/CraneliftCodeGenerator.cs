using System.Text;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.IR.Cranelift;

/// <summary>
/// Cranelift-based code generator implementation.
///
/// Status: SCAFFOLDING - generates CLIF text but does not produce executable code yet.
///
/// Cranelift is a fast code generator designed for JIT compilation.
/// This implementation will provide faster compilation times for debug builds
/// compared to LLVM.
///
/// Implementation roadmap:
/// 1. [Current] Generate CLIF text representation
/// 2. Add native Cranelift bindings via P/Invoke or wasmtime-dotnet
/// 3. Implement JIT compilation to native code
/// 4. Full feature parity with LLVM backend
/// </summary>
public sealed class CraneliftCodeGenerator : ICodeGenerator
{
    private readonly string _moduleName;
    private string _lastIr = string.Empty;
    private bool _disposed;

    public CraneliftCodeGenerator(string moduleName = "module")
    {
        _moduleName = moduleName;
    }

    /// <inheritdoc />
    public string BackendName => "cranelift";

    /// <inheritdoc />
    public CodeGenerationResult Generate(
        CompilationUnitSyntax compilationUnit,
        SemanticResult semanticResult,
        LayoutPlan layout,
        CodeGenerationOptions options)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);

        var diagnostics = new List<Diagnostic>();

        try
        {
            using var builder = new CraneliftModuleBuilder(_moduleName);

            var (builtins, stringLiterals) = CollectLoweringNeeds(compilationUnit, options.IncludeTests);
            if (options.IncludeTests && options.EmitTestHarness)
            {
                builtins.Add("run_tests");
            }

            // Declare external functions (C runtime) only when needed
            DeclareExternalFunctions(builder, builtins);

            // Define string literals referenced by the program
            foreach (var literal in stringLiterals)
            {
                builder.DefineStringLiteral(literal);
            }

            // Emit globals
            EmitGlobals(compilationUnit, semanticResult.Symbols, layout, builder);

            // Emit functions with bodies
            EmitFunctions(compilationUnit, semanticResult.Symbols, builder, diagnostics, layout, options.IncludeTests, options.EmitTestHarness);

            // Generate CLIF text
            _lastIr = builder.EmitToString();

            return new CodeGenerationResult(_lastIr, diagnostics);
        }
        catch (Exception ex)
        {
            diagnostics.Add(new Diagnostic($"Cranelift code generation failed: {ex.Message}", new SourceSpan(0, 0)));
            return CodeGenerationResult.Fail(diagnostics);
        }
    }

    /// <inheritdoc />
    public string EmitIrString() => _lastIr;

    /// <inheritdoc />
    public void Dispose()
    {
        _disposed = true;
    }

    private static void DeclareExternalFunctions(CraneliftModuleBuilder builder, IReadOnlySet<string> builtins)
    {
        // Declare C standard library functions
        // Since Cranelift doesn't support variadic functions, we declare multiple signatures

        if (builtins.Contains("print_int") || builtins.Contains("print_char") || builtins.Contains("print_string") || builtins.Contains("run_tests"))
        {
            // printf3(format: *i8, arg1: i64, arg2: i64) -> i32 (aliased to printf in AOT)
            builder.DeclareExternal("printf3", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I64,
                CraneliftTypeMapper.ClifType.I64,
                CraneliftTypeMapper.ClifType.I64);
        }


        if (builtins.Contains("read_int") || builtins.Contains("read_char"))
        {
            // scanf(format: *i8, ptr: *i64) -> i32
            builder.DeclareExternal("scanf", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I64,  // format string pointer
                CraneliftTypeMapper.ClifType.I64); // pointer to result
        }

        if (builtins.Contains("time"))
        {
            // time(tloc: *i64) -> i64
            builder.DeclareExternal("time", CraneliftTypeMapper.ClifType.I64,
                CraneliftTypeMapper.ClifType.I64);
        }

        if (builtins.Contains("get_time_ms"))
        {
            // stasis_get_time_ms() -> i32
            builder.DeclareExternal("stasis_get_time_ms", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("sleep_ms"))
        {
            // stasis_sleep_ms(ms: i32) -> void
            builder.DeclareExternal("stasis_sleep_ms", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Overlaps(new[]
            {
                "str_len", "str_is_empty", "str_get", "str_set", "str_eq", "str_cmp",
                "str_copy", "str_append", "str_append_char", "str_clear",
                "str_contains", "str_find", "str_find_char", "str_find_last_char",
                "str_starts_with", "str_ends_with", "str_substr"
            }))
        {
            builder.DeclareExternal("strlen", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("strcmp", CraneliftTypeMapper.ClifType.I32, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("strncmp", CraneliftTypeMapper.ClifType.I32, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("strcpy", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("strcat", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("strchr", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I32);
            builder.DeclareExternal("strrchr", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I32);
            builder.DeclareExternal("strstr", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("memcpy", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("abort", CraneliftTypeMapper.ClifType.Void);
        }

        if (builtins.Contains("print_int"))
        {
            builder.DefineStringLiteral(" %d");
        }

        if (builtins.Contains("print_char"))
        {
            builder.DefineStringLiteral("%c");
        }

        if (builtins.Contains("print_string"))
        {
            builder.DefineStringLiteral("%s");
        }

        if (builtins.Contains("read_int"))
        {
            builder.DefineStringLiteral("%d");
        }

        if (builtins.Contains("read_char"))
        {
            builder.DefineStringLiteral(" %c");
        }

        // TODO: Add more C runtime functions as needed
    }

    private static void EmitGlobals(
        CompilationUnitSyntax compilationUnit,
        IReadOnlyDictionary<string, Symbol> symbols,
        LayoutPlan layout,
        CraneliftModuleBuilder builder)
    {
        var typeMapper = builder.TypeMapper;
        var structs = compilationUnit.Declarations
            .OfType<StructDeclarationSyntax>()
            .ToDictionary(s => s.Name.Text, s => s, StringComparer.Ordinal);
        var globalsByName = compilationUnit.Declarations
            .OfType<GlobalDeclarationSyntax>()
            .ToDictionary(g => g.Name.Text, g => g, StringComparer.Ordinal);

        // Emit globals based on LayoutPlan to support SoA transformation
        foreach (var globalLayout in layout.Globals)
        {
            if (!globalsByName.TryGetValue(globalLayout.Name, out var globalDecl))
            {
                continue;
            }

            switch (globalDecl.Type)
            {
                case ArrayTypeSyntax arrayType when arrayType.ElementType is NamedTypeSyntax named &&
                                                   structs.TryGetValue(named.Name, out var structDecl):
                    // Struct array -> SoA fields
                    var structCount = ParseArrayLength(arrayType.SizeToken?.Text);
                    foreach (var field in structDecl.Fields)
                    {
                        var fieldType = ResolveType(field.Type, symbols);
                        var elemType = typeMapper.Map(fieldType);
                        builder.DefineGlobalArray($"{structDecl.Name.Text}_{field.Identifier.Text}", elemType, structCount);
                    }
                    break;
                case ArrayTypeSyntax arrayType:
                    {
                        var elemType = ResolveType(arrayType.ElementType, symbols);
                        var clifElemType = typeMapper.Map(elemType);
                        var count = ParseArrayLength(arrayType.SizeToken?.Text);
                        builder.DefineGlobalArray(globalLayout.Name, clifElemType, count);
                        break;
                    }
                case NamedTypeSyntax namedType when structs.TryGetValue(namedType.Name, out var structInstance):
                    EmitStructInstanceGlobals(globalDecl.Name.Text, structInstance, symbols, structs, builder);
                    break;
                case NamedTypeSyntax:
                    {
                        if (!symbols.TryGetValue(globalLayout.Name, out var symbol) || symbol.Type == null)
                            continue;

                        var clifType = typeMapper.Map(symbol.Type);
                        builder.DefineGlobal(globalLayout.Name, clifType);
                        break;
                    }
            }
        }
    }

    private static void EmitFunctions(
        CompilationUnitSyntax compilationUnit,
        IReadOnlyDictionary<string, Symbol> symbols,
        CraneliftModuleBuilder builder,
        List<Diagnostic> diagnostics,
        LayoutPlan layout,
        bool includeTests,
        bool emitTestHarness)
    {
        var typeMapper = builder.TypeMapper;
        var structs = compilationUnit.Declarations
            .OfType<StructDeclarationSyntax>()
            .ToDictionary(s => s.Name.Text, s => s, StringComparer.Ordinal);
        var functionBuilder = new CraneliftFunctionBuilder(typeMapper, symbols, structs, builder.GlobalTypes, builder.StringLiterals, layout, diagnostics);

        // Emit regular functions with bodies
        foreach (var func in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
        {
            if (!symbols.TryGetValue(func.Name.Text, out var symbol))
                continue;

            var returnType = symbol.Type != null
                ? typeMapper.Map(symbol.Type)
                : CraneliftTypeMapper.ClifType.I32;

            var paramTypes = func.Parameters
                .Select(p => typeMapper.Map(ResolveType(p.Type, symbols)))
                .ToArray();

            // Generate function body
            var body = functionBuilder.BuildFunctionBody(func, symbol);
            builder.DefineFunctionWithBody(func.Name.Text, returnType, paramTypes, body);
        }

        // Emit test functions if requested
        if (includeTests)
        {
            foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
            {
                var testFuncName = $"test_{SanitizeTestName(test.Name.Text)}";
                var body = functionBuilder.BuildTestBody(test);
                builder.DefineFunctionWithBody(testFuncName, CraneliftTypeMapper.ClifType.I32, Array.Empty<CraneliftTypeMapper.ClifType>(), body);
            }

            if (emitTestHarness)
            {
                EmitTestHarness(compilationUnit, builder, diagnostics);
            }
        }
    }

    private static string SanitizeTestName(string name)
    {
        var sb = new StringBuilder();
        foreach (var c in name)
        {
            if (char.IsLetterOrDigit(c))
                sb.Append(c);
            else if (c == ' ')
                sb.Append('_');
        }
        return sb.ToString();
    }

    private static void EmitTestHarness(
        CompilationUnitSyntax compilationUnit,
        CraneliftModuleBuilder builder,
        List<Diagnostic> diagnostics)
    {
        var tests = compilationUnit.Declarations
            .OfType<TestDeclarationSyntax>()
            .ToList();

        var body = new StringBuilder();
        body.AppendLine("block0:");
        var valueCounter = 0;

        if (tests.Count == 0)
        {
            body.AppendLine("    v0 = iconst.i32 0");
            body.AppendLine("    return v0");
            builder.DefineFunctionWithBody("run_tests", CraneliftTypeMapper.ClifType.I32, Array.Empty<CraneliftTypeMapper.ClifType>(), body.ToString());
            return;
        }

        var failures = NewValue(ref valueCounter);
        body.AppendLine($"    {failures} = iconst.i32 0");

        var passFmt = builder.DefineStringLiteral("PASS: %s\n");
        var failFmt = builder.DefineStringLiteral("FAIL: %s\n");
        var summaryFmt = builder.DefineStringLiteral("Tests: passed=%d failed=%d\n");

        foreach (var testDecl in tests)
        {
            if (testDecl.Parameters.Count > 0)
            {
                diagnostics.Add(new Diagnostic("Test harness supports parameterless tests only.", testDecl.Name.Span));
                continue;
            }

            var testName = testDecl.Name.Text;
            var funcName = $"test_{SanitizeTestName(testName)}";

            var result = NewValue(ref valueCounter);
            body.AppendLine($"    {result} = call %{funcName}()");

            var zero = NewValue(ref valueCounter);
            body.AppendLine($"    {zero} = iconst.i32 0");
            var isFail = NewValue(ref valueCounter);
            body.AppendLine($"    {isFail} = icmp eq {result}, {zero}");

            var failI32 = NewValue(ref valueCounter);
            body.AppendLine($"    {failI32} = bint.i32 {isFail}");
            var nextFailures = NewValue(ref valueCounter);
            body.AppendLine($"    {nextFailures} = iadd {failures}, {failI32}");
            failures = nextFailures;

            var isPass = NewValue(ref valueCounter);
            body.AppendLine($"    {isPass} = icmp eq {failI32}, {zero}");

            var testNameGlobal = builder.DefineStringLiteral(testName);
            var nameAddr = NewValue(ref valueCounter);
            body.AppendLine($"    {nameAddr} = global_value {testNameGlobal}");

            var passAddr = NewValue(ref valueCounter);
            body.AppendLine($"    {passAddr} = global_value {passFmt}");
            var failAddr = NewValue(ref valueCounter);
            body.AppendLine($"    {failAddr} = global_value {failFmt}");

            var fmtAddr = NewValue(ref valueCounter);
            body.AppendLine($"    {fmtAddr} = select {isPass}, {passAddr}, {failAddr}");
            var zero64 = NewValue(ref valueCounter);
            body.AppendLine($"    {zero64} = iconst.i64 0");
            var print = NewValue(ref valueCounter);
            body.AppendLine($"    {print} = call %printf3({fmtAddr}, {nameAddr}, {zero64})");
        }

        var totalTests = tests.Count;
        var totalVal = NewValue(ref valueCounter);
        body.AppendLine($"    {totalVal} = iconst.i32 {totalTests}");
        var passed = NewValue(ref valueCounter);
        body.AppendLine($"    {passed} = isub {totalVal}, {failures}");

        var summaryAddr = NewValue(ref valueCounter);
        body.AppendLine($"    {summaryAddr} = global_value {summaryFmt}");
        var passed64 = NewValue(ref valueCounter);
        body.AppendLine($"    {passed64} = sextend.i64 {passed}");
        var failures64 = NewValue(ref valueCounter);
        body.AppendLine($"    {failures64} = sextend.i64 {failures}");
        var summaryCall = NewValue(ref valueCounter);
        body.AppendLine($"    {summaryCall} = call %printf3({summaryAddr}, {passed64}, {failures64})");

        body.AppendLine($"    return {failures}");

        builder.DefineFunctionWithBody("run_tests", CraneliftTypeMapper.ClifType.I32, Array.Empty<CraneliftTypeMapper.ClifType>(), body.ToString());
    }

    private static string NewValue(ref int counter) => $"v{counter++}";

    private static void EmitStructInstanceGlobals(
        string globalName,
        StructDeclarationSyntax structDecl,
        IReadOnlyDictionary<string, Symbol> symbols,
        IReadOnlyDictionary<string, StructDeclarationSyntax> structs,
        CraneliftModuleBuilder builder)
    {
        var typeMapper = builder.TypeMapper;
        foreach (var field in structDecl.Fields)
        {
            var fieldName = $"{globalName}_{field.Identifier.Text}";
            switch (field.Type)
            {
                case ArrayTypeSyntax arrayType when arrayType.ElementType is NamedTypeSyntax nestedNamed &&
                                                   structs.TryGetValue(nestedNamed.Name, out var nestedStruct):
                    {
                        var count = ParseArrayLength(arrayType.SizeToken?.Text);
                        foreach (var nestedField in nestedStruct.Fields)
                        {
                            var nestedFieldType = ResolveType(nestedField.Type, symbols);
                            var nestedElemType = typeMapper.Map(nestedFieldType);
                            var nestedName = $"{fieldName}_{nestedField.Identifier.Text}";
                            builder.DefineGlobalArray(nestedName, nestedElemType, count);
                        }
                        break;
                    }
                case ArrayTypeSyntax arrayType:
                    {
                        var elemType = ResolveType(arrayType.ElementType, symbols);
                        var clifElemType = typeMapper.Map(elemType);
                        var count = ParseArrayLength(arrayType.SizeToken?.Text);
                        builder.DefineGlobalArray(fieldName, clifElemType, count);
                        break;
                    }
                case NamedTypeSyntax namedField when structs.TryGetValue(namedField.Name, out var nestedStructDecl):
                    EmitStructInstanceGlobals(fieldName, nestedStructDecl, symbols, structs, builder);
                    break;
                default:
                    {
                        var fieldType = ResolveType(field.Type, symbols);
                        var clifType = typeMapper.Map(fieldType);
                        builder.DefineGlobal(fieldName, clifType);
                        break;
                    }
            }
        }
    }

    private static int ParseArrayLength(string? text) =>
        int.TryParse(text, out var n) ? n : 1;

    private static TypeSymbol ResolveType(TypeSyntax syntax, IReadOnlyDictionary<string, Symbol> symbols)
    {
        return syntax switch
        {
            NamedTypeSyntax named when symbols.TryGetValue(named.Name, out var sym) && sym.Type is not null => sym.Type,
            NamedTypeSyntax named when string.Equals(named.Name, "void", StringComparison.Ordinal) => new VoidTypeSymbol(),
            NamedTypeSyntax named => new NamedTypeSymbol(named.Name),
            ArrayTypeSyntax array => new ArrayTypeSymbol(
                ResolveType(array.ElementType, symbols),
                int.TryParse(array.SizeToken?.Text, out var parsed) ? parsed : -1),
            _ => new NamedTypeSymbol("unknown")
        };
    }

    private static (HashSet<string> Builtins, HashSet<string> StringLiterals) CollectLoweringNeeds(
        CompilationUnitSyntax compilationUnit,
        bool includeTests)
    {
        var builtins = new HashSet<string>(StringComparer.Ordinal);
        var stringLiterals = new HashSet<string>(StringComparer.Ordinal);

        foreach (var func in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
        {
            CollectFromBlock(func.Body, builtins, stringLiterals);
        }

        if (includeTests)
        {
            foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
            {
                CollectFromBlock(test.Body, builtins, stringLiterals);
            }
        }

        return (builtins, stringLiterals);
    }

    private static void CollectFromBlock(
        BlockStatementSyntax block,
        HashSet<string> builtins,
        HashSet<string> stringLiterals)
    {
        foreach (var stmt in block.Statements)
        {
            CollectFromStatement(stmt, builtins, stringLiterals);
        }
    }

    private static void CollectFromStatement(
        StatementSyntax stmt,
        HashSet<string> builtins,
        HashSet<string> stringLiterals)
    {
        switch (stmt)
        {
            case VariableDeclarationSyntax decl:
                if (decl.Initializer != null)
                {
                    CollectFromExpression(decl.Initializer, builtins, stringLiterals);
                }
                break;
            case ExpressionStatementSyntax exprStmt:
                CollectFromExpression(exprStmt.Expression, builtins, stringLiterals);
                break;
            case ReturnStatementSyntax ret:
                if (ret.Expression != null)
                {
                    CollectFromExpression(ret.Expression, builtins, stringLiterals);
                }
                break;
            case IfStatementSyntax ifStmt:
                CollectFromExpression(ifStmt.Condition, builtins, stringLiterals);
                CollectFromBlock(ifStmt.ThenBlock, builtins, stringLiterals);
                if (ifStmt.ElseBlock != null)
                {
                    CollectFromBlock(ifStmt.ElseBlock, builtins, stringLiterals);
                }
                break;
            case ForStatementSyntax forStmt:
                if (forStmt.Initializer != null)
                {
                    CollectFromExpression(forStmt.Initializer, builtins, stringLiterals);
                }
                if (forStmt.Condition != null)
                {
                    CollectFromExpression(forStmt.Condition, builtins, stringLiterals);
                }
                if (forStmt.Step != null)
                {
                    CollectFromExpression(forStmt.Step, builtins, stringLiterals);
                }
                CollectFromBlock(forStmt.Body, builtins, stringLiterals);
                break;
            case ForeachStatementSyntax foreachStmt:
                CollectFromExpression(foreachStmt.Iterable, builtins, stringLiterals);
                CollectFromBlock(foreachStmt.Body, builtins, stringLiterals);
                break;
            case BlockStatementSyntax block:
                CollectFromBlock(block, builtins, stringLiterals);
                break;
        }
    }

    private static void CollectFromExpression(
        ExpressionSyntax expr,
        HashSet<string> builtins,
        HashSet<string> stringLiterals)
    {
        switch (expr)
        {
            case LiteralExpressionSyntax lit when lit.Literal.Kind == TokenKind.StringLiteral:
                stringLiterals.Add(UnescapeString(lit.Literal.Text));
                break;
            case IdentifierExpressionSyntax:
                break;
            case ParenthesizedExpressionSyntax paren:
                CollectFromExpression(paren.Expression, builtins, stringLiterals);
                break;
            case UnaryExpressionSyntax unary:
                CollectFromExpression(unary.Operand, builtins, stringLiterals);
                break;
            case MemberAccessExpressionSyntax member:
                CollectFromExpression(member.Receiver, builtins, stringLiterals);
                break;
            case ArrayAccessExpressionSyntax array:
                CollectFromExpression(array.Receiver, builtins, stringLiterals);
                CollectFromExpression(array.Index, builtins, stringLiterals);
                break;
            case AssignmentExpressionSyntax assign:
                CollectFromExpression(assign.Left, builtins, stringLiterals);
                CollectFromExpression(assign.Right, builtins, stringLiterals);
                break;
            case BinaryExpressionSyntax bin:
                CollectFromExpression(bin.Left, builtins, stringLiterals);
                CollectFromExpression(bin.Right, builtins, stringLiterals);
                break;
            case CallExpressionSyntax call:
                if (call.Callee is IdentifierExpressionSyntax id && IsCraneliftBuiltin(id.Identifier.Text))
                {
                    builtins.Add(id.Identifier.Text);
                }
                CollectFromExpression(call.Callee, builtins, stringLiterals);
                foreach (var arg in call.Arguments)
                {
                    CollectFromExpression(arg, builtins, stringLiterals);
                }
                break;
            case OperatorCallExpressionSyntax opCall:
                CollectFromExpression(opCall.Receiver, builtins, stringLiterals);
                foreach (var arg in opCall.Arguments)
                {
                    CollectFromExpression(arg, builtins, stringLiterals);
                }
                break;
        }
    }

    private static bool IsCraneliftBuiltin(string name) =>
        name is "print_int" or "print_char" or "print_string" or "read_int" or "read_char"
            or "time" or "get_time_ms" or "sleep_ms"
            or "str_len" or "str_is_empty" or "str_get" or "str_set" or "str_eq" or "str_cmp"
            or "str_copy" or "str_append" or "str_append_char" or "str_clear"
            or "str_contains" or "str_find" or "str_find_char" or "str_find_last_char"
            or "str_starts_with" or "str_ends_with" or "str_substr";

    private static string UnescapeString(string text)
    {
        if (string.IsNullOrEmpty(text))
        {
            return string.Empty;
        }

        var inner = text.Length >= 2 ? text.Substring(1, text.Length - 2) : text;
        var sb = new StringBuilder(inner.Length);
        for (int i = 0; i < inner.Length; i++)
        {
            var ch = inner[i];
            if (ch == '\\' && i + 1 < inner.Length)
            {
                i++;
                var esc = inner[i];
                sb.Append(esc switch
                {
                    '\\' => '\\',
                    '"' => '"',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    _ => esc
                });
            }
            else
            {
                sb.Append(ch);
            }
        }

        return sb.ToString();
    }
}
