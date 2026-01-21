using System.Diagnostics;
using Xunit.Sdk;

namespace Stasis.Compiler.Tests;

public sealed class HotSwapIntegrationTests
{
    [HotSwapFact]
    public async Task WatchTickHotSwap_SurvivesBadSwap_AndRecoversOnNextBuild()
    {
        var repoRoot = FindRepoRoot();
        var samplePath = Path.Combine(repoRoot, "samples", "hotstate_tick_watch.stasis");
        Assert.True(File.Exists(samplePath), $"missing sample: {samplePath}");

        var cliDll = FindCliDll(repoRoot);
        var runnerExe = FindRunnerExe(repoRoot);
        var aotExe = FindCraneliftAotExe(repoRoot);
        var clangExeDir = FindClangBinDir(repoRoot);

        Assert.NotNull(cliDll);
        Assert.NotNull(runnerExe);
        Assert.NotNull(aotExe);
        Assert.NotNull(clangExeDir);

        var moduleName = "hot";
        var swapDir = Path.Combine(repoRoot, "build", "hotstate");
        Directory.CreateDirectory(swapDir);

        var swapFile = Path.Combine(swapDir, $"hotstate_tick_watch.{moduleName}.swap");
        var runnerErrLog = Path.Combine(swapDir, $"hotstate_tick_watch.{moduleName}.runner.err.log");

        var original = await File.ReadAllTextAsync(samplePath);
        var startTime = DateTime.UtcNow;

        Process? proc = null;
        try
        {
            TryDelete(swapFile);
            TryDelete(runnerErrLog);

            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll, "run", samplePath, "--watch", "--backend", "cranelift", "--module", moduleName, "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_AOT"] = aotExe;
            psi.EnvironmentVariables["STASIS_CRANELIFT_RUNNER_EXE"] = runnerExe;
            psi.EnvironmentVariables["PATH"] = clangExeDir + Path.PathSeparator + (Environment.GetEnvironmentVariable("PATH") ?? string.Empty);

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTRELOAD phases(ms):") || errLines.AnyContains("HOTRELOAD phases(ms):"),
                timeout: TimeSpan.FromMinutes(3));

            // Inject a bad swap (missing DLL path). Older behavior could exit the runner; the watch loop should continue.
            File.WriteAllText(swapFile, @"Z:\this\does\not\exist.swap.dll", System.Text.Encoding.ASCII);

            await WaitForAnyLineAsync(
                proc,
                () =>
                {
                    if (!File.Exists(runnerErrLog))
                    {
                        return false;
                    }
                    return TryReadTextShared(runnerErrLog, out var text) &&
                        text.Contains("HOTSWAP warning:", StringComparison.Ordinal);
                },
                timeout: TimeSpan.FromSeconds(30));

            if (proc.HasExited)
            {
                throw new XunitException($"watch process exited unexpectedly after bad swap (code={proc.ExitCode}).");
            }

            // Trigger a real rebuild + swap.
            await File.AppendAllTextAsync(samplePath, "\n// test edit " + DateTime.UtcNow.Ticks + "\n", System.Text.Encoding.ASCII);

            await WaitForAnyLineAsync(
                proc,
                () =>
                {
                    if (!File.Exists(runnerErrLog))
                    {
                        return false;
                    }
                    return TryReadTextShared(runnerErrLog, out var text) &&
                        text.Contains("HOTSWAP ok:", StringComparison.Ordinal);
                },
                timeout: TimeSpan.FromSeconds(60));
        }
        finally
        {
            try
            {
                if (proc is not null && !proc.HasExited)
                {
                    proc.Kill(entireProcessTree: true);
                    proc.WaitForExit(10_000);
                }
            }
            catch
            {
                // Best-effort: don't fail test cleanup.
            }

            try
            {
                await File.WriteAllTextAsync(samplePath, original, System.Text.Encoding.ASCII);
            }
            catch
            {
                // Best-effort.
            }

            // If anything else is still running, try to clean up only fresh processes (avoid killing a developer's unrelated session).
            try
            {
                foreach (var p in Process.GetProcessesByName("stasis_runner"))
                {
                    try
                    {
                        if (p.StartTime.ToUniversalTime() >= startTime.AddSeconds(-5))
                        {
                            p.Kill(entireProcessTree: true);
                        }
                    }
                    catch
                    {
                        // ignore
                    }
                }
            }
            catch
            {
                // ignore
            }
        }
    }

    private static string FindRepoRoot()
    {
        var dir = AppContext.BaseDirectory;
        for (var i = 0; i < 15; i++)
        {
            var candidate = Directory.GetParent(dir)?.FullName;
            if (candidate is null)
            {
                break;
            }
            dir = candidate;
            if (File.Exists(Path.Combine(dir, "Stasis.sln")))
            {
                return dir;
            }
        }

        // Fallback: current directory.
        var cwd = Directory.GetCurrentDirectory();
        if (File.Exists(Path.Combine(cwd, "Stasis.sln")))
        {
            return cwd;
        }

        throw new InvalidOperationException("unable to locate repo root (Stasis.sln).");
    }

    private static string? FindCliDll(string repoRoot)
    {
        var configs = GuessBuildConfigs();
        foreach (var config in configs)
        {
            var p = Path.Combine(repoRoot, "Stasis.Cli", "bin", config, "net9.0", "Stasis.Cli.dll");
            if (File.Exists(p))
            {
                return p;
            }
        }
        return null;
    }

    private static string? FindRunnerExe(string repoRoot)
    {
        var exe = OperatingSystem.IsWindows() ? "stasis_runner.exe" : "stasis_runner";
        var p = Path.Combine(repoRoot, exe);
        if (File.Exists(p))
        {
            return p;
        }

        var p2 = Path.Combine(repoRoot, "build", exe);
        return File.Exists(p2) ? p2 : null;
    }

    private static string? FindCraneliftAotExe(string repoRoot)
    {
        var exe = OperatingSystem.IsWindows() ? "stasis-cranelift-aot.exe" : "stasis-cranelift-aot";
        foreach (var config in new[] { "release", "debug" })
        {
            var p = Path.Combine(repoRoot, "tools", "cranelift-aot", "target", config, exe);
            if (File.Exists(p))
            {
                return p;
            }
        }
        return null;
    }

    private static string? FindClangBinDir(string repoRoot)
    {
        // Prefer pinned LLVM under .tools/.
        var toolsDir = Path.Combine(repoRoot, ".tools");
        if (Directory.Exists(toolsDir))
        {
            var llvmDirs = Directory.GetDirectories(toolsDir, "llvm-*")
                .OrderByDescending(d => d, StringComparer.OrdinalIgnoreCase)
                .ToArray();

            foreach (var llvmDir in llvmDirs)
            {
                var bin = Path.Combine(llvmDir, "bin");
                var clang = Path.Combine(bin, OperatingSystem.IsWindows() ? "clang.exe" : "clang");
                if (File.Exists(clang))
                {
                    return bin;
                }
            }
        }

        // If clang is already on PATH, do nothing special.
        var path = Environment.GetEnvironmentVariable("PATH") ?? string.Empty;
        foreach (var part in path.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            var clang = Path.Combine(part, OperatingSystem.IsWindows() ? "clang.exe" : "clang");
            if (File.Exists(clang))
            {
                return part;
            }
        }

        return null;
    }

    private static IEnumerable<string> GuessBuildConfigs()
    {
        // Prefer the current test configuration if it's visible in the output path.
        var baseDir = AppContext.BaseDirectory;
        if (baseDir.Contains($"{Path.DirectorySeparatorChar}Release{Path.DirectorySeparatorChar}", StringComparison.OrdinalIgnoreCase))
        {
            return new[] { "Release", "Debug" };
        }
        if (baseDir.Contains($"{Path.DirectorySeparatorChar}Debug{Path.DirectorySeparatorChar}", StringComparison.OrdinalIgnoreCase))
        {
            return new[] { "Debug", "Release" };
        }
        return new[] { "Release", "Debug" };
    }

    private static void TryDelete(string path)
    {
        try
        {
            if (File.Exists(path))
            {
                File.Delete(path);
            }
        }
        catch
        {
            // ignore
        }
    }

    private static bool TryReadTextShared(string path, out string text)
    {
        text = string.Empty;
        try
        {
            using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
            using var sr = new StreamReader(fs, System.Text.Encoding.UTF8, detectEncodingFromByteOrderMarks: true);
            text = sr.ReadToEnd();
            return true;
        }
        catch
        {
            return false;
        }
    }

    private static string QuoteArgs(params string[] args) =>
        string.Join(" ", args.Select(QuoteArg));

    private static string QuoteArg(string arg)
    {
        if (arg.Length == 0)
        {
            return "\"\"";
        }
        if (arg.IndexOfAny([' ', '\t', '\n', '\r', '"']) >= 0)
        {
            return "\"" + arg.Replace("\"", "\"\"", StringComparison.Ordinal) + "\"";
        }
        return arg;
    }

    private static async Task WaitForAnyLineAsync(Process proc, Func<bool> condition, TimeSpan timeout)
    {
        var sw = Stopwatch.StartNew();
        while (sw.Elapsed < timeout)
        {
            if (proc.HasExited)
            {
                throw new XunitException($"process exited early (code={proc.ExitCode}).");
            }

            if (condition())
            {
                return;
            }

            await Task.Delay(50);
        }

        throw new XunitException($"timeout after {timeout.TotalSeconds:0}s.");
    }

    private sealed class AsyncLineCollector : IDisposable
    {
        private readonly CancellationTokenSource _cts = new();
        private readonly Task _task;

        private readonly List<string> _lines = new();

        public AsyncLineCollector(StreamReader reader)
        {
            _task = Task.Run(async () =>
            {
                try
                {
                    while (!_cts.IsCancellationRequested)
                    {
                        var line = await reader.ReadLineAsync(_cts.Token);
                        if (line is null)
                        {
                            return;
                        }
                        lock (_lines)
                        {
                            _lines.Add(line);
                            if (_lines.Count > 2000)
                            {
                                _lines.RemoveRange(0, 500);
                            }
                        }
                    }
                }
                catch
                {
                    // ignore
                }
            });
        }

        public bool AnyContains(string needle)
        {
            lock (_lines)
            {
                for (var i = _lines.Count - 1; i >= 0; i--)
                {
                    if (_lines[i].Contains(needle, StringComparison.Ordinal))
                    {
                        return true;
                    }
                }
            }
            return false;
        }

        public void Dispose()
        {
            try { _cts.Cancel(); } catch { }
            try { _task.Wait(250); } catch { }
            try { _cts.Dispose(); } catch { }
        }
    }
}

internal sealed class HotSwapFactAttribute : FactAttribute
{
    public HotSwapFactAttribute()
    {
        if (!(OperatingSystem.IsWindows() || OperatingSystem.IsLinux()))
        {
            Skip = "hot-swap runner integration is only supported on Windows/Linux.";
            return;
        }

        var repoRoot = TryFindRepoRootFromCwd();
        if (repoRoot is null) return;

        var sample = Path.Combine(repoRoot, "samples", "hotstate_tick_watch.stasis");
        if (!File.Exists(sample))
        {
            Skip = "missing samples/hotstate_tick_watch.stasis.";
            return;
        }

        var cli = Directory.Exists(Path.Combine(repoRoot, "Stasis.Cli")) && File.Exists(Path.Combine(repoRoot, "Stasis.Cli", "Stasis.Cli.csproj"));
        if (!cli)
        {
            Skip = "missing Stasis.Cli project.";
            return;
        }

        // In CI we want this to fail loudly if prerequisites are missing.
        // Locally, skip when native prerequisites aren't present.
        var isCi = !string.IsNullOrEmpty(Environment.GetEnvironmentVariable("CI"));
        if (!isCi)
        {
            var runner = Path.Combine(repoRoot, OperatingSystem.IsWindows() ? "stasis_runner.exe" : "stasis_runner");
            if (!File.Exists(runner))
            {
                Skip = "missing stasis_runner (build runtime/).";
                return;
            }

            var aotRelease = Path.Combine(repoRoot, "tools", "cranelift-aot", "target", "release", OperatingSystem.IsWindows() ? "stasis-cranelift-aot.exe" : "stasis-cranelift-aot");
            var aotDebug = Path.Combine(repoRoot, "tools", "cranelift-aot", "target", "debug", OperatingSystem.IsWindows() ? "stasis-cranelift-aot.exe" : "stasis-cranelift-aot");
            if (!File.Exists(aotRelease) && !File.Exists(aotDebug))
            {
                Skip = "missing stasis-cranelift-aot (build tools/cranelift-aot).";
                return;
            }

            if (!HasClang(repoRoot))
            {
                Skip = "missing clang (install LLVM or ensure .tools/llvm-*/bin exists).";
                return;
            }
        }
    }

    private static string? TryFindRepoRootFromCwd()
    {
        var dir = Directory.GetCurrentDirectory();
        for (var i = 0; i < 10; i++)
        {
            if (File.Exists(Path.Combine(dir, "Stasis.sln")))
            {
                return dir;
            }
            var parent = Directory.GetParent(dir)?.FullName;
            if (parent is null)
            {
                break;
            }
            dir = parent;
        }
        return null;
    }

    private static bool HasClang(string repoRoot)
    {
        var exe = OperatingSystem.IsWindows() ? "clang.exe" : "clang";
        var toolsDir = Path.Combine(repoRoot, ".tools");
        if (Directory.Exists(toolsDir))
        {
            foreach (var llvmDir in Directory.GetDirectories(toolsDir, "llvm-*"))
            {
                if (File.Exists(Path.Combine(llvmDir, "bin", exe)))
                {
                    return true;
                }
            }
        }

        var path = Environment.GetEnvironmentVariable("PATH") ?? string.Empty;
        foreach (var part in path.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            if (File.Exists(Path.Combine(part, exe)))
            {
                return true;
            }
        }

        return false;
    }
}
