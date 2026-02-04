using Stasis.Compiler.IR.Bytecode;
using Xunit;

namespace Stasis.Compiler.Tests;

public sealed class BytecodeVmTests
{
    [Fact]
    public void Tick_IncrementsGlobalCounter_AndHotSwapPreservesState()
    {
        var b1 = new BytecodeBuilder();
        var gCounter = b1.DeclareGlobalI32("counter");
        var f1 = b1.DefineFunction("tick", localCount: 0);
        f1.Emit(BytecodeOp.LoadGlobalI32, gCounter);
        f1.Emit(BytecodeOp.ConstI32, 1);
        f1.Emit(BytecodeOp.AddI32);
        f1.Emit(BytecodeOp.StoreGlobalI32, gCounter);
        f1.Emit(BytecodeOp.LoadGlobalI32, gCounter);
        f1.Emit(BytecodeOp.ReturnI32);
        f1.Finish();
        var m1 = b1.Build();

        var vm = new BytecodeVm(m1);
        Assert.Equal(0, vm.GetGlobalI32("counter"));
        Assert.Equal(1, vm.CallI32("tick"));
        Assert.Equal(1, vm.GetGlobalI32("counter"));

        var b2 = new BytecodeBuilder();
        var gCounter2 = b2.DeclareGlobalI32("counter");
        var f2 = b2.DefineFunction("tick", localCount: 0);
        f2.Emit(BytecodeOp.LoadGlobalI32, gCounter2);
        f2.Emit(BytecodeOp.ConstI32, 2);
        f2.Emit(BytecodeOp.AddI32);
        f2.Emit(BytecodeOp.StoreGlobalI32, gCounter2);
        f2.Emit(BytecodeOp.LoadGlobalI32, gCounter2);
        f2.Emit(BytecodeOp.ReturnI32);
        f2.Finish();
        var m2 = b2.Build();

        vm.HotSwap(m2);
        Assert.Equal(1, vm.GetGlobalI32("counter"));
        Assert.Equal(3, vm.CallI32("tick"));
        Assert.Equal(3, vm.GetGlobalI32("counter"));
    }

    [Fact]
    public void JumpIfZero_SkipsAdd_WhenConditionIsZero()
    {
        var b = new BytecodeBuilder();
        var f = b.DefineFunction("f", localCount: 0);

        // if (0) { return 123 } else { return 7 }
        f.Emit(BytecodeOp.ConstI32, 0);
        var jz = f.Emit(BytecodeOp.JumpIfZeroI32, a: 0);
        f.Emit(BytecodeOp.ConstI32, 123);
        f.Emit(BytecodeOp.ReturnI32);
        var elseIp = f.CurrentIp;
        f.PatchJump(jz, elseIp);
        f.Emit(BytecodeOp.ConstI32, 7);
        f.Emit(BytecodeOp.ReturnI32);
        f.Finish();

        var vm = new BytecodeVm(b.Build());
        Assert.Equal(7, vm.CallI32("f"));
    }
}

