using System.Collections.Immutable;

namespace Stasis.Compiler.IR.Bytecode;

public sealed class BytecodeModule
{
    public required ImmutableArray<string> GlobalNames { get; init; }
    public required ImmutableArray<BytecodeFunction> Functions { get; init; }

    public int FindGlobalIndex(string name)
    {
        for (var i = 0; i < GlobalNames.Length; i++)
        {
            if (string.Equals(GlobalNames[i], name, StringComparison.Ordinal))
            {
                return i;
            }
        }

        return -1;
    }

    public int FindFunctionIndex(string name)
    {
        for (var i = 0; i < Functions.Length; i++)
        {
            if (string.Equals(Functions[i].Name, name, StringComparison.Ordinal))
            {
                return i;
            }
        }

        return -1;
    }
}

