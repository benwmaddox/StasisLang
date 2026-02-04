using System.Collections.Immutable;

namespace Stasis.Compiler.IR.Bytecode;

public sealed class BytecodeBuilder
{
    private readonly List<BytecodeGlobal> _globals = new();
    private readonly List<BytecodeFunction> _functions = new();

    public int DeclareGlobalI32(string name)
    {
        return DeclareGlobal(name, BytecodeValueKind.I32);
    }

    public int DeclareGlobalF32(string name)
    {
        return DeclareGlobal(name, BytecodeValueKind.F32);
    }

    private int DeclareGlobal(string name, BytecodeValueKind kind)
    {
        for (var i = 0; i < _globals.Count; i++)
        {
            if (string.Equals(_globals[i].Name, name, StringComparison.Ordinal))
            {
                if (_globals[i].Kind != kind)
                {
                    throw new InvalidOperationException($"Global '{name}' kind mismatch: {_globals[i].Kind} vs {kind}.");
                }
                return i;
            }
        }

        _globals.Add(new BytecodeGlobal { Name = name, Kind = kind });
        return _globals.Count - 1;
    }

    public FunctionBuilder DefineFunction(string name, BytecodeValueKind returnKind, ImmutableArray<BytecodeValueKind> paramKinds, int localCount)
    {
        return new FunctionBuilder(this, name, returnKind, paramKinds, localCount);
    }

    public BytecodeModule Build()
    {
        return new BytecodeModule
        {
            Globals = _globals.ToImmutableArray(),
            Functions = _functions.ToImmutableArray()
        };
    }

    public sealed class FunctionBuilder
    {
        private readonly BytecodeBuilder _module;
        private readonly string _name;
        private readonly BytecodeValueKind _returnKind;
        private readonly ImmutableArray<BytecodeValueKind> _paramKinds;
        private readonly int _localCount;
        private readonly List<BytecodeInst> _code = new();

        internal FunctionBuilder(BytecodeBuilder module, string name, BytecodeValueKind returnKind, ImmutableArray<BytecodeValueKind> paramKinds, int localCount)
        {
            _module = module;
            _name = name;
            _returnKind = returnKind;
            _paramKinds = paramKinds;
            _localCount = localCount;
        }

        public int Emit(BytecodeOp op, int a = 0, int b = 0)
        {
            _code.Add(new BytecodeInst(op, a, b));
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
                ReturnKind = _returnKind,
                ParamKinds = _paramKinds,
                LocalCount = _localCount,
                Code = _code.ToImmutableArray()
            });
        }
    }
}
