namespace Stasis.Compiler.IR;

/// <summary>
/// Result of code generation from a backend.
/// </summary>
public sealed record CodeGenerationResult(
    /// <summary>
    /// The generated intermediate representation as a string (LLVM IR, CLIF, etc.).
    /// </summary>
    string Ir,

    /// <summary>
    /// Diagnostics generated during code generation.
    /// </summary>
    IReadOnlyList<Diagnostic> Diagnostics)
{
    /// <summary>
    /// Whether code generation succeeded without errors.
    /// </summary>
    public bool Success => Diagnostics.Count == 0;

    /// <summary>
    /// Creates a successful result with the given IR.
    /// </summary>
    public static CodeGenerationResult Ok(string ir) => new(ir, Array.Empty<Diagnostic>());

    /// <summary>
    /// Creates a failed result with the given diagnostics.
    /// </summary>
    public static CodeGenerationResult Fail(IReadOnlyList<Diagnostic> diagnostics) => new(string.Empty, diagnostics);
}
