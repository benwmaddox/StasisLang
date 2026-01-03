using System.Diagnostics;
using System.Linq;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;
using Stasis.Compiler;
using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;
using Stasis.Cli;

var cliArgs = new Queue<string>(Environment.GetCommandLineArgs().Skip(1));
if (cliArgs.Count == 0 || cliArgs.Contains("--help"))
{
    PrintUsage();
    return;
}

var mode = "run";
var devMode = false;
string? path = null;
var includeTests = false;
var moduleName = "module";
var emitIrOnly = false;
string? outputPath = null;
var runAllInDirectory = false;
var watch = false;
string? optLevel = null;
var enableLto = false;
var enableGraphics = false;
string? graphicsLibPath = null;
BackendType? selectedBackend = null;
string? llvmTargetTriple = null;
bool? craneliftRunnerOverride = null;
var runnerEnv = Environment.GetEnvironmentVariable("STASIS_CRANELIFT_RUNNER");
if (runnerEnv == "1")
{
    craneliftRunnerOverride = true;
}
else if (runnerEnv == "0")
{
    craneliftRunnerOverride = false;
}
var useCraneliftRunner = craneliftRunnerOverride ?? false;
var enableHotState = false;
var tickHostFps = 60;

while (cliArgs.Count > 0)
{
    var arg = cliArgs.Dequeue();
    switch (arg)
    {
        case "dev":
            // Back-compat alias for the run dev workflow.
            mode = "run";
            break;
        case "build":
            mode = arg;
            break;
        case "release":
            mode = arg;
            optLevel ??= "3";
            enableLto = true;
            break;
        case "format":
            mode = arg;
            break;
        case "run":
        case "test":
            mode = arg;
            includeTests = includeTests || arg == "test";
            break;
        case "--with-tests":
            includeTests = true;
            break;
        case "--module" when cliArgs.Count > 0:
            moduleName = cliArgs.Dequeue();
            break;
        case "--out" when cliArgs.Count > 0:
            outputPath = cliArgs.Dequeue();
            break;
        case "--emit-ir":
            emitIrOnly = true;
            break;
        case "--all":
            runAllInDirectory = true;
            break;
        case "--watch":
            watch = true;
            break;
        case "--opt-level" when cliArgs.Count > 0:
            optLevel = cliArgs.Dequeue();
            break;
        case "--lto":
            enableLto = true;
            break;
        case "--no-lto":
            enableLto = false;
            break;
        case "--graphics":
            enableGraphics = true;
            break;
        case "--graphics-lib" when cliArgs.Count > 0:
            graphicsLibPath = cliArgs.Dequeue();
            enableGraphics = true;
            break;
        case "--backend" when cliArgs.Count > 0:
            var backendArg = cliArgs.Dequeue().ToLowerInvariant();
            selectedBackend = backendArg switch
            {
                "llvm" => BackendType.Llvm,
                "cranelift" => BackendType.Cranelift,
                _ => null
            };
            if (selectedBackend is null)
            {
                Console.Error.WriteLine($"error: invalid --backend '{backendArg}'. Use 'llvm' or 'cranelift'.");
                Environment.Exit(1);
            }
            break;
        case "--llvm-target" when cliArgs.Count > 0:
            llvmTargetTriple = cliArgs.Dequeue();
            break;
        case "--cranelift-runner":
            craneliftRunnerOverride = true;
            useCraneliftRunner = true;
            break;
        case "--no-cranelift-runner":
            craneliftRunnerOverride = false;
            useCraneliftRunner = false;
            break;
        case "--hot-state":
            enableHotState = true;
            break;
        case "--fps" when cliArgs.Count > 0:
            if (!int.TryParse(cliArgs.Dequeue(), out tickHostFps) || tickHostFps < 1 || tickHostFps > 240)
            {
                Console.Error.WriteLine("error: --fps expects an integer between 1 and 240.");
                Environment.Exit(1);
            }
            break;
        case "--help":
            PrintUsage();
            return;
        default:
            if (path is null)
            {
                path = arg;
            }
            else
            {
                Console.Error.WriteLine($"error: unexpected argument '{arg}'");
                Environment.Exit(1);
            }
            break;
    }
}

if (path is null)
{
    if (mode == "test")
    {
        runAllInDirectory = true;
        path = Directory.GetCurrentDirectory();
    }
    else
    {
        PrintUsage();
        return;
    }
}

if (optLevel is not null && !IsValidOptLevel(optLevel))
{
    Console.Error.WriteLine($"error: invalid --opt-level '{optLevel}'. Use 0,1,2,3,s,z.");
    Environment.Exit(1);
}

devMode = mode == "run";

// Set default backend based on mode if not explicitly specified
var backend = selectedBackend ?? CodeGeneratorFactory.GetDefaultBackend(mode == "release");
if (backend == BackendType.Cranelift && selectedBackend is null && !CanUseCranelift(emitIrOnly))
{
    Console.Error.WriteLine("warning: Cranelift backend unavailable; defaulting to LLVM.");
    backend = BackendType.Llvm;
}

if (backend == BackendType.Cranelift && (mode == "test" || mode == "run") && craneliftRunnerOverride is null)
{
    useCraneliftRunner = true;
}

if (enableHotState)
{
    if (mode != "run")
    {
        Console.Error.WriteLine("error: --hot-state is only supported in run mode.");
        Environment.Exit(1);
    }
    if (backend != BackendType.Cranelift)
    {
        Console.Error.WriteLine("error: --hot-state currently requires --backend cranelift.");
        Environment.Exit(1);
    }
    if (!useCraneliftRunner)
    {
        Console.Error.WriteLine("error: --hot-state requires the native runner (stasis_runner.exe).");
        Environment.Exit(1);
    }
}

// Warn if Cranelift is explicitly selected on unsupported platforms.
if (!ShouldSuppressWarnings() && backend == BackendType.Cranelift && selectedBackend is not null && !emitIrOnly)
{
    var craneliftTargetTriple = GetCraneliftTargetTriple();
    if (string.IsNullOrEmpty(craneliftTargetTriple))
    {
        Console.Error.WriteLine("warning: forcing --emit-ir mode because the Cranelift target triple is unknown for this host. Set STASIS_CRANELIFT_TARGET to override.");
        emitIrOnly = true;
    }
}

if (!File.Exists(path) && !Directory.Exists(path))
{
    Console.Error.WriteLine($"error: file not found: {path}");
    Environment.Exit(1);
}

static bool ShouldSuppressWarnings() =>
    string.Equals(Environment.GetEnvironmentVariable("STASIS_SUPPRESS_WARNINGS"), "1", StringComparison.OrdinalIgnoreCase);

static bool ContainsTopLevelFunction(CompilationUnitSyntax unit, string name) =>
    unit.Declarations
        .OfType<FunctionDeclarationSyntax>()
        .Any(f => string.Equals(f.Name.Text, name, StringComparison.Ordinal));

static IReadOnlyList<string> BuildRunnerExports(string moduleName, string mode, bool hasTick, bool includeTests)
{
    if (mode == "test" || includeTests)
    {
        return new[] { $"{moduleName}__run_tests" };
    }

    if (hasTick)
    {
        return new[] { $"{moduleName}__main", $"{moduleName}__tick" };
    }

    return new[] { $"{moduleName}__main" };
}

if (mode == "format")
{
    var input = File.ReadAllText(path);
    var formatted = Stasis.Cli.StasisFormatter.Format(input);
    if (!string.Equals(input, formatted, StringComparison.Ordinal))
    {
        File.WriteAllText(path, formatted);
    }

    return;
}

// Only load LLVM native libraries when using the LLVM backend.
// This keeps Cranelift run/test/build usable on machines where LLVM is unavailable or too heavy to load.
if (backend == BackendType.Llvm)
{
    LlvmNativeLoader.EnsureLoaded();
}

// Dev defaults: enable phase timing output when watching. (Explicit --watch always wins.)

if (devMode && watch)
{
    Environment.SetEnvironmentVariable("STASIS_PHASE_TIMING", "1");
}

if (watch)
{
    if (runAllInDirectory)
    {
        Console.Error.WriteLine("error: --watch cannot be combined with --all.");
        Environment.Exit(1);
    }
    if (mode is "build" or "release" or "format")
    {
        Console.Error.WriteLine("error: --watch is only supported for run/test modes.");
        Environment.Exit(1);
    }
}

if (emitIrOnly && outputPath is not null && (watch || runAllInDirectory))
{
    Console.Error.WriteLine("error: --out with --emit-ir is only supported for single-file runs.");
    Environment.Exit(1);
}

if (runAllInDirectory && mode == "test")
{
    var root = Directory.Exists(path) ? path : Path.GetDirectoryName(path)!;
    var files = Directory.GetFiles(root, "*.stasis", SearchOption.AllDirectories)
        .Where(LikelyContainsTestBlock)
        .OrderBy(p => p)
        .ToArray();
    if (files.Length == 0)
    {
        Console.Error.WriteLine($"error: no .stasis files found under {root}");
        Environment.Exit(1);
    }

    var allowReachabilityFallback = mode != "release";
    var enableTestCache = true;
    var overallExit = RunAllTestsInDirectoryParallel(files, includeTests, moduleName, emitIrOnly, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, useCraneliftRunner, allowReachabilityFallback, enableTestCache, llvmTargetTriple);
    Environment.Exit(overallExit);
}

if (watch)
{
    if (backend == BackendType.Cranelift)
    {
        Environment.SetEnvironmentVariable("STASIS_CRANELIFT_AOT_SERVER", "1");
        Environment.SetEnvironmentVariable("STASIS_CRANELIFT_RUNNER_SERVER", "1");
        useCraneliftRunner = true;
    }

    var watchExit = WatchFile(path, mode, includeTests, moduleName, emitIrOnly, outputPath, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, useCraneliftRunner, enableHotState, tickHostFps, llvmTargetTriple);
    Environment.Exit(watchExit);
}

var singleExit = ProcessFile(path, mode, includeTests, moduleName, emitIrOnly, outputPath, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, tickHostFps, llvmTargetTriple, useCraneliftRunner: useCraneliftRunner, enableHotState: enableHotState);
Environment.Exit(singleExit);

static int ProcessFile(string path, string mode, bool includeTests, string moduleName, bool emitIrOnly, string? outputPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, BackendType backend, int tickHostFps, string? llvmTargetTriple, bool useLowerLock = true, bool useCraneliftRunner = false, bool enableHotState = false)
{
    var fileStopwatch = System.Diagnostics.Stopwatch.StartNew();
    var logPhaseTiming = Environment.GetEnvironmentVariable("STASIS_PHASE_TIMING") == "1";
    var phaseStopwatch = System.Diagnostics.Stopwatch.StartNew();
    long readMs = 0;
    long parseMs = 0;
    long semaMs = 0;
    long layoutMs = 0;
    long lowerMs = 0;
    long irWriteMs = 0;
    long aotMs = 0;
    long aotSpawnMs = 0;
    long linkMs = 0;
    long runMs = 0;
    long llvmWriteMs = 0;
    long llvmExecMs = 0;
    var tempLl = string.Empty;
    var tempObj = string.Empty;
    var tempClif = string.Empty;

    try
    {
        var source = LoadSourceWithImports(path, out var importDiagnostics, out var importSource);
        if (logPhaseTiming)
        {
            readMs = phaseStopwatch.ElapsedMilliseconds;
            phaseStopwatch.Restart();
        }
        if (importDiagnostics.Count > 0)
        {
            PrintDiagnostics(importDiagnostics, importSource, path);
            return 1;
        }

        var runtimeImports = GetRuntimeImportFlags(path);

        // Auto-detect runtime usage via stdlib module imports if not explicitly enabled.
        // Keep LLVM tests deterministic by default; Cranelift tests need the runtime hooks for some programs.
        if (!enableGraphics && (mode != "test" || backend == BackendType.Cranelift) && (runtimeImports.graphics || runtimeImports.audio))
        {
            enableGraphics = true;
        }

        var parse = Parser.Parse(source);
        if (logPhaseTiming)
        {
            parseMs = phaseStopwatch.ElapsedMilliseconds;
            phaseStopwatch.Restart();
        }
        if (parse.Diagnostics.Count > 0)
        {
            
            PrintDiagnostics(parse.Diagnostics, source, path);
            return 1;
        }

        var sema = new SemanticAnalyzer(new SemanticAnalyzerOptions(runtimeImports.graphics, runtimeImports.audio)).Analyze(parse.CompilationUnit);
        if (logPhaseTiming)
        {
            semaMs = phaseStopwatch.ElapsedMilliseconds;
            phaseStopwatch.Restart();
        }
        if (sema.Diagnostics.Count > 0)
        {
            PrintDiagnostics(sema.Diagnostics, source, path);
            return 1;
        }

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        if (logPhaseTiming)
        {
            layoutMs = phaseStopwatch.ElapsedMilliseconds;
            phaseStopwatch.Restart();
        }

        // Generate code using selected backend
        string ir;
        IReadOnlyList<Diagnostic> lowerDiagnostics;

        if (backend == BackendType.Cranelift)
        {
            // Use Cranelift backend via ICodeGenerator interface (CLIF for now).
            var options = new CodeGenerationOptions(
                ModuleName: moduleName,
                IncludeTests: includeTests,
                EmitTestHarness: includeTests,
                HeadlessGraphics: !enableGraphics,
                AllowReachabilityFallback: mode != "release");

            using var generator = CodeGeneratorFactory.Create(backend, moduleName);
            var result = generator.Generate(parse.CompilationUnit, sema, layout, options);
            ir = result.Ir;
            lowerDiagnostics = result.Diagnostics;
        }
        else
        {
            // Use LLVM backend via ModuleLowerer (existing path)
            var lowerer = new ModuleLowerer();
            var lowerOptions = enableGraphics
                ? new LowerOptions(IncludeTests: includeTests, EmitTestHarness: includeTests, HeadlessGraphics: false, TargetTriple: llvmTargetTriple)
                : (includeTests ? LowerOptions.Default : LowerOptions.Production) with { TargetTriple = llvmTargetTriple };
            LowerResult lower;
            if (useLowerLock)
            {
                lock (LlvmLock.Lower)
                {
                    lower = lowerer.LowerToIr(parse.CompilationUnit, sema, layout, moduleName, lowerOptions);
                }
            }
            else
            {
                lower = lowerer.LowerToIr(parse.CompilationUnit, sema, layout, moduleName, lowerOptions);
            }
            ir = lower.Ir;
            lowerDiagnostics = lower.Diagnostics;
        }
        if (logPhaseTiming)
        {
            lowerMs = phaseStopwatch.ElapsedMilliseconds;
            phaseStopwatch.Restart();
        }
        if (lowerDiagnostics.Count > 0)
        {
            PrintDiagnostics(lowerDiagnostics, source, path);
            WriteIrOutput(ir, outputPath);
            return 1;
        }

        if (emitIrOnly)
        {
            WriteIrOutput(ir, outputPath);
            return lowerDiagnostics.Count > 0 ? 1 : 0;
        }

        if (backend == BackendType.Cranelift)
        {
            // Artifact cache: skip AOT + link on warm runs when the source and build options are unchanged.
            if (IsCraneliftArtifactCacheEnabled() &&
                !enableHotState &&
                mode != "build" &&
                mode != "release")
            {
                var cacheDir = GetCraneliftArtifactCacheDirectory(mode);
                Directory.CreateDirectory(cacheDir);
                var craneliftTargetTriple = GetCraneliftTargetTriple();
                var compilerCacheSalt = GetCompilerCacheSalt();
                var cacheKey = ComputeCraneliftArtifactCacheKey(path, source, mode, backend, moduleName, includeTests, optLevel, enableLto, graphicsLibPath, useCraneliftRunner, enableGraphics, craneliftTargetTriple, compilerCacheSalt);
                var cachedClif = Path.Combine(cacheDir, $"{cacheKey}.clif");
                var cachedObj = Path.Combine(cacheDir, $"{cacheKey}{GetObjectFileExtension()}");
                var cachedOut = Path.Combine(cacheDir, cacheKey + (useCraneliftRunner ? (RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".dll" : ".so") : (OperatingSystem.IsWindows() ? ".exe" : string.Empty)));

                var hasTick = mode == "run" && ContainsTopLevelFunction(parse.CompilationUnit, "tick");
                var runnerExports = useCraneliftRunner
                    ? BuildRunnerExports(moduleName, mode, hasTick, includeTests)
                    : Array.Empty<string>();
                DataBindingPlan? dataBindingPlan = null;
                if (useCraneliftRunner)
                {
                    if (!TryGetDataBindingPlan(path, layout, moduleName, runnerExports, out dataBindingPlan))
                    {
                        return 1;
                    }
                }

                if (File.Exists(cachedOut) && dataBindingPlan is null)
                {
                    var entryBase = mode == "test" ? "run_tests" : "main";
                    var entryName = $"{moduleName}__{entryBase}";
                    if (useCraneliftRunner)
                    {
                        return RunCachedRunnerDll(cachedOut, entryName, enableGraphics, graphicsLibPath, dataBindingPlan, tickHostFps: hasTick ? tickHostFps : null);
                    }
                    return RunCachedExecutable(mode, cachedOut, enableGraphics, graphicsLibPath);
                }

                File.WriteAllText(cachedClif, ir);
                if (logPhaseTiming)
                {
                    irWriteMs = phaseStopwatch.ElapsedMilliseconds;
                    phaseStopwatch.Restart();
                }

                if (useCraneliftRunner)
                {
                    var exports = runnerExports;
                    var sw = Stopwatch.StartNew();
                    var ensureExit = EnsureCraneliftCachedRunnerDll(cachedClif, cachedObj, cachedOut, moduleName, mode, optLevel, enableLto, enableGraphics, graphicsLibPath, dataBindingPlan?.DefPath, exports);
                    sw.Stop();
                    if (logPhaseTiming)
                    {
                        aotMs = sw.ElapsedMilliseconds;
                        linkMs = 0;
                        runMs = 0;
                    }
                    if (ensureExit != 0)
                    {
                        return ensureExit;
                    }

                    var runSw = Stopwatch.StartNew();
                    var entryBase = mode == "test" ? "run_tests" : "main";
                    var entryName = $"{moduleName}__{entryBase}";
                    var exit = RunCachedRunnerDll(cachedOut, entryName, enableGraphics, graphicsLibPath, dataBindingPlan, tickHostFps: hasTick ? tickHostFps : null);
                    runSw.Stop();
                    if (logPhaseTiming)
                    {
                        runMs = runSw.ElapsedMilliseconds;
                    }
                    return exit;
                }
                else
                {
                    var sw = Stopwatch.StartNew();
                    var ensureExit = EnsureCraneliftCachedExecutable(cachedClif, cachedObj, cachedOut, moduleName, mode, optLevel, enableLto, enableGraphics, graphicsLibPath);
                    sw.Stop();
                    if (logPhaseTiming)
                    {
                        aotMs = sw.ElapsedMilliseconds;
                        linkMs = 0;
                        runMs = 0;
                    }
                    if (ensureExit != 0)
                    {
                        return ensureExit;
                    }
                    var runSw = Stopwatch.StartNew();
                    var exit = RunCachedExecutable(mode, cachedOut, enableGraphics, graphicsLibPath);
                    runSw.Stop();
                    if (logPhaseTiming)
                    {
                        runMs = runSw.ElapsedMilliseconds;
                    }
                    return exit;
                }
            }

            // Native Cranelift path: CLIF -> object -> clang link -> executable.
            if (!TryFindCraneliftAot(out var aotTool))
            {
                Console.Error.WriteLine("error: stasis-cranelift-aot not found. Build it with `cargo build -p stasis-cranelift-aot` (in tools/cranelift-aot) or set STASIS_CRANELIFT_AOT.");
                return 1;
            }

            var useAotServer = UseCraneliftAotServer();
            var useInMemoryClif = useAotServer && useCraneliftRunner && mode != "build" && mode != "release" && !enableHotState;
            if (!useInMemoryClif)
            {
                tempClif = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.clif");
                File.WriteAllText(tempClif, ir);
                if (logPhaseTiming)
                {
                    irWriteMs = phaseStopwatch.ElapsedMilliseconds;
                    phaseStopwatch.Restart();
                }
            }
            else if (logPhaseTiming)
            {
                irWriteMs = 0;
                phaseStopwatch.Restart();
            }

            if (useCraneliftRunner && mode != "build" && mode != "release")
            {
                HotStatePlan? hotStatePlan = null;
                var hasTick = ContainsTopLevelFunction(parse.CompilationUnit, "tick");
                var exports = BuildRunnerExports(moduleName, mode, hasTick, includeTests);
                if (enableHotState)
                {
                    if (!TryCreateHotStatePlan(path, layout, moduleName, exports, excludeSpriteFields: true, out var createdPlan))
                    {
                        return 1;
                    }
                    hotStatePlan = createdPlan;
                }
                if (!TryGetDataBindingPlan(path, layout, moduleName, exports, out var dataBindingPlan))
                {
                    return 1;
                }

                long? runAotSpawn;
                long runAotCompile;
                long runLink;
                long runRun;
                var runExit = useInMemoryClif
                    ? ExecuteClifWithRunnerFromString(mode, ir, optLevel, enableLto, enableGraphics, graphicsLibPath, aotTool, moduleName, hotStatePlan, dataBindingPlan, hasTick ? tickHostFps : (int?)null, out runAotSpawn, out runAotCompile, out runLink, out runRun)
                    : ExecuteClifWithRunner(mode, tempClif, optLevel, enableLto, enableGraphics, graphicsLibPath, aotTool, moduleName, hotStatePlan, dataBindingPlan, hasTick ? tickHostFps : (int?)null, out runAotSpawn, out runAotCompile, out runLink, out runRun);
                if (logPhaseTiming)
                {
                    aotSpawnMs = runAotSpawn ?? 0;
                    aotMs = runAotCompile;
                    linkMs = runLink;
                    runMs = runRun;
                }
                return runExit;
            }

            tempObj = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}{GetObjectFileExtension()}");
            var aotExit = RunCraneliftAot(aotTool, tempClif, tempObj, moduleName, optLevel, out var aotSpawnFallback, out var aotCompileFallback);
            if (logPhaseTiming)
            {
                aotSpawnMs = aotSpawnFallback ?? 0;
                aotMs = aotCompileFallback;
                phaseStopwatch.Restart();
            }
            if (aotExit != 0)
            {
                return aotExit;
            }

            if (mode == "build" || mode == "release")
            {
                var outPath = outputPath ?? BuildDefaultOutputPath(path);
                var entryBase = includeTests ? "run_tests" : "main";
                var entryName = $"{moduleName}__{entryBase}";
                var exitCode = BuildExecutableFromObject(tempObj, outPath, includeTests, optLevel, enableLto, enableGraphics, graphicsLibPath, entryName);
                if (logPhaseTiming)
                {
                    linkMs = phaseStopwatch.ElapsedMilliseconds;
                }
                return exitCode;
            }

            var execExit = ExecuteObject(mode, tempObj, optLevel, enableLto, enableGraphics, graphicsLibPath, moduleName);
            if (logPhaseTiming)
            {
                linkMs = phaseStopwatch.ElapsedMilliseconds;
            }
            return execExit;
        }

        // LLVM path: emit .ll and run via lli/clang.
        tempLl = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.ll");
        File.WriteAllText(tempLl, ir);
        if (logPhaseTiming)
        {
            llvmWriteMs = phaseStopwatch.ElapsedMilliseconds;
            phaseStopwatch.Restart();
        }

        if (mode == "build" || mode == "release")
        {
            var outPath = outputPath ?? BuildDefaultOutputPath(path);
            var exitCode = BuildExecutable(tempLl, outPath, includeTests, optLevel, enableLto, enableGraphics, graphicsLibPath);
            if (logPhaseTiming)
            {
                llvmExecMs = phaseStopwatch.ElapsedMilliseconds;
            }
            return exitCode;
        }

        var executeExit = Execute(mode, tempLl, optLevel, enableLto, enableGraphics, graphicsLibPath);
        if (logPhaseTiming)
        {
            llvmExecMs = phaseStopwatch.ElapsedMilliseconds;
        }
        return executeExit;
    }
    finally
    {
        if (!string.IsNullOrEmpty(tempLl) && File.Exists(tempLl))
        {
            File.Delete(tempLl);
        }
        if (!string.IsNullOrEmpty(tempObj) && File.Exists(tempObj))
        {
            File.Delete(tempObj);
        }
        if (!string.IsNullOrEmpty(tempClif) && File.Exists(tempClif))
        {
            File.Delete(tempClif);
        }

        fileStopwatch.Stop();
        if (logPhaseTiming)
        {
            if (backend == BackendType.Cranelift)
            {
                Console.WriteLine($"Phase time: read={readMs}ms parse={parseMs}ms sema={semaMs}ms layout={layoutMs}ms lower={lowerMs}ms clif_write={irWriteMs}ms aot_spawn={aotSpawnMs}ms aot_compile={aotMs}ms link={linkMs}ms run={runMs}ms");
            }
            else
            {
                Console.WriteLine($"Phase time: read={readMs}ms parse={parseMs}ms sema={semaMs}ms layout={layoutMs}ms lower={lowerMs}ms ll_write={llvmWriteMs}ms llvm_exec={llvmExecMs}ms");
            }
        }
        Console.WriteLine($"Total time={fileStopwatch.ElapsedMilliseconds}ms");
    }
}

static int ExecuteObject(string mode, string objPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, string moduleName)
{
    if (!TryFindTool("clang", out var clang))
    {
        Console.Error.WriteLine("error: run requires clang in PATH.");
        return 1;
    }

    var exePath = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.exe");
    try
    {
        var entryBase = mode == "test" ? "run_tests" : "main";
        var entryName = $"{moduleName}__{entryBase}";
        var args = BuildClangArgsForObject(objPath, exePath, mode == "test", optLevel, enableLto, enableGraphics, graphicsLibPath, entryName: entryName);
        var exit = RunProcess(clang, args, suppressOutput: true);
        if (exit != 0)
        {
            return exit;
        }

        if (enableGraphics)
        {
            var exeDir = Path.GetDirectoryName(exePath);
            if (!string.IsNullOrEmpty(exeDir))
            {
                CopyGraphicsRuntimeDependencies(exeDir, graphicsLibPath);
            }
        }

        return RunProcess(exePath, string.Empty, psi =>
        {
            if (enableGraphics)
            {
                var runTest = Environment.GetEnvironmentVariable("STASIS_RUN_RENDER_TEST");
                if (string.IsNullOrEmpty(runTest) || runTest == "0")
                {
                    if (Environment.GetEnvironmentVariable("STASIS_SKIP_RENDER_TEST") is null)
                    {
                        psi.Environment["STASIS_SKIP_RENDER_TEST"] = "1";
                    }
                }
            }
        });
    }
    finally
    {
        if (File.Exists(exePath))
        {
            File.Delete(exePath);
        }
    }
}

static int ExecuteObjectWithRunner(string mode, string objPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, string moduleName, HotStatePlan? hotStatePlan, DataBindingPlan? dataBindingPlan, IReadOnlyList<string> dllExports, int? tickHostFps, out long linkMs, out long runMs)
{
    linkMs = 0;
    runMs = 0;
    if (!TryFindTool("clang", out var clang))
    {
        Console.Error.WriteLine("error: run requires clang in PATH.");
        return 1;
    }

    if (!TryFindCraneliftRunner(out var runnerPath))
    {
        Console.Error.WriteLine("error: stasis_runner not found. Build it in runtime/ and set STASIS_CRANELIFT_RUNNER_EXE if needed.");
        return 1;
    }

    var dllPath = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.dll");
    try
    {
        var entryBase = mode == "test" ? "run_tests" : "main";
        var entryName = $"{moduleName}__{entryBase}";
        var defPath = hotStatePlan?.DefPath ?? dataBindingPlan?.DefPath;
        var args = BuildClangArgsForObject(objPath, dllPath, mode == "test", optLevel, enableLto, enableGraphics, graphicsLibPath, entryName: entryName, isDll: true, windowsDefFilePath: defPath, windowsExports: dllExports);
        var linkStopwatch = Stopwatch.StartNew();
        var exit = RunProcess(clang, args, suppressOutput: true);
        linkMs = linkStopwatch.ElapsedMilliseconds;
        if (exit != 0)
        {
            return exit;
        }

        var dllDir = Path.GetDirectoryName(dllPath);
        if (enableGraphics && !string.IsNullOrEmpty(dllDir))
        {
            CopyGraphicsRuntimeDependencies(dllDir, graphicsLibPath);
        }

        var entry = entryName;
        if (UseCraneliftRunnerServer() && hotStatePlan is null && dataBindingPlan is null)
        {
            var runner = GetCraneliftRunnerServer(runnerPath);
            return runner.Run(dllPath, entry, out runMs);
        }

        var runStopwatch = Stopwatch.StartNew();
        var runnerArgs = $"\"{dllPath}\" {entry}";
        if (hotStatePlan is not null)
        {
            try
            {
                if (File.Exists(hotStatePlan.HotExitPath))
                {
                    File.Delete(hotStatePlan.HotExitPath);
                }
            }
            catch
            {
                // Best-effort; stale trigger file will be handled by runner.
            }
            runnerArgs += $" --state \"{hotStatePlan.SnapshotPath}\" --state-map \"{hotStatePlan.MapPath}\"";
            runnerArgs += $" --hot-exit-file \"{hotStatePlan.HotExitPath}\"";
        }
        if (dataBindingPlan is not null)
        {
            runnerArgs += $" --data-bind \"{dataBindingPlan.JsonPath}\" \"{dataBindingPlan.StructMetaPath}\"";
            Console.WriteLine($"Data binding: {dataBindingPlan.JsonPath}");
        }
        if (tickHostFps is not null)
        {
            runnerArgs += $" --fps {tickHostFps.Value}";
        }
        var runExit = RunProcess(runnerPath, runnerArgs);
        runMs = runStopwatch.ElapsedMilliseconds;
        return runExit;
    }
    finally
    {
        if (File.Exists(dllPath))
        {
            try
            {
                File.Delete(dllPath);
            }
            catch
            {
                // Best-effort cleanup; DLLs can be locked if still in use.
            }
        }
    }
}

static int ExecuteClifWithRunner(string mode, string clifPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, string aotTool, string moduleName, HotStatePlan? hotStatePlan, DataBindingPlan? dataBindingPlan, int? tickHostFps, out long? aotSpawnMs, out long aotCompileMs, out long linkMs, out long runMs)
{
    aotSpawnMs = null;
    aotCompileMs = 0;
    linkMs = 0;
    runMs = 0;
    var tempObj = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}{GetObjectFileExtension()}");
    try
    {
        var aotExit = RunCraneliftAot(aotTool, clifPath, tempObj, moduleName, optLevel, out aotSpawnMs, out aotCompileMs);
        if (aotExit != 0)
        {
            return aotExit;
        }

        IReadOnlyList<string> exports;
        if (mode == "test")
        {
            exports = new[] { $"{moduleName}__run_tests" };
        }
        else if (tickHostFps is not null)
        {
            exports = new[] { $"{moduleName}__main", $"{moduleName}__tick" };
        }
        else
        {
            exports = new[] { $"{moduleName}__main" };
        }
        return ExecuteObjectWithRunner(mode, tempObj, optLevel, enableLto, enableGraphics, graphicsLibPath, moduleName, hotStatePlan, dataBindingPlan, exports, tickHostFps, out linkMs, out runMs);
    }
    finally
    {
        if (File.Exists(tempObj))
        {
            File.Delete(tempObj);
        }
    }
}

static int ExecuteClifWithRunnerFromString(string mode, string clif, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, string aotTool, string moduleName, HotStatePlan? hotStatePlan, DataBindingPlan? dataBindingPlan, int? tickHostFps, out long? aotSpawnMs, out long aotCompileMs, out long linkMs, out long runMs)
{
    aotSpawnMs = null;
    aotCompileMs = 0;
    linkMs = 0;
    runMs = 0;
    var tempObj = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}{GetObjectFileExtension()}");
    try
    {
        var aotExit = RunCraneliftAotFromString(aotTool, clif, tempObj, moduleName, optLevel, out aotSpawnMs, out aotCompileMs);
        if (aotExit != 0)
        {
            return aotExit;
        }

        IReadOnlyList<string> exports;
        if (mode == "test")
        {
            exports = new[] { $"{moduleName}__run_tests" };
        }
        else if (tickHostFps is not null)
        {
            exports = new[] { $"{moduleName}__main", $"{moduleName}__tick" };
        }
        else
        {
            exports = new[] { $"{moduleName}__main" };
        }
        return ExecuteObjectWithRunner(mode, tempObj, optLevel, enableLto, enableGraphics, graphicsLibPath, moduleName, hotStatePlan, dataBindingPlan, exports, tickHostFps, out linkMs, out runMs);
    }
    finally
    {
        if (File.Exists(tempObj))
        {
            File.Delete(tempObj);
        }
    }
}

static int BuildExecutableFromObject(string objPath, string outputPath, bool isTest, string? optLevel, bool enableLto, bool enableGraphics = false, string? graphicsLibPath = null, string? entryName = null)
{
    if (!TryFindTool("clang", out var clang))
    {
        Console.Error.WriteLine("error: build requires clang in PATH.");
        return 1;
    }

    var outDir = Path.GetDirectoryName(outputPath);
    if (!string.IsNullOrEmpty(outDir))
    {
        Directory.CreateDirectory(outDir);
    }

    var args = BuildClangArgsForObject(objPath, outputPath, isTest, optLevel, enableLto, enableGraphics, graphicsLibPath, entryName: entryName);
    var exit = RunProcess(clang, args, suppressOutput: true);
    if (exit != 0)
    {
        return exit;
    }

    if (enableGraphics)
    {
        var exeDir = Path.GetDirectoryName(outputPath);
        if (!string.IsNullOrEmpty(exeDir))
        {
            CopyGraphicsRuntimeDependencies(exeDir, graphicsLibPath);
        }
    }

    Console.WriteLine($"built: {outputPath}");
    return 0;
}

static string BuildClangArgsForObject(string objPath, string outputPath, bool isTest, string? optLevel, bool enableLto, bool enableGraphics = false, string? graphicsLibPath = null, string? entryName = null, bool isDll = false, string? windowsDefFilePath = null, IReadOnlyList<string>? windowsExports = null)
{
    // Link the object file into a normal executable (use CRT defaults).
    var args = new List<string> { $"\"{objPath}\"" };
    if (isDll)
    {
        args.Add("-shared");
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            args.Add("-Wl,/NOIMPLIB");
        }
    }
    args.Add("-o");
    args.Add($"\"{outputPath}\"");
    if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
    {
        args.Add("-Wl,/NOLOGO");
    }

    if (!string.IsNullOrWhiteSpace(optLevel))
    {
        args.Add($"-O{optLevel}");
    }

    if (enableLto)
    {
        args.Add("-flto");
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            args.Add("-fuse-ld=lld");
            args.Add("-Wl,/nodefaultlib:libucrt");
        }
    }

    // Reuse the existing link setup logic (libs, graphics runtime, Windows SDK libs).
    // This intentionally mirrors BuildClangArgs as closely as possible.
    var linkingStaticGraphics = false;
    if (enableGraphics)
    {
        var libPath = graphicsLibPath ?? FindGraphicsLibrary(preferShared: isDll);
        if (!string.IsNullOrEmpty(libPath))
        {
            var libraryFile = Path.GetFileName(libPath);
            var isStaticLibrary = libraryFile != null && libraryFile.Contains("static", StringComparison.OrdinalIgnoreCase);
            if (isStaticLibrary)
            {
                linkingStaticGraphics = true;
            }

            args.Add($"\"{libPath}\"");
            var libraryDirectory = Path.GetDirectoryName(libPath);
            if (!string.IsNullOrEmpty(libraryDirectory))
            {
                args.Add($"-L\"{libraryDirectory}\"");
            }

            if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows) &&
                !isStaticLibrary &&
                !string.IsNullOrEmpty(libraryDirectory))
            {
                args.Add($"-Wl,-rpath,\"{libraryDirectory}\"");
            }

            if (isStaticLibrary)
            {
                args.Add("-lSDL2main");
                args.Add("-lSDL2-static");
                args.Add("-lglew32");
                args.Add("-lopengl32");
                args.Add("-luser32");
                args.Add("-lgdi32");
                args.Add("-limm32");
                args.Add("-lshell32");
                args.Add("-lsetupapi");
                args.Add("-lwinmm");
                args.Add("-lversion");
                args.Add("-lole32");
                args.Add("-loleaut32");
                args.Add("-ladvapi32");
                args.Add("-lcfgmgr32");
                args.Add("-lbcrypt");
            }
            else if (OperatingSystem.IsWindows())
            {
                args.Add("-Wl,/NODEFAULTLIB:libcmt");
            }
        }
        else
        {
            Console.Error.WriteLine("warning: --graphics specified but stasis_graphics library not found. Build runtime/stasis_graphics.c first.");
        }
    }

    if (isDll)
    {
        var exportName = entryName ?? (isTest ? "run_tests" : "main");
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            if (!string.IsNullOrEmpty(windowsDefFilePath))
            {
                args.Add($"-Wl,/DEF:\"{windowsDefFilePath}\"");
            }
            else
            {
                var exports = windowsExports is { Count: > 0 }
                    ? windowsExports
                    : new[] { exportName };
                foreach (var ex in exports)
                {
                    args.Add($"-Wl,/EXPORT:{ex}");
                }
            }
        }
    }
    else if (isTest || entryName is not null)
    {
        var entry = entryName ?? "run_tests";
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            args.Add($"-Wl,/entry:{entry}");
        }
        else
        {
            if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX))
            {
                entry = $"_{entry}";
            }
            args.Add($"-Wl,-e,{entry}");
            args.Add("-nostartfiles");
        }
    }

    if (!isDll && RuntimeInformation.IsOSPlatform(OSPlatform.OSX))
    {
        args.Add("-Wl,-no_pie");
    }

    if (!isDll && RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
    {
        args.Add("-Wl,/subsystem:console");
        args.Add("-Wl,/ignore:4210");
        args.Add("-Wl,/STACK:8388608");
    }

    if (linkingStaticGraphics && RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
    {
        args.Add("-Wl,/NODEFAULTLIB:libcmt");
    }

    var sdkRoot = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? GetLatestWindowsSdkLib() : null;
    if (sdkRoot is not null)
    {
        var ucrt = Path.Combine(sdkRoot, "ucrt", "x64");
        var um = Path.Combine(sdkRoot, "um", "x64");
        args.Add($"-L\"{ucrt}\"");
        args.Add($"-L\"{um}\"");
        args.Add("-lkernel32");
        args.Add("-lmsvcrt");
        args.Add("-llegacy_stdio_definitions");
        args.Add("-lucrt");
        args.Add("-lvcruntime");
        args.Add("-loldnames");
    }

    if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
    {
        args.Add("-lm");
    }

    return string.Join(" ", args);
}

static bool TryFindCraneliftAot(out string path)
{
    var env = Environment.GetEnvironmentVariable("STASIS_CRANELIFT_AOT");
    if (!string.IsNullOrEmpty(env) && File.Exists(env))
    {
        path = env;
        return true;
    }

    var repoRoot = FindRepoRoot();
    if (!string.IsNullOrEmpty(repoRoot))
    {
        var exeName = OperatingSystem.IsWindows() ? "stasis-cranelift-aot.exe" : "stasis-cranelift-aot";
        var release = Path.Combine(repoRoot, "tools", "cranelift-aot", "target", "release", exeName);
        if (File.Exists(release))
        {
            path = release;
            return true;
        }

        var debug = Path.Combine(repoRoot, "tools", "cranelift-aot", "target", "debug", exeName);
        if (File.Exists(debug))
        {
            path = debug;
            return true;
        }
    }

    return TryFindTool("stasis-cranelift-aot", out path);
}

static bool TryFindCraneliftRunner(out string path)
{
    var env = Environment.GetEnvironmentVariable("STASIS_CRANELIFT_RUNNER_EXE");
    if (!string.IsNullOrEmpty(env) && File.Exists(env))
    {
        path = env;
        return true;
    }

    var repoRoot = FindRepoRoot();
    if (!string.IsNullOrEmpty(repoRoot))
    {
        var exeName = OperatingSystem.IsWindows() ? "stasis_runner.exe" : "stasis_runner";
        var release = Path.Combine(repoRoot, "runtime", "build", "bin", "Release", exeName);
        if (File.Exists(release))
        {
            path = release;
            return true;
        }

        var root = Path.Combine(repoRoot, exeName);
        if (File.Exists(root))
        {
            path = root;
            return true;
        }
    }

    path = string.Empty;
    return false;
}

static bool CanUseCranelift(bool emitIrOnly)
{
    if (emitIrOnly)
    {
        return true;
    }

    if (!OperatingSystem.IsWindows())
    {
        return false;
    }

    if (!TryFindCraneliftAot(out _))
    {
        return false;
    }

    return TryFindTool("clang", out _);
}

static string? FindRepoRoot()
{
    var current = Directory.GetCurrentDirectory();
    while (!string.IsNullOrEmpty(current))
    {
        if (File.Exists(Path.Combine(current, "Stasis.sln")))
        {
            return current;
        }

        var parent = Directory.GetParent(current)?.FullName;
        if (string.IsNullOrEmpty(parent) || string.Equals(parent, current, StringComparison.OrdinalIgnoreCase))
        {
            break;
        }

        current = parent;
    }

    return null;
}

static int Execute(string mode, string llPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, string? cachedExecutablePath = null, bool keepExecutable = false)
{
    var cachedExecutableUsed = false;
    if (!string.IsNullOrWhiteSpace(cachedExecutablePath))
    {
        if (File.Exists(cachedExecutablePath))
        {
            cachedExecutableUsed = true;
        }
        else if (TryFindTool("clang", out var clangForCache))
        {
            var args = BuildClangArgs(llPath, cachedExecutablePath, mode == "test", optLevel, enableLto, enableGraphics, graphicsLibPath);
            var exit = RunProcess(clangForCache, args, suppressOutput: true);
            if (exit != 0)
            {
                return exit;
            }

            cachedExecutableUsed = true;
        }
    }

    if (cachedExecutableUsed)
    {
        var cachedExecutableDirectory = Path.GetDirectoryName(cachedExecutablePath);
        if (enableGraphics && !string.IsNullOrEmpty(cachedExecutableDirectory))
        {
            CopyGraphicsRuntimeDependencies(cachedExecutableDirectory, graphicsLibPath);
        }

        return RunProcess(cachedExecutablePath!, string.Empty, psi =>
        {
            if (enableGraphics)
            {
                var runTest = Environment.GetEnvironmentVariable("STASIS_RUN_RENDER_TEST");
                if (string.IsNullOrEmpty(runTest) || runTest == "0")
                {
                    if (Environment.GetEnvironmentVariable("STASIS_SKIP_RENDER_TEST") is null)
                    {
                        psi.Environment["STASIS_SKIP_RENDER_TEST"] = "1";
                    }
                }
            }
        });
    }

    // lli doesn't support external libraries easily, so use clang when graphics is enabled
    if (!enableGraphics && TryFindTool("lli", out var llvmInterpreter))
    {
        return ExecuteWithLlvmInterpreter(llvmInterpreter, mode, llPath, optLevel, enableLto, enableGraphics, graphicsLibPath);
    }

    if (TryFindTool("clang", out var clang))
    {
        var exePath = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}" + (RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".exe" : string.Empty));
        try
        {
            var args = BuildClangArgs(llPath, exePath, mode == "test", optLevel, enableLto, enableGraphics, graphicsLibPath);
            var exit = RunProcess(clang, args, suppressOutput: true);
            if (exit != 0)
            {
                return exit;
            }

            if (enableGraphics)
            {
                var exeDir = Path.GetDirectoryName(exePath);
                if (!string.IsNullOrEmpty(exeDir))
                {
                    CopyGraphicsRuntimeDependencies(exeDir, graphicsLibPath);
                }
            }

            return RunProcess(exePath, string.Empty, psi =>
            {
                if (enableGraphics)
                {
                    var runTest = Environment.GetEnvironmentVariable("STASIS_RUN_RENDER_TEST");
                    if (string.IsNullOrEmpty(runTest) || runTest == "0")
                    {
                        if (Environment.GetEnvironmentVariable("STASIS_SKIP_RENDER_TEST") is null)
                        {
                            psi.Environment["STASIS_SKIP_RENDER_TEST"] = "1";
                        }
                    }
                }
            });
        }
        finally
        {
            if (!keepExecutable && File.Exists(exePath))
            {
                File.Delete(exePath);
            }
        }
    }

    Console.Error.WriteLine("error: neither lli nor clang found. Install LLVM or add to PATH.");
    return 1;
}

static string BuildClangArgs(string llPath, string exePath, bool isTest, string? optLevel, bool enableLto, bool enableGraphics = false, string? graphicsLibPath = null)
{
    var args = new List<string> { $"\"{llPath}\"", "-o", $"\"{exePath}\"" };
    if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
    {
        args.Add("-Wl,/NOLOGO");
    }
    var linkingStaticGraphics = false;
    args.Add("-Wno-override-module");
    if (!string.IsNullOrWhiteSpace(optLevel))
    {
        args.Add($"-O{optLevel}");
    }

    if (enableLto)
    {
        args.Add("-flto");
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            args.Add("-fuse-ld=lld");
            args.Add("-Wl,/nodefaultlib:libucrt");
        }
    }

    if (enableGraphics)
    {
        // Link against the stasis graphics runtime library
        var libPath = graphicsLibPath ?? FindGraphicsLibrary();
        if (!string.IsNullOrEmpty(libPath))
        {
            var libraryDirectory = Path.GetDirectoryName(libPath);
            var libraryFile = Path.GetFileName(libPath);
            var isStaticLibrary = libraryFile != null && libraryFile.Contains("static", StringComparison.OrdinalIgnoreCase);
            linkingStaticGraphics = isStaticLibrary;

            if (!string.IsNullOrEmpty(libraryDirectory))
            {
                args.Add($"-L\"{libraryDirectory}\"");
            }

            // When a full path is known, pass it directly so clang doesn't guess the name
            if (!string.IsNullOrEmpty(libraryFile))
            {
                args.Add($"\"{libPath}\"");
            }
            else
            {
                args.Add("-lstasis_graphics");
            }

            if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows) &&
                !isStaticLibrary &&
                !string.IsNullOrEmpty(libraryDirectory))
            {
                args.Add($"-Wl,-rpath,\"{libraryDirectory}\"");
            }

            // If we are linking the static runtime, pull in its static deps for a single EXE.
            if (isStaticLibrary && RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
            {
                args.Add("-lSDL2main");
                args.Add("-lSDL2-static");
                args.Add("-lglew32");
                args.Add("-lopengl32");
                args.Add("-luser32");
                args.Add("-lgdi32");
                args.Add("-limm32");
                args.Add("-lshell32");
                args.Add("-lsetupapi");
                args.Add("-lwinmm");
                args.Add("-lversion");
                args.Add("-lole32");
                args.Add("-loleaut32");
                args.Add("-ladvapi32");
                args.Add("-lcfgmgr32");
                args.Add("-lbcrypt");
            }
        }
        else
        {
            Console.Error.WriteLine("warning: --graphics specified but stasis_graphics library not found. Build runtime/stasis_graphics.c first.");
        }
    }

    if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
    {
        if (isTest)
        {
            args.Add("-Wl,/entry:run_tests");
        }

        args.Add("-Wl,/subsystem:console");
        args.Add("-Wl,/ignore:4210");
        args.Add("-Wl,/STACK:8388608");

        // Suppress CRT conflict warning when linking SDL2-static (built with /MT) with dynamic CRT
        if (linkingStaticGraphics)
        {
            args.Add("-Wl,/NODEFAULTLIB:libcmt");
        }

        // For release builds with optimization, strip debug info and enable aggressive dead code elimination
        if (!string.IsNullOrWhiteSpace(optLevel) && optLevel != "0")
        {
            args.Add("-Wl,/DEBUG:NONE");     // No debug info
            args.Add("-Wl,/OPT:REF");        // Remove unreferenced functions/data
            args.Add("-Wl,/OPT:ICF");        // Fold identical functions
            args.Add("-Wl,/MERGE:.rdata=.text"); // Merge read-only sections
        }

        var sdkRoot = GetLatestWindowsSdkLib();
        if (sdkRoot is not null)
        {
            var ucrt = Path.Combine(sdkRoot, "ucrt", "x64");
            var um = Path.Combine(sdkRoot, "um", "x64");
            args.Add($"-L\"{ucrt}\"");
            args.Add($"-L\"{um}\"");
            // When linking static graphics, let clang pick CRT defaults to avoid duplicate ucrt linkage.
            args.Add("-lkernel32");
            args.Add("-lmsvcrt");
            // legacy_stdio_definitions provides printf and related functions
            args.Add("-llegacy_stdio_definitions");
            args.Add("-lucrt");
            args.Add("-lvcruntime");
            args.Add("-loldnames");
        }
    }
    else if (isTest)
    {
        args.Add("-Wl,-e,run_tests");
    }

    if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
    {
        args.Add("-lm");
    }

    return string.Join(" ", args);
}

static int ExecuteWithLlvmInterpreter(string llvmInterpreter, string mode, string llvmIrPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath)
{
    var interpreterArguments = mode == "test"
        ? $"-entry-function=run_tests \"{llvmIrPath}\""
        : $"\"{llvmIrPath}\"";
    var interpreterExitCode = RunProcess(llvmInterpreter, interpreterArguments);
    if (interpreterExitCode == 0)
    {
        return 0;
    }

    // Retry with clang for stability if the interpreter fails (e.g., SIGSEGV on some inputs).
    if (TryExecuteWithClangFallback(mode, llvmIrPath, optLevel, enableLto, enableGraphics, graphicsLibPath, out var clangExitCode))
    {
        return clangExitCode;
    }

    return interpreterExitCode;
}

static bool TryExecuteWithClangFallback(string mode, string llvmIrPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, out int exitCode)
{
    if (!TryFindTool("clang", out var clangPath))
    {
        exitCode = 1;
        return false;
    }

    var executablePath = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}" + (RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".exe" : string.Empty));
    try
    {
        var clangArguments = BuildClangArgs(llvmIrPath, executablePath, mode == "test", optLevel, enableLto, enableGraphics, graphicsLibPath);
        var clangExitCode = RunProcess(clangPath, clangArguments, suppressOutput: true);
        if (clangExitCode != 0)
        {
            exitCode = clangExitCode;
            return true;
        }

        exitCode = RunProcess(executablePath, string.Empty);
        return true;
    }
    finally
    {
        if (File.Exists(executablePath))
        {
            File.Delete(executablePath);
        }
    }
}

static void CopyGraphicsRuntimeDependencies(string targetDir, string? graphicsLibPath)
{
    try
    {
        Directory.CreateDirectory(targetDir);

        var dllCandidates = new List<string>();

        void AddGraphicsDllCandidate(string? path)
        {
            if (string.IsNullOrEmpty(path) || !File.Exists(path))
            {
                return;
            }

            var ext = Path.GetExtension(path);
            if (ext.Equals(".dll", StringComparison.OrdinalIgnoreCase) ||
                ext.Equals(".so", StringComparison.OrdinalIgnoreCase) ||
                ext.Equals(".dylib", StringComparison.OrdinalIgnoreCase))
            {
                dllCandidates.Add(path);
                return;
            }

            if (ext.Equals(".lib", StringComparison.OrdinalIgnoreCase))
            {
                var dllGuess = Path.ChangeExtension(path, ".dll");
                if (File.Exists(dllGuess))
                {
                    dllCandidates.Add(dllGuess);
                }
            }
        }

        // Prefer explicit lib path (derive DLL alongside .lib)
        if (!string.IsNullOrEmpty(graphicsLibPath))
        {
            var libFile = Path.GetFileName(graphicsLibPath);
            if (libFile != null && libFile.Contains("static", StringComparison.OrdinalIgnoreCase))
            {
                // Static runtime: nothing to copy
                return;
            }

            AddGraphicsDllCandidate(graphicsLibPath);
        }

        // Fall back to a direct shared-DLL search (do not return import libs).
        var foundSharedDll = FindGraphicsSharedLibrary();
        if (!string.IsNullOrEmpty(foundSharedDll))
        {
            dllCandidates.Add(foundSharedDll);
        }

        // Copy primary graphics shared lib + common deps if present in the same directory
        foreach (var src in dllCandidates.Distinct(StringComparer.OrdinalIgnoreCase))
        {
            var fileName = Path.GetFileName(src);
            if (string.IsNullOrEmpty(fileName))
            {
                continue;
            }

            var dest = Path.Combine(targetDir, fileName);
            File.Copy(src, dest, overwrite: true);

            var depDir = Path.GetDirectoryName(src);
            if (string.IsNullOrEmpty(depDir))
            {
                continue;
            }

            var deps = new[] { "SDL2.dll", "glew32.dll" };
            foreach (var dep in deps)
            {
                var depSrc = Path.Combine(depDir, dep);
                if (File.Exists(depSrc))
                {
                    var depDest = Path.Combine(targetDir, dep);
                    File.Copy(depSrc, depDest, overwrite: true);
                }
            }
        }
    }
    catch
    {
        // Best-effort; missing copies will surface as runtime load errors.
    }
}

static string? FindGraphicsSharedLibrary()
{
    var searchPaths = new List<string>();

    // Check relative to the CLI executable
    var exeDir = AppContext.BaseDirectory;
    searchPaths.Add(exeDir);
    searchPaths.Add(Path.Combine(exeDir, "runtime"));

    // Prefer workspace runtime outputs before falling back to cwd root
    var cwd = Directory.GetCurrentDirectory();
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "bin", "Release"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "bin"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "Release"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "bin", "Debug"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "Debug"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build"));
    searchPaths.Add(Path.Combine(cwd, "runtime"));
    searchPaths.Add(Path.Combine(cwd, "build"));
    searchPaths.Add(cwd);

    string[] candidates;
    if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
    {
        candidates = new[] { "stasis_graphics.dll" };
    }
    else if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX))
    {
        candidates = new[] { "libstasis_graphics.dylib" };
    }
    else
    {
        candidates = new[] { "libstasis_graphics.so" };
    }

    foreach (var dir in searchPaths)
    {
        foreach (var name in candidates)
        {
            var candidate = Path.Combine(dir, name);
            if (File.Exists(candidate))
            {
                return candidate;
            }
        }
    }

    return null;
}

static (bool graphics, bool audio) GetRuntimeImportFlags(string entryPath)
{
    var graphics = DetectsModuleImport(entryPath, "graphics.stasis");
    var audio = DetectsModuleImport(entryPath, "audio.stasis");
    return (graphics, audio);
}

static bool DetectsRuntimeImports(string entryPath)
{
    var imports = GetRuntimeImportFlags(entryPath);
    return imports.graphics || imports.audio;
}

static bool DetectsModuleImport(string entryPath, string moduleFileName)
{
    var visited = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
    var targetPath = ResolveStdlibModulePath(moduleFileName);
    return DetectsModuleImportInner(Path.GetFullPath(entryPath), moduleFileName, targetPath, visited);
}

static bool DetectsModuleImportInner(string path, string moduleFileName, string? targetPath, HashSet<string> visited)
{
    if (!visited.Add(path))
    {
        return false;
    }

    string source;
    try
    {
        source = File.ReadAllText(path);
    }
    catch
    {
        return false;
    }

    var baseDir = Path.GetDirectoryName(path) ?? string.Empty;
    var lineStart = 0;
    for (var index = 0; index <= source.Length; index++)
    {
        var isEnd = index == source.Length;
        var ch = isEnd ? '\n' : source[index];
        if (ch != '\n' && !isEnd)
        {
            continue;
        }

        var lineLength = index - lineStart;
        var line = source.Substring(lineStart, lineLength).TrimEnd('\r');
        if (TryParseImportLine(line, out var importPath))
        {
            var resolved = Path.GetFullPath(Path.Combine(baseDir, importPath));
            if (IsTargetModule(resolved, moduleFileName, targetPath))
            {
                return true;
            }

            if (File.Exists(resolved) && DetectsModuleImportInner(resolved, moduleFileName, targetPath, visited))
            {
                return true;
            }
        }

        lineStart = index + 1;
    }

    return false;
}

static bool IsTargetModule(string resolvedPath, string moduleFileName, string? targetPath)
{
    if (!string.IsNullOrEmpty(targetPath) &&
        string.Equals(resolvedPath, targetPath, StringComparison.OrdinalIgnoreCase))
    {
        return true;
    }

    return string.Equals(Path.GetFileName(resolvedPath), moduleFileName, StringComparison.OrdinalIgnoreCase);
}

static string? ResolveStdlibModulePath(string moduleFileName)
{
    var repoRoot = FindRepoRoot() ?? Directory.GetCurrentDirectory();
    var candidate = Path.GetFullPath(Path.Combine(repoRoot, "src", "stdlib", moduleFileName));
    return File.Exists(candidate) ? candidate : null;
}

static bool TryParseImportLine(string line, out string path)
{
    path = string.Empty;
    var trimmed = line.Trim();
    if (!trimmed.StartsWith("import", StringComparison.Ordinal))
    {
        return false;
    }

    var remainder = trimmed.Substring("import".Length).TrimStart();
    if (remainder.Length < 2 || remainder[0] != '\"')
    {
        return false;
    }

    var endQuote = remainder.IndexOf('\"', 1);
    if (endQuote < 0)
    {
        return false;
    }

    path = remainder.Substring(1, endQuote - 1);
    var tail = remainder.Substring(endQuote + 1).Trim();
    return tail.Length == 0 || tail == ";";
}

static bool DetectsTickUsage(string source) =>
    Regex.IsMatch(source, @"(?m)^\s*function\s+tick\s*\(", RegexOptions.CultureInvariant);

const int CraneliftArtifactCacheVersion = 1;

static bool IsCraneliftArtifactCacheEnabled() =>
    !string.Equals(Environment.GetEnvironmentVariable("STASIS_DISABLE_ARTIFACT_CACHE"), "1", StringComparison.OrdinalIgnoreCase);

static string GetCraneliftArtifactCacheDirectory(string mode)
{
    var currentDirectory = Directory.GetCurrentDirectory();
    var bucket = mode == "test" ? "test" : "run";
    return Path.Combine(currentDirectory, ".stasis_cache", bucket);
}

static string ComputeCraneliftArtifactCacheKey(string path, string source, string mode, BackendType backend, string moduleName, bool includeTests, string? optLevel, bool enableLto, string? graphicsLibPath, bool useCraneliftRunner, bool usesGraphics, string? craneliftTargetTriple, string compilerCacheSalt)
{
    var fullPath = Path.GetFullPath(path);
    var identity = new StringBuilder();
    identity.Append("version=").Append(CraneliftArtifactCacheVersion).Append('\n');
    identity.Append("mode=").Append(mode).Append('\n');
    identity.Append("backend=").Append(backend).Append('\n');
    identity.Append("module=").Append(moduleName).Append('\n');
    identity.Append("includeTests=").Append(includeTests).Append('\n');
    identity.Append("optLevel=").Append(optLevel ?? string.Empty).Append('\n');
    identity.Append("enableLto=").Append(enableLto).Append('\n');
    identity.Append("graphicsLibPath=").Append(graphicsLibPath ?? string.Empty).Append('\n');
    identity.Append("useCraneliftRunner=").Append(useCraneliftRunner).Append('\n');
    identity.Append("usesGraphics=").Append(usesGraphics).Append('\n');
    identity.Append("craneliftTarget=").Append(craneliftTargetTriple ?? string.Empty).Append('\n');
    identity.Append("compilerCacheSalt=").Append(compilerCacheSalt).Append('\n');
    identity.Append("path=").Append(fullPath).Append('\n');
    identity.Append("source=").Append(source);
    return ComputeSha256Hex(identity.ToString());
}

static string? FindGraphicsLibrary(bool preferShared = false)
{
    // Look for the graphics library in common locations
    var searchPaths = new List<string>();

    // Check relative to the CLI executable
    var exeDir = AppContext.BaseDirectory;
    searchPaths.Add(exeDir);
    searchPaths.Add(Path.Combine(exeDir, "runtime"));

    // Prefer workspace runtime outputs before falling back to cwd root
    var cwd = Directory.GetCurrentDirectory();
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "bin", "Release"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "bin"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "Release"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "bin", "Debug"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "Debug"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build"));
    searchPaths.Add(Path.Combine(cwd, "runtime"));
    searchPaths.Add(Path.Combine(cwd, "build"));
    searchPaths.Add(cwd);

    string[] candidates;
    string[]? fallbackCandidates = null;
    if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
    {
        if (preferShared)
        {
            candidates = new[]
            {
                "stasis_graphics.lib",
                "stasis_graphics.dll"
            };
            fallbackCandidates = new[]
            {
                "stasis_graphics_static.lib"
            };
        }
        else
        {
            candidates = new[]
            {
                "stasis_graphics_static.lib",
                "stasis_graphics.lib",
                "stasis_graphics.dll"
            };
        }
    }
    else if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX))
    {
        // Prefer dynamic linking on Unix platforms
        candidates = new[]
        {
            "libstasis_graphics.dylib"
        };
        if (preferShared)
        {
            fallbackCandidates = new[]
            {
                "libstasis_graphics_static.a"
            };
        }
        else
        {
            candidates = new[]
            {
                "libstasis_graphics.dylib",
                "libstasis_graphics_static.a"
            };
        }
    }
    else
    {
        // Prefer dynamic linking on Unix platforms
        candidates = new[]
        {
            "libstasis_graphics.so"
        };
        if (preferShared)
        {
            fallbackCandidates = new[]
            {
                "libstasis_graphics_static.a"
            };
        }
        else
        {
            candidates = new[]
            {
                "libstasis_graphics.so",
                "libstasis_graphics_static.a"
            };
        }
    }

    var found = FindFirstCandidate(searchPaths, candidates);
    if (found != null)
    {
        return found;
    }

    if (fallbackCandidates == null)
    {
        return null;
    }

    return FindFirstCandidate(searchPaths, fallbackCandidates);
}

static string? FindFirstCandidate(IEnumerable<string> searchPaths, IEnumerable<string> candidates)
{
    foreach (var dir in searchPaths)
    {
        foreach (var name in candidates)
        {
            var candidate = Path.Combine(dir, name);
            if (File.Exists(candidate))
            {
                if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows) &&
                    Path.GetExtension(candidate).Equals(".dll", StringComparison.OrdinalIgnoreCase))
                {
                    var importLib = Path.ChangeExtension(candidate, ".lib");
                    if (File.Exists(importLib))
                    {
                        return importLib;
                    }

                    // Skip DLLs without an import lib when linking.
                    continue;
                }

                return candidate;
            }
        }
    }

    return null;
}

static string? GetLatestWindowsSdkLib()
{
    var root = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86), "Windows Kits", "10", "Lib");
    if (!Directory.Exists(root))
    {
        return null;
    }

    return Directory.GetDirectories(root)
        .OrderByDescending(Path.GetFileName)
        .FirstOrDefault();
}

static bool TryFindTool(string name, out string path)
{
    var search = (Environment.GetEnvironmentVariable("PATH") ?? string.Empty)
        .Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries)
        .ToList();

    if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
    {
        var programFiles = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles);
        var programFilesX86 = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86);
        search.Add(Path.Combine(programFiles, "LLVM", "bin"));
        search.Add(Path.Combine(programFilesX86, "LLVM", "bin"));
    }

    foreach (var dir in search)
    {
        var candidate = Path.Combine(dir, RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? $"{name}.exe" : name);
        if (File.Exists(candidate))
        {
            path = candidate;
            return true;
        }
    }

    path = string.Empty;
    return false;
}

static int RunProcess(string fileName, string arguments, Action<ProcessStartInfo>? configure = null, bool suppressOutput = false)
{
    if (Environment.GetEnvironmentVariable("STASIS_LOG_COMMANDS") == "1")
    {
        Console.WriteLine($"{fileName} {arguments}");
    }

    var psi = new ProcessStartInfo
    {
        FileName = fileName,
        Arguments = arguments,
        UseShellExecute = false
    };
    // Pass asset root to child processes so relative sprite paths resolve to the workspace.
    var assetRoot = Directory.GetCurrentDirectory();
    psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = assetRoot;
    configure?.Invoke(psi);
    if (suppressOutput)
    {
        psi.RedirectStandardOutput = true;
        psi.RedirectStandardError = true;
    }

    using var proc = Process.Start(psi)!;
    string? stdOut = null;
    string? stdErr = null;
    if (psi.RedirectStandardOutput)
    {
        stdOut = proc.StandardOutput.ReadToEnd();
    }
    if (psi.RedirectStandardError)
    {
        stdErr = proc.StandardError.ReadToEnd();
    }
    proc.WaitForExit();
    if (proc.ExitCode != 0)
    {
        if (!string.IsNullOrWhiteSpace(stdOut))
        {
            Console.Write(stdOut);
        }
        if (!string.IsNullOrWhiteSpace(stdErr))
        {
            Console.Error.Write(stdErr);
        }
    }
    return proc.ExitCode;
}


static bool UseCraneliftAotServer() =>
    Environment.GetEnvironmentVariable("STASIS_CRANELIFT_AOT_SERVER") == "1";

static bool UseCraneliftRunnerServer() =>
    Environment.GetEnvironmentVariable("STASIS_CRANELIFT_RUNNER_SERVER") == "1";

static CraneliftAotServer GetCraneliftAotServer(string aotTool, out long? spawnMs)
{
    lock (CraneliftAotState.Lock)
    {
        if (CraneliftAotState.Instance == null || !CraneliftAotState.Instance.IsAlive)
        {
            CraneliftAotState.Instance?.Dispose();
            CraneliftAotState.Instance = CraneliftAotServer.Start(aotTool);
            spawnMs = CraneliftAotState.Instance.SpawnMs;

            AppDomain.CurrentDomain.ProcessExit += (_, _) =>
            {
                CraneliftAotState.Instance?.Dispose();
            };
        }
        else
        {
            spawnMs = null;
        }

        return CraneliftAotState.Instance;
    }
}

static CraneliftRunnerServer GetCraneliftRunnerServer(string runnerPath)
{
    lock (CraneliftRunnerState.Lock)
    {
        if (CraneliftRunnerState.Instance == null || !CraneliftRunnerState.Instance.IsAlive)
        {
            CraneliftRunnerState.Instance?.Dispose();
            CraneliftRunnerState.Instance = CraneliftRunnerServer.Start(runnerPath);

            AppDomain.CurrentDomain.ProcessExit += (_, _) =>
            {
                CraneliftRunnerState.Instance?.Dispose();
            };
        }

        return CraneliftRunnerState.Instance;
    }
}

static string NormalizeCraneliftOptLevel(string? optLevel)
{
    if (string.IsNullOrWhiteSpace(optLevel))
    {
        return "none";
    }

    return optLevel.Trim().ToLowerInvariant() switch
    {
        "0" => "none",
        "1" => "speed",
        "2" => "speed",
        "3" => "speed",
        "s" => "speed_and_size",
        "z" => "speed_and_size",
        "speed" => "speed",
        "speed_and_size" => "speed_and_size",
        _ => "none"
    };
}

static string? GetCraneliftTargetTriple()
{
    var overrideTriple = Environment.GetEnvironmentVariable("STASIS_CRANELIFT_TARGET");
    if (!string.IsNullOrWhiteSpace(overrideTriple))
    {
        return overrideTriple.Trim();
    }

    if (OperatingSystem.IsWindows())
    {
        return RuntimeInformation.ProcessArchitecture switch
        {
            Architecture.Arm64 => "aarch64-pc-windows-msvc",
            Architecture.X64 => "x86_64-pc-windows-msvc",
            _ => null
        };
    }

    if (OperatingSystem.IsLinux())
    {
        return RuntimeInformation.ProcessArchitecture switch
        {
            Architecture.Arm64 => "aarch64-unknown-linux-gnu",
            Architecture.X64 => "x86_64-unknown-linux-gnu",
            _ => null
        };
    }

    if (OperatingSystem.IsMacOS())
    {
        return RuntimeInformation.ProcessArchitecture switch
        {
            Architecture.Arm64 => "aarch64-apple-darwin",
            Architecture.X64 => "x86_64-apple-darwin",
            _ => null
        };
    }

    return null;
}

static string GetObjectFileExtension()
{
    return OperatingSystem.IsWindows() ? ".obj" : ".o";
}

static string GetCompilerCacheSalt()
{
    var assembly = typeof(Program).Assembly;
    var version = assembly.GetName().Version?.ToString() ?? "unknown";
    var informationalVersion = assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion ?? "unknown";
    var exePath = Environment.ProcessPath;
    if (!string.IsNullOrEmpty(exePath) && File.Exists(exePath))
    {
        var lastWriteTicks = File.GetLastWriteTimeUtc(exePath).Ticks;
        return $"{version}:{informationalVersion}:{lastWriteTicks}";
    }

    var baseDir = AppContext.BaseDirectory;
    if (!string.IsNullOrEmpty(baseDir) && Directory.Exists(baseDir))
    {
        var lastWriteTicks = Directory.GetLastWriteTimeUtc(baseDir).Ticks;
        return $"{version}:{informationalVersion}:{lastWriteTicks}";
    }

    return $"{version}:{informationalVersion}";
}

static int RunCraneliftAot(string aotTool, string clifPath, string objPath, string moduleName, string? optLevel, out long? spawnMs, out long compileMs)
{
    var target = GetCraneliftTargetTriple();
    if (string.IsNullOrEmpty(target))
    {
        Console.Error.WriteLine("error: unable to determine Cranelift target triple for this host. Set STASIS_CRANELIFT_TARGET to override.");
        spawnMs = null;
        compileMs = 0;
        return 1;
    }
    var normalizedOpt = NormalizeCraneliftOptLevel(optLevel);

    spawnMs = null;
    compileMs = 0;

    if (UseCraneliftAotServer())
    {
        var server = GetCraneliftAotServer(aotTool, out spawnMs);
        return server.Compile(clifPath, objPath, target, moduleName, normalizedOpt, out compileMs);
    }

    var sw = Stopwatch.StartNew();
    var exit = RunProcess(aotTool, $"--input \"{clifPath}\" --output \"{objPath}\" --target {target} --module-name \"{moduleName}\" --opt-level {normalizedOpt}");
    compileMs = sw.ElapsedMilliseconds;
    return exit;
}

static int RunCraneliftAotFromString(string aotTool, string clif, string objPath, string moduleName, string? optLevel, out long? spawnMs, out long compileMs)
{
    var target = GetCraneliftTargetTriple();
    if (string.IsNullOrEmpty(target))
    {
        Console.Error.WriteLine("error: unable to determine Cranelift target triple for this host. Set STASIS_CRANELIFT_TARGET to override.");
        spawnMs = null;
        compileMs = 0;
        return 1;
    }
    var normalizedOpt = NormalizeCraneliftOptLevel(optLevel);

    spawnMs = null;
    compileMs = 0;

    if (UseCraneliftAotServer())
    {
        var server = GetCraneliftAotServer(aotTool, out spawnMs);
        return server.CompileFromBytes(Encoding.UTF8.GetBytes(clif), objPath, target, moduleName, normalizedOpt, out compileMs);
    }

    var tempClif = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.clif");
    try
    {
        File.WriteAllText(tempClif, clif);
        return RunCraneliftAot(aotTool, tempClif, objPath, moduleName, optLevel, out spawnMs, out compileMs);
    }
    finally
    {
        if (File.Exists(tempClif))
        {
            File.Delete(tempClif);
        }
    }
}

static int BuildExecutable(string llPath, string outputPath, bool isTest, string? optLevel, bool enableLto, bool enableGraphics = false, string? graphicsLibPath = null)
{
    if (!TryFindTool("clang", out var clang))
    {
        Console.Error.WriteLine("error: build requires clang in PATH.");
        return 1;
    }

    var outDir = Path.GetDirectoryName(outputPath);
    if (!string.IsNullOrEmpty(outDir))
    {
        Directory.CreateDirectory(outDir);
    }

    var args = BuildClangArgs(llPath, outputPath, isTest, optLevel, enableLto, enableGraphics, graphicsLibPath);
    var exit = RunProcess(clang, args, suppressOutput: true);
    if (exit != 0)
    {
        return exit;
    }

    if (enableGraphics)
    {
        var exeDir = Path.GetDirectoryName(outputPath);
        if (!string.IsNullOrEmpty(exeDir))
        {
            CopyGraphicsRuntimeDependencies(exeDir, graphicsLibPath);
        }
    }

    Console.WriteLine($"built: {outputPath}");
    return 0;
}

static bool IsValidOptLevel(string level) =>
    level is "0" or "1" or "2" or "3" or "s" or "z";

static string BuildDefaultOutputPath(string sourcePath)
{
    var dir = Path.GetDirectoryName(sourcePath);
    var name = Path.GetFileNameWithoutExtension(sourcePath);
    var ext = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".exe" : string.Empty;

    var baseDir = string.IsNullOrEmpty(dir) ? Directory.GetCurrentDirectory() : dir;
    var projectBuildDir = Path.Combine(baseDir, "build");
    var hasProjectStructure =
        Directory.Exists(Path.Combine(baseDir, "assets")) ||
        Directory.Exists(Path.Combine(baseDir, "data")) ||
        Directory.Exists(projectBuildDir);

    if (hasProjectStructure)
    {
        return Path.Combine(projectBuildDir, name + ext);
    }

    return Path.Combine(baseDir, name + ext);
}

static string GetHotExitFilePath(string sourcePath, string moduleName)
{
    var repoRoot = FindRepoRoot() ?? Directory.GetCurrentDirectory();
    var hotDir = Path.Combine(repoRoot, "build", "hotstate");
    var baseName = Path.GetFileNameWithoutExtension(sourcePath);
    return Path.Combine(hotDir, $"{baseName}.{moduleName}.hot-exit");
}

static string? FindDataBindingJson(string sourcePath, string repoRoot)
{
    static string? FirstJsonInDir(string dir)
    {
        if (!Directory.Exists(dir))
        {
            return null;
        }

        var preferred = Path.Combine(dir, "config.json");
        if (File.Exists(preferred))
        {
            return preferred;
        }

        return Directory.GetFiles(dir, "*.json", SearchOption.TopDirectoryOnly)
            .OrderBy(p => p, StringComparer.OrdinalIgnoreCase)
            .FirstOrDefault();
    }

    var sourceDir = Path.GetDirectoryName(sourcePath);
    if (!string.IsNullOrEmpty(sourceDir))
    {
        var inLocalDataDir = FirstJsonInDir(Path.Combine(sourceDir, "data"));
        if (!string.IsNullOrEmpty(inLocalDataDir))
        {
            return inLocalDataDir;
        }

        var inLocalDir = FirstJsonInDir(sourceDir);
        if (!string.IsNullOrEmpty(inLocalDir))
        {
            return inLocalDir;
        }
    }

    var srcBaseName = Path.GetFileNameWithoutExtension(sourcePath);
    var inRepoDataDir = FirstJsonInDir(Path.Combine(repoRoot, "data", srcBaseName));
    if (!string.IsNullOrEmpty(inRepoDataDir))
    {
        return inRepoDataDir;
    }

    return null;
}

static bool TryGetDataBindingPlan(string sourcePath, LayoutPlan layout, string moduleName, IReadOnlyList<string> exportedFunctions, out DataBindingPlan? plan)
{
    plan = null;
    if (string.Equals(Environment.GetEnvironmentVariable("STASIS_DISABLE_DATA_BIND"), "1", StringComparison.OrdinalIgnoreCase))
    {
        return true;
    }

    var repoRoot = FindRepoRoot() ?? Directory.GetCurrentDirectory();
    var dataFile = FindDataBindingJson(sourcePath, repoRoot);
    if (string.IsNullOrEmpty(dataFile))
    {
        return true;
    }

    dataFile = Path.GetFullPath(dataFile);
    if (!TryCreateHotStatePlan(sourcePath, layout, moduleName, exportedFunctions, excludeSpriteFields: true, out var hotPlan))
    {
        return false;
    }

    plan = new DataBindingPlan(dataFile, hotPlan.StructMetaPath, hotPlan.DefPath);
    return true;
}

static int WatchCraneliftTickHotSwap(string sourcePath, string moduleName, int fps, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath)
{
    if (!TryFindCraneliftRunner(out var runnerPath))
    {
        Console.Error.WriteLine("error: stasis_runner not found. Build it in runtime/ and set STASIS_CRANELIFT_RUNNER_EXE if needed.");
        return 1;
    }
    if (!TryFindCraneliftAot(out var aotTool))
    {
        Console.Error.WriteLine("error: stasis-cranelift-aot not found. Build it with `cargo build -p stasis-cranelift-aot` (in tools/cranelift-aot) or set STASIS_CRANELIFT_AOT.");
        return 1;
    }
    if (!TryFindTool("clang", out var clang))
    {
        Console.Error.WriteLine("error: run requires clang in PATH.");
        return 1;
    }

    var repoRoot = FindRepoRoot() ?? Directory.GetCurrentDirectory();
    var swapFile = Path.Combine(repoRoot, "build", "hotstate", $"{Path.GetFileNameWithoutExtension(sourcePath)}.{moduleName}.swap");
    var baseName = Path.GetFileNameWithoutExtension(sourcePath);
    var swapDir = Path.Combine(repoRoot, "build", "hotstate");
    var swapDllA = Path.Combine(swapDir, $"{baseName}.{moduleName}.swapA.dll");
    var swapDllB = Path.Combine(swapDir, $"{baseName}.{moduleName}.swapB.dll");
    var pid = Environment.ProcessId;
    var hotClifPath = Path.Combine(swapDir, $"{baseName}.{moduleName}.{pid}.hotswap.clif");
    var hotObjPath = Path.Combine(swapDir, $"{baseName}.{moduleName}.{pid}.hotswap{GetObjectFileExtension()}");
    Directory.CreateDirectory(Path.GetDirectoryName(swapFile)!);
    try
    {
        if (File.Exists(swapFile))
        {
            File.Delete(swapFile);
        }
    }
    catch
    {
        // ignore
    }

    using var cts = new CancellationTokenSource();
    Console.CancelKeyPress += (_, e) =>
    {
        e.Cancel = true;
        cts.Cancel();
    };

    string? activeDll = null;
    Process? runner = null;

    int BuildAndSwap(bool startRunner, out string timingLine)
    {
        timingLine = string.Empty;
        var swTotal = Stopwatch.StartNew();
        var readMs = 0L;
        var parseMs = 0L;
        var semaMs = 0L;
        var layoutMs = 0L;
        var lowerMs = 0L;
        var clifWriteMs = 0L;
        var aotSpawnMs = 0L;
        var aotCompileMs = 0L;
        var linkMs = 0L;
        var planMs = 0L;
        var swapWriteMs = 0L;
        var runnerSpawnMs = 0L;

        var phase = Stopwatch.StartNew();
        var source = LoadSourceWithImports(sourcePath, out var importDiagnostics, out var importSource);
        readMs = phase.ElapsedMilliseconds;
        phase.Restart();
        if (importDiagnostics.Count > 0)
        {
            PrintDiagnostics(importDiagnostics, importSource, sourcePath);
            return 1;
        }

        var usesGraphics = enableGraphics || DetectsRuntimeImports(sourcePath);
        var parse = Parser.Parse(source);
        parseMs = phase.ElapsedMilliseconds;
        phase.Restart();
        if (parse.Diagnostics.Count > 0)
        {
            PrintDiagnostics(parse.Diagnostics, source, sourcePath);
            return 1;
        }
        var runtimeImports = GetRuntimeImportFlags(sourcePath);
        var sema = new SemanticAnalyzer(new SemanticAnalyzerOptions(runtimeImports.graphics, runtimeImports.audio)).Analyze(parse.CompilationUnit);
        semaMs = phase.ElapsedMilliseconds;
        phase.Restart();
        if (sema.Diagnostics.Count > 0)
        {
            PrintDiagnostics(sema.Diagnostics, source, sourcePath);
            return 1;
        }

        if (!ContainsTopLevelFunction(parse.CompilationUnit, "tick"))
        {
            Console.Error.WriteLine("error: tick hot-swap mode requires a top-level `function tick()`.");
            return 1;
        }

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        layoutMs = phase.ElapsedMilliseconds;
        phase.Restart();
        var options = new CodeGenerationOptions(
            ModuleName: moduleName,
            IncludeTests: false,
            EmitTestHarness: false,
            HeadlessGraphics: !usesGraphics,
            AllowReachabilityFallback: true);

        using var generator = CodeGeneratorFactory.Create(BackendType.Cranelift, moduleName);
        var result = generator.Generate(parse.CompilationUnit, sema, layout, options);
        lowerMs = phase.ElapsedMilliseconds;
        phase.Restart();
        if (result.Diagnostics.Count > 0)
        {
            PrintDiagnostics(result.Diagnostics, source, sourcePath);
            Console.WriteLine(result.Ir);
            return 1;
        }

        var hotDll = activeDll is null
            ? swapDllA
            : (string.Equals(activeDll, swapDllA, StringComparison.OrdinalIgnoreCase) ? swapDllB : swapDllA);
        File.WriteAllText(hotClifPath, result.Ir);
        clifWriteMs = phase.ElapsedMilliseconds;
        phase.Restart();

        try
        {
            var aotExit = RunCraneliftAot(aotTool, hotClifPath, hotObjPath, moduleName, optLevel, out var spawnFallback, out var compileFallback);
            aotSpawnMs = spawnFallback ?? 0;
            aotCompileMs = compileFallback;
            if (aotExit != 0)
            {
                return aotExit;
            }
            phase.Restart();

            if (!TryCreateHotStatePlan(sourcePath, layout, moduleName, new[] { $"{moduleName}__main", $"{moduleName}__tick" }, excludeSpriteFields: false, out var plan))
            {
                return 1;
            }
            planMs = phase.ElapsedMilliseconds;
            phase.Restart();

            var linkArgs = BuildClangArgsForObject(hotObjPath, hotDll, isTest: false, optLevel, enableLto, usesGraphics, graphicsLibPath, entryName: $"{moduleName}__main", isDll: true, windowsDefFilePath: plan.DefPath);
            if (OperatingSystem.IsWindows())
            {
                // Hot-swap speed: skip expensive pruning/dedup; we don't care about DLL size for dev.
                linkArgs += " -Wl,/OPT:NOREF -Wl,/OPT:NOICF -Wl,/DEBUG:NONE";
            }
            var linkExit = RunProcess(clang, linkArgs, suppressOutput: true);
            linkMs = phase.ElapsedMilliseconds;
            phase.Restart();
            if (linkExit != 0)
            {
                return linkExit;
            }

            var dllDir = Path.GetDirectoryName(hotDll);
            if (usesGraphics && !string.IsNullOrEmpty(dllDir))
            {
                CopyGraphicsRuntimeDependencies(dllDir, graphicsLibPath);
            }

            activeDll = hotDll;

            if (startRunner)
            {
                var entry = $"{moduleName}__main";
                var runnerArgs = $"\"{hotDll}\" {entry} --state-map \"{plan.MapPath}\" --swap-file \"{swapFile}\" --fps {fps}";

                var dataFile = FindDataBindingJson(sourcePath, repoRoot);
                if (!string.IsNullOrEmpty(dataFile))
                {
                    runnerArgs += $" --data-bind \"{dataFile}\" \"{plan.StructMetaPath}\"";
                    Console.WriteLine($"Data binding: {dataFile}");
                }

                var psi = new ProcessStartInfo
                {
                    FileName = runnerPath,
                    Arguments = runnerArgs,
                    UseShellExecute = false,
                    WorkingDirectory = repoRoot
                };
                psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = repoRoot;
                var spawnSw = Stopwatch.StartNew();
                runner = Process.Start(psi);
                spawnSw.Stop();
                runnerSpawnMs = spawnSw.ElapsedMilliseconds;
                if (runner is null)
                {
                    Console.Error.WriteLine("error: failed to start runner.");
                    return 1;
                }
                swTotal.Stop();
                timingLine =
                    $"HOTRELOAD phases(ms): read={readMs} parse={parseMs} sema={semaMs} layout={layoutMs} lower={lowerMs} clif={clifWriteMs} aotSpawn={aotSpawnMs} aotCompile={aotCompileMs} plan={planMs} link={linkMs} swapWrite=0 runnerSpawn={runnerSpawnMs} total={swTotal.ElapsedMilliseconds}";
                return 0;
            }

            var swapText = hotDll + "\n" + plan.MapPath;
            File.WriteAllText(swapFile, swapText, Encoding.ASCII);
            swapWriteMs = phase.ElapsedMilliseconds;
            swTotal.Stop();
            timingLine =
                $"HOTRELOAD phases(ms): read={readMs} parse={parseMs} sema={semaMs} layout={layoutMs} lower={lowerMs} clif={clifWriteMs} aotSpawn={aotSpawnMs} aotCompile={aotCompileMs} plan={planMs} link={linkMs} swapWrite={swapWriteMs} total={swTotal.ElapsedMilliseconds}";
            return 0;
        }
        finally
        {
            try
            {
                if (File.Exists(hotClifPath))
                {
                    File.Delete(hotClifPath);
                }
                if (File.Exists(hotObjPath))
                {
                    File.Delete(hotObjPath);
                }
            }
            catch
            {
                // Best-effort cleanup.
            }
        }
    }

    var initial = BuildAndSwap(startRunner: true, out var initialTimingLine);
    if (initial == 0)
    {
        Console.Error.WriteLine(initialTimingLine);
    }
    else
    {
        Console.Error.WriteLine("warning: initial build failed; waiting for changes.");
    }

    var dir = Path.GetDirectoryName(sourcePath) ?? Directory.GetCurrentDirectory();
    var fileName = Path.GetFileName(sourcePath);
    using var watcher = new FileSystemWatcher(dir, fileName)
    {
        NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.Size | NotifyFilters.FileName
    };

    var debounce = TimeSpan.FromMilliseconds(75);
    var lastChange = DateTime.UtcNow;
    using var changeSignal = new ManualResetEventSlim(false);
    void OnChange(object? _, FileSystemEventArgs __)
    {
        lastChange = DateTime.UtcNow;
        changeSignal.Set();
    }

    watcher.Changed += OnChange;
    watcher.Created += OnChange;
    watcher.Renamed += OnChange;
    watcher.EnableRaisingEvents = true;

    while (!cts.IsCancellationRequested)
    {
        if (runner is not null && runner.HasExited)
        {
            if (runner.ExitCode == 0)
            {
                return 0;
            }

            Console.Error.WriteLine($"error: runner exited with code {runner.ExitCode}");
            return 1;
        }

        try
        {
            changeSignal.Wait(TimeSpan.FromMilliseconds(50), cts.Token);
        }
        catch (OperationCanceledException)
        {
            break;
        }

        if (!changeSignal.IsSet)
        {
            continue;
        }

        while (DateTime.UtcNow - lastChange < debounce)
        {
            Thread.Sleep(10);
        }
        changeSignal.Reset();

        var exit = BuildAndSwap(startRunner: runner is null, out var timingLine);
        if (exit == 0)
        {
            Console.Error.WriteLine(timingLine);
        }
    }

    if (runner is not null && !runner.HasExited)
    {
        runner.Kill(entireProcessTree: true);
        runner.WaitForExit();
    }

    return 0;
}

static bool TryCreateHotStatePlan(string sourcePath, LayoutPlan layout, string moduleName, IReadOnlyList<string> exportedFunctions, bool excludeSpriteFields, out HotStatePlan plan)
{
    plan = new HotStatePlan(string.Empty, string.Empty, string.Empty, string.Empty, string.Empty);
    if (exportedFunctions.Count == 0)
    {
        Console.Error.WriteLine("error: --hot-state requires at least one exported function.");
        return false;
    }

    var state = layout.Globals.FirstOrDefault(g => string.Equals(g.Name, "state", StringComparison.Ordinal));
    if (state is null)
    {
        Console.Error.WriteLine("error: --hot-state requires a global named 'state'.");
        return false;
    }

    var entries = state.Fields
        .Where(f => !excludeSpriteFields || !f.Name.StartsWith("state__sprites__", StringComparison.Ordinal))
        .ToArray();

    if (entries.Length == 0)
    {
        Console.Error.WriteLine("error: --hot-state: state has no persisted fields (all fields were filtered).");
        return false;
    }

    var totalBytes = 0;
    ulong hash = 14695981039346656037UL; // FNV-1a 64 offset basis
    foreach (var entry in entries)
    {
        totalBytes += entry.Size;
        var nameBytes = Encoding.UTF8.GetBytes(entry.Name);
        foreach (var b in nameBytes)
        {
            hash ^= b;
            hash *= 1099511628211UL;
        }
        hash ^= 0;
        hash *= 1099511628211UL;

        unchecked
        {
            var u = (uint)entry.Size;
            for (var i = 0; i < 4; i++)
            {
                hash ^= (byte)(u & 0xFF);
                hash *= 1099511628211UL;
                u >>= 8;
            }
        }
    }

    var repoRoot = FindRepoRoot() ?? Directory.GetCurrentDirectory();
    var hotDir = Path.Combine(repoRoot, "build", "hotstate");
    Directory.CreateDirectory(hotDir);

    var baseName = Path.GetFileNameWithoutExtension(sourcePath);
    var hashHex = hash.ToString("x16");
    var mapPath = Path.Combine(hotDir, $"{baseName}.{moduleName}.{hashHex}.state-map.txt");
    var snapshotPath = Path.Combine(hotDir, $"{baseName}.{moduleName}.{hashHex}.state-snap.bin");
    var defPath = Path.Combine(hotDir, $"{baseName}.{moduleName}.{hashHex}.exports.def");
    var hotExitPath = Path.Combine(hotDir, $"{baseName}.{moduleName}.hot-exit");

    var map = new StringBuilder();
    map.Append("STASIS_STATE_MAP 1\n");
    map.Append($"hash={hashHex} count={entries.Length} bytes={totalBytes}\n");
    foreach (var entry in entries)
    {
        map.Append(entry.Name);
        map.Append(' ');
        map.Append(entry.Size);
        map.Append('\n');
    }
    File.WriteAllText(mapPath, map.ToString(), Encoding.ASCII);

    var def = new StringBuilder();
    def.Append("EXPORTS\n");
    foreach (var fn in exportedFunctions)
    {
        def.Append("  ");
        def.Append(fn);
        def.Append('\n');
    }
    foreach (var entry in entries)
    {
        def.Append("  ");
        def.Append(entry.Name);
        def.Append(" DATA\n");
    }
    File.WriteAllText(defPath, def.ToString(), Encoding.ASCII);

    // Emit struct metadata JSON for data binding
    var structMetaPath = Path.Combine(hotDir, $"{baseName}.{moduleName}.struct-meta.json");
    EmitStructMetadataJson(structMetaPath, state, entries);

    plan = new HotStatePlan(mapPath, snapshotPath, defPath, hotExitPath, structMetaPath);
    return true;
}

static void EmitStructMetadataJson(string path, GlobalLayout state, IReadOnlyList<FieldLayout> entries)
{
    var fields = new List<StructFieldMetadata>(entries.Count);
    var globalPrefix = state.Name + "__";
    foreach (var entry in entries)
    {
        // Prefer non-lowered, source-style JSON paths (e.g. balance.speed) while keeping the
        // lowered symbol name for DLL lookup (e.g. state__balance__speed).
        //
        // Notes:
        // - For the hot-state workflow, we're binding to the global named `state`, so we drop
        //   the `state__` prefix from JSON paths.
        // - `__` encodes struct nesting in lowered symbol names; JSON paths use `.`.
        var jsonPath = entry.Name;
        if (jsonPath.StartsWith(globalPrefix, StringComparison.Ordinal))
        {
            jsonPath = jsonPath.Substring(globalPrefix.Length);
        }
        jsonPath = jsonPath.Replace("__", ".", StringComparison.Ordinal);

        fields.Add(new StructFieldMetadata
        {
            Name = entry.Name,        // Flattened symbol name (for DLL lookup)
            JsonPath = jsonPath,      // Preferred JSON path (non-lowered)
            Offset = entry.Offset,
            Size = entry.Size,
            Type = entry.Type.ToString().ToLowerInvariant(),
            ArrayCount = entry.ArrayCount
        });
    }

    var metadata = new StructMetadata
    {
        Version = 1,
        GlobalName = state.Name,
        TotalSize = state.Size,
        Fields = fields
    };

    var json = JsonSerializer.Serialize(metadata, StasisCliJson.Indented.StructMetadata);
    File.WriteAllText(path, json, Encoding.UTF8);
}

static int WatchFile(string path, string mode, bool includeTests, string moduleName, bool emitIrOnly, string? outputPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, BackendType backend, bool useCraneliftRunner, bool enableHotState, int tickHostFps, string? llvmTargetTriple)
{
    var fullPath = Path.GetFullPath(path);
    var dir = Path.GetDirectoryName(fullPath) ?? Directory.GetCurrentDirectory();
    var fileName = Path.GetFileName(fullPath);
    var debounce = TimeSpan.FromMilliseconds(75);
    var lastChange = DateTime.UtcNow;
    var pendingRestart = false;
    var restartRequestedAt = DateTime.MinValue;
    var hotExitPath = enableHotState && mode == "run" ? GetHotExitFilePath(fullPath, moduleName) : null;

    if (mode == "run" &&
        backend == BackendType.Cranelift &&
        useCraneliftRunner &&
        OperatingSystem.IsWindows() &&
        DetectsTickUsage(File.ReadAllText(fullPath)))
    {
        return WatchCraneliftTickHotSwap(fullPath, moduleName, tickHostFps, optLevel, enableLto, enableGraphics, graphicsLibPath);
    }

    using var watcher = new FileSystemWatcher(dir, fileName)
    {
        NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.Size | NotifyFilters.FileName
    };

    using var changeSignal = new ManualResetEventSlim(false);
    void OnChange(object? _, FileSystemEventArgs __)
    {
        lastChange = DateTime.UtcNow;
        changeSignal.Set();
    }

    watcher.Changed += OnChange;
    watcher.Created += OnChange;
    watcher.Renamed += OnChange;
    watcher.EnableRaisingEvents = true;

    using var cts = new CancellationTokenSource();
    Console.CancelKeyPress += (_, e) =>
    {
        e.Cancel = true;
        cts.Cancel();
        changeSignal.Set();
    };

    var childArgs = Environment.GetCommandLineArgs()
        .Skip(1)
        .Where(arg => !string.Equals(arg, "--watch", StringComparison.OrdinalIgnoreCase))
        .Select(QuoteArg)
        .ToArray();

    var exePath = Environment.ProcessPath ?? Process.GetCurrentProcess().MainModule?.FileName;
    if (string.IsNullOrEmpty(exePath))
    {
        Console.Error.WriteLine("error: unable to resolve stasis executable for --watch.");
        return 1;
    }
    if (Path.GetFileNameWithoutExtension(exePath).Equals("dotnet", StringComparison.OrdinalIgnoreCase) &&
        childArgs.Length > 0 && childArgs[0].Equals("run", StringComparison.OrdinalIgnoreCase))
    {
        Console.Error.WriteLine("error: --watch requires running the built CLI (not `dotnet run`).");
        return 1;
    }

    Process? child = null;
    if (mode == "run")
    {
        child = StartWatchChild(exePath, childArgs);
    }
    else
    {
        _ = ProcessFile(fullPath, mode, includeTests, moduleName, emitIrOnly, outputPath, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, tickHostFps, llvmTargetTriple, useCraneliftRunner: useCraneliftRunner, enableHotState: enableHotState);
    }

    while (!cts.IsCancellationRequested)
    {
        try
        {
            if (enableHotState && mode == "run")
            {
                changeSignal.Wait(TimeSpan.FromMilliseconds(100), cts.Token);
            }
            else
            {
                changeSignal.Wait(cts.Token);
            }
        }
        catch (OperationCanceledException)
        {
            break;
        }

        if (changeSignal.IsSet)
        {
            while (DateTime.UtcNow - lastChange < debounce)
            {
                Thread.Sleep(10);
            }
            changeSignal.Reset();

            if (mode == "run")
            {
                if (enableHotState)
                {
                    pendingRestart = true;
                    restartRequestedAt = DateTime.UtcNow;
                    if (!string.IsNullOrEmpty(hotExitPath) && child is not null && !child.HasExited)
                    {
                        try
                        {
                            Directory.CreateDirectory(Path.GetDirectoryName(hotExitPath)!);
                            File.WriteAllText(hotExitPath, "1", Encoding.ASCII);
                        }
                        catch (Exception ex)
                        {
                            Console.Error.WriteLine($"warning: failed to signal hot-state exit: {ex.Message}");
                        }
                    }
                }
                else
                {
                    if (child is not null && !child.HasExited)
                    {
                        child.Kill(entireProcessTree: true);
                        child.WaitForExit();
                    }
                    child = StartWatchChild(exePath, childArgs);
                }
            }
            else
            {
                _ = ProcessFile(fullPath, mode, includeTests, moduleName, emitIrOnly, outputPath, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, tickHostFps, llvmTargetTriple, useCraneliftRunner: useCraneliftRunner, enableHotState: enableHotState);
            }
        }

        if (mode == "run" && enableHotState && pendingRestart && (child is null || child.HasExited))
        {
            child = StartWatchChild(exePath, childArgs);
            pendingRestart = false;
            if (restartRequestedAt != DateTime.MinValue)
            {
                var latency = (DateTime.UtcNow - restartRequestedAt).TotalMilliseconds;
                Console.Error.WriteLine($"HOTRELOAD restart latency={latency:0}ms");
            }
        }
    }

    if (child is not null && !child.HasExited)
    {
        child.Kill(entireProcessTree: true);
        child.WaitForExit();
    }

    return 0;
}

static Process StartWatchChild(string exePath, string[] args)
{
    var psi = new ProcessStartInfo
    {
        FileName = exePath,
        Arguments = string.Join(" ", args),
        UseShellExecute = false
    };
    psi.EnvironmentVariables["STASIS_ASSET_ROOT"] = Directory.GetCurrentDirectory();
    return Process.Start(psi)!;
}

static string QuoteArg(string arg) =>
    arg.Contains(' ') ? $"\"{arg}\"" : arg;

static void WriteIrOutput(string ir, string? outputPath)
{
    if (!string.IsNullOrWhiteSpace(outputPath))
    {
        var outDir = Path.GetDirectoryName(outputPath);
        if (!string.IsNullOrWhiteSpace(outDir))
        {
            Directory.CreateDirectory(outDir);
        }
        File.WriteAllText(outputPath, ir);
        return;
    }

    Console.WriteLine(ir);
}

static void PrintUsage()
{
    Console.WriteLine("Usage:");
    Console.WriteLine("  stasisc run <file> [--fps <1..240>] [--module <name>] [--emit-ir] [--out <path>]");
    Console.WriteLine("  stasisc release <file> [--out <path>] [--module <name>]");
    Console.WriteLine();
    Console.WriteLine("Other commands:");
    Console.WriteLine("  stasisc test [<file>|--all] [--watch] [--module <name>] [--emit-ir] [--backend <llvm|cranelift>]");
    Console.WriteLine("  stasisc build <file> [--module <name>] [--with-tests] [--out <path>] [--opt-level <0|1|2|3|s|z>] [--lto|--no-lto] [--backend <llvm|cranelift>] [--graphics] [--graphics-lib <path>]");
    Console.WriteLine("  stasisc format <file>");
    Console.WriteLine();
    Console.WriteLine("Defaults: execute via lli if available, else clang. Use --emit-ir to only write IR to stdout (or --out to write to a file). With no path (or --all), 'test' runs every .stasis file under the working directory. Build/release require clang in PATH. 'release' defaults to -O3 with LTO.");
    Console.WriteLine("Run: use --watch for a dev loop (auto-rebuild + tick hot-swap + phase timings) with state preserved between swaps and no re-running main().");
    Console.WriteLine("Hot state: use --hot-state (Cranelift run only) to restore and save the global 'state' across process runs (restart-based experiments).");
    Console.WriteLine("Graphics: enabled automatically when graphics APIs are used; use --graphics to force it on. Use --graphics-lib to override library path.");
    Console.WriteLine("Backend: use --backend to select code generation backend. Defaults to 'cranelift' for run/test/build (when available) and 'llvm' for release; Cranelift is experimental.");
    Console.WriteLine("LLVM: pass --llvm-target <triple> to set the LLVM module target triple (useful for cross-compiling emitted IR).");
    Console.WriteLine("Cranelift: run/test uses the native DLL runner when available (stasis_runner.exe). Set STASIS_CRANELIFT_RUNNER_EXE to override, or pass --no-cranelift-runner to force EXE mode.");
    Console.WriteLine("Cache: set STASIS_DISABLE_ARTIFACT_CACHE=1 to disable binary caching for Cranelift run/test.");
}

static int RunAllTestsInDirectoryParallel(string[] files, bool includeTests, string moduleName, bool emitIrOnly, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, BackendType backend, bool useCraneliftRunner, bool allowReachabilityFallback, bool enableTestCache, string? llvmTargetTriple, bool useLowerLock = true, int lowerDegree = 1) =>
    RunAllTestsInDirectoryParallelAsync(files, includeTests, moduleName, emitIrOnly, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, useCraneliftRunner, allowReachabilityFallback, enableTestCache, llvmTargetTriple, useLowerLock, lowerDegree).GetAwaiter().GetResult();

static async Task<int> RunAllTestsInDirectoryParallelAsync(string[] files, bool includeTests, string moduleName, bool emitIrOnly, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, BackendType backend, bool useCraneliftRunner, bool allowReachabilityFallback, bool enableTestCache, string? llvmTargetTriple, bool useLowerLock, int lowerDegree)
{
    var prepChannel = Channel.CreateUnbounded<PreparedForLower>(new UnboundedChannelOptions { SingleReader = true, SingleWriter = false });
    var resultChannel = Channel.CreateUnbounded<CompileResult>(new UnboundedChannelOptions { SingleReader = true, SingleWriter = false });
    var concurrency = Math.Max(1, Environment.ProcessorCount);
    var gate = new SemaphoreSlim(concurrency);

    var producers = files.Select(file => Task.Run(async () =>
    {
        await gate.WaitAsync();
        try
        {
            var prep = PrepareForLower(file, includeTests, moduleName, emitIrOnly, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, useCraneliftRunner, enableTestCache);
            if (prep.Prepared is not null)
            {
                await prepChannel.Writer.WriteAsync(prep.Prepared);
            }

            if (prep.Result is not null)
            {
                await resultChannel.Writer.WriteAsync(prep.Result);
            }
        }
        finally
        {
            gate.Release();
        }
    })).ToArray();

    var lowerWorkers = Enumerable.Range(0, Math.Max(1, lowerDegree)).Select(_ => Task.Run(async () =>
    {
        await foreach (var item in prepChannel.Reader.ReadAllAsync())
        {
            var effectiveGraphics = enableGraphics || item.UsesGraphics;
            var result = LowerPrepared(item, includeTests, moduleName, emitIrOnly, optLevel, enableLto, effectiveGraphics, graphicsLibPath, backend, useCraneliftRunner, useLowerLock, allowReachabilityFallback, enableTestCache, llvmTargetTriple);
            await resultChannel.Writer.WriteAsync(result);
        }
    })).ToArray();

    var exitCode = 0;
    var consumer = Task.Run(async () =>
    {
        await foreach (var result in resultChannel.Reader.ReadAllAsync())
        {
            exitCode = Math.Max(exitCode, ConsumeCompileResult(result, emitIrOnly, optLevel, enableLto, graphicsLibPath, moduleName, useCraneliftRunner));
        }
    });

    await Task.WhenAll(producers);
    prepChannel.Writer.Complete();
    await Task.WhenAll(lowerWorkers);
    resultChannel.Writer.Complete();
    await consumer;
    return exitCode;
}

static PrepareResult PrepareForLower(string path, bool includeTests, string moduleName, bool emitIrOnly, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, BackendType backend, bool useCraneliftRunner, bool enableTestCache)
{
    var stopwatch = Stopwatch.StartNew();
    var diagnostics = new List<Diagnostic>();
    try
    {
        var source = LoadSourceWithImports(path, out var importDiagnostics, out var importSource);
        if (importDiagnostics.Count > 0)
        {
            PrintDiagnostics(importDiagnostics, importSource, path);
            return new PrepareResult(null, new CompileResult(path, importSource, false, false, backend, null, null, importDiagnostics, emitIrOnly, stopwatch.ElapsedMilliseconds, false));
        }
        // Tests should be deterministic and avoid IO-heavy dependencies, but the Cranelift backend still relies on
        // runtime hooks for some builtins (e.g., get_time_ms), so keep auto-detection there.
        var usesGraphics = includeTests && backend == BackendType.Llvm ? false : DetectsRuntimeImports(path);
        var effectiveGraphics = enableGraphics || usesGraphics;
        TestCacheLocation? testCacheLocation = null;
        var craneliftTargetTriple = backend == BackendType.Cranelift ? GetCraneliftTargetTriple() : null;
        var compilerCacheSalt = GetCompilerCacheSalt();
        if (enableTestCache)
        {
            testCacheLocation = CreateTestCacheLocation(path, source, backend, moduleName, includeTests, emitIrOnly, optLevel, enableLto, graphicsLibPath, useCraneliftRunner, effectiveGraphics, craneliftTargetTriple, compilerCacheSalt);
            var cachedResult = TryLoadTestCache(testCacheLocation, source, backend, moduleName, includeTests, emitIrOnly, optLevel, enableLto, graphicsLibPath, useCraneliftRunner, effectiveGraphics, craneliftTargetTriple, compilerCacheSalt);
            if (cachedResult is not null)
            {
                return new PrepareResult(null, cachedResult);
            }
        }
        var parse = Parser.Parse(source);
        diagnostics.AddRange(parse.Diagnostics);
        var hasTests = parse.CompilationUnit.Declarations.OfType<TestDeclarationSyntax>().Any();

        if (parse.Diagnostics.Count > 0 || (!hasTests && !emitIrOnly))
        {
            return new PrepareResult(null, new CompileResult(path, source, hasTests, usesGraphics, backend, null, null, diagnostics, emitIrOnly, stopwatch.ElapsedMilliseconds, false));
        }

        var runtimeImports = GetRuntimeImportFlags(path);
        var sema = new SemanticAnalyzer(new SemanticAnalyzerOptions(runtimeImports.graphics, runtimeImports.audio)).Analyze(parse.CompilationUnit);
        diagnostics.AddRange(sema.Diagnostics);
        if (sema.Diagnostics.Count > 0)
        {
            return new PrepareResult(null, new CompileResult(path, source, hasTests, usesGraphics, backend, null, null, diagnostics, emitIrOnly, stopwatch.ElapsedMilliseconds, false));
        }

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        stopwatch.Stop();
        return new PrepareResult(new PreparedForLower(path, source, parse.CompilationUnit, sema, layout, hasTests, usesGraphics, stopwatch.ElapsedMilliseconds, testCacheLocation), null);
    }
    finally
    {
        stopwatch.Stop();
    }
}

static string LoadSourceWithImports(string path, out List<Diagnostic> importDiagnostics, out string sourceForDiagnostics)
{
    var original = File.ReadAllText(path);
    var diagnostics = new List<Diagnostic>();
    var result = SourceImporter.ExpandImports(path, original, diagnostics);
    importDiagnostics = diagnostics;
    sourceForDiagnostics = result.OriginalSource;
    return result.ExpandedSource;
}

const int TestCacheVersion = 1;

static string GetTestCacheDirectory()
{
    var currentDirectory = Directory.GetCurrentDirectory();
    return Path.Combine(currentDirectory, ".stasis_cache", "test");
}

static TestCacheLocation CreateTestCacheLocation(string path, string source, BackendType backend, string moduleName, bool includeTests, bool emitIrOnly, string? optLevel, bool enableLto, string? graphicsLibPath, bool useCraneliftRunner, bool usesGraphics, string? craneliftTargetTriple, string compilerCacheSalt)
{
    var cacheDirectory = GetTestCacheDirectory();
    Directory.CreateDirectory(cacheDirectory);
    var cacheKey = ComputeTestCacheKey(path, source, backend, moduleName, includeTests, emitIrOnly, optLevel, enableLto, graphicsLibPath, useCraneliftRunner, usesGraphics, craneliftTargetTriple, compilerCacheSalt);
    var extension = backend == BackendType.Cranelift ? "clif" : "ll";
    var artifactPath = Path.Combine(cacheDirectory, $"{cacheKey}.{extension}");
    var entryPath = Path.Combine(cacheDirectory, $"{cacheKey}.json");
    var sourceHash = ComputeSha256Hex(source);
    return new TestCacheLocation(cacheKey, artifactPath, entryPath, sourceHash);
}

static CompileResult? TryLoadTestCache(TestCacheLocation cacheLocation, string source, BackendType backend, string moduleName, bool includeTests, bool emitIrOnly, string? optLevel, bool enableLto, string? graphicsLibPath, bool useCraneliftRunner, bool usesGraphics, string? craneliftTargetTriple, string compilerCacheSalt)
{
    if (!File.Exists(cacheLocation.EntryPath))
    {
        return null;
    }

    TestCacheEntry? entry;
    try
    {
        var json = File.ReadAllText(cacheLocation.EntryPath);
        entry = JsonSerializer.Deserialize(json, StasisCliJson.Default.TestCacheEntry);
    }
    catch
    {
        return null;
    }

    if (entry is null ||
        entry.Version != TestCacheVersion ||
        !string.Equals(entry.CacheKey, cacheLocation.CacheKey, StringComparison.Ordinal) ||
        !string.Equals(entry.SourceHash, cacheLocation.SourceHash, StringComparison.Ordinal) ||
        entry.Backend != backend ||
        !string.Equals(entry.ModuleName, moduleName, StringComparison.Ordinal) ||
        entry.IncludeTests != includeTests ||
        entry.EmitIrOnly != emitIrOnly ||
        entry.EnableLto != enableLto ||
        !string.Equals(entry.OptLevel, optLevel, StringComparison.Ordinal) ||
        !string.Equals(entry.GraphicsLibPath, graphicsLibPath, StringComparison.Ordinal) ||
        entry.UseCraneliftRunner != useCraneliftRunner ||
        !string.Equals(entry.CraneliftTargetTriple, craneliftTargetTriple, StringComparison.Ordinal) ||
        !string.Equals(entry.CompilerCacheSalt, compilerCacheSalt, StringComparison.Ordinal) ||
        entry.UsesGraphics != usesGraphics)
    {
        return null;
    }

    if (!File.Exists(entry.ArtifactPath))
    {
        return null;
    }

    return new CompileResult(entry.FilePath, source, entry.HasTests, entry.UsesGraphics, backend, entry.ArtifactPath, null, new List<Diagnostic>(), emitIrOnly, 0, true);
}

static void WriteTestCacheEntry(PreparedForLower prep, BackendType backend, string moduleName, bool includeTests, bool emitIrOnly, string? optLevel, bool enableLto, string? graphicsLibPath, bool useCraneliftRunner, bool usesGraphics, string? craneliftTargetTriple, string compilerCacheSalt)
{
    if (prep.TestCacheLocation is null)
    {
        return;
    }

    var cacheDirectory = Path.GetDirectoryName(prep.TestCacheLocation.EntryPath);
    if (!string.IsNullOrEmpty(cacheDirectory))
    {
        Directory.CreateDirectory(cacheDirectory);
    }

    var entry = new TestCacheEntry(
        TestCacheVersion,
        prep.TestCacheLocation.CacheKey,
        prep.FilePath,
        prep.TestCacheLocation.ArtifactPath,
        prep.TestCacheLocation.SourceHash,
        prep.HasTests,
        usesGraphics,
        backend,
        moduleName,
        includeTests,
        emitIrOnly,
        optLevel,
        enableLto,
        graphicsLibPath,
        useCraneliftRunner,
        craneliftTargetTriple,
        compilerCacheSalt);

    try
    {
        var json = JsonSerializer.Serialize(entry, StasisCliJson.Indented.TestCacheEntry);
        File.WriteAllText(prep.TestCacheLocation.EntryPath, json, Encoding.UTF8);
    }
    catch
    {
        // Native AOT disables reflection-based JSON serialization by default.
        // The test cache is an optional optimization, so ignore failures here and continue.
    }
}

static string ComputeTestCacheKey(string path, string source, BackendType backend, string moduleName, bool includeTests, bool emitIrOnly, string? optLevel, bool enableLto, string? graphicsLibPath, bool useCraneliftRunner, bool usesGraphics, string? craneliftTargetTriple, string compilerCacheSalt)
{
    var fullPath = Path.GetFullPath(path);
    var identity = new StringBuilder();
    identity.Append("version=").Append(TestCacheVersion).Append('\n');
    identity.Append("backend=").Append(backend).Append('\n');
    identity.Append("module=").Append(moduleName).Append('\n');
    identity.Append("includeTests=").Append(includeTests).Append('\n');
    identity.Append("emitIrOnly=").Append(emitIrOnly).Append('\n');
    identity.Append("optLevel=").Append(optLevel ?? string.Empty).Append('\n');
    identity.Append("enableLto=").Append(enableLto).Append('\n');
    identity.Append("graphicsLibPath=").Append(graphicsLibPath ?? string.Empty).Append('\n');
    identity.Append("useCraneliftRunner=").Append(useCraneliftRunner).Append('\n');
    identity.Append("craneliftTargetTriple=").Append(craneliftTargetTriple ?? string.Empty).Append('\n');
    identity.Append("compilerCacheSalt=").Append(compilerCacheSalt).Append('\n');
    identity.Append("usesGraphics=").Append(usesGraphics).Append('\n');
    identity.Append("path=").Append(fullPath).Append('\n');
    identity.Append("source=").Append(source);
    return ComputeSha256Hex(identity.ToString());
}

static string ComputeSha256Hex(string value)
{
    var bytes = Encoding.UTF8.GetBytes(value);
    using var sha256 = SHA256.Create();
    var hash = sha256.ComputeHash(bytes);
    var builder = new StringBuilder(hash.Length * 2);
    foreach (var hashByte in hash)
    {
        builder.Append(hashByte.ToString("x2"));
    }
    return builder.ToString();
}

static string? TryGetCachedExecutablePath(CompileResult result)
{
    if (!result.IsCacheArtifact || string.IsNullOrEmpty(result.ArtifactPath))
    {
        return null;
    }

    var cacheDirectory = Path.GetDirectoryName(result.ArtifactPath);
    var cacheFileName = Path.GetFileNameWithoutExtension(result.ArtifactPath);
    if (string.IsNullOrEmpty(cacheDirectory) || string.IsNullOrEmpty(cacheFileName))
    {
        return null;
    }

    var extension = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".exe" : string.Empty;
    return Path.Combine(cacheDirectory, cacheFileName + extension);
}

static string? TryGetCachedObjectPath(CompileResult result)
{
    if (result.Backend != BackendType.Cranelift || !result.IsCacheArtifact || string.IsNullOrEmpty(result.ArtifactPath))
    {
        return null;
    }

    var cacheDirectory = Path.GetDirectoryName(result.ArtifactPath);
    var cacheFileName = Path.GetFileNameWithoutExtension(result.ArtifactPath);
    if (string.IsNullOrEmpty(cacheDirectory) || string.IsNullOrEmpty(cacheFileName))
    {
        return null;
    }

    return Path.Combine(cacheDirectory, cacheFileName + GetObjectFileExtension());
}

static string? TryGetCachedRunnerDllPath(CompileResult result)
{
    if (result.Backend != BackendType.Cranelift || !result.IsCacheArtifact || string.IsNullOrEmpty(result.ArtifactPath))
    {
        return null;
    }

    var cacheDirectory = Path.GetDirectoryName(result.ArtifactPath);
    var cacheFileName = Path.GetFileNameWithoutExtension(result.ArtifactPath);
    if (string.IsNullOrEmpty(cacheDirectory) || string.IsNullOrEmpty(cacheFileName))
    {
        return null;
    }

    var extension = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".dll" : ".so";
    return Path.Combine(cacheDirectory, cacheFileName + extension);
}

static int RunCachedExecutable(string mode, string executablePath, bool enableGraphics, string? graphicsLibPath)
{
    if (enableGraphics)
    {
        var exeDir = Path.GetDirectoryName(executablePath);
        if (!string.IsNullOrEmpty(exeDir))
        {
            CopyGraphicsRuntimeDependencies(exeDir, graphicsLibPath);
        }
    }

    return RunProcess(executablePath, string.Empty, psi =>
    {
        if (enableGraphics)
        {
            var runTest = Environment.GetEnvironmentVariable("STASIS_RUN_RENDER_TEST");
            if (string.IsNullOrEmpty(runTest) || runTest == "0")
            {
                if (Environment.GetEnvironmentVariable("STASIS_SKIP_RENDER_TEST") is null)
                {
                    psi.Environment["STASIS_SKIP_RENDER_TEST"] = "1";
                }
            }
        }
    });
}

static int RunCachedRunnerDll(string dllPath, string entryName, bool enableGraphics, string? graphicsLibPath, DataBindingPlan? dataBindingPlan, int? tickHostFps = null)
{
    if (!TryFindCraneliftRunner(out var runnerPath))
    {
        Console.Error.WriteLine("error: stasis_runner not found. Build it in runtime/ and set STASIS_CRANELIFT_RUNNER_EXE if needed.");
        return 1;
    }

    if (enableGraphics)
    {
        var dllDir = Path.GetDirectoryName(dllPath);
        if (!string.IsNullOrEmpty(dllDir))
        {
            CopyGraphicsRuntimeDependencies(dllDir, graphicsLibPath);
        }
    }

    if (tickHostFps is null && dataBindingPlan is null && UseCraneliftRunnerServer())
    {
        var runner = GetCraneliftRunnerServer(runnerPath);
        return runner.Run(dllPath, entryName, out _);
    }

    var args = $"\"{dllPath}\" {entryName}";
    if (dataBindingPlan is not null)
    {
        args += $" --data-bind \"{dataBindingPlan.JsonPath}\" \"{dataBindingPlan.StructMetaPath}\"";
        Console.WriteLine($"Data binding: {dataBindingPlan.JsonPath}");
    }
    if (tickHostFps is not null)
    {
        args += $" --fps {tickHostFps.Value}";
    }
    return RunProcess(runnerPath, args);
}

static int EnsureCraneliftCachedExecutable(string clifPath, string objPath, string exePath, string moduleName, string mode, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath)
{
    if (!TryFindCraneliftAot(out var aotTool))
    {
        Console.Error.WriteLine("error: stasis-cranelift-aot not found. Build it with `cargo build -p stasis-cranelift-aot` (in tools/cranelift-aot) or set STASIS_CRANELIFT_AOT.");
        return 1;
    }

    if (!TryFindTool("clang", out var clang))
    {
        Console.Error.WriteLine("error: run requires clang in PATH.");
        return 1;
    }

    var entryBase = mode == "test" ? "run_tests" : "main";
    var entryName = $"{moduleName}__{entryBase}";

    Directory.CreateDirectory(Path.GetDirectoryName(objPath)!);
    Directory.CreateDirectory(Path.GetDirectoryName(exePath)!);

    var tempObj = objPath + ".tmp";
    var tempExe = exePath + ".tmp";
    try
    {
        if (!File.Exists(objPath))
        {
            var aotExit = RunCraneliftAot(aotTool, clifPath, tempObj, moduleName, optLevel, out _, out _);
            if (aotExit != 0)
            {
                return aotExit;
            }

            if (File.Exists(objPath))
            {
                File.Delete(objPath);
            }
            File.Move(tempObj, objPath);
        }

        if (!File.Exists(exePath))
        {
            var args = BuildClangArgsForObject(objPath, tempExe, mode == "test", optLevel, enableLto, enableGraphics, graphicsLibPath, entryName: entryName);
            var exit = RunProcess(clang, args, suppressOutput: true);
            if (exit != 0)
            {
                return exit;
            }

            if (File.Exists(exePath))
            {
                File.Delete(exePath);
            }
            File.Move(tempExe, exePath);
        }

        return 0;
    }
    finally
    {
        if (File.Exists(tempObj))
        {
            File.Delete(tempObj);
        }
        if (File.Exists(tempExe))
        {
            File.Delete(tempExe);
        }
    }
}

static int EnsureCraneliftCachedRunnerDll(string clifPath, string objPath, string dllPath, string moduleName, string mode, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, string? windowsDefFilePath, IReadOnlyList<string>? exports = null)
{
    if (!TryFindCraneliftAot(out var aotTool))
    {
        Console.Error.WriteLine("error: stasis-cranelift-aot not found. Build it with `cargo build -p stasis-cranelift-aot` (in tools/cranelift-aot) or set STASIS_CRANELIFT_AOT.");
        return 1;
    }

    if (!TryFindTool("clang", out var clang))
    {
        Console.Error.WriteLine("error: run requires clang in PATH.");
        return 1;
    }

    var entryBase = mode == "test" ? "run_tests" : "main";
    var entryName = $"{moduleName}__{entryBase}";

    Directory.CreateDirectory(Path.GetDirectoryName(objPath)!);
    Directory.CreateDirectory(Path.GetDirectoryName(dllPath)!);

    var tempObj = objPath + ".tmp";
    var tempDll = dllPath + ".tmp";
    try
    {
        if (!File.Exists(objPath))
        {
            var aotExit = RunCraneliftAot(aotTool, clifPath, tempObj, moduleName, optLevel, out _, out _);
            if (aotExit != 0)
            {
                return aotExit;
            }

            if (File.Exists(objPath))
            {
                File.Delete(objPath);
            }
            File.Move(tempObj, objPath);
        }

        if (!File.Exists(dllPath))
        {
            var dllExports = exports is { Count: > 0 } ? exports : new[] { entryName };
            var args = BuildClangArgsForObject(objPath, tempDll, mode == "test", optLevel, enableLto, enableGraphics, graphicsLibPath, entryName: entryName, isDll: true, windowsDefFilePath: windowsDefFilePath, windowsExports: dllExports);
            var exit = RunProcess(clang, args, suppressOutput: true);
            if (exit != 0)
            {
                return exit;
            }

            if (File.Exists(dllPath))
            {
                File.Delete(dllPath);
            }
            File.Move(tempDll, dllPath);
        }

        return 0;
    }
    finally
    {
        if (File.Exists(tempObj))
        {
            File.Delete(tempObj);
        }
        if (File.Exists(tempDll))
        {
            File.Delete(tempDll);
        }
    }
}

 
static CompileResult LowerPrepared(PreparedForLower prep, bool includeTests, string moduleName, bool emitIrOnly, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, BackendType backend, bool useCraneliftRunner, bool useLowerLock, bool allowReachabilityFallback, bool enableTestCache, string? llvmTargetTriple)
{
    var stopwatch = Stopwatch.StartNew();
    var diagnostics = new List<Diagnostic>();
    string? tempArtifact = null;
    string? irForOutput = null;
    var isCacheArtifact = false;
    var craneliftTargetTriple = backend == BackendType.Cranelift ? GetCraneliftTargetTriple() : null;
    var compilerCacheSalt = GetCompilerCacheSalt();

    try
    {
        if (backend == BackendType.Cranelift)
        {
            var options = new CodeGenerationOptions(
                ModuleName: moduleName,
                IncludeTests: includeTests,
                EmitTestHarness: includeTests,
                HeadlessGraphics: !enableGraphics,
                AllowReachabilityFallback: allowReachabilityFallback);

            using var generator = CodeGeneratorFactory.Create(backend, moduleName);
            var result = generator.Generate(prep.CompilationUnit, prep.Sema, prep.Layout, options);
            diagnostics.AddRange(result.Diagnostics);
            irForOutput = emitIrOnly || result.Diagnostics.Count > 0 ? result.Ir : null;

            if (emitIrOnly || result.Diagnostics.Count > 0)
            {
                return new CompileResult(prep.FilePath, prep.Source, prep.HasTests, enableGraphics, backend, tempArtifact, irForOutput, diagnostics, emitIrOnly, prep.PrepMilliseconds + stopwatch.ElapsedMilliseconds, false);
            }

            if (enableTestCache && prep.TestCacheLocation is not null)
            {
                tempArtifact = prep.TestCacheLocation.ArtifactPath;
                isCacheArtifact = true;
            }
            else
            {
                tempArtifact = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.clif");
            }
            File.WriteAllText(tempArtifact, result.Ir);
            if (isCacheArtifact && prep.TestCacheLocation is not null)
            {
                WriteTestCacheEntry(prep, backend, moduleName, includeTests, emitIrOnly, optLevel, enableLto, graphicsLibPath, useCraneliftRunner, enableGraphics, craneliftTargetTriple, compilerCacheSalt);
            }

            return new CompileResult(prep.FilePath, prep.Source, prep.HasTests, enableGraphics, backend, tempArtifact, irForOutput, diagnostics, emitIrOnly, prep.PrepMilliseconds + stopwatch.ElapsedMilliseconds, isCacheArtifact);
        }
        else
        {
            var lowerer = new ModuleLowerer();
            var lowerOptions = enableGraphics
                ? new LowerOptions(IncludeTests: includeTests, EmitTestHarness: includeTests, HeadlessGraphics: false, TargetTriple: llvmTargetTriple)
                : (includeTests ? LowerOptions.Default : LowerOptions.Production) with { TargetTriple = llvmTargetTriple };
            LowerResult lower;
            if (useLowerLock)
            {
                lock (LlvmLock.Lower)
                {
                    lower = lowerer.LowerToIr(prep.CompilationUnit, prep.Sema, prep.Layout, moduleName, lowerOptions);
                }
            }
            else
            {
                lower = lowerer.LowerToIr(prep.CompilationUnit, prep.Sema, prep.Layout, moduleName, lowerOptions);
            }
            diagnostics.AddRange(lower.Diagnostics);
            irForOutput = emitIrOnly || lower.Diagnostics.Count > 0 ? lower.Ir : null;

            if (emitIrOnly || lower.Diagnostics.Count > 0)
            {
                return new CompileResult(prep.FilePath, prep.Source, prep.HasTests, enableGraphics, backend, tempArtifact, irForOutput, diagnostics, emitIrOnly, prep.PrepMilliseconds + stopwatch.ElapsedMilliseconds, false);
            }

            if (enableTestCache && prep.TestCacheLocation is not null)
            {
                tempArtifact = prep.TestCacheLocation.ArtifactPath;
                isCacheArtifact = true;
            }
            else
            {
                tempArtifact = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.ll");
            }
            File.WriteAllText(tempArtifact, lower.Ir);
            if (isCacheArtifact && prep.TestCacheLocation is not null)
            {
                WriteTestCacheEntry(prep, backend, moduleName, includeTests, emitIrOnly, optLevel, enableLto, graphicsLibPath, useCraneliftRunner, enableGraphics, craneliftTargetTriple, compilerCacheSalt);
            }

            return new CompileResult(prep.FilePath, prep.Source, prep.HasTests, enableGraphics, backend, tempArtifact, irForOutput, diagnostics, emitIrOnly, prep.PrepMilliseconds + stopwatch.ElapsedMilliseconds, isCacheArtifact);
        }
    }
    finally
    {
        stopwatch.Stop();
    }
}

static int ConsumeCompileResult(CompileResult result, bool emitIrOnly, string? optLevel, bool enableLto, string? graphicsLibPath, string moduleName, bool useCraneliftRunner)
{
    var testStopwatch = Stopwatch.StartNew();

    if (result.Diagnostics.Count > 0)
    {
        Console.WriteLine($"=== {result.FilePath} ===");
        PrintDiagnostics(result.Diagnostics, result.Source, result.FilePath);
        if (!string.IsNullOrEmpty(result.IrForOutput))
        {
            Console.WriteLine(result.IrForOutput);
        }

        Console.WriteLine($"Total time={result.CompileMilliseconds}ms");
        return 1;
    }

    if (emitIrOnly)
    {
        if (!string.IsNullOrEmpty(result.IrForOutput))
        {
            Console.WriteLine(result.IrForOutput);
        }

        Console.WriteLine($"Total time={result.CompileMilliseconds}ms");
        return 0;
    }

    if (!result.HasTests || string.IsNullOrEmpty(result.ArtifactPath))
    {
        return 0;
    }

    Console.WriteLine($"=== {result.FilePath} ===");
    var executeExit = 1;
    if (result.Backend == BackendType.Cranelift)
    {
        if (useCraneliftRunner)
        {
            var cachedObjectPath = TryGetCachedObjectPath(result);
            var cachedDllPath = TryGetCachedRunnerDllPath(result);
            var entryName = $"{moduleName}__run_tests";

            if (!string.IsNullOrWhiteSpace(cachedObjectPath) &&
                !string.IsNullOrWhiteSpace(cachedDllPath) &&
                !string.IsNullOrWhiteSpace(result.ArtifactPath))
            {
                if (!File.Exists(cachedDllPath))
                {
                    var ensureExit = EnsureCraneliftCachedRunnerDll(result.ArtifactPath, cachedObjectPath, cachedDllPath, moduleName, "test", optLevel, enableLto, result.UsesGraphics, graphicsLibPath, windowsDefFilePath: null);
                    if (ensureExit != 0)
                    {
                        executeExit = ensureExit;
                    }
                    else
                    {
                        executeExit = RunCachedRunnerDll(cachedDllPath, entryName, result.UsesGraphics, graphicsLibPath, dataBindingPlan: null);
                    }
                }
                else
                {
                    executeExit = RunCachedRunnerDll(cachedDllPath, entryName, result.UsesGraphics, graphicsLibPath, dataBindingPlan: null);
                }
            }
            else
            {
                if (!TryFindCraneliftAot(out var aotTool))
                {
                    Console.Error.WriteLine("error: stasis-cranelift-aot not found. Build it with `cargo build -p stasis-cranelift-aot` (in tools/cranelift-aot) or set STASIS_CRANELIFT_AOT.");
                }
                else
                {
                    executeExit = ExecuteClifWithRunner("test", result.ArtifactPath, optLevel, enableLto, result.UsesGraphics, graphicsLibPath, aotTool, moduleName, hotStatePlan: null, dataBindingPlan: null, tickHostFps: null, out _, out _, out _, out _);
                }
            }
        }
        else
        {
            var cachedExecutablePath = TryGetCachedExecutablePath(result);
            var cachedObjectPath = TryGetCachedObjectPath(result);
            if (!string.IsNullOrWhiteSpace(cachedExecutablePath) &&
                !string.IsNullOrWhiteSpace(cachedObjectPath) &&
                !string.IsNullOrWhiteSpace(result.ArtifactPath))
            {
                if (!File.Exists(cachedExecutablePath))
                {
                    var ensureExit = EnsureCraneliftCachedExecutable(result.ArtifactPath, cachedObjectPath, cachedExecutablePath, moduleName, "test", optLevel, enableLto, result.UsesGraphics, graphicsLibPath);
                    if (ensureExit != 0)
                    {
                        executeExit = ensureExit;
                    }
                    else
                    {
                        executeExit = RunCachedExecutable("test", cachedExecutablePath, result.UsesGraphics, graphicsLibPath);
                    }
                }
                else
                {
                    executeExit = RunCachedExecutable("test", cachedExecutablePath, result.UsesGraphics, graphicsLibPath);
                }
            }
            else
            {
                if (!TryFindCraneliftAot(out var aotTool))
                {
                    Console.Error.WriteLine("error: stasis-cranelift-aot not found. Build it with `cargo build -p stasis-cranelift-aot` (in tools/cranelift-aot) or set STASIS_CRANELIFT_AOT.");
                }
                else
                {
                    var tempObj = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}{GetObjectFileExtension()}");
                    try
                    {
                        var aotExit = RunCraneliftAot(aotTool, result.ArtifactPath, tempObj, moduleName, optLevel, out _, out _);
                        if (aotExit != 0)
                        {
                            executeExit = aotExit;
                        }
                        else
                        {
                            executeExit = ExecuteObject("test", tempObj, optLevel, enableLto, result.UsesGraphics, graphicsLibPath, moduleName);
                        }
                    }
                    finally
                    {
                        if (File.Exists(tempObj))
                        {
                            File.Delete(tempObj);
                        }
                    }
                }
            }
        }
    }
    else
    {
        var cachedExecutablePath = TryGetCachedExecutablePath(result);
        var keepExecutable = !string.IsNullOrEmpty(cachedExecutablePath);
        executeExit = Execute("test", result.ArtifactPath, optLevel, enableLto, result.UsesGraphics, graphicsLibPath, cachedExecutablePath, keepExecutable);
    }
    testStopwatch.Stop();
    var total = result.CompileMilliseconds + testStopwatch.ElapsedMilliseconds;
    Console.WriteLine($"Total time={total}ms");

    if (!result.IsCacheArtifact)
    {
        try
        {
            if (File.Exists(result.ArtifactPath))
            {
                File.Delete(result.ArtifactPath);
            }
        }
        catch
        {
            // Best-effort cleanup
        }
    }

    return executeExit;
}

static void PrintDiagnostics(IEnumerable<Diagnostic> diagnostics, string source, string? filePath = null)
{
    foreach (var d in diagnostics)
    {
        var (line, column, lineText) = GetLineInfo(source, d.Span.Start);
        var length = Math.Max(1, d.Span.Length);
        var markerLen = Math.Min(length, Math.Max(1, Math.Max(0, lineText.Length - (column - 1))));
        var marker = new string(' ', Math.Max(0, column - 1)) + new string('^', markerLen);
        var location = filePath is null ? $"line {line}, column {column}" : $"{filePath}:{line}:{column}";
        Console.Error.WriteLine($"error: {d.Message} ({location})");
        Console.Error.WriteLine(lineText);
        Console.Error.WriteLine(marker);
    }
}

static (int line, int column, string lineText) GetLineInfo(string source, int offset)
{
    var clamped = Math.Max(0, Math.Min(source.Length, offset));
    var line = 1;
    var lineStart = 0;
    for (int i = 0; i < clamped; i++)
    {
        if (source[i] == '\n')
        {
            line++;
            lineStart = i + 1;
        }
    }

    var lineEnd = source.IndexOf('\n', lineStart);
    if (lineEnd < 0)
    {
        lineEnd = source.Length;
    }

    var column = clamped - lineStart + 1;
    var lineText = source.Substring(lineStart, Math.Max(0, lineEnd - lineStart));
    return (line, column, lineText);
}

static bool LikelyContainsTestBlock(string path)
{
    foreach (var line in File.ReadLines(path))
    {
        var trimmed = line.AsSpan().TrimStart();
        if (trimmed.StartsWith("//", StringComparison.Ordinal))
        {
            continue;
        }

        if (Regex.IsMatch(line, @"^\s*test\b", RegexOptions.CultureInvariant))
        {
            return true;
        }
    }

    return false;
}

static class LlvmLock
{
    public static readonly object Lower = new();
}

static class CraneliftAotState
{
    public static readonly object Lock = new();
    public static CraneliftAotServer? Instance;
}

sealed class CraneliftAotServer : IDisposable
{
    private readonly Process process;
    private readonly Stream input;
    private readonly StreamReader output;
    private readonly object gate = new();

    public long SpawnMs { get; }
    public bool IsAlive => !process.HasExited;

    private CraneliftAotServer(Process process, Stream input, StreamReader output, long spawnMs)
    {
        this.process = process;
        this.input = input;
        this.output = output;
        SpawnMs = spawnMs;
    }

    public static CraneliftAotServer Start(string aotTool)
    {
        var psi = new ProcessStartInfo
        {
            FileName = aotTool,
            Arguments = "--server",
            UseShellExecute = false,
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true
        };

        var sw = Stopwatch.StartNew();
        var process = Process.Start(psi)!;
        var output = process.StandardOutput;
        var ready = output.ReadLine();
        var spawnMs = sw.ElapsedMilliseconds;
        if (!string.Equals(ready, "READY", StringComparison.OrdinalIgnoreCase))
        {
            throw new InvalidOperationException("stasis-cranelift-aot server failed to start.");
        }

        return new CraneliftAotServer(process, process.StandardInput.BaseStream, output, spawnMs);
    }

    public int Compile(string clifPath, string outputPath, string target, string moduleName, string optLevel, out long compileMs)
    {
        var clifBytes = File.ReadAllBytes(clifPath);
        return CompileFromBytes(clifBytes, outputPath, target, moduleName, optLevel, out compileMs);
    }

    public int CompileFromBytes(byte[] clifBytes, string outputPath, string target, string moduleName, string optLevel, out long compileMs)
    {
        var outputBytes = Encoding.UTF8.GetBytes(outputPath);
        var targetBytes = Encoding.UTF8.GetBytes(target);
        var moduleBytes = Encoding.UTF8.GetBytes(moduleName);
        var optBytes = Encoding.UTF8.GetBytes(optLevel);

        lock (gate)
        {
            var sw = Stopwatch.StartNew();
            var header = $"REQ {outputBytes.Length} {targetBytes.Length} {moduleBytes.Length} {optBytes.Length} {clifBytes.Length}\n";
            var headerBytes = Encoding.UTF8.GetBytes(header);
            input.Write(headerBytes, 0, headerBytes.Length);
            input.Write(outputBytes, 0, outputBytes.Length);
            input.Write(targetBytes, 0, targetBytes.Length);
            input.Write(moduleBytes, 0, moduleBytes.Length);
            input.Write(optBytes, 0, optBytes.Length);
            input.Write(clifBytes, 0, clifBytes.Length);
            input.Flush();

            var response = output.ReadLine();
            compileMs = sw.ElapsedMilliseconds;
            if (response is null)
            {
                Console.Error.WriteLine("error: stasis-cranelift-aot server closed unexpectedly.");
                return 1;
            }

            if (response.StartsWith("ERR ", StringComparison.OrdinalIgnoreCase))
            {
                Console.Error.WriteLine(response);
                return 1;
            }

            return 0;
        }
    }

    public void Dispose()
    {
        try
        {
            if (!process.HasExited)
            {
                try
                {
                    var quit = Encoding.UTF8.GetBytes("QUIT\n");
                    input.Write(quit, 0, quit.Length);
                    input.Flush();
                }
                catch
                {
                    // Ignore shutdown errors; best effort.
                }

                process.Kill(entireProcessTree: true);
            }
        }
        catch
        {
            // Ignore disposal errors.
        }
    }
}

static class CraneliftRunnerState
{
    public static readonly object Lock = new();
    public static CraneliftRunnerServer? Instance;
}

sealed class CraneliftRunnerServer : IDisposable
{
    private readonly Process process;
    private readonly Stream input;
    private readonly StreamReader control;
    private readonly object gate = new();

    public bool IsAlive => !process.HasExited;

    private CraneliftRunnerServer(Process process, Stream input, StreamReader control)
    {
        this.process = process;
        this.input = input;
        this.control = control;
    }

    public static CraneliftRunnerServer Start(string runnerPath)
    {
        var psi = new ProcessStartInfo
        {
            FileName = runnerPath,
            Arguments = "--server",
            UseShellExecute = false,
            RedirectStandardInput = true,
            RedirectStandardError = true,
            RedirectStandardOutput = false
        };

        var process = Process.Start(psi)!;
        var control = process.StandardError;
        var ready = control.ReadLine();
        if (!string.Equals(ready, "READY", StringComparison.OrdinalIgnoreCase))
        {
            throw new InvalidOperationException("stasis_runner server failed to start.");
        }

        return new CraneliftRunnerServer(process, process.StandardInput.BaseStream, control);
    }

    public int Run(string dllPath, string entryName, out long runMs)
    {
        var dllBytes = Encoding.UTF8.GetBytes(dllPath);
        var entryBytes = Encoding.UTF8.GetBytes(entryName);

        lock (gate)
        {
            var sw = Stopwatch.StartNew();
            var header = $"RUN {dllBytes.Length} {entryBytes.Length}\n";
            var headerBytes = Encoding.UTF8.GetBytes(header);
            input.Write(headerBytes, 0, headerBytes.Length);
            input.Write(dllBytes, 0, dllBytes.Length);
            input.Write(entryBytes, 0, entryBytes.Length);
            input.Flush();

            string? response;
            while (true)
            {
                response = control.ReadLine();
                if (response == null)
                {
                    Console.Error.WriteLine("error: stasis_runner server closed unexpectedly.");
                    runMs = sw.ElapsedMilliseconds;
                    return 1;
                }
                if (response.Length == 0)
                {
                    continue;
                }
                if (response.StartsWith("ERR ", StringComparison.OrdinalIgnoreCase) ||
                    response.StartsWith("OK ", StringComparison.OrdinalIgnoreCase))
                {
                    break;
                }
                // Ignore non-control stderr output.
            }

            runMs = sw.ElapsedMilliseconds;
            if (response.StartsWith("ERR ", StringComparison.OrdinalIgnoreCase))
            {
                Console.Error.WriteLine(response);
                return 1;
            }

            if (response.StartsWith("OK ", StringComparison.OrdinalIgnoreCase))
            {
                var parts = response.Split(' ', StringSplitOptions.RemoveEmptyEntries);
                if (parts.Length >= 2 && int.TryParse(parts[1], out var exitCode))
                {
                    return exitCode;
                }
            }

            Console.Error.WriteLine("error: invalid runner response.");
            return 1;
        }
    }

    public void Dispose()
    {
        try
        {
            if (!process.HasExited)
            {
                try
                {
                    var quit = Encoding.UTF8.GetBytes("QUIT\n");
                    input.Write(quit, 0, quit.Length);
                    input.Flush();
                }
                catch
                {
                    // Ignore shutdown errors; best effort.
                }

                process.Kill(entireProcessTree: true);
            }
        }
        catch
        {
            // Ignore disposal errors.
        }
    }
}

sealed record CompileResult(
    string FilePath,
    string Source,
    bool HasTests,
    bool UsesGraphics,
    BackendType Backend,
    string? ArtifactPath,
    string? IrForOutput,
    List<Diagnostic> Diagnostics,
    bool EmitIrOnly,
    long CompileMilliseconds,
    bool IsCacheArtifact);

sealed record PreparedForLower(
    string FilePath,
    string Source,
    CompilationUnitSyntax CompilationUnit,
    SemanticResult Sema,
    LayoutPlan Layout,
    bool HasTests,
    bool UsesGraphics,
    long PrepMilliseconds,
    TestCacheLocation? TestCacheLocation);

sealed record PrepareResult(PreparedForLower? Prepared, CompileResult? Result);

sealed record TestCacheLocation(
    string CacheKey,
    string ArtifactPath,
    string EntryPath,
    string SourceHash);

sealed record TestCacheEntry(
    int Version,
    string CacheKey,
    string FilePath,
    string ArtifactPath,
    string SourceHash,
    bool HasTests,
    bool UsesGraphics,
    BackendType Backend,
    string ModuleName,
    bool IncludeTests,
    bool EmitIrOnly,
    string? OptLevel,
    bool EnableLto,
    string? GraphicsLibPath,
    bool UseCraneliftRunner,
    string? CraneliftTargetTriple,
    string CompilerCacheSalt);
