using System.Collections.Immutable;

namespace Stasis.Compiler.IR.Bytecode;

public sealed class BytecodeVm
{
    private ulong[] _globals;

    public BytecodeVm(BytecodeModule module)
    {
        Module = module;
        _globals = new ulong[module.Globals.Length];
    }

    public BytecodeModule Module { get; private set; }

    public int GetGlobalI32(string name)
    {
        var (idx, kind) = RequireGlobal(name);
        if (kind != BytecodeValueKind.I32) throw new InvalidOperationException($"Global '{name}' is not i32.");
        return (int)(uint)_globals[idx];
    }

    public void SetGlobalI32(string name, int value)
    {
        var (idx, kind) = RequireGlobal(name);
        if (kind != BytecodeValueKind.I32) throw new InvalidOperationException($"Global '{name}' is not i32.");
        _globals[idx] = (uint)value;
    }

    public float GetGlobalF32(string name)
    {
        var (idx, kind) = RequireGlobal(name);
        if (kind != BytecodeValueKind.F32) throw new InvalidOperationException($"Global '{name}' is not f32.");
        return BitsToF32((int)(uint)_globals[idx]);
    }

    public void SetGlobalF32(string name, float value)
    {
        var (idx, kind) = RequireGlobal(name);
        if (kind != BytecodeValueKind.F32) throw new InvalidOperationException($"Global '{name}' is not f32.");
        _globals[idx] = (uint)F32ToBits(value);
    }

    public void HotSwap(BytecodeModule next)
    {
        var nextGlobals = new ulong[next.Globals.Length];
        for (var i = 0; i < next.Globals.Length; i++)
        {
            var nextG = next.Globals[i];
            var oldIdx = Module.FindGlobalIndex(nextG.Name);
            if (oldIdx < 0)
            {
                continue;
            }
            var oldG = Module.Globals[oldIdx];
            if (oldG.Kind != nextG.Kind)
            {
                continue;
            }
            nextGlobals[i] = _globals[oldIdx];
        }

        Module = next;
        _globals = nextGlobals;
    }

    public int CallI32(string functionName, params ulong[] args)
    {
        var fn = RequireFunction(functionName);
        if (fn.ReturnKind != BytecodeValueKind.I32) throw new InvalidOperationException($"Function '{functionName}' does not return i32.");
        return (int)(uint)Execute(fn, args);
    }

    public void CallVoid(string functionName, params ulong[] args)
    {
        var fn = RequireFunction(functionName);
        if (fn.ReturnKind != BytecodeValueKind.Void) throw new InvalidOperationException($"Function '{functionName}' does not return void.");
        _ = Execute(fn, args);
    }

    private static int F32ToBits(float v) => BitConverter.SingleToInt32Bits(v);
    private static float BitsToF32(int bits) => BitConverter.Int32BitsToSingle(bits);

    private (int idx, BytecodeValueKind kind) RequireGlobal(string name)
    {
        var idx = Module.FindGlobalIndex(name);
        if (idx < 0) throw new InvalidOperationException($"Global not found: {name}");
        return (idx, Module.Globals[idx].Kind);
    }

    private BytecodeFunction RequireFunction(string name)
    {
        var idx = Module.FindFunctionIndex(name);
        if (idx < 0) throw new InvalidOperationException($"Function not found: {name}");
        return Module.Functions[idx];
    }

    private sealed class Frame
    {
        public required BytecodeFunction Fn { get; init; }
        public required int Ip { get; set; }
        public required ulong[] Locals { get; init; }
        public required int StackBase { get; init; }
    }

    private ulong Execute(BytecodeFunction entry, ulong[] args)
    {
        static void EnsureArgsMatch(BytecodeFunction fn, ulong[] a)
        {
            if (a.Length != fn.ParamKinds.Length)
            {
                throw new InvalidOperationException($"Call arg count mismatch for '{fn.Name}': expected {fn.ParamKinds.Length}, got {a.Length}.");
            }
        }

        EnsureArgsMatch(entry, args);

        var stack = new ulong[256];
        var sp = 0;

        int I32(ulong v) => (int)(uint)v;
        ulong U32(int v) => (uint)v;
        float F32(ulong v) => BitsToF32((int)(uint)v);
        ulong F32Bits(float v) => (uint)F32ToBits(v);

        ulong Pop()
        {
            if (sp <= 0) throw new InvalidOperationException("Bytecode stack underflow.");
            return stack[--sp];
        }

        void Push(ulong v)
        {
            if (sp >= stack.Length) Array.Resize(ref stack, stack.Length * 2);
            stack[sp++] = v;
        }

        var callStack = new Stack<Frame>(64);
        var fn = entry;
        var ip = 0;
        var locals = new ulong[fn.LocalCount];
        for (var i = 0; i < args.Length; i++)
        {
            locals[i] = args[i];
        }

        while (true)
        {
            if (ip < 0 || ip >= fn.Code.Length)
            {
                throw new InvalidOperationException($"Bytecode function '{fn.Name}' fell off end without return.");
            }

            var inst = fn.Code[ip++];
            switch (inst.Op)
            {
                case BytecodeOp.Nop:
                    break;
                case BytecodeOp.Pop:
                    _ = Pop();
                    break;
                case BytecodeOp.Dup:
                    {
                        var v = Pop();
                        Push(v);
                        Push(v);
                        break;
                    }

                case BytecodeOp.ConstI32:
                    Push(U32(inst.A));
                    break;
                case BytecodeOp.ConstF32:
                    Push(U32(inst.A));
                    break;

                case BytecodeOp.LoadLocalI32:
                case BytecodeOp.LoadLocalF32:
                    Push(locals[inst.A]);
                    break;
                case BytecodeOp.StoreLocalI32:
                case BytecodeOp.StoreLocalF32:
                    locals[inst.A] = Pop();
                    break;

                case BytecodeOp.LoadGlobalI32:
                case BytecodeOp.LoadGlobalF32:
                    Push(_globals[inst.A]);
                    break;
                case BytecodeOp.StoreGlobalI32:
                case BytecodeOp.StoreGlobalF32:
                    _globals[inst.A] = Pop();
                    break;

                case BytecodeOp.AddI32:
                    {
                        var b = I32(Pop());
                        var a = I32(Pop());
                        Push(U32(a + b));
                        break;
                    }
                case BytecodeOp.SubI32:
                    {
                        var b = I32(Pop());
                        var a = I32(Pop());
                        Push(U32(a - b));
                        break;
                    }
                case BytecodeOp.MulI32:
                    {
                        var b = I32(Pop());
                        var a = I32(Pop());
                        Push(U32(a * b));
                        break;
                    }
                case BytecodeOp.DivI32:
                    {
                        var b = I32(Pop());
                        var a = I32(Pop());
                        Push(U32(a / b));
                        break;
                    }
                case BytecodeOp.NegI32:
                    Push(U32(-I32(Pop())));
                    break;

                case BytecodeOp.AddF32:
                    {
                        var b = F32(Pop());
                        var a = F32(Pop());
                        Push(F32Bits(a + b));
                        break;
                    }
                case BytecodeOp.SubF32:
                    {
                        var b = F32(Pop());
                        var a = F32(Pop());
                        Push(F32Bits(a - b));
                        break;
                    }
                case BytecodeOp.MulF32:
                    {
                        var b = F32(Pop());
                        var a = F32(Pop());
                        Push(F32Bits(a * b));
                        break;
                    }
                case BytecodeOp.DivF32:
                    {
                        var b = F32(Pop());
                        var a = F32(Pop());
                        Push(F32Bits(a / b));
                        break;
                    }
                case BytecodeOp.NegF32:
                    Push(F32Bits(-F32(Pop())));
                    break;

                case BytecodeOp.NotI32:
                    {
                        var v = I32(Pop());
                        Push(U32(v == 0 ? 1 : 0));
                        break;
                    }

                case BytecodeOp.CmpEqI32:
                    {
                        var b = I32(Pop());
                        var a = I32(Pop());
                        Push(U32(a == b ? 1 : 0));
                        break;
                    }
                case BytecodeOp.CmpNeI32:
                    {
                        var b = I32(Pop());
                        var a = I32(Pop());
                        Push(U32(a != b ? 1 : 0));
                        break;
                    }
                case BytecodeOp.CmpLtI32:
                    {
                        var b = I32(Pop());
                        var a = I32(Pop());
                        Push(U32(a < b ? 1 : 0));
                        break;
                    }
                case BytecodeOp.CmpLeI32:
                    {
                        var b = I32(Pop());
                        var a = I32(Pop());
                        Push(U32(a <= b ? 1 : 0));
                        break;
                    }
                case BytecodeOp.CmpGtI32:
                    {
                        var b = I32(Pop());
                        var a = I32(Pop());
                        Push(U32(a > b ? 1 : 0));
                        break;
                    }
                case BytecodeOp.CmpGeI32:
                    {
                        var b = I32(Pop());
                        var a = I32(Pop());
                        Push(U32(a >= b ? 1 : 0));
                        break;
                    }

                case BytecodeOp.CmpEqF32:
                    {
                        var b = F32(Pop());
                        var a = F32(Pop());
                        Push(U32(a == b ? 1 : 0));
                        break;
                    }
                case BytecodeOp.CmpNeF32:
                    {
                        var b = F32(Pop());
                        var a = F32(Pop());
                        Push(U32(a != b ? 1 : 0));
                        break;
                    }
                case BytecodeOp.CmpLtF32:
                    {
                        var b = F32(Pop());
                        var a = F32(Pop());
                        Push(U32(a < b ? 1 : 0));
                        break;
                    }
                case BytecodeOp.CmpLeF32:
                    {
                        var b = F32(Pop());
                        var a = F32(Pop());
                        Push(U32(a <= b ? 1 : 0));
                        break;
                    }
                case BytecodeOp.CmpGtF32:
                    {
                        var b = F32(Pop());
                        var a = F32(Pop());
                        Push(U32(a > b ? 1 : 0));
                        break;
                    }
                case BytecodeOp.CmpGeF32:
                    {
                        var b = F32(Pop());
                        var a = F32(Pop());
                        Push(U32(a >= b ? 1 : 0));
                        break;
                    }

                case BytecodeOp.Jump:
                    ip = inst.A;
                    break;
                case BytecodeOp.JumpIfZeroI32:
                    {
                        var cond = I32(Pop());
                        if (cond == 0)
                        {
                            ip = inst.A;
                        }
                        break;
                    }

                case BytecodeOp.Call:
                    {
                        var callee = Module.Functions[inst.A];
                        var argc = inst.B;
                        if (argc != callee.ParamKinds.Length)
                        {
                            throw new InvalidOperationException($"Call arg count mismatch for '{callee.Name}': expected {callee.ParamKinds.Length}, got {argc}.");
                        }

                        var callArgs = new ulong[argc];
                        for (var i = argc - 1; i >= 0; i--)
                        {
                            callArgs[i] = Pop();
                        }

                        callStack.Push(new Frame
                        {
                            Fn = fn,
                            Ip = ip,
                            Locals = locals,
                            StackBase = sp
                        });

                        fn = callee;
                        ip = 0;
                        locals = new ulong[fn.LocalCount];
                        for (var i = 0; i < callArgs.Length; i++)
                        {
                            locals[i] = callArgs[i];
                        }
                        break;
                    }

                case BytecodeOp.ReturnVoid:
                    {
                        if (callStack.Count == 0)
                        {
                            return 0;
                        }
                        var caller = callStack.Pop();
                        fn = caller.Fn;
                        ip = caller.Ip;
                        locals = caller.Locals;
                        sp = caller.StackBase;
                        break;
                    }
                case BytecodeOp.ReturnI32:
                case BytecodeOp.ReturnF32:
                    {
                        var ret = Pop();
                        if (callStack.Count == 0)
                        {
                            return ret;
                        }
                        var caller = callStack.Pop();
                        fn = caller.Fn;
                        ip = caller.Ip;
                        locals = caller.Locals;
                        sp = caller.StackBase;
                        Push(ret);
                        break;
                    }

                default:
                    throw new InvalidOperationException($"Unknown bytecode op: {inst.Op}.");
            }
        }
    }
}

