using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;

namespace Stasis.Compiler.Tests;

public class LoweringTests
{
    static LoweringTests()
    {
        LlvmNativeLoader.EnsureLoaded();
    }

    private static string Lower(string source)
    {
        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        Assert.Empty(sema.Diagnostics);

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        return new ModuleLowerer().LowerToIr(parse.CompilationUnit, sema, layout, "testmodule");
    }

    [Fact]
    public void Emits_struct_array_as_soa_byte_arrays()
    {
        var ir = Lower("""
            struct Player { hp: u8; score: i32; }
            global players: Player[2];
            """);

        Assert.Contains("@Player_hp = internal global [2 x i8] zeroinitializer", ir);
        Assert.Contains("@Player_score = internal global [2 x i32] zeroinitializer", ir);
    }

    [Fact]
    public void Emits_function_prototype_with_parameters()
    {
        var ir = Lower("""
            function add(a: i32, b: i32): i32 {
                return a.+(b);
            }
            """);

        Assert.Contains("define i32 @add(", ir);
        Assert.Contains("addtmp", ir);
        Assert.Contains("ret i32", ir);
    }

    [Fact]
    public void Emits_ret_void_when_missing()
    {
        var ir = Lower("""
            function tick(): void {
            }
            """);

        Assert.Contains("define void @tick()", ir);
        Assert.Contains("ret void", ir);
    }

    [Fact]
    public void Lowers_assignment_into_local()
    {
        var ir = Lower("""
            function one(): i32 {
                let x: i32;
                x.=(1);
                return x;
            }
            """);

        Assert.Contains("store i32 1", ir);
        Assert.Contains("ret i32", ir);
    }

    [Fact]
    public void Lowers_store_into_global_array()
    {
        var ir = Lower("""
            global temps: f32[3];
            function set(i: i32, v: f32): void {
                temps[i].=(v);
            }
            """);

        Assert.Contains("@temps = internal global [3 x float] zeroinitializer", ir);
        Assert.Contains("getelementptr", ir);
        Assert.Contains("store float", ir);
    }
}
