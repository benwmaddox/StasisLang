using System.Diagnostics;
using System.IO.Pipes;
using System.Text;
using System.Text.Json;
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
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);

        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var moduleName = "hot";
        var swapDir = Path.Combine(repoRoot, "build", "hotstate");
        Directory.CreateDirectory(swapDir);

        var runnerOutLog = Path.Combine(swapDir, $"hotstate_tick_watch.{moduleName}.runner.out.log");
        var runnerErrLog = Path.Combine(swapDir, $"hotstate_tick_watch.{moduleName}.runner.err.log");

        var original = await File.ReadAllTextAsync(samplePath);
        var startTime = DateTime.UtcNow;

        Process? proc = null;
        try
        {
            TryDelete(runnerOutLog);
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
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe;

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            try
            {
                await WaitForAnyLineAsync(
                    proc,
                    () =>
                        outLines.AnyContains("HOTSWAP(ms):") ||
                        errLines.AnyContains("warning: initial build failed") ||
                        errLines.AnyContains("error:"),
                    timeout: TimeSpan.FromMinutes(3));
            }
            catch (XunitException ex)
            {
                throw new XunitException(
                    $"{ex.Message}\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");
            }

            if (!outLines.AnyContains("HOTSWAP(ms):"))
            {
                throw new XunitException(
                    $"watch failed to produce initial HOTSWAP(ms) marker.\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");
            }

            // Kill the JIT runner process; the watch loop should notice and restart it.
            await WaitForAnyLineAsync(
                proc,
                () => Process.GetProcessesByName("stasis-cranelift-jit-runner")
                    .Any(p => p.StartTime.ToUniversalTime() >= startTime.AddSeconds(-5)),
                timeout: TimeSpan.FromSeconds(30));

            foreach (var p in Process.GetProcessesByName("stasis-cranelift-jit-runner"))
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

            await WaitForAnyLineAsync(
                proc,
                () => errLines.AnyContains("warning: jit runner exited"),
                timeout: TimeSpan.FromSeconds(60));

            if (proc.HasExited)
            {
                throw new XunitException($"watch process exited unexpectedly after killing jit runner (code={proc.ExitCode}).");
            }

            // Trigger a real semantic rebuild + swap.
            var semanticEdit = ApplyTickSemanticEdit(original, 7);
            await File.WriteAllTextAsync(samplePath, semanticEdit, System.Text.Encoding.ASCII);

            var initialSwapCount = outLines.CountContains("HOTSWAP(ms):");
            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > initialSwapCount,
                timeout: TimeSpan.FromMinutes(5));

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(state): compiled") &&
                      outLines.AnyContains("HOTSWAP(state): queued") &&
                      outLines.AnyContains("HOTSWAP(state): applied"),
                timeout: TimeSpan.FromMinutes(2));
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
                foreach (var p in Process.GetProcessesByName("stasis-cranelift-jit-runner"))
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

    [HotSwapFact]
    public async Task WatchTickJitSwap_SwapsOnEdit()
    {
        var repoRoot = FindRepoRoot();
        var samplePath = Path.Combine(repoRoot, "samples", "hotstate_tick_watch.stasis");
        Assert.True(File.Exists(samplePath), $"missing sample: {samplePath}");

        var cliDll = FindCliDll(repoRoot);
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);

        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var moduleName = "hot";
        var swapDir = Path.Combine(repoRoot, "build", "hotstate");
        Directory.CreateDirectory(swapDir);

        var runnerErrLog = Path.Combine(swapDir, $"hotstate_tick_watch.{moduleName}.runner.err.log");
        var original = await File.ReadAllTextAsync(samplePath);
        var startTime = DateTime.UtcNow;

        Process? proc = null;
        try
        {
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
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe;

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));
            var initialSwapCount = outLines.CountContains("HOTSWAP(ms):");

            var semanticEdit = ApplyTickSemanticEdit(original, 11);
            await File.WriteAllTextAsync(samplePath, semanticEdit, System.Text.Encoding.ASCII);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > initialSwapCount,
                timeout: TimeSpan.FromMinutes(5));

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(state): compiled") &&
                      outLines.AnyContains("HOTSWAP(state): queued") &&
                      outLines.AnyContains("HOTSWAP(state): applied"),
                timeout: TimeSpan.FromMinutes(2));
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

            try
            {
                foreach (var p in Process.GetProcessesByName("stasis-cranelift-jit-runner"))
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

    [HotSwapFact]
    public async Task WatchTickJitSwap_SwapsFromPipeOverlayWithoutDiskEdit()
    {
        var repoRoot = FindRepoRoot();
        var cliDll = FindCliDll(repoRoot);
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);

        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var pipeName = $"stasis-jit-overlay-{Guid.NewGuid():N}";
        var tempDir = Directory.CreateTempSubdirectory("stasis_jit_overlay_swap");
        var stasisPath = Path.Combine(tempDir.FullName, "jit_overlay_watch.stasis");
        var onDiskSource = BuildInProcessTickSource(5);
        var overlaySource = BuildInProcessTickSource(11);
        await File.WriteAllTextAsync(stasisPath, onDiskSource, Encoding.ASCII);

        Process? proc = null;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", stasisPath, "--watch", "--backend", "cranelift", "--module", "hot", "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe;
            psi.EnvironmentVariables["STASIS_BUFFER_OVERLAY_PIPE"] = pipeName;

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));

            var initialSwapCount = outLines.CountContains("HOTSWAP(ms):");
            Assert.True(initialSwapCount > 0, $"watch did not report initial HOTSWAP(ms).\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");

            await SendOverlayPipeCommandAsync(
                pipeName,
                new
                {
                    kind = "set",
                    path = stasisPath,
                    text = overlaySource
                },
                timeout: TimeSpan.FromSeconds(30));

            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > initialSwapCount,
                timeout: TimeSpan.FromMinutes(5));

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(state): compiled") &&
                      outLines.AnyContains("HOTSWAP(state): queued") &&
                      outLines.AnyContains("HOTSWAP(state): applied"),
                timeout: TimeSpan.FromMinutes(2));

            var diskSource = await File.ReadAllTextAsync(stasisPath);
            Assert.Equal(onDiskSource, diskSource);
            Assert.False(proc.HasExited);
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
                // Best-effort cleanup.
            }

            try
            {
                tempDir.Delete(true);
            }
            catch
            {
                // ignore
            }
        }
    }

    [HotSwapFact]
    public async Task WatchTickJitSwap_PipeOverlay_SetClearAndClearAll()
    {
        var repoRoot = FindRepoRoot();
        var cliDll = FindCliDll(repoRoot);
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);

        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var pipeName = $"stasis-jit-overlay-cycle-{Guid.NewGuid():N}";
        var tempDir = Directory.CreateTempSubdirectory("stasis_jit_overlay_cycle");
        var stasisPath = Path.Combine(tempDir.FullName, "jit_overlay_cycle.stasis");
        var onDiskSource = BuildInProcessTickSource(5);
        var overlayA = BuildInProcessTickSource(11);
        var overlayB = BuildInProcessTickSource(13);
        await File.WriteAllTextAsync(stasisPath, onDiskSource, Encoding.ASCII);

        Process? proc = null;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", stasisPath, "--watch", "--backend", "cranelift", "--module", "hot", "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe;
            psi.EnvironmentVariables["STASIS_BUFFER_OVERLAY_PIPE"] = pipeName;

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));

            var swapCount = outLines.CountContains("HOTSWAP(ms):");
            Assert.True(swapCount > 0, $"watch did not report initial HOTSWAP(ms).\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");

            await SendOverlayPipeCommandAsync(
                pipeName,
                new { kind = "set", path = stasisPath, text = overlayA },
                timeout: TimeSpan.FromSeconds(30));
            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > swapCount,
                timeout: TimeSpan.FromMinutes(5));
            swapCount = outLines.CountContains("HOTSWAP(ms):");

            await SendOverlayPipeCommandAsync(
                pipeName,
                new { kind = "clear", path = stasisPath },
                timeout: TimeSpan.FromSeconds(30));
            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > swapCount,
                timeout: TimeSpan.FromMinutes(5));
            swapCount = outLines.CountContains("HOTSWAP(ms):");

            await SendOverlayPipeCommandAsync(
                pipeName,
                new { kind = "set", path = stasisPath, text = overlayB },
                timeout: TimeSpan.FromSeconds(30));
            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > swapCount,
                timeout: TimeSpan.FromMinutes(5));
            swapCount = outLines.CountContains("HOTSWAP(ms):");

            await SendOverlayPipeCommandAsync(
                pipeName,
                new { kind = "clear_all" },
                timeout: TimeSpan.FromSeconds(30));
            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > swapCount,
                timeout: TimeSpan.FromMinutes(5));

            var diskSource = await File.ReadAllTextAsync(stasisPath);
            Assert.Equal(onDiskSource, diskSource);
            Assert.False(proc.HasExited);
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
                // Best-effort cleanup.
            }

            try
            {
                tempDir.Delete(true);
            }
            catch
            {
                // ignore
            }
        }
    }

    [HotSwapFact]
    public async Task WatchTickJitSwap_PipeOverlay_MultiSwapStability()
    {
        var repoRoot = FindRepoRoot();
        var cliDll = FindCliDll(repoRoot);
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);

        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var pipeName = $"stasis-jit-overlay-soak-{Guid.NewGuid():N}";
        var tempDir = Directory.CreateTempSubdirectory("stasis_jit_overlay_soak");
        var stasisPath = Path.Combine(tempDir.FullName, "jit_overlay_soak.stasis");
        var onDiskSource = BuildInProcessTickSource(5);
        await File.WriteAllTextAsync(stasisPath, onDiskSource, Encoding.ASCII);

        Process? proc = null;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", stasisPath, "--watch", "--backend", "cranelift", "--module", "hot", "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe;
            psi.EnvironmentVariables["STASIS_BUFFER_OVERLAY_PIPE"] = pipeName;

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));

            var swapCount = outLines.CountContains("HOTSWAP(ms):");
            Assert.True(swapCount > 0, $"watch did not report initial HOTSWAP(ms).\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");

            foreach (var seed in new[] { 11, 13, 17, 19, 23, 29, 31, 37 })
            {
                await SendOverlayPipeCommandAsync(
                    pipeName,
                    new { kind = "set", path = stasisPath, text = BuildInProcessTickSource(seed) },
                    timeout: TimeSpan.FromSeconds(30));

                await WaitForAnyLineAsync(
                    proc,
                    () => outLines.CountContains("HOTSWAP(ms):") > swapCount,
                    timeout: TimeSpan.FromMinutes(5));

                swapCount = outLines.CountContains("HOTSWAP(ms):");
                Assert.False(proc.HasExited, $"watch process exited during soak cycle seed={seed}.");
            }

            var diskSource = await File.ReadAllTextAsync(stasisPath);
            Assert.Equal(onDiskSource, diskSource);
            Assert.False(proc.HasExited);
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
                // Best-effort cleanup.
            }

            try
            {
                tempDir.Delete(true);
            }
            catch
            {
                // ignore
            }
        }
    }

    [HotSwapFact]
    public async Task WatchTickJitSwap_PipeOverlay_LongSoak_100PlusSwaps()
    {
        if (!IsEnvFlagEnabled("STASIS_RUN_LONG_HOTSWAP"))
        {
            Console.WriteLine("LONG_SOAK disabled (set STASIS_RUN_LONG_HOTSWAP=1 to enable).");
            return;
        }

        var repoRoot = FindRepoRoot();
        var cliDll = FindCliDll(repoRoot);
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);

        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var swapCycles = Math.Max(100, GetEnvInt("STASIS_LONG_HOTSWAP_CYCLES", 120));
        var pipeName = $"stasis-jit-overlay-long-soak-{Guid.NewGuid():N}";
        var tempDir = Directory.CreateTempSubdirectory("stasis_jit_overlay_long_soak");
        var stasisPath = Path.Combine(tempDir.FullName, "jit_overlay_long_soak.stasis");
        var onDiskSource = BuildInProcessTickSource(7);
        await File.WriteAllTextAsync(stasisPath, onDiskSource, Encoding.ASCII);

        Process? proc = null;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", stasisPath, "--watch", "--backend", "cranelift", "--module", "hot", "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe;
            psi.EnvironmentVariables["STASIS_BUFFER_OVERLAY_PIPE"] = pipeName;
            psi.EnvironmentVariables["STASIS_JIT_WATCHDOG_MS"] = "30000";

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));

            var latencyCount = CountHotSwapLatencyMetrics(outLines);
            var collectedLatencyMs = new List<double>(swapCycles);

            for (var i = 0; i < swapCycles; i++)
            {
                var seed = 1000 + i;
                await SendOverlayPipeCommandAsync(
                    pipeName,
                    new { kind = "set", path = stasisPath, text = BuildInProcessTickSource(seed) },
                    timeout: TimeSpan.FromSeconds(30));

                await WaitForAnyLineAsync(
                    proc,
                    () => CountHotSwapLatencyMetrics(outLines) > latencyCount || errLines.AnyContains("error:"),
                    timeout: TimeSpan.FromMinutes(2));

                Assert.False(proc.HasExited, $"watch process exited during long soak cycle={i}.");
                Assert.True(TryGetLatestHotSwapLatencyMetric(outLines, out _, out var latencyMs, out _), $"missing latency metric at cycle={i}.");
                collectedLatencyMs.Add(latencyMs);
                latencyCount = CountHotSwapLatencyMetrics(outLines);
            }

            Assert.True(collectedLatencyMs.Count >= swapCycles, $"expected at least {swapCycles} latency samples, got {collectedLatencyMs.Count}.");
            var avgLatency = collectedLatencyMs.Average();
            var p95Latency = Percentile(collectedLatencyMs, 0.95);
            Console.WriteLine($"LONG_SOAK swaps={collectedLatencyMs.Count} avg_latency_ms={avgLatency:0.###} p95_latency_ms={p95Latency:0.###}");
            Assert.False(proc.HasExited);
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
                // Best-effort cleanup.
            }

            try
            {
                tempDir.Delete(true);
            }
            catch
            {
                // ignore
            }
        }
    }

    [HotSwapFact]
    public async Task WatchTickJitSwap_PipeOverlay_LatencyHarness_SingleVsMultiFunction()
    {
        if (!IsEnvFlagEnabled("STASIS_RUN_HOTSWAP_PERF"))
        {
            Console.WriteLine("PERF_HARNESS disabled (set STASIS_RUN_HOTSWAP_PERF=1 to enable).");
            return;
        }

        var repoRoot = FindRepoRoot();
        var cliDll = FindCliDll(repoRoot);
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);

        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var iterations = Math.Max(4, GetEnvInt("STASIS_HOTSWAP_PERF_ITERATIONS", 8));
        var pipeName = $"stasis-jit-overlay-perf-{Guid.NewGuid():N}";
        var tempDir = Directory.CreateTempSubdirectory("stasis_jit_overlay_perf");
        var stasisPath = Path.Combine(tempDir.FullName, "jit_overlay_perf.stasis");
        await File.WriteAllTextAsync(stasisPath, BuildPerfSingleEditSource(1), Encoding.ASCII);

        Process? proc = null;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", stasisPath, "--watch", "--backend", "cranelift", "--module", "hot", "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe;
            psi.EnvironmentVariables["STASIS_BUFFER_OVERLAY_PIPE"] = pipeName;
            psi.EnvironmentVariables["STASIS_JIT_WATCHDOG_MS"] = "30000";

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));

            async Task<List<double>> RunPhaseAsync(Func<int, string> builder, int seedBase)
            {
                var latencies = new List<double>(iterations);
                var latencyCount = CountHotSwapLatencyMetrics(outLines);
                for (var i = 0; i < iterations; i++)
                {
                    await SendOverlayPipeCommandAsync(
                        pipeName,
                        new { kind = "set", path = stasisPath, text = builder(seedBase + i) },
                        timeout: TimeSpan.FromSeconds(30));

                    await WaitForAnyLineAsync(
                        proc!,
                        () => CountHotSwapLatencyMetrics(outLines) > latencyCount || errLines.AnyContains("error:"),
                        timeout: TimeSpan.FromMinutes(2));

                    Assert.False(proc!.HasExited, $"watch process exited during latency phase edit={i}.");
                    Assert.True(TryGetLatestHotSwapLatencyMetric(outLines, out _, out var latencyMs, out _), $"missing latency metric for edit={i}.");
                    latencies.Add(latencyMs);
                    latencyCount = CountHotSwapLatencyMetrics(outLines);
                }

                return latencies;
            }

            var singleLatencies = await RunPhaseAsync(BuildPerfSingleEditSource, 10);
            var multiLatencies = await RunPhaseAsync(BuildPerfMultiEditSource, 200);

            var singleAvg = singleLatencies.Average();
            var singleP95 = Percentile(singleLatencies, 0.95);
            var multiAvg = multiLatencies.Average();
            var multiP95 = Percentile(multiLatencies, 0.95);

            Console.WriteLine(
                $"PERF_HARNESS iterations={iterations} single_avg_ms={singleAvg:0.###} single_p95_ms={singleP95:0.###} multi_avg_ms={multiAvg:0.###} multi_p95_ms={multiP95:0.###}");

            var singleBudgetMs = GetEnvDouble("STASIS_PERF_MAX_SINGLE_LATENCY_MS", -1);
            if (singleBudgetMs > 0)
            {
                Assert.True(singleAvg <= singleBudgetMs, $"single-function avg latency {singleAvg:0.###}ms exceeded budget {singleBudgetMs:0.###}ms");
            }

            var multiBudgetMs = GetEnvDouble("STASIS_PERF_MAX_MULTI_LATENCY_MS", -1);
            if (multiBudgetMs > 0)
            {
                Assert.True(multiAvg <= multiBudgetMs, $"multi-function avg latency {multiAvg:0.###}ms exceeded budget {multiBudgetMs:0.###}ms");
            }

            Assert.False(proc.HasExited);
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
                // Best-effort cleanup.
            }

            try
            {
                tempDir.Delete(true);
            }
            catch
            {
                // ignore
            }
        }
    }

    [HotSwapFact]
    public async Task WatchTickJitSwap_SurvivesBuildError_AndSwapsAfterFix()
    {
        var repoRoot = FindRepoRoot();
        var samplePath = Path.Combine(repoRoot, "samples", "hotstate_tick_watch.stasis");
        Assert.True(File.Exists(samplePath), $"missing sample: {samplePath}");

        var cliDll = FindCliDll(repoRoot);
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);

        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var moduleName = "hot";
        var swapDir = Path.Combine(repoRoot, "build", "hotstate");
        Directory.CreateDirectory(swapDir);

        var runnerErrLog = Path.Combine(swapDir, $"hotstate_tick_watch.{moduleName}.runner.err.log");
        var original = await File.ReadAllTextAsync(samplePath);
        var startTime = DateTime.UtcNow;

        Process? proc = null;
        try
        {
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
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe;

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));
            var initialSwapCount = outLines.CountContains("HOTSWAP(ms):");

            // Introduce a compiler error. The watch loop should stay alive and keep watching.
            await File.AppendAllTextAsync(samplePath, "\nfunction __jit_build_error(): i32 { return x; }\n", System.Text.Encoding.ASCII);

            await WaitForAnyLineAsync(
                proc,
                () => errLines.AnyContains("error:") || errLines.AnyContains("Error:"),
                timeout: TimeSpan.FromSeconds(30));

            if (proc.HasExited)
            {
                throw new XunitException($"watch process exited unexpectedly after build error (code={proc.ExitCode}).");
            }

            // Fix the file; next rebuild should hot-swap again.
            var semanticRecovery = ApplyTickSemanticEdit(original, 13);
            await File.WriteAllTextAsync(samplePath, semanticRecovery, System.Text.Encoding.ASCII);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > initialSwapCount,
                timeout: TimeSpan.FromMinutes(5));
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

            try
            {
                foreach (var p in Process.GetProcessesByName("stasis-cranelift-jit-runner"))
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

    [HotSwapFact]
    public async Task WatchTickJitSwap_DataBind_DoesNotRaceStructMeta()
    {
        var repoRoot = FindRepoRoot();
        var cliDll = FindCliDll(repoRoot);
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);
        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var tempDir = Directory.CreateTempSubdirectory("stasis_jit_databind");
        var stasisPath = Path.Combine(tempDir.FullName, "jit_databind_watch.stasis");
        var dataDir = Path.Combine(tempDir.FullName, "data");
        Directory.CreateDirectory(dataDir);
        var jsonPath = Path.Combine(dataDir, "config.json");

        // Minimal program: define a config field and a tick loop.
        // Data binding should apply config.foo from config.json to state__config__foo.
        File.WriteAllText(stasisPath, """
            struct Config {
                foo: i32;
            }

            struct GameState {
                config: Config;
            }

            global state: GameState;

            function main(): i32 {
                return 0;
            }

            function tick(): i32 {
                return 0;
            }
            """, System.Text.Encoding.ASCII);

        File.WriteAllText(jsonPath, """
            {
              "config": {
                "foo": 123
              }
            }
            """, System.Text.Encoding.ASCII);

        var moduleName = "brick";
        var swapDir = Path.Combine(repoRoot, "build", "hotstate");
        Directory.CreateDirectory(swapDir);
        var baseName = Path.GetFileNameWithoutExtension(stasisPath);
        var runnerOutLog = Path.Combine(swapDir, $"{baseName}.{moduleName}.runner.out.log");
        var runnerErrLog = Path.Combine(swapDir, $"{baseName}.{moduleName}.runner.err.log");
        var startTime = DateTime.UtcNow;

        Process? proc = null;
        try
        {
            TryDelete(runnerOutLog);
            TryDelete(runnerErrLog);

            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", stasisPath, "--watch", "--backend", "cranelift", "--module", moduleName, "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe!;
            psi.EnvironmentVariables["STASIS_JIT_HEARTBEAT_MS"] = "0";
            psi.EnvironmentVariables["STASIS_JIT_WATCHDOG_MS"] = "15000";

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));

            // The CLI should not report a bind failure, and the runner logs should not contain ERR databind...
            // We don't want the CLI to report a bind failure (this was caused by a non-atomic struct-meta write).
            Assert.False(errLines.AnyContains("jit runner data bind failed"), "CLI reported jit runner data bind failed.");

            // Give the runner a moment to process the BIND message and flush logs.
            await Task.Delay(250);

            if (File.Exists(runnerOutLog) && TryReadTextShared(runnerOutLog, out var outLog))
            {
                Assert.DoesNotContain("ERR databind", outLog, StringComparison.OrdinalIgnoreCase);
            }
            if (File.Exists(runnerErrLog) && TryReadTextShared(runnerErrLog, out var errLog))
            {
                Assert.DoesNotContain("DATABIND error:", errLog, StringComparison.OrdinalIgnoreCase);
            }
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
                // Best-effort cleanup.
            }

            try
            {
                tempDir.Delete(true);
            }
            catch
            {
                // ignore
            }

            // Best-effort: cleanup fresh jit runner processes.
            try
            {
                foreach (var p in Process.GetProcessesByName("stasis-cranelift-jit-runner"))
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

    [HotSwapFact]
    public async Task WatchTickJitSwap_LoadsSvgSprite()
    {
        RequireGraphicsHotSwapSupport();

        var repoRoot = FindRepoRoot();
        var samplePath = Path.Combine(repoRoot, "samples", "gfx_cmd_smoke.stasis");
        Assert.True(File.Exists(samplePath), $"missing sample: {samplePath}");

        var cliDll = FindCliDll(repoRoot);
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);

        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var moduleName = "gfx";
        var swapDir = Path.Combine(repoRoot, "build", "hotstate");
        Directory.CreateDirectory(swapDir);

        var runnerOutLog = Path.Combine(swapDir, $"gfx_cmd_smoke.{moduleName}.runner.out.log");
        var runnerErrLog = Path.Combine(swapDir, $"gfx_cmd_smoke.{moduleName}.runner.err.log");
        var startTime = DateTime.UtcNow;

        Process? proc = null;
        try
        {
            TryDelete(runnerOutLog);
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
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe;

            // Make the runner emit a positive "gfx_load_sprite: ... handle=.." line on success.
            psi.EnvironmentVariables["STASIS_GFX_LOG_SPRITES"] = "1";

            // CI / headless stability knobs (these are no-ops on platforms where they don't apply).
            psi.EnvironmentVariables["STASIS_SKIP_RENDER_TEST"] = "1";
            psi.EnvironmentVariables["STASIS_USE_SDL"] = "1";
            if (OperatingSystem.IsLinux())
            {
                psi.EnvironmentVariables["SDL_VIDEODRIVER"] = "dummy";
            }

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));

            // Wait for the sprite load log (stasis_graphics prints via SDL_Log -> stderr).
            await WaitForAnyLineAsync(
                proc,
                () =>
                {
                    if (!File.Exists(runnerErrLog))
                    {
                        return false;
                    }
                    if (!TryReadTextShared(runnerErrLog, out var errText))
                    {
                        return false;
                    }

                    // Success path (requires STASIS_GFX_LOG_SPRITES=1).
                    if (errText.Contains("gfx_load_sprite:", StringComparison.OrdinalIgnoreCase) &&
                        errText.Contains("handle=", StringComparison.OrdinalIgnoreCase))
                    {
                        return true;
                    }

                    // Failure signals: stop waiting so we can assert with the log content.
                    if (errText.Contains("gfx_load_sprite: failed", StringComparison.OrdinalIgnoreCase) ||
                        errText.Contains("gfx_load_sprite: could not resolve", StringComparison.OrdinalIgnoreCase) ||
                        errText.Contains("failed to parse", StringComparison.OrdinalIgnoreCase))
                    {
                        return true;
                    }

                    return false;
                },
                timeout: TimeSpan.FromSeconds(60));

            Assert.True(File.Exists(runnerErrLog), $"missing runner err log: {runnerErrLog}");
            Assert.True(TryReadTextShared(runnerErrLog, out var finalErr), "failed to read runner err log");
            Assert.DoesNotContain("gfx_load_sprite: failed", finalErr, StringComparison.OrdinalIgnoreCase);
            Assert.DoesNotContain("gfx_load_sprite: could not resolve", finalErr, StringComparison.OrdinalIgnoreCase);
            Assert.DoesNotContain("failed to parse", finalErr, StringComparison.OrdinalIgnoreCase);
            Assert.Contains("gfx_load_sprite:", finalErr, StringComparison.OrdinalIgnoreCase);
            Assert.Contains("handle=", finalErr, StringComparison.OrdinalIgnoreCase);
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
                // Best-effort cleanup.
            }

            // Best-effort: cleanup fresh jit runner processes.
            try
            {
                foreach (var p in Process.GetProcessesByName("stasis-cranelift-jit-runner"))
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

    [HotSwapFact]
    public async Task WatchTickJitSwap_BrickoutV1_StartsPortraitWindow()
    {
        RequireGraphicsHotSwapSupport();

        var repoRoot = FindRepoRoot();
        var samplePath = Path.Combine(repoRoot, "samples", "brickout_revenge", "brickout_revenge_v1.stasis");
        Assert.True(File.Exists(samplePath), $"missing sample: {samplePath}");

        var cliDll = FindCliDll(repoRoot);
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);
        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var moduleName = "brickportrait";
        var swapDir = Path.Combine(repoRoot, "build", "hotstate");
        Directory.CreateDirectory(swapDir);

        var runnerErrLog = Path.Combine(swapDir, $"brickout_revenge_v1.{moduleName}.runner.err.log");
        var startTime = DateTime.UtcNow;

        Process? proc = null;
        try
        {
            TryDelete(runnerErrLog);

            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", samplePath, "--watch", "--backend", "cranelift", "--graphics", "--module", moduleName, "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe!;
            psi.EnvironmentVariables["STASIS_SKIP_RENDER_TEST"] = "1";
            psi.EnvironmentVariables["STASIS_USE_SDL"] = "1";
            psi.EnvironmentVariables["STASIS_DISABLE_AUDIO"] = "1";
            psi.EnvironmentVariables["STASIS_WINDOW_START_MINIMIZED"] = "1";
            psi.EnvironmentVariables["STASIS_JIT_LOG_WINDOW_SIZE"] = "1";
            if (OperatingSystem.IsLinux())
            {
                psi.EnvironmentVariables["SDL_VIDEODRIVER"] = "dummy";
            }

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));

            Assert.True(outLines.AnyContains("HOTSWAP(ms):"), $"watch did not report initial HOTSWAP(ms).\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");

            await WaitForAnyLineAsync(
                proc,
                () =>
                {
                    if (!File.Exists(runnerErrLog))
                    {
                        return false;
                    }
                    if (!TryReadTextShared(runnerErrLog, out var errText))
                    {
                        return false;
                    }
                    return errText.Contains("WINDOW init size=", StringComparison.Ordinal);
                },
                timeout: TimeSpan.FromSeconds(60));

            Assert.True(File.Exists(runnerErrLog), $"missing runner err log: {runnerErrLog}");
            Assert.True(TryReadTextShared(runnerErrLog, out var finalErr), "failed to read runner err log");

            var initLine = finalErr
                .Split(new[] { '\r', '\n' }, StringSplitOptions.RemoveEmptyEntries)
                .LastOrDefault(line => line.Contains("WINDOW init size=", StringComparison.Ordinal));

            Assert.NotNull(initLine);
            Assert.Contains("orientation=portrait", initLine!, StringComparison.Ordinal);

            var sizeToken = initLine!
                .Split(' ', StringSplitOptions.RemoveEmptyEntries)
                .FirstOrDefault(token => token.StartsWith("size=", StringComparison.Ordinal));
            Assert.NotNull(sizeToken);
            var sizeValue = sizeToken!["size=".Length..];
            var dims = sizeValue.Split('x');
            Assert.Equal(2, dims.Length);
            Assert.True(int.TryParse(dims[0], out var width), $"failed to parse width from: {sizeToken}");
            Assert.True(int.TryParse(dims[1], out var height), $"failed to parse height from: {sizeToken}");
            Assert.True(height > width, $"expected portrait window but got {width}x{height}");
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
                // Best-effort cleanup.
            }

            try
            {
                foreach (var p in Process.GetProcessesByName("stasis-cranelift-jit-runner"))
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

    [HotSwapFact]
    public async Task WatchTickJitSwap_UsesSwapHook_AndRejectsLayoutChanges()
    {
        var repoRoot = FindRepoRoot();
        var cliDll = FindCliDll(repoRoot);
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);
        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var tempDir = Directory.CreateTempSubdirectory("stasis_jit_swap_hook");
        var stasisPath = Path.Combine(tempDir.FullName, "jit_swap_hook_watch.stasis");

        var initialSource = """
            struct GameState {
                value: i32;
                swaps: i32;
            }

            global state: GameState;

            function main(): i32 {
                state.value = 1;
                state.swaps = 0;
                return 0;
            }

            function on_code_swap(): i32 {
                state.swaps = state.swaps + 1;
                print_string("SWAP_HOOK_OK\n");
                return 0;
            }

            function tick(): i32 {
                return 0;
            }
            """;

        var layoutChangedSource = """
            struct GameState {
                value: i32;
                swaps: i32;
                extra: i32;
            }

            global state: GameState;

            function main(): i32 {
                state.value = 1;
                state.swaps = 0;
                state.extra = 0;
                return 0;
            }

            function on_code_swap(): i32 {
                state.swaps = state.swaps + 1;
                print_string("SWAP_HOOK_OK\n");
                return 0;
            }

            function tick(): i32 {
                return 0;
            }
            """;

        File.WriteAllText(stasisPath, initialSource, System.Text.Encoding.ASCII);

        var moduleName = "hook";
        var swapDir = Path.Combine(repoRoot, "build", "hotstate");
        Directory.CreateDirectory(swapDir);
        var baseName = Path.GetFileNameWithoutExtension(stasisPath);
        var runnerOutLog = Path.Combine(swapDir, $"{baseName}.{moduleName}.runner.out.log");
        var runnerErrLog = Path.Combine(swapDir, $"{baseName}.{moduleName}.runner.err.log");
        var startTime = DateTime.UtcNow;

        Process? proc = null;
        try
        {
            TryDelete(runnerOutLog);
            TryDelete(runnerErrLog);

            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", stasisPath, "--watch", "--backend", "cranelift", "--module", moduleName, "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe!;

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));

            var initialSwapCount = outLines.CountContains("HOTSWAP(ms):");
            Assert.True(initialSwapCount > 0, "watch did not report initial HOTSWAP(ms).");

            var semanticEdit = ApplyTickSemanticEdit(initialSource, 19);
            await File.WriteAllTextAsync(stasisPath, semanticEdit, System.Text.Encoding.ASCII);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > initialSwapCount,
                timeout: TimeSpan.FromMinutes(5));

            await WaitForAnyLineAsync(
                proc,
                () =>
                {
                    if (!File.Exists(runnerErrLog))
                    {
                        return false;
                    }

                    return TryReadTextShared(runnerErrLog, out var errLog) &&
                        errLog.Contains("HOTSWAP hook: on_code_swap rc=0", StringComparison.Ordinal);
                },
                timeout: TimeSpan.FromSeconds(60));

            var swapCountAfterHook = outLines.CountContains("HOTSWAP(ms):");

            await File.WriteAllTextAsync(stasisPath, layoutChangedSource, System.Text.Encoding.ASCII);

            await WaitForAnyLineAsync(
                proc,
                () =>
                {
                    if (!File.Exists(runnerOutLog))
                    {
                        return false;
                    }

                    return TryReadTextShared(runnerOutLog, out var outLog) &&
                        outLog.Contains("swap layout changed", StringComparison.OrdinalIgnoreCase);
                },
                timeout: TimeSpan.FromMinutes(2));

            await Task.Delay(500);

            Assert.Equal(swapCountAfterHook, outLines.CountContains("HOTSWAP(ms):"));
            Assert.True(outLines.AnyContains("HOTSWAP(state): rejected"), "watch did not report a rejected swap state.");
            Assert.False(proc.HasExited, "watch process exited after layout rejection.");
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
                // Best-effort cleanup.
            }

            try
            {
                tempDir.Delete(true);
            }
            catch
            {
                // ignore
            }

            try
            {
                foreach (var p in Process.GetProcessesByName("stasis-cranelift-jit-runner"))
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

    [HotSwapFact]
    public async Task WatchTickInProcessSwap_SwapsOnEdit_WithoutJitRunnerProcess()
    {
        var repoRoot = FindRepoRoot();
        var cliDll = FindCliDll(repoRoot);
        Assert.NotNull(cliDll);
        var clangBinDir = FindClangBinDir(repoRoot);
        if (string.IsNullOrWhiteSpace(clangBinDir))
        {
            throw SkipException.ForSkip("clang not found; skipping in-process tick swap test.");
        }

        var tempDir = Directory.CreateTempSubdirectory("stasis_inproc_tick_swap");
        var stasisPath = Path.Combine(tempDir.FullName, "inproc_tick_watch.stasis");
        File.WriteAllText(stasisPath, """
            struct WatchState {
                ticks: i32;
            }

            global state: WatchState;

            function main(): i32 {
                state.ticks = 0;
                return 0;
            }

            function tick(): i32 {
                return 0;
            }
            """, System.Text.Encoding.ASCII);

        var startTime = DateTime.UtcNow;
        Process? proc = null;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", stasisPath, "--watch", "--backend", "cranelift", "--module", "hot", "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_INPROC_TICK"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "0";
            psi.EnvironmentVariables["STASIS_WATCH_EVENT_JSON"] = "1";
            psi.EnvironmentVariables["PATH"] = clangBinDir + Path.PathSeparator + (Environment.GetEnvironmentVariable("PATH") ?? string.Empty);

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));

            var initialSwapCount = outLines.CountContains("HOTSWAP(ms):");
            Assert.True(initialSwapCount > 0, "watch did not report initial HOTSWAP(ms).");

            var original = await File.ReadAllTextAsync(stasisPath);
            var edited = ApplyTickSemanticEdit(original, 23);
            await File.WriteAllTextAsync(stasisPath, edited, System.Text.Encoding.ASCII);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > initialSwapCount,
                timeout: TimeSpan.FromMinutes(5));

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(state): compiled") &&
                      outLines.AnyContains("HOTSWAP(state): queued") &&
                      outLines.AnyContains("HOTSWAP(state): applied"),
                timeout: TimeSpan.FromMinutes(2));

            Assert.True(
                outLines.AnyContains("WATCH_EVENT {\"type\":\"swap_state\""),
                $"structured swap_state events were not emitted.\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");

            var jitRunnerCount = Process.GetProcessesByName("stasis-cranelift-jit-runner")
                .Count(p =>
                {
                    try
                    {
                        return p.StartTime.ToUniversalTime() >= startTime.AddSeconds(-5);
                    }
                    catch
                    {
                        return false;
                    }
                });

            Assert.Equal(0, jitRunnerCount);
            Assert.False(proc.HasExited);
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
                // Best-effort cleanup.
            }

            try
            {
                tempDir.Delete(true);
            }
            catch
            {
                // ignore
            }
        }
    }

    [HotSwapFact]
    public async Task WatchTickInProcessSwap_AppliesAndReloadsDataBinding()
    {
        var repoRoot = FindRepoRoot();
        var cliDll = FindCliDll(repoRoot);
        Assert.NotNull(cliDll);
        var clangBinDir = FindClangBinDir(repoRoot);
        if (string.IsNullOrWhiteSpace(clangBinDir))
        {
            throw SkipException.ForSkip("clang not found; skipping in-process data binding test.");
        }

        var tempDir = Directory.CreateTempSubdirectory("stasis_inproc_tick_databind");
        var stasisPath = Path.Combine(tempDir.FullName, "inproc_tick_databind.stasis");
        var dataDir = Path.Combine(tempDir.FullName, "data");
        Directory.CreateDirectory(dataDir);
        var dataPath = Path.Combine(dataDir, "config.json");

        File.WriteAllText(stasisPath, """
            struct WatchState {
                value: i32;
                phase: i32;
            }

            global state: WatchState;

            function main(): i32 {
                state.value = 0;
                state.phase = 0;
                return 0;
            }

            function tick(): i32 {
                if (state.value == 5 && state.phase == 0) {
                    state.phase = 1;
                }

                if (state.value == 9 && state.phase == 1) {
                    return 2;
                }
                return 0;
            }
            """, System.Text.Encoding.ASCII);

        File.WriteAllText(dataPath, "{\"value\":5}\n", System.Text.Encoding.ASCII);

        Process? proc = null;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", stasisPath, "--watch", "--backend", "cranelift", "--module", "hot", "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_INPROC_TICK"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "0";
            psi.EnvironmentVariables["PATH"] = clangBinDir + Path.PathSeparator + (Environment.GetEnvironmentVariable("PATH") ?? string.Empty);

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            try
            {
                await WaitForAnyLineAsync(
                    proc,
                    () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                    timeout: TimeSpan.FromMinutes(5));

                await WaitForAnyLineAsync(
                    proc,
                    () => errLines.AnyContains("DATABIND: registered"),
                    timeout: TimeSpan.FromSeconds(20));

                await WaitForAnyLineAsync(
                    proc,
                    () => errLines.AnyContains("DATABIND: reloaded"),
                    timeout: TimeSpan.FromSeconds(30));

                var initialReloadCount = errLines.CountContains("DATABIND: reloaded");
                File.WriteAllText(dataPath, "{\"value\":9,\"_\":\"x\"}\n", System.Text.Encoding.ASCII);

                await WaitForProcessExitAsync(proc, TimeSpan.FromSeconds(30));
                Assert.Equal(2, proc.ExitCode);
                Assert.True(
                    errLines.CountContains("DATABIND: reloaded") > initialReloadCount,
                    $"expected a data-binding reload after config update.\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");
            }
            catch (Exception ex)
            {
                throw new XunitException(
                    $"{ex.Message}\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");
            }
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
                // Best-effort cleanup.
            }

            try
            {
                tempDir.Delete(true);
            }
            catch
            {
                // ignore
            }
        }
    }

    [HotSwapFact]
    public async Task WatchTickInProcessSwap_ReportsGenerationRetirementTelemetry()
    {
        var repoRoot = FindRepoRoot();
        var cliDll = FindCliDll(repoRoot);
        Assert.NotNull(cliDll);
        var clangBinDir = FindClangBinDir(repoRoot);
        if (string.IsNullOrWhiteSpace(clangBinDir))
        {
            throw SkipException.ForSkip("clang not found; skipping in-process generation retirement test.");
        }

        const int retireWindow = 2;
        var tempDir = Directory.CreateTempSubdirectory("stasis_inproc_tick_generation_retire");
        var stasisPath = Path.Combine(tempDir.FullName, "inproc_tick_generation_retire.stasis");
        await File.WriteAllTextAsync(stasisPath, BuildInProcessTickSource(1), System.Text.Encoding.ASCII);

        Process? proc = null;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", stasisPath, "--watch", "--backend", "cranelift", "--module", "hot", "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_INPROC_TICK"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "0";
            psi.EnvironmentVariables["STASIS_INPROC_RETIRE_WINDOW_FRAMES"] = retireWindow.ToString(System.Globalization.CultureInfo.InvariantCulture);
            psi.EnvironmentVariables["PATH"] = clangBinDir + Path.PathSeparator + (Environment.GetEnvironmentVariable("PATH") ?? string.Empty);

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));

            var initialSwapCount = outLines.CountContains("HOTSWAP(ms):");
            var appliedCount = outLines.CountContains("HOTSWAP(state): applied");
            Assert.True(initialSwapCount > 0, "watch did not report initial HOTSWAP(ms).");

            var observedGenerations = new List<long>();
            var observedPending = new List<long>();
            var observedRetired = new List<long>();

            foreach (var seed in new[] { 31, 37, 43, 47 })
            {
                await File.WriteAllTextAsync(stasisPath, BuildInProcessTickSource(seed), System.Text.Encoding.ASCII);

                await WaitForAnyLineAsync(
                    proc,
                    () => outLines.CountContains("HOTSWAP(ms):") > initialSwapCount &&
                          outLines.CountContains("HOTSWAP(state): applied") > appliedCount,
                    timeout: TimeSpan.FromMinutes(5));

                initialSwapCount = outLines.CountContains("HOTSWAP(ms):");
                appliedCount = outLines.CountContains("HOTSWAP(state): applied");

                var appliedLines = outLines.SnapshotLinesContaining("HOTSWAP(state): applied");
                Assert.NotEmpty(appliedLines);
                var latest = appliedLines[^1];

                Assert.True(TryParseLongMetric(latest, "gen", out var generation), $"missing gen telemetry in line: {latest}");
                Assert.True(TryParseLongMetric(latest, "retire_pending", out var pendingRetired), $"missing retire_pending telemetry in line: {latest}");
                Assert.True(TryParseLongMetric(latest, "retired", out var retiredCount), $"missing retired telemetry in line: {latest}");

                observedGenerations.Add(generation);
                observedPending.Add(pendingRetired);
                observedRetired.Add(retiredCount);
            }

            Assert.Equal(4, observedGenerations.Count);
            Assert.True(observedGenerations[0] >= 2, "expected first applied generation >= 2.");
            for (var i = 1; i < observedGenerations.Count; i++)
            {
                Assert.Equal(observedGenerations[i - 1] + 1, observedGenerations[i]);
            }

            var maxPending = observedPending.Count == 0 ? 0 : observedPending.Max();
            Assert.True(
                maxPending <= retireWindow + 1,
                $"pending retired generations exceeded expected bound. max_pending={maxPending} retire_window={retireWindow}");

            Assert.Contains(observedRetired, count => count > 0);
            Assert.False(proc.HasExited);
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
                // Best-effort cleanup.
            }

            try
            {
                tempDir.Delete(true);
            }
            catch
            {
                // ignore
            }
        }
    }

    [HotSwapFact]
    public async Task WatchTickInProcessSwap_RecoversAfterInitialBuildFailure()
    {
        var repoRoot = FindRepoRoot();
        var cliDll = FindCliDll(repoRoot);
        Assert.NotNull(cliDll);
        var clangBinDir = FindClangBinDir(repoRoot);
        if (string.IsNullOrWhiteSpace(clangBinDir))
        {
            throw SkipException.ForSkip("clang not found; skipping in-process tick swap recovery test.");
        }

        var tempDir = Directory.CreateTempSubdirectory("stasis_inproc_tick_recover");
        var stasisPath = Path.Combine(tempDir.FullName, "inproc_tick_recover.stasis");
        var brokenSource = """
            struct WatchState {
                ticks: i32;
            }

            global state: WatchState;

            function main(): i32 {
                state.ticks = 0;
                return 0;
            }

            function tick(): i32 {
                return missing_symbol;
            }
            """;
        var fixedSource = """
            struct WatchState {
                ticks: i32;
            }

            global state: WatchState;

            function main(): i32 {
                state.ticks = 0;
                return 0;
            }

            function tick(): i32 {
                return 0;
            }
            """;
        File.WriteAllText(stasisPath, brokenSource, System.Text.Encoding.ASCII);

        Process? proc = null;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", stasisPath, "--watch", "--backend", "cranelift", "--module", "hot", "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_INPROC_TICK"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "0";
            psi.EnvironmentVariables["PATH"] = clangBinDir + Path.PathSeparator + (Environment.GetEnvironmentVariable("PATH") ?? string.Empty);

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => errLines.AnyContains("warning: initial build failed; waiting for changes.") ||
                      outLines.AnyContains("HOTSWAP(ms):"),
                timeout: TimeSpan.FromMinutes(3));

            var initialSwapCount = outLines.CountContains("HOTSWAP(ms):");
            Assert.Equal(
                0,
                initialSwapCount);

            var initialErrorCount = errLines.CountContains("error:");
            await File.WriteAllTextAsync(stasisPath, fixedSource, System.Text.Encoding.ASCII);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > initialSwapCount ||
                      errLines.CountContains("error:") > initialErrorCount,
                timeout: TimeSpan.FromMinutes(5));

            Assert.True(
                outLines.CountContains("HOTSWAP(ms):") > initialSwapCount,
                $"watch did not recover after fixing startup source.\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");
            Assert.False(proc.HasExited);
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
                // Best-effort cleanup.
            }

            try
            {
                tempDir.Delete(true);
            }
            catch
            {
                // ignore
            }
        }
    }

    [HotSwapFact]
    public async Task WatchTickInProcessSwap_PreservesHookStateMigration()
    {
        var repoRoot = FindRepoRoot();
        var cliDll = FindCliDll(repoRoot);
        Assert.NotNull(cliDll);
        var clangBinDir = FindClangBinDir(repoRoot);
        if (string.IsNullOrWhiteSpace(clangBinDir))
        {
            throw SkipException.ForSkip("clang not found; skipping in-process hook migration test.");
        }

        var tempDir = Directory.CreateTempSubdirectory("stasis_inproc_tick_hook_migration");
        var stasisPath = Path.Combine(tempDir.FullName, "inproc_tick_hook_migration.stasis");
        var initialSource = """
            struct WatchState {
                migrated: i32;
            }

            global state: WatchState;

            function main(): i32 {
                state.migrated = 0;
                return 0;
            }

            function on_code_swap(): i32 {
                state.migrated = 42;
                return 0;
            }

            function tick(): i32 {
                return 0;
            }
            """;
        var migratedSource = """
            struct WatchState {
                migrated: i32;
            }

            global state: WatchState;

            function main(): i32 {
                state.migrated = 0;
                return 0;
            }

            function on_code_swap(): i32 {
                state.migrated = 42;
                return 0;
            }

            function tick(): i32 {
                if (state.migrated == 42) {
                    return 0;
                }
                return 99;
            }
            """;
        File.WriteAllText(stasisPath, initialSource, System.Text.Encoding.ASCII);

        Process? proc = null;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll!, "run", stasisPath, "--watch", "--backend", "cranelift", "--module", "hot", "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_INPROC_TICK"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "0";
            psi.EnvironmentVariables["PATH"] = clangBinDir + Path.PathSeparator + (Environment.GetEnvironmentVariable("PATH") ?? string.Empty);

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.AnyContains("HOTSWAP(ms):") || errLines.AnyContains("error:"),
                timeout: TimeSpan.FromMinutes(5));

            var initialSwapCount = outLines.CountContains("HOTSWAP(ms):");
            Assert.True(
                initialSwapCount > 0,
                $"watch did not report initial HOTSWAP(ms).\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");

            var errorCountBeforeSwap = errLines.CountContains("error:");
            await File.WriteAllTextAsync(stasisPath, migratedSource, System.Text.Encoding.ASCII);

            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > initialSwapCount ||
                      errLines.CountContains("error:") > errorCountBeforeSwap,
                timeout: TimeSpan.FromMinutes(5));

            Assert.True(
                outLines.CountContains("HOTSWAP(ms):") > initialSwapCount,
                $"watch did not report swap after migrated hook edit.\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");
            await Task.Delay(500);
            Assert.False(proc.HasExited);
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
                // Best-effort cleanup.
            }

            try
            {
                tempDir.Delete(true);
            }
            catch
            {
                // ignore
            }
        }
    }

    [HotSwapFact]
    public async Task WatchTickHotSwap_ReportsSemanticErrors_AndKeepsRunning()
    {
        var repoRoot = FindRepoRoot();
        var samplePath = Path.Combine(repoRoot, "samples", "hotstate_tick_watch.stasis");
        Assert.True(File.Exists(samplePath), $"missing sample: {samplePath}");

        var cliDll = FindCliDll(repoRoot);
        var jitRunnerExe = FindCraneliftJitRunnerExe(repoRoot);

        Assert.NotNull(cliDll);
        Assert.NotNull(jitRunnerExe);

        var original = await File.ReadAllTextAsync(samplePath);

        Process? proc = null;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "dotnet",
                Arguments = QuoteArgs(cliDll, "run", samplePath, "--watch", "--backend", "cranelift", "--module", "hot", "--fps", "60"),
                UseShellExecute = false,
                WorkingDirectory = repoRoot,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            };

            psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER"] = "1";
            psi.EnvironmentVariables["STASIS_CRANELIFT_JIT_RUNNER_EXE"] = jitRunnerExe;

            proc = Process.Start(psi);
            Assert.NotNull(proc);

            using var outLines = new AsyncLineCollector(proc!.StandardOutput);
            using var errLines = new AsyncLineCollector(proc.StandardError);

            try
            {
                await WaitForAnyLineAsync(
                    proc,
                    () =>
                        outLines.AnyContains("HOTSWAP(ms):") ||
                        errLines.AnyContains("warning: initial build failed") ||
                        errLines.AnyContains("error:"),
                    timeout: TimeSpan.FromMinutes(3));
            }
            catch (XunitException ex)
            {
                throw new XunitException(
                    $"{ex.Message}\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");
            }

            var initialSwapCount = outLines.CountContains("HOTSWAP(ms):");
            if (initialSwapCount == 0)
            {
                throw new XunitException(
                    $"watch failed to produce initial HOTSWAP(ms) marker.\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");
            }

            // Introduce a semantic error: unknown field on the global struct.
            var nl = original.Contains("\r\n", StringComparison.Ordinal) ? "\r\n" : "\n";
            var bad = original.Replace("function tick(): i32 {" + nl + "    return 0;" + nl + "}" + nl,
                "function tick(): i32 {" + nl + "    state.missing = 1;" + nl + "    return 0;" + nl + "}" + nl,
                StringComparison.Ordinal);
            Assert.NotEqual(original, bad);
            await File.WriteAllTextAsync(samplePath, bad, System.Text.Encoding.ASCII);

            try
            {
                await WaitForAnyLineAsync(
                    proc,
                    () => outLines.AnyContains("Unknown field") || errLines.AnyContains("Unknown field"),
                    timeout: TimeSpan.FromSeconds(60));
            }
            catch (XunitException ex)
            {
                throw new XunitException(
                    $"{ex.Message}\n\nwatch stdout tail:\n{outLines.GetTail()}\n\nwatch stderr tail:\n{errLines.GetTail()}");
            }

            if (proc.HasExited)
            {
                throw new XunitException($"watch process exited unexpectedly after semantic error (code={proc.ExitCode}).");
            }

            // Fix the file with a semantic edit; watch should recover on the next build.
            var semanticRecovery = ApplyTickSemanticEdit(original, 17);
            await File.WriteAllTextAsync(samplePath, semanticRecovery, System.Text.Encoding.ASCII);
            await WaitForAnyLineAsync(
                proc,
                () => outLines.CountContains("HOTSWAP(ms):") > initialSwapCount,
                timeout: TimeSpan.FromMinutes(5));

            Assert.False(proc.HasExited);
        }
        finally
        {
            await File.WriteAllTextAsync(samplePath, original, System.Text.Encoding.ASCII);

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
                // Best-effort cleanup.
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
        var candidates = new[]
        {
            Path.Combine(repoRoot, exe),
            Path.Combine(repoRoot, "build", exe),
            Path.Combine(repoRoot, "runtime", "build", "bin", "Release", exe),
            Path.Combine(repoRoot, "runtime", "build", "bin", "Debug", exe),
            Path.Combine(repoRoot, "runtime", "build", "bin", exe),
            Path.Combine(repoRoot, "runtime", "build", exe)
        };

        return candidates.FirstOrDefault(File.Exists);
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

    private static string? FindCraneliftJitRunnerExe(string repoRoot)
    {
        var exe = OperatingSystem.IsWindows() ? "stasis-cranelift-jit-runner.exe" : "stasis-cranelift-jit-runner";
        foreach (var config in new[] { "release", "debug" })
        {
            var p = Path.Combine(repoRoot, "tools", "cranelift-jit-runner", "target", config, exe);
            if (File.Exists(p))
            {
                return p;
            }
        }

        if (TryBuildCraneliftJitRunner(repoRoot))
        {
            var built = Path.Combine(repoRoot, "tools", "cranelift-jit-runner", "target", "release", exe);
            if (File.Exists(built))
            {
                return built;
            }
        }
        return null;
    }

    private static bool TryBuildCraneliftJitRunner(string repoRoot)
    {
        try
        {
            var cargo = OperatingSystem.IsWindows() ? "cargo.exe" : "cargo";
            var toolDir = Path.Combine(repoRoot, "tools", "cranelift-jit-runner");
            if (!Directory.Exists(toolDir))
            {
                return false;
            }

            var psi = new ProcessStartInfo
            {
                FileName = cargo,
                Arguments = "build -p stasis-cranelift-jit-runner --release",
                UseShellExecute = false,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                WorkingDirectory = toolDir
            };

            using var process = Process.Start(psi);
            if (process is null)
            {
                return false;
            }
            process.WaitForExit();
            return process.ExitCode == 0;
        }
        catch
        {
            return false;
        }
    }

    private static string? FindClangBinDir(string repoRoot)
    {
        var clangExe = OperatingSystem.IsWindows() ? "clang.exe" : "clang";
        var candidates = new List<string>();

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
                if (File.Exists(Path.Combine(bin, clangExe)))
                {
                    candidates.Add(bin);
                }
            }
        }

        // If clang is already on PATH, do nothing special.
        var path = Environment.GetEnvironmentVariable("PATH") ?? string.Empty;
        foreach (var part in path.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            if (File.Exists(Path.Combine(part, clangExe)))
            {
                candidates.Add(part);
            }
        }

        var best = PickBestClangBin(candidates);
        return best ?? candidates.FirstOrDefault();
    }

    private static string? PickBestClangBin(IEnumerable<string> bins)
    {
        const int minOpaquePtrMajor = 15;
        var best = (path: (string?)null, version: -1);

        foreach (var bin in bins.Distinct(StringComparer.OrdinalIgnoreCase))
        {
            var clangPath = Path.Combine(bin, OperatingSystem.IsWindows() ? "clang.exe" : "clang");
            if (!File.Exists(clangPath))
            {
                continue;
            }

            if (TryGetClangMajorVersion(clangPath, out var major))
            {
                if (major >= minOpaquePtrMajor && major > best.version)
                {
                    best = (bin, major);
                }
            }
        }

        return best.path;
    }

    private static bool TryGetClangMajorVersion(string clangPath, out int major)
    {
        major = 0;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = clangPath,
                Arguments = "--version",
                UseShellExecute = false,
                RedirectStandardOutput = true,
                RedirectStandardError = true
            };
            using var proc = Process.Start(psi);
            if (proc is null)
            {
                return false;
            }
            var line = proc.StandardOutput.ReadLine() ?? string.Empty;
            proc.WaitForExit(2000);
            var idx = line.IndexOf("version ", StringComparison.OrdinalIgnoreCase);
            if (idx < 0)
            {
                return false;
            }
            var ver = line[(idx + "version ".Length)..];
            var dot = ver.IndexOf('.');
            if (dot < 0)
            {
                return false;
            }
            return int.TryParse(ver[..dot], out major);
        }
        catch
        {
            return false;
        }
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

    private static string AppendSemanticEditFunction(string source, string functionName)
    {
        var nl = source.Contains("\r\n", StringComparison.Ordinal) ? "\r\n" : "\n";
        return source + nl + $"function {functionName}(): i32 {{ return 0; }}" + nl;
    }

    private static string ApplyTickSemanticEdit(string source, int seed)
    {
        var nl = source.Contains("\r\n", StringComparison.Ordinal) ? "\r\n" : "\n";
        var tick = "function tick(): i32 {" + nl + "    return 0;" + nl + "}";
        var replacement = "function tick(): i32 {" + nl + $"    return {seed} - {seed};" + nl + "}";
        return source.Replace(tick, replacement, StringComparison.Ordinal);
    }

    private static string BuildInProcessTickSource(int seed) =>
        $$"""
        struct WatchState {
            ticks: i32;
        }

        global state: WatchState;

        function main(): i32 {
            state.ticks = 0;
            return 0;
        }

        function tick(): i32 {
            state.ticks = state.ticks + 1;
            return {{seed}} - {{seed}};
        }
        """;

    private static string BuildPerfSingleEditSource(int seed) =>
        $$"""
        struct PerfState {
            ticks: i32;
        }

        global state: PerfState;

        function main(): i32 {
            state.ticks = 0;
            return 0;
        }

        function hot_single(): i32 {
            return {{seed}} - {{seed}};
        }

        function cold_identity(v: i32): i32 {
            return v;
        }

        function tick(): i32 {
            state.ticks = state.ticks + 1;
            let v: i32 = hot_single();
            return cold_identity(v);
        }
        """;

    private static string BuildPerfMultiEditSource(int seed) =>
        $$"""
        struct PerfState {
            ticks: i32;
        }

        global state: PerfState;

        function main(): i32 {
            state.ticks = 0;
            return 0;
        }

        function hot_a(): i32 { return {{seed}} - {{seed}}; }
        function hot_b(): i32 { return {{seed + 1}} - {{seed + 1}}; }
        function hot_c(): i32 { return {{seed + 2}} - {{seed + 2}}; }
        function hot_d(): i32 { return {{seed + 3}} - {{seed + 3}}; }
        function hot_e(): i32 { return {{seed + 4}} - {{seed + 4}}; }

        function cold_identity(v: i32): i32 {
            return v;
        }

        function tick(): i32 {
            state.ticks = state.ticks + 1;
            let v: i32 = hot_a() + hot_b() + hot_c() + hot_d() + hot_e();
            return cold_identity(v - v);
        }
        """;

    private static bool IsEnvFlagEnabled(string name) =>
        string.Equals(Environment.GetEnvironmentVariable(name), "1", StringComparison.OrdinalIgnoreCase);

    private static int GetEnvInt(string name, int fallback)
    {
        var value = Environment.GetEnvironmentVariable(name);
        return int.TryParse(value, out var parsed) ? parsed : fallback;
    }

    private static double GetEnvDouble(string name, double fallback)
    {
        var value = Environment.GetEnvironmentVariable(name);
        return double.TryParse(
            value,
            System.Globalization.NumberStyles.Float,
            System.Globalization.CultureInfo.InvariantCulture,
            out var parsed)
            ? parsed
            : fallback;
    }

    private static int CountHotSwapLatencyMetrics(AsyncLineCollector lines)
    {
        var count = 0;
        foreach (var line in lines.SnapshotLinesContaining("HOTSWAP(ms):"))
        {
            if (TryParseDoubleMetric(line, "latency", out var latencyMs) && latencyMs >= 0)
            {
                count++;
            }
        }

        return count;
    }

    private static bool TryGetLatestHotSwapLatencyMetric(AsyncLineCollector lines, out string metricLine, out double latencyMs, out double totalMs)
    {
        metricLine = string.Empty;
        latencyMs = 0;
        totalMs = 0;

        var metrics = lines.SnapshotLinesContaining("HOTSWAP(ms):");
        for (var i = metrics.Count - 1; i >= 0; i--)
        {
            var line = metrics[i];
            if (!TryParseDoubleMetric(line, "latency", out var latency) || latency < 0)
            {
                continue;
            }

            metricLine = line;
            latencyMs = latency;
            _ = TryParseDoubleMetric(line, "total", out totalMs);
            return true;
        }

        return false;
    }

    private static double Percentile(IReadOnlyList<double> values, double p)
    {
        if (values.Count == 0)
        {
            return 0;
        }

        var sorted = values.OrderBy(v => v).ToArray();
        var clamped = Math.Max(0.0, Math.Min(1.0, p));
        var idx = (int)Math.Ceiling(sorted.Length * clamped) - 1;
        if (idx < 0)
        {
            idx = 0;
        }
        if (idx >= sorted.Length)
        {
            idx = sorted.Length - 1;
        }
        return sorted[idx];
    }

    private static bool TryParseLongMetric(string line, string key, out long value)
    {
        value = 0;
        var needle = key + "=";
        var idx = line.IndexOf(needle, StringComparison.Ordinal);
        if (idx < 0)
        {
            return false;
        }

        idx += needle.Length;
        var end = idx;
        if (end < line.Length && line[end] == '-')
        {
            end++;
        }
        while (end < line.Length && char.IsDigit(line[end]))
        {
            end++;
        }
        if (end <= idx)
        {
            return false;
        }

        return long.TryParse(line.AsSpan(idx, end - idx), out value);
    }

    private static bool TryParseDoubleMetric(string line, string key, out double value)
    {
        value = 0;
        var needle = key + "=";
        var idx = line.IndexOf(needle, StringComparison.Ordinal);
        if (idx < 0)
        {
            return false;
        }

        idx += needle.Length;
        var end = idx;
        if (end < line.Length && line[end] == '-')
        {
            end++;
        }

        var sawDigit = false;
        while (end < line.Length)
        {
            var c = line[end];
            if (char.IsDigit(c))
            {
                sawDigit = true;
                end++;
                continue;
            }

            if (c == '.')
            {
                end++;
                continue;
            }

            break;
        }

        if (!sawDigit || end <= idx)
        {
            return false;
        }

        return double.TryParse(
            line.AsSpan(idx, end - idx),
            System.Globalization.NumberStyles.Float,
            System.Globalization.CultureInfo.InvariantCulture,
            out value);
    }

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

    private static async Task SendOverlayPipeCommandAsync(string pipeName, object command, TimeSpan timeout)
    {
        using var timeoutCts = new CancellationTokenSource(timeout);
        using var client = new NamedPipeClientStream(
            ".",
            pipeName,
            PipeDirection.Out,
            PipeOptions.Asynchronous);

        await client.ConnectAsync((int)timeout.TotalMilliseconds, timeoutCts.Token);
        using var writer = new StreamWriter(client, new UTF8Encoding(false), bufferSize: 1024, leaveOpen: true)
        {
            AutoFlush = true
        };
        var payload = JsonSerializer.Serialize(command);
        await writer.WriteLineAsync(payload);
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

    private static void RequireGraphicsHotSwapSupport()
    {
        if (IsEnvFlagEnabled("STASIS_RUN_GRAPHICS_HOTSWAP"))
        {
            return;
        }

        if (string.Equals(Environment.GetEnvironmentVariable("CI"), "true", StringComparison.OrdinalIgnoreCase))
        {
            throw SkipException.ForSkip(
                "graphics hot-swap integration tests are disabled in CI by default; set STASIS_RUN_GRAPHICS_HOTSWAP=1 to enable.");
        }
    }

    private static async Task WaitForProcessExitAsync(Process proc, TimeSpan timeout)
    {
        var sw = Stopwatch.StartNew();
        while (sw.Elapsed < timeout)
        {
            if (proc.HasExited)
            {
                return;
            }

            await Task.Delay(50);
        }

        throw new XunitException($"timeout after {timeout.TotalSeconds:0}s waiting for process exit.");
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

        public int CountContains(string needle)
        {
            var count = 0;
            lock (_lines)
            {
                for (var i = 0; i < _lines.Count; i++)
                {
                    if (_lines[i].Contains(needle, StringComparison.Ordinal))
                    {
                        count++;
                    }
                }
            }
            return count;
        }

        public List<string> SnapshotLinesContaining(string needle)
        {
            lock (_lines)
            {
                var lines = new List<string>();
                for (var i = 0; i < _lines.Count; i++)
                {
                    if (_lines[i].Contains(needle, StringComparison.Ordinal))
                    {
                        lines.Add(_lines[i]);
                    }
                }
                return lines;
            }
        }

        public string GetTail(int maxLines = 60)
        {
            lock (_lines)
            {
                if (_lines.Count == 0)
                {
                    return "<empty>";
                }

                var start = Math.Max(0, _lines.Count - maxLines);
                return string.Join("\n", _lines.Skip(start));
            }
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
    }
}
