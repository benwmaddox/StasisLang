using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;
using Xunit.Sdk;

namespace Stasis.Compiler.Tests;

public class ExecutionTests
{
    static ExecutionTests()
    {
        Stasis.Compiler.LlvmNativeLoader.EnsureLoaded();
    }

    [Fact]
    public void Runs_main_via_lli()
    {
        if (!TryFindLli(out var lliPath))
        {
            return;
        }

        var source = """
            function add(a: i32, b: i32): i32 {
                return a.+(b);
            }

            function main(): i32 {
                return add(2, 3);
            }
            """;

        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        Assert.Empty(sema.Diagnostics);

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        var lower = new ModuleLowerer().LowerToIr(parse.CompilationUnit, sema, layout, "execmodule", LowerOptions.Production);
        Assert.Empty(lower.Diagnostics);

        var temp = Path.GetTempFileName();
        File.WriteAllText(temp, lower.Ir);

        var (exitCode, stderr) = RunProcess(lliPath, temp);
        try
        {
            Assert.Equal(5, exitCode);
            Assert.True(string.IsNullOrWhiteSpace(stderr), stderr);
        }
        finally
        {
            File.Delete(temp);
        }
    }

    private static (int ExitCode, string Stderr) RunProcess(string fileName, string arg)
    {
        var psi = new ProcessStartInfo
        {
            FileName = fileName,
            Arguments = arg,
            RedirectStandardError = true,
            RedirectStandardOutput = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };

        using var proc = Process.Start(psi)!;
        proc.WaitForExit();
        var stderr = proc.StandardError.ReadToEnd();
        return (proc.ExitCode, stderr);
    }

    private static bool TryFindLli(out string path)
    {
        var search = Environment.GetEnvironmentVariable("PATH")?.Split(Path.PathSeparator) ?? Array.Empty<string>();
        foreach (var dir in search)
        {
            var candidate = Path.Combine(dir, RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "lli.exe" : "lli");
            if (File.Exists(candidate))
            {
                path = candidate;
                return true;
            }
        }

        path = string.Empty;
        return false;
    }

    [Fact]
    public void Runs_stasis_tests_via_run_tests_harness()
    {
        if (!TryFindLli(out var lliPath))
        {
            return;
        }

        var source = """
            function add(a: i32, b: i32): i32 {
                return a.+(b);
            }

            test check_math(): bool {
                return add(2, 3).==(5);
            }

            test always_true(): bool {
                return true;
            }
            """;

        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        Assert.Empty(sema.Diagnostics);

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        var lower = new ModuleLowerer().LowerToIr(parse.CompilationUnit, sema, layout, "testmodule", LowerOptions.Default);
        Assert.Empty(lower.Diagnostics);

        var temp = Path.GetTempFileName();
        File.WriteAllText(temp, lower.Ir);

        var (exitCode, stderr) = RunProcess(lliPath, $"-entry-function=run_tests \"{temp}\"");
        try
        {
            Assert.Equal(0, exitCode);
            Assert.True(string.IsNullOrWhiteSpace(stderr), stderr);
        }
        finally
        {
            File.Delete(temp);
        }
    }

    [Fact]
    public void Runs_compound_and_precedence()
    {
        if (!TryFindLli(out var lliPath))
        {
            return;
        }

        var source = """
            function main(): i32 {
                let x: i32 = 1;
                x += 2 * 3;
                x -= 4 / 2;
                return x;
            }
            """;

        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        Assert.Empty(sema.Diagnostics);

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        var lower = new ModuleLowerer().LowerToIr(parse.CompilationUnit, sema, layout, "execops", LowerOptions.Production);
        Assert.Empty(lower.Diagnostics);

        var temp = Path.GetTempFileName();
        File.WriteAllText(temp, lower.Ir);

        var (exitCode, stderr) = RunProcess(lliPath, temp);
        try
        {
            Assert.Equal(5, exitCode);
            Assert.True(string.IsNullOrWhiteSpace(stderr), stderr);
        }
        finally
        {
            File.Delete(temp);
        }
    }

    [Fact]
    public void Runs_headless_graphics_builtins()
    {
        if (!TryFindLli(out var lliPath))
        {
            return;
        }

        var source = """
            function main(): i32 {
                let ok: bool = init_window(640, 480, "Stasis");
                begin_frame();
                clear(0.0, 0.0, 0.0, 1.0);
                draw_line(-1.0, -1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0);
                let down: bool = is_key_down(32);
                let t: i32 = get_time_ms();
                sleep_ms(0);
                end_frame();
                return 0;
            }
            """;

        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        Assert.Empty(sema.Diagnostics);

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        var lower = new ModuleLowerer().LowerToIr(parse.CompilationUnit, sema, layout, "gfxmodule", LowerOptions.Production);
        Assert.Empty(lower.Diagnostics);

        var temp = Path.GetTempFileName();
        File.WriteAllText(temp, lower.Ir);

        var (exitCode, stderr) = RunProcess(lliPath, temp);
        try
        {
            Assert.Equal(0, exitCode);
            Assert.True(string.IsNullOrWhiteSpace(stderr), stderr);
        }
        finally
        {
            File.Delete(temp);
        }
    }

    [Fact]
    public void StrSubstr_copies_full_codepoint()
    {
        if (!TryFindLli(out var lliPath))
        {
            return;
        }

        var source = """
            global src: u8[16];
            global dst: u8[16];

            function main(): i32 {
                // src = 0xE2 0x82 0xAC 'A' '\0'
                src[0] = 226;
                src[1] = 130;
                src[2] = 172;
                src[3] = 65;
                src[4] = 0;
                let len: i32 = str_substr(dst, src, 0, 3);
                return (len == 3) && (dst[0] == 226) && (dst[1] == 130) && (dst[2] == 172) && (dst[3] == 0);
            }
            """;

        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        Assert.Empty(sema.Diagnostics);

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        var lower = new ModuleLowerer().LowerToIr(parse.CompilationUnit, sema, layout, "substr_ok", LowerOptions.Production);
        Assert.Empty(lower.Diagnostics);

        var temp = Path.GetTempFileName();
        File.WriteAllText(temp, lower.Ir);

        var (exitCode, stderr) = RunProcess(lliPath, temp);
        try
        {
            Assert.Equal(1, exitCode);
            Assert.True(string.IsNullOrWhiteSpace(stderr), stderr);
        }
        finally
        {
            File.Delete(temp);
        }
    }

    [Fact]
    public void StrSubstr_aborts_on_misaligned_boundary()
    {
        if (!TryFindLli(out var lliPath))
        {
            return;
        }

        var source = """
            global src: u8[16];
            global dst: u8[16];

            function main(): i32 {
                // src = 0xE2 0x82 0xAC 'A' '\0'
                src[0] = 226;
                src[1] = 130;
                src[2] = 172;
                src[3] = 65;
                src[4] = 0;
                return str_substr(dst, src, 1, 2); // mid-codepoint start should abort
            }
            """;

        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        Assert.Empty(sema.Diagnostics);

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        var lower = new ModuleLowerer().LowerToIr(parse.CompilationUnit, sema, layout, "substr_abort", LowerOptions.Production);
        Assert.Empty(lower.Diagnostics);

        var temp = Path.GetTempFileName();
        File.WriteAllText(temp, lower.Ir);

        var (exitCode, stderr) = RunProcess(lliPath, temp);
        try
        {
            Assert.NotEqual(0, exitCode);
            Assert.True(string.IsNullOrWhiteSpace(stderr) || stderr.Contains("abort", StringComparison.OrdinalIgnoreCase), stderr);
        }
        finally
        {
            File.Delete(temp);
        }
    }
}
