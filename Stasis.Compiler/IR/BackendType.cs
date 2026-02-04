namespace Stasis.Compiler.IR;

/// <summary>
/// Supported code generation backends.
/// </summary>
public enum BackendType
{
    /// <summary>
    /// Cranelift backend - fast compilation, suitable for debug/dev builds.
    /// </summary>
    Cranelift,

    /// <summary>
    /// Bytecode backend - interpreter-friendly, intended for ultra-fast dev hot swaps.
    /// </summary>
    Bytecode,

    /// <summary>
    /// LLVM backend - optimized compilation, suitable for release builds.
    /// </summary>
    Llvm
}
