using System.Diagnostics;
using System.Text;

namespace Stasis.Compiler.Tests;

public class TemplateOutputTests
{
    private const string CliProject = "Stasis.Cli";
    private static readonly object CliBuildLock = new();
    private static string? BuiltCliConfiguration;

    private static string GetRepoRoot()
    {
        var current = FindRepoRoot(Directory.GetCurrentDirectory());
        if (!string.IsNullOrEmpty(current))
        {
            return current;
        }

        var assemblyDir = Path.GetDirectoryName(typeof(TemplateOutputTests).Assembly.Location);
        current = FindRepoRoot(assemblyDir);
        if (!string.IsNullOrEmpty(current))
        {
            return current;
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

    private static string GetBuildConfiguration()
    {
        var assemblyPath = typeof(TemplateOutputTests).Assembly.Location;
        if (assemblyPath.Contains("Release", StringComparison.OrdinalIgnoreCase))
        {
            return "Release";
        }
        return "Debug";
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
            if (e.Data != null) stdout.AppendLine(e.Data);
        };
        process.ErrorDataReceived += (_, e) =>
        {
            if (e.Data != null) stderr.AppendLine(e.Data);
        };

        process.Start();
        process.BeginOutputReadLine();
        process.BeginErrorReadLine();
        process.WaitForExit();

        return (process.ExitCode, stdout.ToString(), stderr.ToString());
    }

    private static bool TryFindLlvmTooling()
    {
        var search = Environment.GetEnvironmentVariable("PATH")?.Split(Path.PathSeparator) ?? Array.Empty<string>();
        foreach (var dir in search)
        {
            var candidate = Path.Combine(dir, OperatingSystem.IsWindows() ? "clang.exe" : "clang");
            if (File.Exists(candidate))
            {
                return true;
            }
        }

        return false;
    }

    [Fact]
    public void FactorioLite_ProducesSvgSnapshots()
    {
        if (!TryFindLlvmTooling())
        {
            return;
        }

        var root = GetRepoRoot();
        var outPath = Path.Combine(root, "build", "factorio_lite_200.svg");

        if (File.Exists(outPath)) File.Delete(outPath);

        try
        {
            var (exitCode, stdout, stderr) = RunCli("run", "examples/templates/factorio_lite.stasis", "--backend", "llvm");
            Assert.Equal(0, exitCode);
            Assert.True(File.Exists(outPath), $"Expected '{outPath}' to be created.\nstdout:\n{stdout}\nstderr:\n{stderr}");

            var svg = File.ReadAllText(outPath);
            Assert.Contains("<svg", svg, StringComparison.OrdinalIgnoreCase);
            Assert.Contains("<circle", svg, StringComparison.OrdinalIgnoreCase);
        }
        finally
        {
            if (File.Exists(outPath)) File.Delete(outPath);
        }
    }

    [Fact]
    public void BreakoutDefense_ProducesSvgSnapshots()
    {
        if (!TryFindLlvmTooling())
        {
            return;
        }

        var root = GetRepoRoot();
        var outPath = Path.Combine(root, "build", "breakout_defense_200.svg");

        if (File.Exists(outPath)) File.Delete(outPath);

        try
        {
            var (exitCode, stdout, stderr) = RunCli("run", "examples/templates/breakout_defense.stasis", "--backend", "llvm");
            Assert.Equal(0, exitCode);
            Assert.True(File.Exists(outPath), $"Expected '{outPath}' to be created.\nstdout:\n{stdout}\nstderr:\n{stderr}");

            var svg = File.ReadAllText(outPath);
            Assert.Contains("<svg", svg, StringComparison.OrdinalIgnoreCase);
            Assert.Contains("<circle", svg, StringComparison.OrdinalIgnoreCase);
        }
        finally
        {
            if (File.Exists(outPath)) File.Delete(outPath);
        }
    }

    [Fact]
    public void Match3Overlay_ProducesCsvHistogram()
    {
        if (!TryFindLlvmTooling())
        {
            return;
        }

        var root = GetRepoRoot();
        var outPath = Path.Combine(root, "build", "match3_combo_hist.csv");

        if (File.Exists(outPath)) File.Delete(outPath);

        try
        {
            var (exitCode, stdout, stderr) = RunCli("run", "examples/templates/match3_overlay.stasis", "--backend", "llvm");
            Assert.Equal(0, exitCode);
            Assert.True(File.Exists(outPath), $"Expected '{outPath}' to be created.\nstdout:\n{stdout}\nstderr:\n{stderr}");

            var csv = File.ReadAllText(outPath);
            Assert.Contains("combo,count", csv, StringComparison.OrdinalIgnoreCase);
            Assert.Contains("\n0,", csv, StringComparison.OrdinalIgnoreCase);
        }
        finally
        {
            if (File.Exists(outPath)) File.Delete(outPath);
        }
    }
}
