using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;

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

    private static bool TryFindClangSibling(string llvmToolPath, out string clangPath)
    {
        var dir = Path.GetDirectoryName(llvmToolPath);
        if (!string.IsNullOrEmpty(dir))
        {
            var candidate = Path.Combine(dir, RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "clang.exe" : "clang");
            if (File.Exists(candidate))
            {
                clangPath = candidate;
                return true;
            }
        }

        clangPath = string.Empty;
        return false;
    }

    private static bool LooksLikeLlvmCrashReport(string stderr)
    {
        return stderr.Contains("PLEASE submit a bug report", StringComparison.OrdinalIgnoreCase)
            || stderr.Contains("Stack dump:", StringComparison.OrdinalIgnoreCase);
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
                let sprite: i32 = gfx_load_sprite("dummy", 64, 64);
                let reloaded: bool = gfx_poll_reload(sprite);
                let font: i32 = load_font("dummy", 16);
                let w: f32 = measure_text(font, "hello");
                let t: i32 = get_time_ms();
                sleep_ms(0);
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
                return len;
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
            Assert.Equal(3, exitCode);
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
            if (string.IsNullOrWhiteSpace(stderr) || stderr.Contains("abort", StringComparison.OrdinalIgnoreCase))
            {
                return;
            }

            // On some Windows setups, abort() under lli is reported as an LLVM crash dump (illegal instruction).
            // Fall back to compiling the IR with clang (same LLVM toolchain) and executing the native binary.
            if (!LooksLikeLlvmCrashReport(stderr) || !TryFindClangSibling(lliPath, out var clangPath))
            {
                return;
            }

            var exePath = Path.Combine(Path.GetTempPath(), $"stasis_exec_{Guid.NewGuid():N}.exe");
            try
            {
                var (clangExit, clangStderr) = RunProcess(clangPath, $"-x ir \"{temp}\" -o \"{exePath}\"");
                if (clangExit != 0)
                {
                    return;
                }

                var (nativeExit, nativeStderr) = RunProcess(exePath, string.Empty);
                Assert.NotEqual(0, nativeExit);
                Assert.True(string.IsNullOrWhiteSpace(nativeStderr) || nativeStderr.Contains("abort", StringComparison.OrdinalIgnoreCase), nativeStderr);
            }
            finally
            {
                if (File.Exists(exePath))
                {
                    File.Delete(exePath);
                }
            }
        }
        finally
        {
            File.Delete(temp);
        }
    }
}
