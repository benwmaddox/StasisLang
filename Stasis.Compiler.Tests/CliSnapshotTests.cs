using System.Diagnostics;
using System.Text;
using VerifyXunit;

namespace Stasis.Compiler.Tests;

public class CliSnapshotTests
{
    private const string CliProject = "Stasis.Cli";

    private static string GetRepoRoot()
    {
        var current = Directory.GetCurrentDirectory();
        while (current != null && !File.Exists(Path.Combine(current, "Stasis.sln")))
        {
            current = Directory.GetParent(current)?.FullName;
        }
        return current ?? throw new InvalidOperationException("Could not find repo root");
    }

    private static string GetSamplePath(string name)
    {
        var root = GetRepoRoot();
        return Path.Combine(root, "samples", name);
    }

    private static string GetBuildConfiguration()
    {
        // Detect configuration from the test assembly path
        var assemblyPath = typeof(CliSnapshotTests).Assembly.Location;
        if (assemblyPath.Contains("Release", StringComparison.OrdinalIgnoreCase))
        {
            return "Release";
        }
        return "Debug";
    }

    private static (int exitCode, string stdout, string stderr) RunCli(params string[] args)
    {
        var root = GetRepoRoot();
        var cliProj = Path.Combine(root, CliProject, $"{CliProject}.csproj");
        var config = GetBuildConfiguration();

        var psi = new ProcessStartInfo
        {
            FileName = "dotnet",
            Arguments = $"run --no-build --configuration {config} --project \"{cliProj}\" -- {string.Join(" ", args)}",
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            WorkingDirectory = root
        };

        var stdout = new StringBuilder();
        var stderr = new StringBuilder();

        using var process = new Process { StartInfo = psi };
        process.OutputDataReceived += (_, e) =>
        {
            if (e.Data != null)
            {
                stdout.AppendLine(e.Data);
            }
        };
        process.ErrorDataReceived += (_, e) =>
        {
            if (e.Data != null)
            {
                stderr.AppendLine(e.Data);
            }
        };

        process.Start();
        process.BeginOutputReadLine();
        process.BeginErrorReadLine();
        process.WaitForExit();

        return (process.ExitCode, stdout.ToString().TrimEnd(), stderr.ToString().TrimEnd());
    }

    private static string ScrubOutput(string output)
    {
        // Remove timing information as it varies between runs
        var lines = output.Split('\n', StringSplitOptions.RemoveEmptyEntries);
        var filtered = lines
            .Where(line => !line.Contains("Total time=") && !line.Contains("ms"))
            .Select(line => line.TrimEnd('\r'));
        return string.Join("\n", filtered);
    }

    [Fact]
    public Task EmitIr_Basic()
    {
        var (exitCode, stdout, stderr) = RunCli("run", GetSamplePath("basic.stasis"), "--emit-ir");
        var result = new
        {
            ExitCode = exitCode,
            Stdout = ScrubOutput(stdout),
            Stderr = ScrubOutput(stderr)
        };
        return Verifier.Verify(result).UseDirectory("Snapshots");
    }

    [Fact]
    public Task EmitIr_Tests()
    {
        var (exitCode, stdout, stderr) = RunCli("test", GetSamplePath("tests.stasis"), "--emit-ir");
        var result = new
        {
            ExitCode = exitCode,
            Stdout = ScrubOutput(stdout),
            Stderr = ScrubOutput(stderr)
        };
        return Verifier.Verify(result).UseDirectory("Snapshots");
    }

    [Fact]
    public Task Run_Basic()
    {
        if (!TryFindLli())
        {
            // Skip if lli not available
            return Task.CompletedTask;
        }

        var (exitCode, stdout, stderr) = RunCli("run", GetSamplePath("basic.stasis"));
        var result = new
        {
            ExitCode = exitCode,
            Stdout = ScrubOutput(stdout),
            Stderr = ScrubOutput(stderr)
        };
        return Verifier.Verify(result).UseDirectory("Snapshots");
    }

    [Fact]
    public Task Test_TestsFile()
    {
        if (!TryFindLli())
        {
            // Skip if lli not available
            return Task.CompletedTask;
        }

        var (exitCode, stdout, stderr) = RunCli("test", GetSamplePath("tests.stasis"));
        var result = new
        {
            ExitCode = exitCode,
            Stdout = ScrubOutput(stdout),
            Stderr = ScrubOutput(stderr)
        };
        return Verifier.Verify(result).UseDirectory("Snapshots");
    }

    [Fact]
    public Task Error_FileNotFound()
    {
        var (exitCode, stdout, stderr) = RunCli("run", "nonexistent.stasis");
        var result = new
        {
            ExitCode = exitCode,
            Stdout = ScrubOutput(stdout),
            Stderr = ScrubOutput(stderr)
        };
        return Verifier.Verify(result).UseDirectory("Snapshots");
    }

    [Fact]
    public Task Error_ParseError()
    {
        // Create a temp file with invalid syntax
        var temp = Path.GetTempFileName();
        File.WriteAllText(temp, "function broken {");

        try
        {
            var (exitCode, stdout, stderr) = RunCli("run", temp);
            var result = new
            {
                ExitCode = exitCode,
                Stdout = ScrubOutput(stdout),
                // Scrub the temp file path from stderr
                Stderr = ScrubOutput(stderr).Replace(temp, "<temp-file>")
            };
            return Verifier.Verify(result).UseDirectory("Snapshots");
        }
        finally
        {
            File.Delete(temp);
        }
    }

    private static bool TryFindLli()
    {
        var search = (Environment.GetEnvironmentVariable("PATH") ?? string.Empty)
            .Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries);

        foreach (var dir in search)
        {
            var candidate = Path.Combine(dir, OperatingSystem.IsWindows() ? "lli.exe" : "lli");
            if (File.Exists(candidate))
            {
                return true;
            }
        }

        return false;
    }
}
