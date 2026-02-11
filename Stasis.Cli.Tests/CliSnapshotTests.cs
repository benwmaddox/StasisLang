using System.Diagnostics;
using System.Text;
using VerifyXunit;

namespace Stasis.Cli.Tests;

public class CliSnapshotTests
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

        var assemblyDir = Path.GetDirectoryName(typeof(CliSnapshotTests).Assembly.Location);
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

        var psi = CreateCliStartInfo(cliProj, root, config, args);

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

    private static (int exitCode, string stdout, string stderr) RunProcess(ProcessStartInfo psi)
    {
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

    private static (int exitCode, string stdout, string stderr) RunCliWithEnv(IDictionary<string, string?> environment, params string[] args)
    {
        var root = GetRepoRoot();
        var cliProj = Path.Combine(root, CliProject, $"{CliProject}.csproj");
        var config = GetBuildConfiguration();

        EnsureCliBuilt(cliProj, root, config);

        var psi = CreateCliStartInfo(cliProj, root, config, args);
        foreach (var (key, value) in environment)
        {
            psi.Environment[key] = value;
        }

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

    private static ProcessStartInfo CreateCliStartInfo(string cliProj, string root, string configuration, string[] args)
    {
        // Use `dotnet <dll>` instead of `dotnet run` so we don't execute the generated apphost .exe.
        // Some Windows environments enforce Application Control policies that can block running newly-built exe files.
        var dll = FindCliDll(root, configuration);
        var psi = new ProcessStartInfo
        {
            FileName = "dotnet",
            Arguments = $"\"{dll}\" {string.Join(" ", args)}",
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            WorkingDirectory = root
        };

        // Ensure the CLI can find LLVM tools (clang/ld) during tests even when the caller didn't run env.bat.
        // CI provisions a repo-local `.tools/llvm-*/bin` shim; local dev may have `.tools/llvm-*` or Program Files LLVM.
        var existingPath = psi.Environment.TryGetValue("PATH", out var inheritedPath) && inheritedPath is not null
            ? inheritedPath
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
            psi.Environment["PATH"] = $"{prependPath}{Path.PathSeparator}{existingPath}";
        }

        return psi;
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

            var (exitCode, stdout, stderr) = RunProcess(psi);
            if (exitCode != 0)
            {
                throw new InvalidOperationException($"Failed to build CLI ({exitCode}).\nstdout:\n{stdout}\nstderr:\n{stderr}");
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
    public Task CraneliftRunner_UsesGraphicsImportLib()
    {
        if (!OperatingSystem.IsWindows())
        {
            return Task.CompletedTask;
        }

        var root = GetRepoRoot();
        if (!TryFindCraneliftAot(root) || !TryFindRunner(root) || !TryFindClang())
        {
            return Task.CompletedTask;
        }

        var importLib = FindGraphicsImportLib(root);
        if (importLib is null)
        {
            return Task.CompletedTask;
        }

        var tempDir = Directory.CreateTempSubdirectory("stasis_graphics_import");
        var temp = Path.Combine(tempDir.FullName, "graphics_import.stasis");
        File.WriteAllText(temp, """
            test `dummy`(): bool {
                return true;
            }
            """);

        try
        {
            var env = new Dictionary<string, string?>
            {
                ["STASIS_LOG_COMMANDS"] = "1",
                ["STASIS_SUPPRESS_WARNINGS"] = "1"
            };
            var (exitCode, stdout, stderr) = RunCliWithEnv(env, "test", temp, "--backend", "cranelift", "--graphics");
            var combined = $"{stdout}\n{stderr}";

            Assert.Equal(0, exitCode);
            Assert.Contains("stasis_graphics.lib", combined, StringComparison.OrdinalIgnoreCase);
            Assert.Contains("NODEFAULTLIB:libcmt", combined, StringComparison.OrdinalIgnoreCase);
        }
        finally
        {
            tempDir.Delete(true);
        }

        return Task.CompletedTask;
    }

    [Fact]
    public Task EmitIr_Basic()
    {
        var outPath = Path.GetTempFileName();
        try
        {
            var (exitCode, stdout, stderr) = RunCli("run", GetSamplePath("basic.stasis"), "--emit-ir", "--backend", "llvm", "--out", outPath);
            var result = new
            {
                ExitCode = exitCode,
                Stdout = ScrubOutput(stdout),
                Stderr = ScrubOutput(stderr),
                Ir = ScrubOutput(File.ReadAllText(outPath))
            };
            return Verifier.Verify(result).UseDirectory("Snapshots");
        }
        finally
        {
            File.Delete(outPath);
        }
    }

    [Fact]
    public Task EmitIr_Cranelift_Minimal()
    {
        var temp = Path.GetTempFileName();
        var outPath = Path.GetTempFileName();
        File.WriteAllText(temp, """
            function main(): i32 {
                let x: i32 = 2 + 3;
                return x;
            }
            """);

        try
        {
            var (exitCode, stdout, stderr) = RunCli("run", temp, "--backend", "cranelift", "--emit-ir", "--out", outPath);
            var result = new
            {
                ExitCode = exitCode,
                Stdout = NormalizeCraneliftCallConv(ScrubOutput(stdout).Replace(temp, "<temp-file>")),
                Stderr = NormalizeCraneliftCallConv(ScrubOutput(stderr).Replace(temp, "<temp-file>")),
                Ir = NormalizeCraneliftCallConv(ScrubOutput(File.ReadAllText(outPath)).Replace(temp, "<temp-file>"))
            };
            return Verifier.Verify(result).UseDirectory("Snapshots");
        }
        finally
        {
            File.Delete(temp);
            File.Delete(outPath);
        }
    }

    [Fact]
    public Task EmitIr_Tests()
    {
        var outPath = Path.GetTempFileName();
        try
        {
            var (exitCode, stdout, stderr) = RunCli("test", GetSamplePath("tests.stasis"), "--emit-ir", "--backend", "llvm", "--out", outPath);
            var result = new
            {
                ExitCode = exitCode,
                Stdout = ScrubOutput(stdout),
                Stderr = ScrubOutput(stderr),
                Ir = ScrubOutput(File.ReadAllText(outPath))
            };
            return Verifier.Verify(result).UseDirectory("Snapshots");
        }
        finally
        {
            File.Delete(outPath);
        }
    }

    [Fact]
    public Task EmitIr_Tests_DefaultBackend_Cranelift()
    {
        var temp = Path.GetTempFileName();
        var outPath = Path.GetTempFileName();
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
            var (exitCode, stdout, stderr) = RunCli("test", temp, "--emit-ir", "--out", outPath);
            var result = new
            {
                ExitCode = exitCode,
                Stdout = NormalizeCraneliftCallConv(ScrubOutput(stdout).Replace(temp, "<temp-file>")),
                Stderr = NormalizeCraneliftCallConv(ScrubOutput(stderr).Replace(temp, "<temp-file>")),
                Ir = NormalizeCraneliftCallConv(ScrubOutput(File.ReadAllText(outPath)).Replace(temp, "<temp-file>"))
            };
            return Verifier.Verify(result).UseDirectory("Snapshots");
        }
        finally
        {
            File.Delete(temp);
            File.Delete(outPath);
        }
    }

    [Fact]
    public Task EmitIr_Cranelift_WithTests_Minimal()
    {
        var temp = Path.GetTempFileName();
        var outPath = Path.GetTempFileName();
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
            var (exitCode, stdout, stderr) = RunCli("test", temp, "--backend", "cranelift", "--emit-ir", "--out", outPath);
            var result = new
            {
                ExitCode = exitCode,
                Stdout = NormalizeCraneliftCallConv(ScrubOutput(stdout).Replace(temp, "<temp-file>")),
                Stderr = NormalizeCraneliftCallConv(ScrubOutput(stderr).Replace(temp, "<temp-file>")),
                Ir = NormalizeCraneliftCallConv(ScrubOutput(File.ReadAllText(outPath)).Replace(temp, "<temp-file>"))
            };
            return Verifier.Verify(result).UseDirectory("Snapshots");
        }
        finally
        {
            File.Delete(temp);
            File.Delete(outPath);
        }
    }

    static string NormalizeCraneliftCallConv(string text) =>
        text.Replace(" windows_fastcall", " <call_conv>", StringComparison.Ordinal)
            .Replace(" system_v", " <call_conv>", StringComparison.Ordinal);

    [Fact]
    public Task Run_Basic()
    {
        Assert.True(TryFindClang(), "Missing LLVM tooling (clang) on PATH. CI should install an LLVM toolchain.");

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
        Assert.True(TryFindClang(), "Missing LLVM tooling (clang) on PATH. CI should install an LLVM toolchain.");

        var (exitCode, stdout, stderr) = RunCli("test", GetSamplePath("tests.stasis"), "--backend", "llvm");
        var scrubbedStdout = ScrubOutput(stdout);
        var scrubbedStderr = ScrubOutput(stderr);

        Assert.True(exitCode == 0, $"stasis test failed (exit={exitCode}).\nstdout:\n{scrubbedStdout}\nstderr:\n{scrubbedStderr}");
        Assert.Contains("PASS: `adds numbers`", scrubbedStdout, StringComparison.Ordinal);
        Assert.Contains("PASS: `true is true`", scrubbedStdout, StringComparison.Ordinal);
        Assert.Contains("Tests: passed=2 failed=0", scrubbedStdout, StringComparison.Ordinal);
        Assert.True(string.IsNullOrWhiteSpace(scrubbedStderr), scrubbedStderr);
        return Task.CompletedTask;
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

    [Fact]
    public void Error_ParseError_EmitsStructuredDiagnosticEvent_WhenEnabled()
    {
        var temp = Path.GetTempFileName();
        File.WriteAllText(temp, "function broken {");

        try
        {
            var env = new Dictionary<string, string?>
            {
                ["STASIS_WATCH_EVENT_JSON"] = "1"
            };
            var (exitCode, stdout, stderr) = RunCliWithEnv(env, "run", temp, "--backend", "llvm");

            Assert.NotEqual(0, exitCode);
            Assert.Contains("WATCH_EVENT {\"type\":\"diagnostic\"", stdout, StringComparison.Ordinal);
            Assert.Contains("error", stderr, StringComparison.OrdinalIgnoreCase);
        }
        finally
        {
            File.Delete(temp);
        }
    }

    [Fact]
    public void Import_LineNumbers_UseExpandedSource()
    {
        var tempDir = Directory.CreateTempSubdirectory("stasis_import_line");
        try
        {
            var imported = Path.Combine(tempDir.FullName, "lib.stasis");
            var entry = Path.Combine(tempDir.FullName, "main.stasis");
            File.WriteAllText(imported, "function ok(): i32 { return 1; }\nfunction broken {");
            File.WriteAllText(entry, "import \"lib.stasis\";\nfunction main(): i32 { return 0; }");

            var (exitCode, _, stderr) = RunCli("run", entry, "--backend", "llvm");

            Assert.Equal(1, exitCode);
            Assert.Contains("function broken {", stderr);
            Assert.Contains(":2:17)", stderr);
        }
        finally
        {
            tempDir.Delete(true);
        }
    }

    private static bool TryFindCraneliftAot(string root)
    {
        var exeName = OperatingSystem.IsWindows() ? "stasis-cranelift-aot.exe" : "stasis-cranelift-aot";
        var release = Path.Combine(root, "tools", "cranelift-aot", "target", "release", exeName);
        if (File.Exists(release))
        {
            return true;
        }

        var debug = Path.Combine(root, "tools", "cranelift-aot", "target", "debug", exeName);
        return File.Exists(debug);
    }

    private static bool TryFindRunner(string root)
    {
        var exeName = OperatingSystem.IsWindows() ? "stasis_runner.exe" : "stasis_runner";
        var release = Path.Combine(root, "runtime", "build", "bin", "Release", exeName);
        if (File.Exists(release))
        {
            return true;
        }

        var repoRoot = Path.Combine(root, exeName);
        return File.Exists(repoRoot);
    }

    private static bool TryFindClang()
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

    [Fact]
    public void Import_GameModule_Works()
    {
        var root = GetRepoRoot();
        var tempDir = Directory.CreateTempSubdirectory("stasis_imports_cli");
        try
        {
            var gamePath = Path.Combine(root, "src", "stdlib", "game.stasis").Replace("\\", "/");
            var entryPath = Path.Combine(tempDir.FullName, "import_game.stasis");
            File.WriteAllText(entryPath, $"import \"{gamePath}\";\n\ntest `import works`() {{ return game_aabb_intersects(0.0, 0.0, 1.0, 1.0, 0.5, 0.5, 1.5, 1.5); }}");

            var result = RunCli("test", entryPath, "--backend", "cranelift", "--emit-ir");

            Assert.Equal(0, result.exitCode);
        }
        finally
        {
            tempDir.Delete(true);
        }
    }

    private static string? FindGraphicsImportLib(string root)
    {
        var candidates = new[]
        {
            Path.Combine(root, "runtime", "build", "bin", "Release", "stasis_graphics.lib"),
            Path.Combine(root, "stasis_graphics.lib")
        };

        return candidates.FirstOrDefault(File.Exists);
    }
}
