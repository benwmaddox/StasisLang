using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.IR.Llvm;

/// <summary>
/// LLVM-based code generator implementation.
/// Wraps the existing ModuleLowerer to provide ICodeGenerator interface.
/// </summary>
public sealed class LlvmCodeGenerator : ICodeGenerator
{
    private readonly string _moduleName;
    private readonly ModuleLowerer _lowerer;
    private string _lastIr = string.Empty;

    public LlvmCodeGenerator(string moduleName = "module")
    {
        _moduleName = moduleName;
        _lowerer = new ModuleLowerer();
    }

    /// <inheritdoc />
    public string BackendName => "llvm";

    /// <inheritdoc />
    public CodeGenerationResult Generate(
        CompilationUnitSyntax compilationUnit,
        SemanticResult semanticResult,
        LayoutPlan layout,
        CodeGenerationOptions options)
    {
        var lowerOptions = options.ToLowerOptions();
        var result = _lowerer.LowerToIr(compilationUnit, semanticResult, layout, _moduleName, lowerOptions);

        _lastIr = result.Ir;

        return new CodeGenerationResult(result.Ir, result.Diagnostics);
    }

    /// <inheritdoc />
    public string EmitIrString() => _lastIr;

    /// <inheritdoc />
    public void Dispose()
    {
        // ModuleLowerer disposes its own LlvmModuleBuilder internally
        // Nothing to dispose here
    }
}
