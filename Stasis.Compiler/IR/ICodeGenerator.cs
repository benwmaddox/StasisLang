using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.IR;

/// <summary>
/// Interface for code generation backends (LLVM, Cranelift, etc.).
/// Implementations transform analyzed Stasis code into executable form.
/// </summary>
public interface ICodeGenerator : IDisposable
{
    /// <summary>
    /// Unique identifier for this backend (e.g., "llvm", "cranelift").
    /// </summary>
    string BackendName { get; }

    /// <summary>
    /// Generates code from the analyzed Stasis compilation unit.
    /// </summary>
    /// <param name="compilationUnit">The parsed syntax tree.</param>
    /// <param name="semanticResult">Semantic analysis result with symbol table.</param>
    /// <param name="layout">Memory layout plan (SoA transformation).</param>
    /// <param name="options">Code generation options.</param>
    /// <returns>The code generation result containing IR and diagnostics.</returns>
    CodeGenerationResult Generate(
        CompilationUnitSyntax compilationUnit,
        SemanticResult semanticResult,
        LayoutPlan layout,
        CodeGenerationOptions options);

    /// <summary>
    /// Returns the generated intermediate representation as a string.
    /// Call after Generate() to retrieve the IR for debugging or output.
    /// </summary>
    string EmitIrString();
}
