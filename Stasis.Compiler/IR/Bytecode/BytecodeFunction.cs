using System.Collections.Immutable;

namespace Stasis.Compiler.IR.Bytecode;

public sealed class BytecodeFunction
{
    public required string Name { get; init; }
    public required BytecodeValueKind ReturnKind { get; init; }
    public required ImmutableArray<BytecodeValueKind> ParamKinds { get; init; }
    public required int LocalCount { get; init; }
    public required ImmutableArray<BytecodeInst> Code { get; init; }
}
