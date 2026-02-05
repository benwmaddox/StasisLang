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

            // Trigger a real rebuild + swap.
            await File.AppendAllTextAsync(samplePath, "\n// test edit " + DateTime.UtcNow.Ticks + "\n", System.Text.Encoding.ASCII);

            var initialSwapCount = outLines.CountContains("HOTSWAP(ms):");
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

            await File.AppendAllTextAsync(samplePath, "\n// jit swap test edit " + DateTime.UtcNow.Ticks + "\n", System.Text.Encoding.ASCII);

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
            await File.WriteAllTextAsync(samplePath, original + "\n// jit swap recovery " + DateTime.UtcNow.Ticks + "\n", System.Text.Encoding.ASCII);

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

            // Fix the file; watch should recover on the next build.
            await File.WriteAllTextAsync(samplePath, original, System.Text.Encoding.ASCII);
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
