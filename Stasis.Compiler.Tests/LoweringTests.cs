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
        Assert.DoesNotContain("; error:", result.Ir, StringComparison.Ordinal);
        return result.Ir;
    }

    private static LowerResult LowerWithDiagnostics(string source, bool allowSemanticDiagnostics = true, LowerOptions? options = null)
    {
        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        if (!allowSemanticDiagnostics)
        {
            DiagnosticAsserts.AssertNoErrors(sema.Diagnostics);
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
    public void Lowers_receiver_scoped_callables_with_collision_only_name_mangling()
    {
        var ir = Lower("""
            struct Enemy { hp: i32; }
            struct Hero { hp: i32; }

            function damage(enemy: Enemy, amount: i32): i32 {
                return amount;
            }

            function damage(hero: Hero, amount: i32): i32 {
                return amount.+(1);
            }

            function add(a: i32, b: i32): i32 {
                return a.+(b);
            }

            function tick(): void {
                let enemy: Enemy = 0;
                let hero: Hero = 0;
                let x: i32 = enemy.damage(5);
                let y: i32 = damage(hero, 6);
                let z: i32 = add(1, 2);
            }
            """);

        Assert.Contains("define i32 @damage__recv__Enemy(", ir);
        Assert.Contains("define i32 @damage__recv__Hero(", ir);
        Assert.Contains("call i32 @damage__recv__Enemy(", ir);
        Assert.Contains("call i32 @damage__recv__Hero(", ir);
        Assert.Contains("define i32 @add(", ir);
        Assert.DoesNotContain("@add__recv__", ir, StringComparison.Ordinal);
    }

    [Fact]
    public void Lowers_function_form_receiver_dispatch_with_literal_first_arguments()
    {
        var ir = Lower("""
            function tag(value: string): i32 {
                return 1;
            }

            function tag(value: i32): i32 {
                return 2;
            }

            function tick(): i32 {
                let a: i32 = tag("hello");
                let b: i32 = tag(5);
                return a.+(b);
            }
            """);

        Assert.Contains("define i32 @tag__recv__string(", ir);
        Assert.Contains("define i32 @tag__recv__i32(", ir);
        Assert.Contains("call i32 @tag__recv__string(", ir);
        Assert.Contains("call i32 @tag__recv__i32(", ir);
    }

    [Fact]
    public void Lowers_zero_arg_call_when_receiverless_and_receiver_scoped_share_name()
    {
        var ir = Lower("""
            function ping(): i32 {
                return 7;
            }

            function ping(value: i32): i32 {
                return value;
            }

            function tick(): i32 {
                return ping();
            }
            """);

        Assert.Contains("define i32 @ping()", ir);
        Assert.Contains("define i32 @ping__recv__i32(", ir);
        Assert.Contains("call i32 @ping()", ir);
    }

    [Fact]
    public void Lowers_function_form_overload_for_binary_first_argument()
    {
        var ir = Lower("""
            function tag(value: i32): i32 {
                return 1;
            }

            function tag(value: f32): i32 {
                return 2;
            }

            function tick(): i32 {
                return tag(1.+(2));
            }
            """);

        Assert.Contains("define i32 @tag__recv__i32(", ir);
        Assert.Contains("define i32 @tag__recv__f32(", ir);
        Assert.Contains("call i32 @tag__recv__i32(", ir);
    }

    [Fact]
    public void Reachable_collision_set_does_not_mangle_live_function_for_dead_overload()
    {
        var result = LowerWithDiagnostics("""
            function ping(value: i32): i32 {
                return value;
            }

            function ping(value: f32): i32 {
                return 0;
            }

            function main(): i32 {
                return ping(5);
            }
            """, allowSemanticDiagnostics: false, options: LowerOptions.Production);

        Assert.Empty(result.Diagnostics);
        Assert.Contains("define i32 @ping(i32", result.Ir);
        Assert.Contains("call i32 @ping(", result.Ir);
        Assert.DoesNotContain("@ping__recv__i32", result.Ir, StringComparison.Ordinal);
    }

    [Fact]
    public void Extern_overload_uses_link_name_instead_of_receiver_mangle()
    {
        var result = LowerWithDiagnostics("""
            function @extern("host_damage_enemy") damage(enemy: i32, amount: i32): i32;

            function damage(enemy: f32, amount: i32): i32 {
                return amount;
            }

            function main(): i32 {
                let a: i32 = damage(1, 2);
                let b: i32 = damage(1.0, 2);
                return a.+(b);
            }
            """, allowSemanticDiagnostics: false, options: LowerOptions.Production);

        Assert.Empty(result.Diagnostics);
        Assert.Contains("declare i32 @host_damage_enemy(i32, i32)", result.Ir);
        Assert.Contains("call i32 @host_damage_enemy(", result.Ir);
        Assert.Contains("define i32 @damage__recv__f32(", result.Ir);
        Assert.DoesNotContain("@damage__recv__i32", result.Ir, StringComparison.Ordinal);
    }

    [Fact]
    public void Extern_overloads_without_distinct_link_names_use_collision_safe_fallback_symbols()
    {
        var result = LowerWithDiagnostics("""
            extern function damage(enemy: i32, amount: i32): i32;
            extern function damage(hero: f32, amount: i32): i32;

            function main(): i32 {
                let a: i32 = damage(1, 2);
                let b: i32 = damage(1.0, 2);
                return a.+(b);
            }
            """, allowSemanticDiagnostics: true, options: LowerOptions.Production);

        Assert.Contains("declare i32 @damage__recv__i32(i32, i32)", result.Ir);
        Assert.Contains("declare i32 @damage__recv__f32(float, i32)", result.Ir);
        Assert.Contains("call i32 @damage__recv__i32(", result.Ir);
        Assert.Contains("call i32 @damage__recv__f32(", result.Ir);
    }

    [Fact]
    public void Extern_receiver_callable_falls_back_when_link_name_collides_with_receiverless_callable()
    {
        var result = LowerWithDiagnostics("""
            function foo(): i32 {
                return 7;
            }

            extern function foo(value: i32): i32;

            function main(): i32 {
                let a: i32 = foo();
                let b: i32 = foo(1);
                return a.+(b);
            }
            """, allowSemanticDiagnostics: false, options: LowerOptions.Production);

        Assert.Empty(result.Diagnostics);
        Assert.Contains("define i32 @foo()", result.Ir);
        Assert.Contains("declare i32 @foo__recv__i32(i32)", result.Ir);
        Assert.Contains("call i32 @foo()", result.Ir);
        Assert.Contains("call i32 @foo__recv__i32(", result.Ir);
    }

    [Fact]
    public void Emits_lowering_diagnostic_for_receiver_form_arity_mismatch()
    {
        var result = LowerWithDiagnostics("""
            struct Enemy { hp: i32; }

            function damage(enemy: Enemy, amount: i32): i32 {
                return amount;
            }

            function main(): i32 {
                let enemy: Enemy = 0;
                return enemy.damage();
            }
            """, allowSemanticDiagnostics: true, options: LowerOptions.Production);

        Assert.Contains(
            result.Diagnostics,
            d => d.Message.Contains("expects 1 argument(s) in receiver form, but got 0", StringComparison.Ordinal));
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
            function cmp(a: i32, b: i32): bool {
                let x: bool = a != b;
                let y: bool = a <= b;
                let z: bool = a >= b;
                return x;
            }
            """);

        Assert.Contains("icmp ne", ir);
        Assert.Contains("icmp sle", ir);
        Assert.Contains("icmp sge", ir);
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
            function cmp(a: f32, b: f32): bool {
                let x: bool = a != b;
                let y: bool = a <= b;
                let z: bool = a >= b;
                return x;
            }
            """);

        Assert.Contains("fcmp one", ir);
        Assert.Contains("fcmp ole", ir);
        Assert.Contains("fcmp oge", ir);
    }

    [Fact]
    public void Gfx_debug_helpers_are_not_builtins()
    {
        var diagnostics = LowerWithDiagnostics("""
            function demo(): i32 {
                return gfx_debug_bake_hash("samples/brickout_revenge/assets/paddle.svg");
            }
            """, allowSemanticDiagnostics: true, options: new LowerOptions(IncludeTests: false, EmitTestHarness: false, HeadlessGraphics: false)).Diagnostics;

        Assert.NotEmpty(diagnostics);
        Assert.Contains(diagnostics, d => d.Message.Contains("gfx_debug_bake_hash", StringComparison.Ordinal));
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
    public void Lowers_i32_to_f32_with_explicit_conversion()
    {
        var ir = Lower("""
            function convert(x: i32): f32 {
                let result: f32 = i32_to_f32(x);
                return result;
            }
            """);

        // Should contain sitofp instruction for i32 -> f32 conversion
        Assert.Contains("sitofp i32", ir);
        Assert.Contains("to float", ir);
    }

    [Fact]
    public void Lowers_f32_to_i32_with_explicit_conversion()
    {
        var ir = Lower("""
            function convert(x: f32): i32 {
                let result: i32 = f32_to_i32(x);
                return result;
            }
            """);

        // Should contain fptosi instruction for f32 -> i32 conversion
        Assert.Contains("fptosi float", ir);
        Assert.Contains("to i32", ir);
    }

    [Fact]
    public void StringLiteral_UsesUtf8HeaderPayloadOffset()
    {
        var ir = Lower("""
            function main(): i32 {
                print_string("hi");
                return 0;
            }
            """);

        Assert.Contains("@str_", ir);
        Assert.Contains("getelementptr", ir);
        Assert.Contains("i32 8", ir);
    }

    [Fact]
    public void Lowers_i32_to_f32_in_loop_with_explicit_conversion()
    {
        var ir = Lower("""
            function sum_as_float(): f32 {
                let i: i32 = 0;
                let total: f32 = 0.0;
                for (i = 0; i.<(5); i = i.+(1)) {
                    let if32: f32 = i32_to_f32(i);
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
