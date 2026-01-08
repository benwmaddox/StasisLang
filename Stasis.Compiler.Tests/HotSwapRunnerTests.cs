using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;

namespace Stasis.Compiler.Tests;

public sealed class HotSwapRunnerTests
{
    [Fact]
    public void Runner_ticks_at_60_fps()
    {
        if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            return;
        }

        if (!TryFindRepoRoot(out var repoRoot) || !TryFindRunner(repoRoot, out var runnerExe) || !TryFindClang(out var clangExe))
        {
            return;
        }

        var tempDir = CreateTempDir();
        try
        {
            var dllPath = Path.Combine(tempDir, "tickrate.dll");
            var cPath = Path.Combine(tempDir, "tickrate.c");
            File.WriteAllText(cPath, TickRateDllSource, System.Text.Encoding.ASCII);

            CompileDll(clangExe, cPath, dllPath, tempDir);

            var stderr = RunRunnerAndCaptureStderr(runnerExe, $"\"{dllPath}\" tickrate__main --fps 60", timeoutMs: 10000);

            var match = Regex.Match(stderr, @"TICKRATE ticks=(\d+) elapsed_ms=(\d+)");
            Assert.True(match.Success, $"missing TICKRATE line in stderr:\n{stderr}");

            var ticks = int.Parse(match.Groups[1].Value);
            var elapsedMs = int.Parse(match.Groups[2].Value);
            Assert.True(elapsedMs > 0, $"invalid elapsed_ms={elapsedMs}\n{stderr}");

            var ticksPerSecond = (double)ticks / (elapsedMs / 1000.0);
            Assert.InRange(ticksPerSecond, 52.0, 68.0);
        }
        finally
        {
            TryDeleteDirectory(tempDir);
        }
    }

    [Fact]
    public async Task Runner_processes_one_swap_per_swap_file_write()
    {
        if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            return;
        }

        if (!TryFindRepoRoot(out var repoRoot) || !TryFindRunner(repoRoot, out var runnerExe) || !TryFindClang(out var clangExe))
        {
            return;
        }

        var tempDir = CreateTempDir();
        try
        {
            var dllV1 = Path.Combine(tempDir, "swap_v1.dll");
            var dllV2 = Path.Combine(tempDir, "swap_v2.dll");
            var cV1 = Path.Combine(tempDir, "swap_v1.c");
            var cV2 = Path.Combine(tempDir, "swap_v2.c");
            File.WriteAllText(cV1, SwapDllSource("TICK_V1"), System.Text.Encoding.ASCII);
            File.WriteAllText(cV2, SwapDllSource("TICK_V2"), System.Text.Encoding.ASCII);

            CompileDll(clangExe, cV1, dllV1, tempDir);
            CompileDll(clangExe, cV2, dllV2, tempDir);

            var swapFile = Path.Combine(tempDir, "swap_file.txt");

            using var proc = StartRunner(runnerExe, $"\"{dllV1}\" swap__main --swap-file \"{swapFile}\" --fps 60");

            var stderrLines = new List<string>();
            var stderrDone = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
            proc.ErrorDataReceived += (_, e) =>
            {
                if (e.Data is null)
                {
                    stderrDone.TrySetResult(true);
                    return;
                }
                lock (stderrLines)
                {
                    stderrLines.Add(e.Data);
                }
            };
            proc.BeginErrorReadLine();

            Assert.True(WaitForLine(stderrLines, l => l.Contains("TICK_V1 first", StringComparison.Ordinal), timeoutMs: 5000), "did not observe v1 tick start");

            WriteSwapFileAndWaitConsumed(swapFile, dllV2, timeoutMs: 5000);
            Assert.True(WaitForHotSwapCount(stderrLines, 1, timeoutMs: 5000), "expected 1 hot-swap");
            Assert.True(WaitForLine(stderrLines, l => l.Contains("TICK_V2 first", StringComparison.Ordinal), timeoutMs: 5000), "did not observe v2 tick start");
            Assert.True(WaitForLine(stderrLines, l => l.Contains("SWAP TICK_V2 count=1", StringComparison.Ordinal), timeoutMs: 5000), "did not observe v2 swap hook");

            WriteSwapFileAndWaitConsumed(swapFile, dllV1, timeoutMs: 5000);
            Assert.True(WaitForHotSwapCount(stderrLines, 2, timeoutMs: 5000), "expected 2 hot-swaps");
            Assert.True(WaitForLine(stderrLines, l => l.Contains("SWAP TICK_V1 count=1", StringComparison.Ordinal), timeoutMs: 5000), "did not observe v1 swap hook");

            proc.Kill(entireProcessTree: true);
            proc.WaitForExit(5000);
            try
            {
                await stderrDone.Task.WaitAsync(TimeSpan.FromSeconds(2));
            }
            catch
            {
                // best-effort
            }

            var all = string.Join("\n", stderrLines);
            Assert.Equal(2, Regex.Matches(all, @"^\[.*\]: HOTSWAP \d+ ms$", RegexOptions.Multiline).Count);
            Assert.Equal(2, Regex.Matches(all, @"^HOTSWAP loading:", RegexOptions.Multiline).Count);
            Assert.Equal(2, Regex.Matches(all, @"^HOTSWAP ok:", RegexOptions.Multiline).Count);
            Assert.Equal(2, Regex.Matches(all, @"^SWAP TICK_V[12] count=1$", RegexOptions.Multiline).Count);
        }
        finally
        {
            TryDeleteDirectory(tempDir);
        }
    }

    private static Process StartRunner(string runnerExe, string args)
    {
        var psi = new ProcessStartInfo
        {
            FileName = runnerExe,
            Arguments = args,
            RedirectStandardError = true,
            RedirectStandardOutput = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };
        return Process.Start(psi)!;
    }

    private static string RunRunnerAndCaptureStderr(string runnerExe, string args, int timeoutMs)
    {
        var psi = new ProcessStartInfo
        {
            FileName = runnerExe,
            Arguments = args,
            RedirectStandardError = true,
            RedirectStandardOutput = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };

        using var proc = Process.Start(psi)!;
        if (!proc.WaitForExit(timeoutMs))
        {
            try { proc.Kill(entireProcessTree: true); } catch { }
            throw new TimeoutException($"runner timed out after {timeoutMs}ms");
        }
        return proc.StandardError.ReadToEnd();
    }

    private static void CompileDll(string clangExe, string cPath, string dllOut, string workingDir)
    {
        var args = $"-shared \"{cPath}\" -o \"{dllOut}\"";
        var psi = new ProcessStartInfo
        {
            FileName = clangExe,
            Arguments = args,
            WorkingDirectory = workingDir,
            RedirectStandardError = true,
            RedirectStandardOutput = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };
        using var proc = Process.Start(psi)!;
        proc.WaitForExit();
        var stderr = proc.StandardError.ReadToEnd();
        if (proc.ExitCode != 0 || !File.Exists(dllOut))
        {
            throw new InvalidOperationException($"clang failed (exit {proc.ExitCode}):\n{stderr}");
        }
    }

    private static void WriteSwapFileAndWaitConsumed(string swapFile, string nextDllPath, int timeoutMs)
    {
        File.WriteAllText(swapFile, nextDllPath + "\n", System.Text.Encoding.ASCII);
        var sw = Stopwatch.StartNew();
        while (sw.ElapsedMilliseconds < timeoutMs)
        {
            if (!File.Exists(swapFile))
            {
                return;
            }
            Thread.Sleep(10);
        }
        throw new TimeoutException("swap file was not consumed (deleted) by runner");
    }

    private static bool WaitForHotSwapCount(List<string> stderrLines, int expected, int timeoutMs)
    {
        return WaitForCondition(() =>
        {
            lock (stderrLines)
            {
                var all = string.Join("\n", stderrLines);
                return Regex.Matches(all, @"^\[.*\]: HOTSWAP \d+ ms$", RegexOptions.Multiline).Count >= expected;
            }
        }, timeoutMs);
    }

    private static bool WaitForLine(List<string> stderrLines, Func<string, bool> predicate, int timeoutMs)
    {
        return WaitForCondition(() =>
        {
            lock (stderrLines)
            {
                return stderrLines.Any(predicate);
            }
        }, timeoutMs);
    }

    private static bool WaitForCondition(Func<bool> condition, int timeoutMs)
    {
        var sw = Stopwatch.StartNew();
        while (sw.ElapsedMilliseconds < timeoutMs)
        {
            if (condition())
            {
                return true;
            }
            Thread.Sleep(10);
        }
        return false;
    }

    private static string CreateTempDir()
    {
        var path = Path.Combine(Path.GetTempPath(), "stasis_hotswap_test_" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(path);
        return path;
    }

    private static void TryDeleteDirectory(string path)
    {
        try
        {
            Directory.Delete(path, recursive: true);
        }
        catch
        {
            // best-effort
        }
    }

    private static bool TryFindRepoRoot(out string root)
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null)
        {
            var sln = Path.Combine(dir.FullName, "Stasis.sln");
            if (File.Exists(sln))
            {
                root = dir.FullName;
                return true;
            }
            dir = dir.Parent;
        }
        root = string.Empty;
        return false;
    }

    private static bool TryFindRunner(string repoRoot, out string runnerExe)
    {
        var candidates = new[]
        {
            Path.Combine(repoRoot, "runtime", "build", "bin", "Release", "stasis_runner.exe"),
            Path.Combine(repoRoot, "stasis_runner.exe"),
            Path.Combine(repoRoot, "build", "stasis_runner.exe")
        };

        runnerExe = candidates.FirstOrDefault(File.Exists) ?? string.Empty;
        return !string.IsNullOrEmpty(runnerExe);
    }

    private static bool TryFindClang(out string clangExe)
    {
        var search = Environment.GetEnvironmentVariable("PATH")?.Split(Path.PathSeparator) ?? Array.Empty<string>();
        foreach (var dir in search)
        {
            var candidate = Path.Combine(dir, "clang.exe");
            if (File.Exists(candidate))
            {
                clangExe = candidate;
                return true;
            }
        }

        clangExe = string.Empty;
        return false;
    }

    private const string TickRateDllSource = """
        #include <windows.h>
        #include <stdint.h>
        #include <stdio.h>

        static int g_ticks = 0;
        static LARGE_INTEGER g_t0;
        static LARGE_INTEGER g_freq;
        static int g_started = 0;

        __declspec(dllexport) int tickrate__main(void) {
            return 0;
        }

        __declspec(dllexport) int tickrate__tick(void) {
            if (!g_started) {
                QueryPerformanceFrequency(&g_freq);
                QueryPerformanceCounter(&g_t0);
                g_started = 1;
            }

            g_ticks++;
            if (g_ticks >= 120) {
                LARGE_INTEGER t1;
                QueryPerformanceCounter(&t1);
                long long elapsed_ms = (t1.QuadPart - g_t0.QuadPart) * 1000LL / g_freq.QuadPart;
                if (elapsed_ms < 1) elapsed_ms = 1;
                fprintf(stderr, "TICKRATE ticks=%d elapsed_ms=%lld\n", g_ticks, elapsed_ms);
                return 1;
            }

            return 0;
        }
        """;

    private static string SwapDllSource(string tag)
    {
        return $$"""
            #include <windows.h>
            #include <stdio.h>

            static int g_once = 0;
            static int g_swap_count = 0;

            __declspec(dllexport) int swap__main(void) {
                return 0;
            }

            __declspec(dllexport) int swap__swap(void) {
                g_swap_count++;
                fprintf(stderr, "SWAP %s count=%d\n", "{{tag}}", g_swap_count);
                return 0;
            }

            __declspec(dllexport) int swap__tick(void) {
                if (!g_once) {
                    fprintf(stderr, "%s first\n", "{{tag}}");
                    g_once = 1;
                }
                return 0;
            }
            """;
    }
}
