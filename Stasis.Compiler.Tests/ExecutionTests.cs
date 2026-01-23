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

    private static string GetRepoRoot()
    {
        var current = Directory.GetCurrentDirectory();
        var candidate = FindRepoRoot(current);
        if (!string.IsNullOrEmpty(candidate))
        {
            return candidate;
        }

        var assemblyDir = Path.GetDirectoryName(typeof(ExecutionTests).Assembly.Location);
        candidate = FindRepoRoot(assemblyDir);
        if (!string.IsNullOrEmpty(candidate))
        {
            return candidate;
        }

        throw new InvalidOperationException("Could not find repo root");
    }

    private static string? FindRepoRoot(string? start)
    {
        var current = start;
        while (current != null && !File.Exists(Path.Combine(current, "Stasis.sln")))
        {
            current = Directory.GetParent(current)?.FullName;
        }

        return current;
    }

    [Fact]
    public void Runs_main_via_clang()
    {
        Assert.True(TryFindClang(out var clangPath), "Missing LLVM tooling (clang) on PATH. CI should install an LLVM toolchain.");

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

        var (exitCode, stderr) = CompileAndRunIr(clangPath, temp);
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

    private static (int ExitCode, string Stderr) CompileAndRunIr(string clangPath, string irPath, string? entryFunction = null)
    {
        var objPath = Path.Combine(
            Path.GetTempPath(),
            $"stasis_ir_{Guid.NewGuid():N}{(RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".obj" : ".o")}");

        var exePath = Path.Combine(
            Path.GetTempPath(),
            $"stasis_exec_{Guid.NewGuid():N}{(RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".exe" : string.Empty)}");

        var wrapperPath = string.Empty;
        try
        {
            var (compileExit, compileStderr) = RunProcess(clangPath, $"-x ir -c \"{irPath}\" -O0 -o \"{objPath}\"");
            Assert.True(compileExit == 0, $"clang IR compile failed (exit={compileExit}).\nstderr:\n{compileStderr}");

            if (!string.IsNullOrWhiteSpace(entryFunction))
            {
                wrapperPath = Path.Combine(Path.GetTempPath(), $"stasis_wrap_{Guid.NewGuid():N}.c");
                File.WriteAllText(wrapperPath,
                    $"extern int {entryFunction}(void);\nint main(void) {{ return {entryFunction}(); }}\n");
                var (linkExit, linkStderr) = RunProcess(clangPath, $"\"{wrapperPath}\" \"{objPath}\" -O0 -o \"{exePath}\"");
                Assert.True(linkExit == 0, $"clang link failed (exit={linkExit}).\nstderr:\n{linkStderr}");
            }
            else
            {
                var (linkExit, linkStderr) = RunProcess(clangPath, $"\"{objPath}\" -O0 -o \"{exePath}\"");
                Assert.True(linkExit == 0, $"clang link failed (exit={linkExit}).\nstderr:\n{linkStderr}");
            }

            return RunProcess(exePath, string.Empty);
        }
        finally
        {
            if (!string.IsNullOrEmpty(wrapperPath) && File.Exists(wrapperPath))
            {
                File.Delete(wrapperPath);
            }

            if (File.Exists(objPath))
            {
                File.Delete(objPath);
            }

            if (File.Exists(exePath))
            {
                File.Delete(exePath);
            }
        }
    }

    private static bool LooksLikeLlvmCrashReport(string stderr)
    {
        return stderr.Contains("PLEASE submit a bug report", StringComparison.OrdinalIgnoreCase)
            || stderr.Contains("Stack dump:", StringComparison.OrdinalIgnoreCase);
    }

    private static bool TryFindClang(out string path)
    {
        var root = GetRepoRoot();
        var search = (Environment.GetEnvironmentVariable("PATH") ?? string.Empty)
            .Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries)
            .ToList();

        var toolsDir = Path.Combine(root, ".tools");
        if (Directory.Exists(toolsDir))
        {
            foreach (var llvmDir in Directory.EnumerateDirectories(toolsDir, "llvm-*", SearchOption.TopDirectoryOnly).OrderByDescending(p => p, StringComparer.OrdinalIgnoreCase))
            {
                search.Add(Path.Combine(llvmDir, "bin"));
            }
        }

        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            var programFiles = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles);
            var programFilesX86 = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86);
            search.Add(Path.Combine(programFiles, "LLVM", "bin"));
            search.Add(Path.Combine(programFilesX86, "LLVM", "bin"));
        }

        foreach (var dir in search)
        {
            var candidate = Path.Combine(dir, RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "clang.exe" : "clang");
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
        Assert.True(TryFindClang(out var clangPath), "Missing LLVM tooling (clang) on PATH. CI should install an LLVM toolchain.");

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

        var (exitCode, stderr) = CompileAndRunIr(clangPath, temp, entryFunction: "run_tests");
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
        Assert.True(TryFindClang(out var clangPath), "Missing LLVM tooling (clang) on PATH. CI should install an LLVM toolchain.");

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

        var (exitCode, stderr) = CompileAndRunIr(clangPath, temp);
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
        Assert.True(TryFindClang(out var clangPath), "Missing LLVM tooling (clang) on PATH. CI should install an LLVM toolchain.");

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

        var (exitCode, stderr) = CompileAndRunIr(clangPath, temp);
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
        Assert.True(TryFindClang(out var clangPath), "Missing LLVM tooling (clang) on PATH. CI should install an LLVM toolchain.");

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

        var (exitCode, stderr) = CompileAndRunIr(clangPath, temp);
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
        Assert.True(TryFindClang(out var clangPath), "Missing LLVM tooling (clang) on PATH. CI should install an LLVM toolchain.");

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

        var (exitCode, stderr) = CompileAndRunIr(clangPath, temp);
        try
        {
            Assert.NotEqual(0, exitCode);
            Assert.True(
                string.IsNullOrWhiteSpace(stderr) ||
                stderr.Contains("abort", StringComparison.OrdinalIgnoreCase) ||
                LooksLikeLlvmCrashReport(stderr),
                stderr);
        }
        finally
        {
            File.Delete(temp);
        }
    }
}
