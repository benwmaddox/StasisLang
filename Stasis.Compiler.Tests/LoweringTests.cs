using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;

namespace Stasis.Compiler.Tests;

public class LoweringTests
{
    static LoweringTests()
    {
        Stasis.Compiler.LlvmNativeLoader.EnsureLoaded();
    }

    private static string Lower(string source)
    {
        var result = LowerWithDiagnostics(source, allowSemanticDiagnostics: false);
        Assert.Empty(result.Diagnostics);
        return result.Ir;
    }

    private static LowerResult LowerWithDiagnostics(string source, bool allowSemanticDiagnostics = true, LowerOptions? options = null)
    {
        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        if (!allowSemanticDiagnostics)
        {
            Assert.Empty(sema.Diagnostics);
        }

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        return new ModuleLowerer().LowerToIr(parse.CompilationUnit, sema, layout, "testmodule", options);
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
                x = 1;
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
                temps[i] = v;
            }
            """);

        Assert.Contains("@temps = internal global [3 x float] zeroinitializer", ir);
        Assert.Contains("getelementptr", ir);
        Assert.Contains("store float", ir);
    }

    [Fact]
    public void Lowers_struct_field_access_with_layout_names()
    {
        var ir = Lower("""
            struct Player { hp: u8; score: i32; }
            global players: Player[2];
            function set(i: i32): void {
                players[i].hp = 1;
            }
            """);

        Assert.Contains("@Player_hp", ir);
        Assert.Contains("getelementptr", ir);
        Assert.Contains("store", ir);
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
                i = 0;
                for i = 0; true; i = i.+(1) {
                    i = i;
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
                    values[i] = values[i];
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
                y = (-(x));
                return !(flag);
            }
            """);

        Assert.Contains("fneg", ir);
        Assert.Contains("icmp", ir); // from bool coercion
        Assert.Contains("zext i1", ir);
    }

    [Fact]
    public void Emits_diagnostic_for_bad_operator_arity()
    {
        var result = LowerWithDiagnostics("""
            function bad(): void {
                let x: i32;
                x.+(1, 2);
            }
            """);

        Assert.NotEmpty(result.Diagnostics);
        Assert.Contains(result.Diagnostics, d => d.Message.Contains("requires exactly one argument"));
    }

    [Fact]
    public void Emits_diagnostic_for_non_assignable_target()
    {
        var result = LowerWithDiagnostics("""
            function bad(): void {
                1 = 2;
            }
            """);

        Assert.NotEmpty(result.Diagnostics);
        Assert.Contains(result.Diagnostics, d => d.Message.Contains("assignable"));
    }

    [Fact]
    public void Emits_diagnostic_for_missing_struct_field()
    {
        var result = LowerWithDiagnostics("""
            struct Player { hp: u8; }
            global players: Player[2];
            function bad(i: i32): void {
                players[i].mp = 1;
            }
            """);

        Assert.NotEmpty(result.Diagnostics);
        Assert.Contains(result.Diagnostics, d => d.Message.Contains("Unknown field 'mp'"));
    }

    [Fact]
    public void Emits_diagnostic_for_field_access_on_non_struct_array()
    {
        var result = LowerWithDiagnostics("""
            global temps: i32[2];
            function bad(i: i32): void {
                temps[i].hp = 1;
            }
            """);

        Assert.NotEmpty(result.Diagnostics);
        Assert.Contains(result.Diagnostics, d => d.Message.Contains("not a struct array"));
    }

    [Fact]
    public void Lowers_compound_assignment()
    {
        var ir = Lower("""
            function bump(): i32 {
                let x: i32;
                x = 1;
                x += 2;
                return x;
            }
            """);

        Assert.Contains("addtmp", ir);
        Assert.Contains("store i32", ir);
    }

    [Fact]
    public void Emits_run_tests_harness()
    {
        var ir = Lower("""
            test check(): bool {
                return true;
            }
            """);

        Assert.Contains("define i32 @run_tests()", ir);
        Assert.Contains("call i32 @check", ir);
    }

    [Fact]
    public void Skips_tests_and_harness_when_disabled()
    {
        var result = LowerWithDiagnostics("""
            test check(): bool {
                return true;
            }
            """, options: new LowerOptions(IncludeTests: false, EmitTestHarness: false));

        Assert.DoesNotContain("run_tests", result.Ir);
        Assert.DoesNotContain("check", result.Ir);
    }
}
