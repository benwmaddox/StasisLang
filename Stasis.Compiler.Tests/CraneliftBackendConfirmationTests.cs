using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Xunit;

namespace Stasis.Compiler.Tests;

public class CraneliftBackendConfirmationTests
{
    [Fact]
    public void ComparisonExpressions_ConvertB1ToI32()
    {
        var ir = CompileCraneliftIr("""
            function lt(a: i32, b: i32): bool {
                return a < b;
            }
            """);

        Assert.Contains("icmp slt", ir);
        Assert.Contains("bint.i32", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void UnaryNot_ConvertsB1ToI32()
    {
        var ir = CompileCraneliftIr("""
            function inv(a: bool): bool {
                return !a;
            }
            """);

        Assert.Contains("icmp eq", ir);
        Assert.Contains("bint.i32", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void IfConditions_AreLoweredToB1ForBrif()
    {
        var ir = CompileCraneliftIr("""
            function choose(flag: bool): i32 {
                if (flag) {
                    return 1;
                } else {
                    return 0;
                }
            }
            """);

        Assert.Contains("brif", ir);
        Assert.Contains("icmp ne", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void ForLoopConditions_AreLoweredToB1ForBrif()
    {
        var ir = CompileCraneliftIr("""
            function sum_to_n(n: i32): i32 {
                let total: i32 = 0;
                let i: i32 = 0;
                for (i = 0; i < n; i = i + 1) {
                    total = total + i;
                }
                return total;
            }
            """);

        Assert.Contains("brif", ir);
        Assert.Contains("icmp ne", ir);
        Assert.Contains("icmp slt", ir);
        Assert.Contains("bint.i32", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void ReadInt_UsesStackSlot()
    {
        var ir = CompileCraneliftIr("""
            function main(): i32 {
                let x: i32 = read_int();
                return x;
            }
            """);

        Assert.Contains("stack_slot.i32", ir);
        Assert.Contains("call %scanf", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void ReadChar_UsesStackSlot()
    {
        var ir = CompileCraneliftIr("""
            function main(): i32 {
                let x: i32 = read_char();
                return x;
            }
            """);

        Assert.Contains("stack_slot.i32", ir);
        Assert.Contains("call %scanf", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void PrintString_UsesPrintfStr()
    {
        var ir = CompileCraneliftIr("""
            function main(): i32 {
                print_string("hello");
                return 0;
            }
            """);

        Assert.Contains("call %printf3", ir);
        Assert.Contains("global str_", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void InlineAttribute_InlinesSimpleReturn()
    {
        var ir = CompileCraneliftIr("""
            function @inline add(a: i32, b: i32): i32 {
                return a + b;
            }

            function main(): i32 {
                return add(1, 2);
            }
            """);

        Assert.Contains("iadd", ir);
        Assert.DoesNotContain("call %cranelift_confirm__add", ir);
    }

    [Fact]
    public void Time_UsesTruncationFromI64()
    {
        var ir = CompileCraneliftIr("""
            function main(): i32 {
                let t: i32 = time();
                return t;
            }
            """);

        Assert.Contains("call %time", ir);
        Assert.Contains("ireduce.i32", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void GetTimeMs_UsesRuntimeHook()
    {
        var ir = CompileCraneliftIr("""
            function main(): i32 {
                return get_time_ms();
            }
            """);

        Assert.Contains("call %stasis_get_time_ms", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void SleepMs_UsesRuntimeHook()
    {
        var ir = CompileCraneliftIr("""
            function main(): i32 {
                sleep_ms(5);
                return 0;
            }
            """);

        Assert.Contains("call %stasis_sleep_ms", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void InputPointerCount_UsesRuntimeHook()
    {
        var ir = CompileCraneliftIr("""
            function main(): i32 {
                return input_pointer_count();
            }
            """);

        Assert.Contains("call %stasis_input_pointer_count", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void InputPointerXPx_UsesRuntimeHook()
    {
        var ir = CompileCraneliftIr("""
            function main(): f32 {
                return input_pointer_x_px(0);
            }
            """);

        Assert.Contains("call %stasis_input_pointer_x_px", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void ConstIdentifiers_LowerToConstants()
    {
        var ir = CompileCraneliftIr("""
            const BRICK_WIDTH: f32 = 60.0;
            function main(): f32 {
                return BRICK_WIDTH + 5.0;
            }
            """);

        Assert.Contains("f32const 60", ir);
        Assert.DoesNotContain("unknown identifier BRICK_WIDTH", ir);
    }

    [Fact]
    public void EnumMemberAccess_LowersToI32Const()
    {
        var ir = CompileCraneliftIr("""
            enum Mode { Alpha, Beta, Gamma }
            function main(): i32 {
                return Mode.Gamma;
            }
            """);

        Assert.Contains("iconst.i32 2", ir);
        Assert.DoesNotContain("unknown enum member", ir);
    }

    [Fact]
    public void PrintInt_UsesPrintf()
    {
        var ir = CompileCraneliftIr("""
            function main(): i32 {
                print_int(123);
                return 0;
            }
            """);

        Assert.Contains("call %printf", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void PrintChar_UsesPrintf()
    {
        var ir = CompileCraneliftIr("""
            function main(): i32 {
                print_char(65);
                return 0;
            }
            """);

        Assert.Contains("call %printf", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void StrLen_UsesStrlen()
    {
        var ir = CompileCraneliftIr("""
            global buf: u8[8];
            function main(): i32 {
                return str_len(buf);
            }
            """);

        Assert.Contains("iconst.i64 -8", ir);
        Assert.Contains("load.i32", ir);
        Assert.DoesNotContain("call %strlen", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void StrEq_UsesStrcmp()
    {
        var ir = CompileCraneliftIr("""
            global a: u8[8];
            global b: u8[8];
            function main(): i32 {
                return str_eq(a, b);
            }
            """);

        Assert.Contains("call %strcmp", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void StrFind_UsesStrstr()
    {
        var ir = CompileCraneliftIr("""
            global a: u8[8];
            global b: u8[8];
            function main(): i32 {
                return str_find(a, b);
            }
            """);

        Assert.Contains("call %strstr", ir);
        Assert.Contains("select", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void StrSubstr_UsesMemcpyAndAbort()
    {
        var ir = CompileCraneliftIr("""
            global dst: u8[8];
            global src: u8[8];
            function main(): i32 {
                return str_substr(dst, src, 0, 4);
            }
            """);

        Assert.Contains("call %memcpy", ir);
        Assert.Contains("call %abort", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void TestHarness_EmitsRunTestsEntry()
    {
        var ir = CompileCraneliftIr("""
            function add(a: i32, b: i32): i32 {
                return a + b;
            }

            test `addition works`(): bool {
                return add(2, 3) == 5;
            }
            """, includeTests: true);

        Assert.Contains("function %cranelift_confirm__run_tests()", ir);
        Assert.Contains("call %printf3", ir);
        Assert.Contains("call %printf", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void CompoundAssignment_LowersToBinaryOp()
    {
        var ir = CompileCraneliftIr("""
            function main(): i32 {
                let x: i32 = 1;
                x += 2;
                return x;
            }
            """);

        Assert.Contains("iadd", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void StructArrayLength_UsesConstant()
    {
        var ir = CompileCraneliftIr("""
            struct Foo { values: i32[4]; }
            global foo: Foo;
            function main(): i32 {
                return foo.values.length;
            }
            """);

        Assert.Contains("iconst.i32 4", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void ArrayParameterAccess_LoadsFromPointer()
    {
        var ir = CompileCraneliftIr("""
            function sum(buf: i32[4]): i32 {
                return buf[1];
            }
            """);

        Assert.Contains("load.i32", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void StringLiteral_EmitsUtf8HeaderData()
    {
        var ir = CompileCraneliftIr("""
            function main(): i32 {
                print_string("hi");
                return 0;
            }
            """);

        Assert.Contains("global str_", ir);
        Assert.Contains("bytes:", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    private static string CompileCraneliftIr(string source, bool includeTests = false)
    {
        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var semantic = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        Assert.Empty(semantic.Diagnostics);

        var layout = new LayoutPlanner(parse.CompilationUnit, semantic.Symbols).Plan();

        var options = new CodeGenerationOptions(
            ModuleName: "cranelift_confirm",
            IncludeTests: includeTests,
            EmitTestHarness: includeTests);

        using var generator = CodeGeneratorFactory.Create(BackendType.Cranelift, "cranelift_confirm");
        var result = generator.Generate(parse.CompilationUnit, semantic, layout, options);
        Assert.True(result.Success, string.Join("\n", result.Diagnostics.Select(d => d.Message)));
        Assert.NotEmpty(result.Ir);
        return result.Ir;
    }
}
