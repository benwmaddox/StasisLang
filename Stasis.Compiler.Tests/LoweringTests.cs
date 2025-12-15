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
                let x: i32 = 0;
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
    public void Lowers_array_length_property_to_constant()
    {
        var ir = Lower("""
            global temps: i32[4];
            function len(): i32 {
                return temps.length;
            }
            """);

        Assert.Contains("ret i32 4", ir);
    }

    [Fact]
    public void Lowers_nested_array_length_from_struct_field()
    {
        var ir = Lower("""
            struct GameState { values: f32[5]; }
            global state: GameState;
            function len(): i32 {
                return state.values.length;
            }
            """);

        Assert.Contains("ret i32 5", ir);
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
                let i: i32 = 0;
                for (i = 0; true; i = i.+(1)) {
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
    public void Lowers_integer_extended_comparisons_to_icmp()
    {
        var ir = Lower("""
            function cmp(a: i32, b: i32): i32 {
                let x: i32 = a != b;
                let y: i32 = a <= b;
                let z: i32 = a >= b;
                return x + y + z;
            }
            """);

        Assert.Contains("icmp ne", ir);
        Assert.Contains("icmp sle", ir);
        Assert.Contains("icmp sge", ir);
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
    public void Lowers_float_extended_comparisons_to_fcmp()
    {
        var ir = Lower("""
            function cmp(a: f32, b: f32): i32 {
                let x: i32 = a != b;
                let y: i32 = a <= b;
                let z: i32 = a >= b;
                return x + y + z;
            }
            """);

        Assert.Contains("fcmp one", ir);
        Assert.Contains("fcmp ole", ir);
        Assert.Contains("fcmp oge", ir);
        Assert.Contains("zext i1", ir);
    }

    [Fact]
    public void Lowers_gfx_debug_bake_hash_call()
    {
        var ir = LowerWithDiagnostics("""
            function demo(): i32 {
                return gfx_debug_bake_hash("assets_src/brickout-revenge/paddle.stv");
            }
            """, allowSemanticDiagnostics: true, options: new LowerOptions(IncludeTests: false, EmitTestHarness: false, HeadlessGraphics: false)).Ir;

        Assert.Contains("stasis_gfx_debug_bake_hash", ir);
    }

    [Fact]
    public void Lowers_unary_negation_and_not()
    {
        var ir = Lower("""
            function tweak(x: f32, flag: bool): i32 {
                let y: f32 = (-(x));
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
                let x: i32 = 0;
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
                let x: i32 = 1;
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

    [Fact]
    public void Lowers_i32_to_f32_assignment_with_sitofp()
    {
        var ir = Lower("""
            function convert(x: i32): f32 {
                let result: f32 = x;
                return result;
            }
            """);

        // Should contain sitofp instruction for i32 -> f32 conversion
        Assert.Contains("sitofp i32", ir);
        Assert.Contains("to float", ir);
    }

    [Fact]
    public void Lowers_f32_to_i32_assignment_with_fptosi()
    {
        var ir = Lower("""
            function convert(x: f32): i32 {
                let result: i32 = x;
                return result;
            }
            """);

        // Should contain fptosi instruction for f32 -> i32 conversion
        Assert.Contains("fptosi float", ir);
        Assert.Contains("to i32", ir);
    }

    [Fact]
    public void Lowers_i32_to_f32_in_loop()
    {
        var ir = Lower("""
            function sum_as_float(): f32 {
                let i: i32 = 0;
                let total: f32 = 0.0;
                for (i = 0; i.<(5); i = i.+(1)) {
                    let if32: f32 = i;
                    total = total.+(if32);
                }
                return total;
            }
            """);

        // Should contain sitofp for i32 -> f32 conversion inside loop
        Assert.Contains("sitofp i32", ir);
        Assert.Contains("to float", ir);
    }

    [Fact]
    public void Lowers_let_with_initializer_store()
    {
        var ir = Lower("""
            function f(): i32 {
                let x: i32 = 5;
                return x;
            }
            """);

        Assert.Contains("store i32 5", ir);
    }

    [Fact]
    public void Lowers_fast_math_trig_calls()
    {
        var ir = Lower("""
            function trig(a: f32): f32 {
                return sin_fast(a).+(cos_fast(a));
            }
            """);

        Assert.Contains("call fast float @llvm.sin.f32", ir);
        Assert.Contains("call fast float @llvm.cos.f32", ir);
    }

    [Fact]
    public void Lowers_foreach_over_array_parameter_descriptor()
    {
        var ir = Lower("""
            function reset(values: i32[]): void {
                foreach (let v in values) {
                    v = 0;
                }
            }
            """);

        Assert.Contains("{ ptr, i32 }", ir);
        Assert.Contains("extractvalue { ptr, i32 }", ir);
        Assert.Contains("getelementptr i32", ir);
    }

    [Fact]
    public void Lowers_struct_array_parameter_descriptor_and_field_stores()
    {
        var ir = Lower("""
            struct Bullet { life: i32; ttl: i32; }
            function reset(bullets: Bullet[]): void {
                foreach (let b in bullets) {
                    b.life = 0;
                    b.ttl = 0;
                }
            }
            """);

        Assert.Contains("{ ptr, ptr, i32 }", ir);
        Assert.Contains("extractvalue { ptr, ptr, i32 }", ir);
        Assert.Contains("store i32 0", ir);
        Assert.Contains("getelementptr i32", ir);
    }

    [Fact]
    public void Lowers_foreach_with_index_variable()
    {
        var ir = Lower("""
            global values: i32[4];
            function run(): void {
                foreach (let v, i in values) {
                    values[i] = v;
                }
            }
            """);

        Assert.Contains("foreach.cond", ir);
        Assert.Contains("foreach.latch", ir);
        Assert.Contains("foreach.end", ir);
        Assert.Contains("getelementptr i32", ir);
    }

    [Fact]
    public void Lowers_foreach_with_index_variable_on_struct_array()
    {
        var ir = Lower("""
            struct Item { value: i32; }
            global items: Item[10];
            function run(): void {
                foreach (let item, idx in items) {
                    item.value = idx;
                }
            }
            """);

        Assert.Contains("foreach.cond", ir);
        Assert.Contains("foreach.body", ir);
        Assert.Contains("store i32", ir);
    }

    [Fact]
    public void Builds_descriptor_when_passing_global_array_argument()
    {
        var ir = Lower("""
            global values: i32[4];
            function sink(values: i32[]): void {
            }
            function caller(): void {
                sink(values);
            }
            """);

        Assert.Contains("call void @sink({ ptr, i32 } { ptr @values, i32 4 })", ir);
        Assert.Contains("call void @sink", ir);
    }
}
