using System.Collections.Immutable;

namespace Stasis.Compiler.IR.Bytecode;

public sealed class BytecodeModule
{
    public required ImmutableArray<BytecodeGlobal> Globals { get; init; }
    public required ImmutableArray<BytecodeFunction> Functions { get; init; }

    public int FindGlobalIndex(string name)
    {
        for (var i = 0; i < Globals.Length; i++)
        {
            if (string.Equals(Globals[i].Name, name, StringComparison.Ordinal))
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

