using System.Diagnostics;
using System.Text;
using VerifyXunit;

namespace Stasis.Compiler.Tests;

public class CliSnapshotTests
{
    private const string CliProject = "Stasis.Cli";
    private static readonly object CliBuildLock = new();
    private static string? BuiltCliConfiguration;

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

        EnsureCliBuilt(cliProj, root, config);

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

    private static void EnsureCliBuilt(string cliProj, string root, string configuration)
    {
        lock (CliBuildLock)
        {
            if (string.Equals(BuiltCliConfiguration, configuration, StringComparison.OrdinalIgnoreCase))
            {
                return;
            }

            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = $"build --nologo --configuration {configuration} \"{cliProj}\"",
                UseShellExecute = false,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                WorkingDirectory = root
            };

            using var process = Process.Start(psi)!;
            process.WaitForExit();
            if (process.ExitCode != 0)
            {
                var stdout = process.StandardOutput.ReadToEnd();
                var stderr = process.StandardError.ReadToEnd();
                throw new InvalidOperationException($"Failed to build CLI ({process.ExitCode}).\nstdout:\n{stdout}\nstderr:\n{stderr}");
            }

            BuiltCliConfiguration = configuration;
        }
    }

    private static string ScrubOutput(string output)
    {
        // Remove ANSI color codes
        output = System.Text.RegularExpressions.Regex.Replace(output, @"\x1B\[[0-9;]*m", "");

        // Remove timing information from lines (but keep the rest of the line)
        output = System.Text.RegularExpressions.Regex.Replace(output, @"\s*Total time=\S*", "");
        output = System.Text.RegularExpressions.Regex.Replace(output, @"\s*test-time=\S*", "");

        // Remove platform-specific content
        var lines = output.Split('\n');
        var filtered = lines
            .Where(line => !line.TrimStart().StartsWith("target triple"))  // Platform-specific
            .Select(line => line.TrimEnd('\r'))
            .Select(line => line.TrimEnd());  // Remove trailing whitespace

        // Normalize platform-specific clock constant (CLOCKS_PER_SEC differs: Linux=1000000, Windows=1000)
        filtered = filtered.Select(line =>
            System.Text.RegularExpressions.Regex.Replace(line, @"udiv i64 %clock\.ticks_ms, \d+", "udiv i64 %clock.ticks_ms, <CLOCKS_PER_SEC>"));

        // Normalize consecutive blank lines to single blank line
        var result = new List<string>();
        var lastWasBlank = false;
        foreach (var line in filtered)
        {
            var isBlank = string.IsNullOrWhiteSpace(line);
            if (isBlank && lastWasBlank) continue;
            result.Add(line);
            lastWasBlank = isBlank;
        }

        return string.Join("\n", result).Trim();
    }

    [Fact]
    public Task EmitIr_Basic()
    {
        var (exitCode, stdout, stderr) = RunCli("run", GetSamplePath("basic.stasis"), "--emit-ir", "--backend", "llvm");
        var result = new
        {
            ExitCode = exitCode,
            Stdout = ScrubOutput(stdout),
            Stderr = ScrubOutput(stderr)
        };
        return Verifier.Verify(result).UseDirectory("Snapshots");
    }

    [Fact]
    public Task EmitIr_Cranelift_Minimal()
    {
        var temp = Path.GetTempFileName();
        File.WriteAllText(temp, """
            function main(): i32 {
                let x: i32 = 2 + 3;
                return x;
            }
            """);

        try
        {
            var (exitCode, stdout, stderr) = RunCli("run", temp, "--backend", "cranelift", "--emit-ir");
            var result = new
            {
                ExitCode = exitCode,
                Stdout = ScrubOutput(stdout).Replace(temp, "<temp-file>"),
                Stderr = ScrubOutput(stderr).Replace(temp, "<temp-file>")
            };
            return Verifier.Verify(result).UseDirectory("Snapshots");
        }
        finally
        {
            File.Delete(temp);
        }
    }

    [Fact]
    public Task EmitIr_Tests()
    {
        var (exitCode, stdout, stderr) = RunCli("test", GetSamplePath("tests.stasis"), "--emit-ir", "--backend", "llvm");
        var result = new
        {
            ExitCode = exitCode,
            Stdout = ScrubOutput(stdout),
            Stderr = ScrubOutput(stderr)
        };
        return Verifier.Verify(result).UseDirectory("Snapshots");
    }

    [Fact]
    public Task EmitIr_Tests_DefaultBackend_Cranelift()
    {
        var temp = Path.GetTempFileName();
        File.WriteAllText(temp, """
            function add(a: i32, b: i32): i32 {
                return a + b;
            }

            test `addition works`(): bool {
                return add(2, 3) == 5;
            }
            """);

        try
        {
            var (exitCode, stdout, stderr) = RunCli("test", temp, "--emit-ir");
            var result = new
            {
                ExitCode = exitCode,
                Stdout = ScrubOutput(stdout).Replace(temp, "<temp-file>"),
                Stderr = ScrubOutput(stderr).Replace(temp, "<temp-file>")
            };
            return Verifier.Verify(result).UseDirectory("Snapshots");
        }
        finally
        {
            File.Delete(temp);
        }
    }

    [Fact]
    public Task EmitIr_Cranelift_WithTests_Minimal()
    {
        var temp = Path.GetTempFileName();
        File.WriteAllText(temp, """
            function add(a: i32, b: i32): i32 {
                return a + b;
            }

            test `addition works`(): bool {
                return add(2, 3) == 5;
            }
            """);

        try
        {
            var (exitCode, stdout, stderr) = RunCli("test", temp, "--backend", "cranelift", "--emit-ir");
            var result = new
            {
                ExitCode = exitCode,
                Stdout = ScrubOutput(stdout).Replace(temp, "<temp-file>"),
                Stderr = ScrubOutput(stderr).Replace(temp, "<temp-file>")
            };
            return Verifier.Verify(result).UseDirectory("Snapshots");
        }
        finally
        {
            File.Delete(temp);
        }
    }

    [Fact]
    public Task Run_Basic()
    {
        if (!TryFindLli())
        {
            // lli not available - skip test
            return Task.CompletedTask;
        }

        var (exitCode, stdout, stderr) = RunCli("run", GetSamplePath("basic.stasis"), "--backend", "llvm");
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
            // lli not available - skip test
            return Task.CompletedTask;
        }

        var (exitCode, stdout, stderr) = RunCli("test", GetSamplePath("tests.stasis"), "--backend", "llvm");
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
        var (exitCode, stdout, stderr) = RunCli("run", "nonexistent.stasis", "--backend", "llvm");
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
            var (exitCode, stdout, stderr) = RunCli("run", temp, "--backend", "llvm");
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
