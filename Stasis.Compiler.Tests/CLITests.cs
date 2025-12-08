using System.Diagnostics;

namespace Stasis.Compiler.Tests;

public class CLITests
{
    private static readonly string RepoRoot = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "..", "..", "..", ".."));
    private static readonly string CliProj = Path.Combine(RepoRoot, "Stasis.Cli", "Stasis.Cli.csproj");
    private static readonly string Configuration = new DirectoryInfo(AppContext.BaseDirectory).Parent?.Parent?.Name ?? "Debug";

    [Fact]
    public void Emits_ir_for_basic_sample()
    {
        var (exit, stdout, stderr) = RunCli($"run \"{Path.Combine(RepoRoot, "samples", "basic.stasis")}\" --emit-ir");
        Assert.Equal(0, exit);
        Assert.Contains("define i32 @main()", stdout);
        Assert.Contains("define i32 @add", stdout);
        Assert.True(string.IsNullOrWhiteSpace(stderr), stderr);
    }

    [Fact]
    public void Emits_ir_for_tests_harness()
    {
        var (exit, stdout, stderr) = RunCli($"test \"{Path.Combine(RepoRoot, "samples", "tests.stasis")}\" --emit-ir");
        Assert.Equal(0, exit);
        Assert.Contains("define i32 @run_tests()", stdout);
        Assert.Contains("`adds numbers`", stdout);
        Assert.True(string.IsNullOrWhiteSpace(stderr), stderr);
    }

    [Fact]
    public void Runs_operator_sample_tests()
    {
        var samplePath = Path.Combine(RepoRoot, "samples", "operators.stasis");
        var (exit, stdout, stderr) = RunCli($"test \"{samplePath}\"");
        Assert.Equal(0, exit);
        Assert.Contains("precedence", stdout);
        Assert.Contains("compound", stdout);
        Assert.True(string.IsNullOrWhiteSpace(stderr), stderr);
    }

    private static (int ExitCode, string Stdout, string Stderr) RunCli(string args, string? workingDirectory = null)
    {
        var psi = new ProcessStartInfo
        {
            FileName = "dotnet",
            Arguments = $"run --no-restore --configuration {Configuration} --project \"{CliProj}\" -- {args}",
            WorkingDirectory = workingDirectory ?? RepoRoot,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false
        };

        using var proc = Process.Start(psi)!;
        var stdout = proc.StandardOutput.ReadToEnd();
        var stderr = proc.StandardError.ReadToEnd();
        proc.WaitForExit();
        return (proc.ExitCode, stdout, stderr);
    }

    [Fact]
    public void Runs_tests_in_directory_when_no_path_provided()
    {
        var tempDir = Path.Combine(Path.GetTempPath(), $"stasis_cli_{Guid.NewGuid():N}");
        Directory.CreateDirectory(tempDir);
        try
        {
            File.WriteAllText(Path.Combine(tempDir, "a.stasis"), """
                test one(): bool { return true; }
                """);
            File.WriteAllText(Path.Combine(tempDir, "b.stasis"), """
                test two(): bool { return true; }
                """);

            var (exit, stdout, stderr) = RunCli("test", tempDir);
            Assert.Equal(0, exit);
            Assert.Contains("=== " + Path.Combine(tempDir, "a.stasis"), stdout);
            Assert.Contains("=== " + Path.Combine(tempDir, "b.stasis"), stdout);
            Assert.True(string.IsNullOrWhiteSpace(stderr), stderr);
        }
        finally
        {
            Directory.Delete(tempDir, recursive: true);
        }
    }
}
