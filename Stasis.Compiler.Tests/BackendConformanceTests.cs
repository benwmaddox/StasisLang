using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Xunit;

namespace Stasis.Compiler.Tests;

/// <summary>
/// Tests that verify both LLVM and Cranelift backends can compile the same source code.
/// These tests ensure backend parity for all language features.
/// </summary>
public class BackendConformanceTests
{
    static BackendConformanceTests()
    {
        LlvmNativeLoader.EnsureLoaded();
    }

    [Fact]
    public void CompoundAssignment_U8_TruncatesBeforeStore_OnLlvm()
    {
        var source = @"
global b: u8;

function main(): i32 {
    // RHS is typed as u8 because (b + 1) resolves to the left operand type.
    // This exercises the compound-assignment lowering without relying on implicit numeric conversions.
    b += b + 1;
    return 0;
}
";
        var result = CompileWithBackend(source, BackendType.Llvm);

        Assert.True(result.Success, $"Backend LLVM failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);

        Assert.DoesNotMatch("(?m)^\\s*store\\s+i32\\b.*\\bptr\\s+@b\\b", result.Ir);
        Assert.Matches("(?m)^\\s*store\\s+i8\\b.*\\bptr\\s+@b\\b", result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void SimpleFunction_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function add(a: i32, b: i32): i32 {
    return a + b;
}

function main(): i32 {
    return add(2, 3);
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void ArithmeticOperations_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function math(a: i32, b: i32): i32 {
    let sum: i32 = a + b;
    let diff: i32 = a - b;
    let prod: i32 = a * b;
    let quot: i32 = a / b;
    let rem: i32 = a % b;
    return sum + diff + prod + quot + rem;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void ComparisonOperations_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function compare(a: i32, b: i32): i32 {
    if (a < b) {
        return 1;
    }
    if (a <= b) {
        return 2;
    }
    if (a > b) {
        return 3;
    }
    if (a >= b) {
        return 4;
    }
    if (a == b) {
        return 5;
    }
    if (a != b) {
        return 6;
    }
    return 0;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void LogicalOperations_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function logic(a: bool, b: bool): bool {
    if (a && b) {
        return true;
    }
    if (a || b) {
        return true;
    }
    if (!a) {
        return false;
    }
    return false;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void IfElseStatement_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function abs(x: i32): i32 {
    if (x < 0) {
        return -x;
    } else {
        return x;
    }
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void ForLoop_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function sum_to_n(n: i32): i32 {
    let total: i32 = 0;
    let i: i32 = 0;
    for (i = 0; i < n; i = i + 1) {
        total = total + i;
    }
    return total;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void NestedIfStatements_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function classify(x: i32): i32 {
    if (x < 0) {
        if (x < -10) {
            return 1;
        } else {
            return 2;
        }
    } else {
        if (x > 10) {
            return 3;
        } else {
            return 4;
        }
    }
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void MultipleFunctions_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function double(x: i32): i32 {
    return x * 2;
}

function triple(x: i32): i32 {
    return x * 3;
}

function compute(x: i32): i32 {
    return double(x) + triple(x);
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void UnaryOperations_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function negate(x: i32): i32 {
    return -x;
}

function not_bool(x: bool): bool {
    return !x;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void TestFunctions_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function add(a: i32, b: i32): i32 {
    return a + b;
}

test `addition works`(): bool {
    return add(2, 3) == 5;
}

test `negative addition`(): bool {
    return add(-1, 1) == 0;
}
";
        var result = CompileWithBackend(source, backend, includeTests: true);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void LocalVariables_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function compute(x: i32): i32 {
    let a: i32 = x + 1;
    let b: i32 = a * 2;
    let c: i32 = b - 3;
    return c;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void BooleanLiterals_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function get_true(): bool {
    return true;
}

function get_false(): bool {
    return false;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void NestedStructMemberAccess_UsesFlattenedGlobal(BackendType backend)
    {
        var source = @"
struct Weapon {
    x: i32;
}

struct Ship {
    weapon: Weapon;
}

struct State {
    ship: Ship;
}

global state: State;

function main(): i32 {
    state.ship.weapon.x = 42;
    return state.ship.weapon.x;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);

        if (backend == BackendType.Llvm)
        {
            Assert.Contains("@state_ship_weapon_x", result.Ir);
            Assert.Contains("store i32 42, ptr @state_ship_weapon_x", result.Ir);
            Assert.Contains("load i32, ptr @state_ship_weapon_x", result.Ir);
        }
        else
        {
            Assert.Contains("global state__ship__weapon__x", result.Ir);
            Assert.Contains("global_value state__ship__weapon__x", result.Ir);
            Assert.DoesNotContain("TODO:", result.Ir);
        }
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void ReadChar_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function main(): i32 {
    let x: i32 = read_char();
    return x;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);

        if (backend == BackendType.Llvm)
        {
            Assert.Contains("scanf", result.Ir);
        }
        else
        {
            Assert.Contains("stack_slot.i32", result.Ir);
            Assert.Contains("call %scanf", result.Ir);
            Assert.DoesNotContain("TODO:", result.Ir);
        }
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void PrintString_CompilesOnBothBackends(BackendType backend)
    {
        var source = @"
function main(): i32 {
    print_string(""hello"");
    return 0;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);

        if (backend == BackendType.Cranelift)
        {
            Assert.Contains("call %printf3", result.Ir);
            Assert.DoesNotContain("TODO:", result.Ir);
        }
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void TimeBuiltins_CompileOnBothBackends(BackendType backend)
    {
        var source = @"
function main(): i32 {
    let t: i32 = time();
    let ms: i32 = get_time_ms();
    sleep_ms(1);
    return t + ms;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);

        if (backend == BackendType.Llvm)
        {
            Assert.Contains("@time", result.Ir);
            Assert.True(result.Ir.Contains("@stasis_get_time_ms") || result.Ir.Contains("@clock"),
                "LLVM IR should call stasis_get_time_ms or clock for get_time_ms.");
        }
        else
        {
            Assert.Contains("call %time", result.Ir);
            Assert.Contains("call %stasis_get_time_ms", result.Ir);
            Assert.Contains("call %stasis_sleep_ms", result.Ir);
            Assert.DoesNotContain("TODO:", result.Ir);
        }
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void PrintIntAndChar_CompileOnBothBackends(BackendType backend)
    {
        var source = @"
function main(): i32 {
    print_int(7);
    print_char(65);
    return 0;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);

        if (backend == BackendType.Cranelift)
        {
            Assert.Contains("call %printf", result.Ir);
            Assert.DoesNotContain("TODO:", result.Ir);
        }
    }

    [Theory]
    [InlineData(BackendType.Llvm)]
    [InlineData(BackendType.Cranelift)]
    public void StringBuiltins_CompileOnBothBackends(BackendType backend)
    {
        var source = @"
global a: u8[16];
global b: u8[16];
global dst: u8[16];

function main(): i32 {
    str_clear(a);
    str_set(a, 0, 65);
    str_set(a, 1, 0);
    let len: i32 = str_len(a);
    let eq: i32 = str_eq(a, b);
    let idx: i32 = str_find(a, b);
    let sub: i32 = str_substr(dst, a, 0, 1);
    return len + eq + idx + sub;
}
";
        var result = CompileWithBackend(source, backend);

        Assert.True(result.Success, $"Backend {backend} failed: {string.Join(", ", result.Diagnostics.Select(d => d.Message))}");
        Assert.NotEmpty(result.Ir);

        if (backend == BackendType.Cranelift)
        {
            Assert.Contains("call %strlen", result.Ir);
            Assert.Contains("call %strcmp", result.Ir);
            Assert.Contains("call %strstr", result.Ir);
            Assert.Contains("call %memcpy", result.Ir);
            Assert.Contains("call %abort", result.Ir);
            Assert.DoesNotContain("TODO:", result.Ir);
        }
        else
        {
            Assert.Contains("memcpy", result.Ir);
            Assert.Contains("abort", result.Ir);
        }
    }

    [Fact]
    public void BothBackends_GenerateIrForSameSource()
    {
        var source = @"
function add(a: i32, b: i32): i32 {
    return a + b;
}
";
        var llvmResult = CompileWithBackend(source, BackendType.Llvm);
        var craneliftResult = CompileWithBackend(source, BackendType.Cranelift);

        Assert.True(llvmResult.Success, "LLVM compilation failed");
        Assert.True(craneliftResult.Success, "Cranelift compilation failed");

        // Both should generate non-empty IR
        Assert.NotEmpty(llvmResult.Ir);
        Assert.NotEmpty(craneliftResult.Ir);

        // LLVM IR contains specific markers
        Assert.Contains("define", llvmResult.Ir);
        Assert.Contains("@add", llvmResult.Ir);

        // Cranelift IR contains specific markers
        Assert.Contains("function", craneliftResult.Ir);
        Assert.Contains("%test_module__add", craneliftResult.Ir);
    }

    private static CodeGenerationResult CompileWithBackend(string source, BackendType backend, bool includeTests = false)
    {
        var parse = Parser.Parse(source);
        if (parse.Diagnostics.Count > 0)
        {
            return new CodeGenerationResult(string.Empty, parse.Diagnostics);
        }

        var semantic = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        if (semantic.Diagnostics.Count > 0)
        {
            return new CodeGenerationResult(string.Empty, semantic.Diagnostics);
        }

        var layout = new LayoutPlanner(parse.CompilationUnit, semantic.Symbols).Plan();

        var options = new CodeGenerationOptions(
            ModuleName: "test_module",
            IncludeTests: includeTests,
            EmitTestHarness: includeTests);

        using var generator = CodeGeneratorFactory.Create(backend, "test_module");
        return generator.Generate(parse.CompilationUnit, semantic, layout, options);
    }
}
