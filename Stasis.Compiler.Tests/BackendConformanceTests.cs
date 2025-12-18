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
        Assert.Contains("%add", craneliftResult.Ir);
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
