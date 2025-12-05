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

    [Fact]
    public void Lowers_if_with_conditional_branches()
    {
        var ir = Lower("""
            function choose(flag: bool): i32 {
                if (flag) {
                    return 1;
                } else {
                    return 0;
                }
            }
            """);

        Assert.Contains("if.then", ir);
        Assert.Contains("if.else", ir);
        Assert.Contains("if.end", ir);
        Assert.Contains("br i1", ir);
    }

    [Fact]
    public void Lowers_for_loop_with_header_and_latch()
    {
        var ir = Lower("""
            function loop(n: i32): void {
                let i: i32;
                i.=(0);
                for i.=(0); true; i.=(i.+(1)) {
                    i.=(i);
                }
            }
            """);

        Assert.Contains("for.cond", ir);
        Assert.Contains("for.body", ir);
        Assert.Contains("for.latch", ir);
        Assert.Contains("for.end", ir);
        Assert.Contains("br label %for.cond", ir);
    }

    [Fact]
    public void Lowers_foreach_into_index_iteration()
    {
        var ir = Lower("""
            global values: i32[4];
            function sum(): void {
                foreach (i in values) {
                    values[i].=(values[i]);
                }
            }
            """);

        Assert.Contains("foreach.cond", ir);
        Assert.Contains("foreach.latch", ir);
        Assert.Contains("foreach.end", ir);
        Assert.Contains("foreach.cmp", ir);
        Assert.Contains("store i32", ir);
    }

    [Fact]
    public void Lowers_integer_comparisons_to_icmp()
    {
        var ir = Lower("""
            function smaller(a: i32, b: i32): i32 {
                return a.<(b);
            }
            """);

        Assert.Contains("icmp slt", ir);
        Assert.Contains("zext i1", ir);
    }

    [Fact]
    public void Lowers_float_comparisons_to_fcmp()
    {
        var ir = Lower("""
            function equals(a: f32, b: f32): i32 {
                return a.==(b);
            }
            """);

        Assert.Contains("fcmp oeq", ir);
        Assert.Contains("zext i1", ir);
    }

    [Fact]
    public void Lowers_unary_negation_and_not()
    {
        var ir = Lower("""
            function tweak(x: f32, flag: bool): i32 {
                let y: f32;
                y.=(-(x));
                return !(flag);
            }
            """);

        Assert.Contains("fneg", ir);
        Assert.Contains("icmp", ir); // from bool coercion
        Assert.Contains("zext i1", ir);
    }
}
