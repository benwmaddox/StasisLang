namespace Stasis.Compiler.IR.Bytecode;

public sealed class BytecodeGlobal
{
    public required string Name { get; init; }
    public required BytecodeValueKind Kind { get; init; }
}

