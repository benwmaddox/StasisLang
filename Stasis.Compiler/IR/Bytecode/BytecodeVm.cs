namespace Stasis.Compiler.IR.Bytecode;

public sealed class BytecodeVm
{
    private readonly int[] _globalsI32;

    public BytecodeVm(BytecodeModule module)
    {
        Module = module;
        _globalsI32 = new int[module.GlobalNames.Length];
    }

    public BytecodeModule Module { get; private set; }

    public int GetGlobalI32(string name)
    {
        var idx = Module.FindGlobalIndex(name);
        if (idx < 0) throw new InvalidOperationException($"Global not found: {name}");
        return _globalsI32[idx];
    }

    public void SetGlobalI32(string name, int value)
    {
        var idx = Module.FindGlobalIndex(name);
        if (idx < 0) throw new InvalidOperationException($"Global not found: {name}");
        _globalsI32[idx] = value;
    }

    public void HotSwap(BytecodeModule next)
    {
        // Dev-friendly state migration: preserve globals by name (i32 only for now).
        var nextGlobals = new int[next.GlobalNames.Length];
        for (var i = 0; i < next.GlobalNames.Length; i++)
        {
            var name = next.GlobalNames[i];
            var oldIdx = Module.FindGlobalIndex(name);
            if (oldIdx >= 0)
            {
                nextGlobals[i] = _globalsI32[oldIdx];
            }
        }

        Module = next;
        Array.Clear(_globalsI32, 0, _globalsI32.Length);
        Array.Copy(nextGlobals, _globalsI32, Math.Min(nextGlobals.Length, _globalsI32.Length));
    }

    public int CallI32(string functionName)
    {
        var fnIdx = Module.FindFunctionIndex(functionName);
        if (fnIdx < 0) throw new InvalidOperationException($"Function not found: {functionName}");
        return ExecuteI32(Module.Functions[fnIdx]);
    }

    private int ExecuteI32(BytecodeFunction fn)
    {
        var locals = new int[fn.LocalCount];
        var stack = new int[Math.Max(64, fn.LocalCount * 2 + 16)];
        var sp = 0;

        int Pop()
        {
            if (sp <= 0) throw new InvalidOperationException("Bytecode stack underflow.");
            return stack[--sp];
        }

        void Push(int v)
        {
            if (sp >= stack.Length) Array.Resize(ref stack, stack.Length * 2);
            stack[sp++] = v;
        }

        var ip = 0;
        while (ip >= 0 && ip < fn.Code.Length)
        {
            var inst = fn.Code[ip++];
            switch (inst.Op)
            {
                case BytecodeOp.Nop:
                    break;
                case BytecodeOp.ConstI32:
                    Push(inst.A);
                    break;
                case BytecodeOp.LoadLocalI32:
                    Push(locals[inst.A]);
                    break;
                case BytecodeOp.StoreLocalI32:
                    locals[inst.A] = Pop();
                    break;
                case BytecodeOp.LoadGlobalI32:
                    Push(_globalsI32[inst.A]);
                    break;
                case BytecodeOp.StoreGlobalI32:
                    _globalsI32[inst.A] = Pop();
                    break;
                case BytecodeOp.AddI32:
                    {
                        var b = Pop();
                        var a = Pop();
                        Push(a + b);
                        break;
                    }
                case BytecodeOp.SubI32:
                    {
                        var b = Pop();
                        var a = Pop();
                        Push(a - b);
                        break;
                    }
                case BytecodeOp.MulI32:
                    {
                        var b = Pop();
                        var a = Pop();
                        Push(a * b);
                        break;
                    }
                case BytecodeOp.DivI32:
                    {
                        var b = Pop();
                        var a = Pop();
                        Push(a / b);
                        break;
                    }
                case BytecodeOp.Jump:
                    ip = inst.A;
                    break;
                case BytecodeOp.JumpIfZeroI32:
                    {
                        var cond = Pop();
                        if (cond == 0)
                        {
                            ip = inst.A;
                        }
                        break;
                    }
                case BytecodeOp.ReturnI32:
                    return Pop();
                case BytecodeOp.ReturnVoid:
                    return 0;
                default:
                    throw new InvalidOperationException($"Unknown bytecode op: {inst.Op}.");
            }
        }

        throw new InvalidOperationException($"Bytecode function '{fn.Name}' fell off end without return.");
    }
}

