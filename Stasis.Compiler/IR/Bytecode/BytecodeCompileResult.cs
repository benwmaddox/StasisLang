namespace Stasis.Compiler.IR.Bytecode;

public sealed class BytecodeCompileResult
{
    public required bool Success { get; init; }
    public required BytecodeModule? Module { get; init; }
    public required string Disassembly { get; init; }
    public required IReadOnlyList<Diagnostic> Diagnostics { get; init; }

    public static BytecodeCompileResult Ok(BytecodeModule module, string disassembly) =>
        new()
        {
            Success = true,
            Module = module,
            Disassembly = disassembly,
            Diagnostics = Array.Empty<Diagnostic>()
        };

    public static BytecodeCompileResult Fail(IReadOnlyList<Diagnostic> diagnostics) =>
        new()
        {
            Success = false,
            Module = null,
            Disassembly = string.Empty,
            Diagnostics = diagnostics
        };
}

