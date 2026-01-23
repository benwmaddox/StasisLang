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

        // Use `dotnet <dll>` instead of `dotnet run` so we don't execute the generated apphost .exe.
        // Some Windows environments enforce Application Control policies that can block running newly-built exe files.
        var cliDll = FindCliDll(root, config);
        var psi = new ProcessStartInfo
        {
            FileName = "dotnet",
            Arguments = $"\"{cliDll}\" {string.Join(" ", args)}",
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            WorkingDirectory = root
        };

        // Ensure the CLI can find LLVM tools (clang/ld) during tests even when the caller didn't run env.bat.
        var inheritedPath = psi.Environment.TryGetValue("PATH", out var existing) && existing is not null
            ? existing
            : (Environment.GetEnvironmentVariable("PATH") ?? string.Empty);

        var prepend = new List<string>();
        var toolsDir = Path.Combine(root, ".tools");
        if (Directory.Exists(toolsDir))
        {
            foreach (var llvmDir in Directory.EnumerateDirectories(toolsDir, "llvm-*", SearchOption.TopDirectoryOnly)
                         .OrderByDescending(p => p, StringComparer.OrdinalIgnoreCase))
            {
                prepend.Add(Path.Combine(llvmDir, "bin"));
            }
        }

        if (OperatingSystem.IsWindows())
        {
            var programFiles = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles);
            var programFilesX86 = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86);
            prepend.Add(Path.Combine(programFiles, "LLVM", "bin"));
            prepend.Add(Path.Combine(programFilesX86, "LLVM", "bin"));
        }

        var prependPath = string.Join(Path.PathSeparator, prepend.Distinct(StringComparer.OrdinalIgnoreCase));
        if (!string.IsNullOrWhiteSpace(prependPath))
        {
            psi.Environment["PATH"] = $"{prependPath}{Path.PathSeparator}{inheritedPath}";
        }

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

    private static string FindCliDll(string root, string configuration)
    {
        var baseDir = Path.Combine(root, CliProject, "bin", configuration);
        if (!Directory.Exists(baseDir))
        {
            throw new InvalidOperationException($"CLI build output not found: {baseDir}");
        }

        var candidates = Directory.GetFiles(baseDir, $"{CliProject}.dll", SearchOption.AllDirectories);
        var preferred = candidates.FirstOrDefault(p => p.Contains($"{Path.DirectorySeparatorChar}net9.0{Path.DirectorySeparatorChar}", StringComparison.OrdinalIgnoreCase));
        return preferred ?? candidates.FirstOrDefault()
            ?? throw new InvalidOperationException($"CLI dll not found under: {baseDir}");
    }

    private static bool TryFindLlvmTooling()
    {
        var repoRoot = GetRepoRoot();
        var search = (Environment.GetEnvironmentVariable("PATH") ?? string.Empty)
            .Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries)
            .ToList();

        var toolsDir = Path.Combine(repoRoot, ".tools");
        if (Directory.Exists(toolsDir))
        {
            foreach (var llvmDir in Directory.EnumerateDirectories(toolsDir, "llvm-*", SearchOption.TopDirectoryOnly).OrderByDescending(p => p, StringComparer.OrdinalIgnoreCase))
            {
                search.Add(Path.Combine(llvmDir, "bin"));
            }
        }

        if (OperatingSystem.IsWindows())
        {
            var programFiles = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles);
            var programFilesX86 = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86);
            search.Add(Path.Combine(programFiles, "LLVM", "bin"));
            search.Add(Path.Combine(programFilesX86, "LLVM", "bin"));
        }

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

    private static void EnsureBuildDirExists(string repoRoot)
    {
        Directory.CreateDirectory(Path.Combine(repoRoot, "build"));
    }

    private static bool TryFindSysLibrary(string repoRoot, out string path)
    {
        var candidates = OperatingSystem.IsWindows()
            ? new[] { "stasis_sys_static.lib" }
            : new[] { "libstasis_sys_static.a" };

        var searchPaths = new[]
        {
            Path.Combine(repoRoot, "runtime", "build", "Release"),
            Path.Combine(repoRoot, "runtime", "build", "Debug"),
            Path.Combine(repoRoot, "runtime", "build"),
            Path.Combine(repoRoot, "runtime"),
            Path.Combine(repoRoot, "build"),
            repoRoot
        };

        foreach (var dir in searchPaths)
        {
            foreach (var name in candidates)
            {
                var candidate = Path.Combine(dir, name);
                if (File.Exists(candidate))
                {
                    path = candidate;
                    return true;
                }
            }
        }

        path = string.Empty;
        return false;
    }

    [Fact]
    public void FactorioLite_ProducesSvgSnapshots()
    {
        Assert.True(TryFindLlvmTooling(), "Missing LLVM tooling (clang) on PATH. CI should install an LLVM toolchain.");

        var root = GetRepoRoot();
        EnsureBuildDirExists(root);
        var outPath = Path.Combine(root, "build", "factorio_lite_200.svg");

        if (File.Exists(outPath)) File.Delete(outPath);

        try
        {
            Assert.True(TryFindSysLibrary(root, out _), "Missing sys runtime library. CI should build the runtime sys library.");

            var (exitCode, stdout, stderr) = RunCli("run", "examples/templates/factorio_lite.stasis", "--backend", "llvm");
            Assert.True(exitCode == 0, $"Template run failed (exit={exitCode}).\nstdout:\n{stdout}\nstderr:\n{stderr}");
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
        Assert.True(TryFindLlvmTooling(), "Missing LLVM tooling (clang) on PATH. CI should install an LLVM toolchain.");

        var root = GetRepoRoot();
        EnsureBuildDirExists(root);
        var outPath = Path.Combine(root, "build", "breakout_defense_200.svg");

        if (File.Exists(outPath)) File.Delete(outPath);

        try
        {
            Assert.True(TryFindSysLibrary(root, out _), "Missing sys runtime library. CI should build the runtime sys library.");

            var (exitCode, stdout, stderr) = RunCli("run", "examples/templates/breakout_defense.stasis", "--backend", "llvm");
            Assert.True(exitCode == 0, $"Template run failed (exit={exitCode}).\nstdout:\n{stdout}\nstderr:\n{stderr}");
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
        Assert.True(TryFindLlvmTooling(), "Missing LLVM tooling (clang) on PATH. CI should install an LLVM toolchain.");

        var root = GetRepoRoot();
        EnsureBuildDirExists(root);
        var outPath = Path.Combine(root, "build", "match3_combo_hist.csv");

        if (File.Exists(outPath)) File.Delete(outPath);

        try
        {
            Assert.True(TryFindSysLibrary(root, out _), "Missing sys runtime library. CI should build the runtime sys library.");

            var (exitCode, stdout, stderr) = RunCli("run", "examples/templates/match3_overlay.stasis", "--backend", "llvm");
            Assert.True(exitCode == 0, $"Template run failed (exit={exitCode}).\nstdout:\n{stdout}\nstderr:\n{stderr}");
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
