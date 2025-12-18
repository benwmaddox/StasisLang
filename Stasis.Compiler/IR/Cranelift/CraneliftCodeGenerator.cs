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

            // Emit globals
            EmitGlobals(compilationUnit, semanticResult.Symbols, layout, builder);

            // Emit functions with bodies
            EmitFunctions(compilationUnit, semanticResult.Symbols, builder, diagnostics, options.IncludeTests);

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

    private static void EmitGlobals(
        CompilationUnitSyntax compilationUnit,
        IReadOnlyDictionary<string, Symbol> symbols,
        LayoutPlan layout,
        CraneliftModuleBuilder builder)
    {
        foreach (var global in compilationUnit.Declarations.OfType<GlobalDeclarationSyntax>())
        {
            var layoutInfo = layout.Globals.FirstOrDefault(g => g.Name == global.Name.Text);
            var size = layoutInfo?.Size ?? 4;
            builder.DefineGlobalData(global.Name.Text, size);
        }
    }

    private static void EmitFunctions(
        CompilationUnitSyntax compilationUnit,
        IReadOnlyDictionary<string, Symbol> symbols,
        CraneliftModuleBuilder builder,
        List<Diagnostic> diagnostics,
        bool includeTests)
    {
        var typeMapper = builder.TypeMapper;
        var functionBuilder = new CraneliftFunctionBuilder(typeMapper, symbols, diagnostics);

        // Emit regular functions with bodies
        foreach (var func in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
        {
            if (!symbols.TryGetValue(func.Name.Text, out var symbol))
                continue;

            var returnType = symbol.Type != null
                ? typeMapper.Map(symbol.Type)
                : CraneliftTypeMapper.ClifType.I32;

            var paramTypes = func.Parameters
                .Select(p => symbols.TryGetValue(p.Name.Text, out var ps) && ps.Type != null
                    ? typeMapper.Map(ps.Type)
                    : CraneliftTypeMapper.ClifType.I32)
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
}
