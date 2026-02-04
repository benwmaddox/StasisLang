using System.Collections.Immutable;

namespace Stasis.Compiler.IR.Bytecode;

public sealed class BytecodeBuilder
{
    private readonly List<string> _globals = new();
    private readonly List<BytecodeFunction> _functions = new();

    public int DeclareGlobalI32(string name)
    {
        var idx = _globals.IndexOf(name);
        if (idx >= 0) return idx;
        _globals.Add(name);
        return _globals.Count - 1;
    }

    public FunctionBuilder DefineFunction(string name, int localCount)
    {
        return new FunctionBuilder(this, name, localCount);
    }

    public BytecodeModule Build()
    {
        return new BytecodeModule
        {
            GlobalNames = _globals.ToImmutableArray(),
            Functions = _functions.ToImmutableArray()
        };
    }

    public sealed class FunctionBuilder
    {
        private readonly BytecodeBuilder _module;
        private readonly string _name;
        private readonly int _localCount;
        private readonly List<BytecodeInst> _code = new();

        internal FunctionBuilder(BytecodeBuilder module, string name, int localCount)
        {
            _module = module;
            _name = name;
            _localCount = localCount;
        }

        public int Emit(BytecodeOp op, int a = 0)
        {
            _code.Add(new BytecodeInst(op, a));
            return _code.Count - 1;
        }

        public void PatchJump(int instIndex, int targetIp)
        {
            var inst = _code[instIndex];
            if (inst.Op is not (BytecodeOp.Jump or BytecodeOp.JumpIfZeroI32))
            {
                throw new InvalidOperationException($"PatchJump expected Jump/JumpIfZeroI32, got {inst.Op}.");
            }
            _code[instIndex] = inst with { A = targetIp };
        }

        public int CurrentIp => _code.Count;

        public void Finish()
        {
            _module._functions.Add(new BytecodeFunction
            {
                Name = _name,
                LocalCount = _localCount,
                Code = _code.ToImmutableArray()
            });
        }
    }
}

