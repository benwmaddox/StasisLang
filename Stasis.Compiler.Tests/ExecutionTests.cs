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
        LlvmNativeLoader.EnsureLoaded();
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
}
