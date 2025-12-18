namespace Stasis.Compiler.IR;

/// <summary>
/// Factory for creating code generator instances.
/// </summary>
public static class CodeGeneratorFactory
{
    /// <summary>
    /// Creates a code generator for the specified backend.
    /// </summary>
    /// <param name="backend">The backend type to use.</param>
    /// <param name="moduleName">Name of the module being compiled.</param>
    /// <returns>A code generator instance.</returns>
    public static ICodeGenerator Create(BackendType backend, string moduleName = "module")
    {
        return backend switch
        {
            BackendType.Llvm => new Llvm.LlvmCodeGenerator(moduleName),
            BackendType.Cranelift => new Cranelift.CraneliftCodeGenerator(moduleName),
            _ => throw new ArgumentException($"Unknown backend type: {backend}", nameof(backend))
        };
    }

    /// <summary>
    /// Gets the default backend for the given build mode.
    /// </summary>
    /// <param name="isRelease">True for release builds, false for debug builds.</param>
    /// <returns>The recommended backend type.</returns>
    public static BackendType GetDefaultBackend(bool isRelease)
    {
        // For now, always use LLVM until Cranelift is fully implemented
        // Future: return isRelease ? BackendType.Llvm : BackendType.Cranelift;
        return BackendType.Llvm;
    }

    /// <summary>
    /// Checks if a backend is available/implemented.
    /// </summary>
    /// <param name="backend">The backend type.</param>
    /// <returns>True if fully implemented and production-ready.</returns>
    public static bool IsBackendAvailable(BackendType backend)
    {
        return backend switch
        {
            BackendType.Llvm => true,
            BackendType.Cranelift => false, // Scaffolding only - not production ready
            _ => false
        };
    }

    /// <summary>
    /// Checks if a backend can generate IR (even if not production-ready).
    /// </summary>
    /// <param name="backend">The backend type.</param>
    /// <returns>True if the backend can generate intermediate representation.</returns>
    public static bool CanGenerateIr(BackendType backend)
    {
        return backend switch
        {
            BackendType.Llvm => true,
            BackendType.Cranelift => true, // Can generate CLIF text
            _ => false
        };
    }
}
