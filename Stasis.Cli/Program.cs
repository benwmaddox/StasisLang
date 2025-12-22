using System.Diagnostics;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
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
var useCraneliftRunner = Environment.GetEnvironmentVariable("STASIS_CRANELIFT_RUNNER") == "1";
var enableHotState = false;
var tickHostFps = 60;

while (cliArgs.Count > 0)
{
    var arg = cliArgs.Dequeue();
    switch (arg)
    {
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
        case "--cranelift-runner":
            useCraneliftRunner = true;
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

// Set default backend based on mode if not explicitly specified
var backend = selectedBackend ?? CodeGeneratorFactory.GetDefaultBackend(mode == "release");
if (backend == BackendType.Cranelift && selectedBackend is null && !CanUseCranelift(emitIrOnly))
{
    Console.Error.WriteLine("warning: Cranelift backend unavailable; defaulting to LLVM.");
    backend = BackendType.Llvm;
}

if (backend == BackendType.Cranelift && (mode == "test" || mode == "run"))
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
if (!ShouldSuppressWarnings() && backend == BackendType.Cranelift && selectedBackend is not null)
{
    if (!OperatingSystem.IsWindows())
    {
        if (!emitIrOnly)
        {
            Console.Error.WriteLine("warning: forcing --emit-ir mode since Cranelift native output is only implemented for Windows x64 currently.");
            emitIrOnly = true;
        }
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

LlvmNativeLoader.EnsureLoaded();

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
    var overallExit = RunAllTestsInDirectoryParallel(files, includeTests, moduleName, emitIrOnly, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, useCraneliftRunner, allowReachabilityFallback);
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

    var watchExit = WatchFile(path, mode, includeTests, moduleName, emitIrOnly, outputPath, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, useCraneliftRunner, enableHotState, tickHostFps);
    Environment.Exit(watchExit);
}

var singleExit = ProcessFile(path, mode, includeTests, moduleName, emitIrOnly, outputPath, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, tickHostFps, useCraneliftRunner: useCraneliftRunner, enableHotState: enableHotState);
Environment.Exit(singleExit);

static int ProcessFile(string path, string mode, bool includeTests, string moduleName, bool emitIrOnly, string? outputPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, BackendType backend, int tickHostFps, bool useLowerLock = true, bool useCraneliftRunner = false, bool enableHotState = false)
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

        // Auto-detect graphics usage if not explicitly enabled
        if (!enableGraphics && DetectsGraphicsUsage(source))
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

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
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
                ? new LowerOptions(IncludeTests: includeTests, EmitTestHarness: includeTests, HeadlessGraphics: false)
                : (includeTests ? LowerOptions.Default : LowerOptions.Production);
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
            Console.WriteLine(ir);
            return 1;
        }

        if (emitIrOnly)
        {
            Console.WriteLine(ir);
            return lowerDiagnostics.Count > 0 ? 1 : 0;
        }

        if (backend == BackendType.Cranelift)
        {
            // Native Cranelift (Windows x64) path: CLIF -> .obj -> clang link -> exe.
            if (!OperatingSystem.IsWindows())
            {
                Console.Error.WriteLine("error: Cranelift native output is only implemented for Windows x64 currently. Use --emit-ir.");
                return 1;
            }

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
                var exports = new List<string>();
                if (mode == "run")
                {
                    exports.Add($"{moduleName}__main");
                    if (hasTick)
                    {
                        exports.Add($"{moduleName}__tick");
                    }
                }
                else
                {
                    exports.Add($"{moduleName}__run_tests");
                }
                if (enableHotState)
                {
                    if (!TryCreateHotStatePlan(path, layout, moduleName, exports, excludeSpriteFields: true, out var createdPlan))
                    {
                        return 1;
                    }
                    hotStatePlan = createdPlan;
                }

                long? runAotSpawn;
                long runAotCompile;
                long runLink;
                long runRun;
                var runExit = useInMemoryClif
                    ? ExecuteClifWithRunnerFromString(mode, ir, optLevel, enableLto, enableGraphics, graphicsLibPath, aotTool, moduleName, hotStatePlan, hasTick ? tickHostFps : (int?)null, out runAotSpawn, out runAotCompile, out runLink, out runRun)
                    : ExecuteClifWithRunner(mode, tempClif, optLevel, enableLto, enableGraphics, graphicsLibPath, aotTool, moduleName, hotStatePlan, hasTick ? tickHostFps : (int?)null, out runAotSpawn, out runAotCompile, out runLink, out runRun);
                if (logPhaseTiming)
                {
                    aotSpawnMs = runAotSpawn ?? 0;
                    aotMs = runAotCompile;
                    linkMs = runLink;
                    runMs = runRun;
                }
                return runExit;
            }

            tempObj = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.obj");
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

static int ExecuteObjectWithRunner(string mode, string objPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, string moduleName, HotStatePlan? hotStatePlan, IReadOnlyList<string> dllExports, int? tickHostFps, out long linkMs, out long runMs)
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
        var args = BuildClangArgsForObject(objPath, dllPath, mode == "test", optLevel, enableLto, enableGraphics, graphicsLibPath, entryName: entryName, isDll: true, windowsDefFilePath: hotStatePlan?.DefPath, windowsExports: dllExports);
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
        if (UseCraneliftRunnerServer() && hotStatePlan is null)
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

static int ExecuteClifWithRunner(string mode, string clifPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, string aotTool, string moduleName, HotStatePlan? hotStatePlan, int? tickHostFps, out long? aotSpawnMs, out long aotCompileMs, out long linkMs, out long runMs)
{
    aotSpawnMs = null;
    aotCompileMs = 0;
    linkMs = 0;
    runMs = 0;
    var tempObj = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.obj");
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
        return ExecuteObjectWithRunner(mode, tempObj, optLevel, enableLto, enableGraphics, graphicsLibPath, moduleName, hotStatePlan, exports, tickHostFps, out linkMs, out runMs);
    }
    finally
    {
        if (File.Exists(tempObj))
        {
            File.Delete(tempObj);
        }
    }
}

static int ExecuteClifWithRunnerFromString(string mode, string clif, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, string aotTool, string moduleName, HotStatePlan? hotStatePlan, int? tickHostFps, out long? aotSpawnMs, out long aotCompileMs, out long linkMs, out long runMs)
{
    aotSpawnMs = null;
    aotCompileMs = 0;
    linkMs = 0;
    runMs = 0;
    var tempObj = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.obj");
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
        return ExecuteObjectWithRunner(mode, tempObj, optLevel, enableLto, enableGraphics, graphicsLibPath, moduleName, hotStatePlan, exports, tickHostFps, out linkMs, out runMs);
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
    // Link .obj into a normal executable (use CRT defaults).
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
            var libFile = Path.GetFileName(libPath);
            var isStaticLib = libFile != null && libFile.Contains("static", StringComparison.OrdinalIgnoreCase);
            if (isStaticLib)
            {
                linkingStaticGraphics = true;
            }

            args.Add($"\"{libPath}\"");
            var libDir = Path.GetDirectoryName(libPath);
            if (!string.IsNullOrEmpty(libDir))
            {
                args.Add($"-L\"{libDir}\"");
            }

            if (isStaticLib)
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
            args.Add($"-Wl,-e,{entry}");
        }
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

static int Execute(string mode, string llPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath)
{
    // lli doesn't support external libraries easily, so use clang when graphics is enabled
    if (!enableGraphics && TryFindTool("lli", out var lli))
    {
        return RunProcess(lli, mode == "test" ? $"-entry-function=run_tests \"{llPath}\"" : $"\"{llPath}\"");
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
            if (File.Exists(exePath))
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
            var libDir = Path.GetDirectoryName(libPath);
            var libFile = Path.GetFileName(libPath);
            var isStaticLib = libFile != null && libFile.Contains("static", StringComparison.OrdinalIgnoreCase);
            linkingStaticGraphics = isStaticLib;

            if (!string.IsNullOrEmpty(libDir))
            {
                args.Add($"-L\"{libDir}\"");
            }

            // When a full path is known, pass it directly so clang doesn't guess the name
            if (!string.IsNullOrEmpty(libFile))
            {
                args.Add($"\"{libPath}\"");
            }
            else
            {
                args.Add("-lstasis_graphics");
            }

            // If we are linking the static runtime, pull in its static deps for a single EXE.
            if (isStaticLib && RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
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

    return string.Join(" ", args);
}

static void CopyGraphicsRuntimeDependencies(string targetDir, string? graphicsLibPath)
{
    try
    {
        Directory.CreateDirectory(targetDir);

        var candidates = new List<string>();

        // Prefer explicit lib path (derive DLL alongside .lib)
        if (!string.IsNullOrEmpty(graphicsLibPath))
        {
            var libFile = Path.GetFileName(graphicsLibPath);
            if (libFile != null && libFile.Contains("static", StringComparison.OrdinalIgnoreCase))
            {
                // Static runtime: nothing to copy
                return;
            }

            if (Path.GetExtension(graphicsLibPath).Equals(".lib", StringComparison.OrdinalIgnoreCase))
            {
                var dllGuess = Path.ChangeExtension(graphicsLibPath, ".dll");
                if (File.Exists(dllGuess))
                {
                    candidates.Add(dllGuess);
                }
            }

            if (File.Exists(graphicsLibPath) && Path.GetExtension(graphicsLibPath).Equals(".dll", StringComparison.OrdinalIgnoreCase))
            {
                candidates.Add(graphicsLibPath);
            }
        }

        // Fall back to search helper
        var foundDll = FindGraphicsLibrary(preferShared: true);
        if (!string.IsNullOrEmpty(foundDll))
        {
            candidates.Add(foundDll);
        }

        // Copy primary graphics DLL + common deps if present in the same directory
        foreach (var src in candidates)
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

static bool DetectsGraphicsUsage(string source)
{
    // Detect direct calls to the graphics runtime API. Use a "not preceded by identifier char"
    // check so we don't trigger on helpers like ascii_clear() in the stdlib.
    return Regex.IsMatch(
        source,
        @"(?<![A-Za-z0-9_])" +
        @"(init_window|begin_frame|end_frame|draw_line|clear|gfx_load_sprite|gfx_draw_sprite|gfx_poll_reload|gfx_debug_bake_hash|should_quit|is_key_down|get_mouse_x|get_mouse_y|is_mouse_down|time|get_time_ms|sleep_ms)" +
        @"\s*\(",
        RegexOptions.CultureInvariant);
}

static bool DetectsTickUsage(string source) =>
    Regex.IsMatch(source, @"(?m)^\s*function\s+tick\s*\(", RegexOptions.CultureInvariant);

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
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "Release"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "bin"));
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
                "libstasis_graphics_static.a",
                "libstasis_graphics.dylib"
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
                "libstasis_graphics_static.a",
                "libstasis_graphics.so"
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

static int RunCraneliftAot(string aotTool, string clifPath, string objPath, string moduleName, string? optLevel, out long? spawnMs, out long compileMs)
{
    const string target = "x86_64-pc-windows-msvc";
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
    const string target = "x86_64-pc-windows-msvc";
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
    return Path.Combine(string.IsNullOrEmpty(dir) ? Directory.GetCurrentDirectory() : dir, name + ext);
}

static string GetHotExitFilePath(string sourcePath, string moduleName)
{
    var repoRoot = FindRepoRoot() ?? Directory.GetCurrentDirectory();
    var hotDir = Path.Combine(repoRoot, "build", "hotstate");
    var baseName = Path.GetFileNameWithoutExtension(sourcePath);
    return Path.Combine(hotDir, $"{baseName}.{moduleName}.hot-exit");
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

        var usesGraphics = enableGraphics || DetectsGraphicsUsage(source);
        var parse = Parser.Parse(source);
        parseMs = phase.ElapsedMilliseconds;
        phase.Restart();
        if (parse.Diagnostics.Count > 0)
        {
            PrintDiagnostics(parse.Diagnostics, source, sourcePath);
            return 1;
        }
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
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
        var clifPath = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.clif");
        var objPath = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.obj");
        File.WriteAllText(clifPath, result.Ir);
        clifWriteMs = phase.ElapsedMilliseconds;
        phase.Restart();

        try
        {
            var aotExit = RunCraneliftAot(aotTool, clifPath, objPath, moduleName, optLevel, out var spawnFallback, out var compileFallback);
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

            var linkArgs = BuildClangArgsForObject(objPath, hotDll, isTest: false, optLevel, enableLto, usesGraphics, graphicsLibPath, entryName: $"{moduleName}__main", isDll: true, windowsDefFilePath: plan.DefPath);
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
                var runnerArgs = $"\"{hotDll}\" {entry} --state \"{plan.SnapshotPath}\" --state-map \"{plan.MapPath}\" --swap-file \"{swapFile}\" --fps {fps}";
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

            File.WriteAllText(swapFile, hotDll, Encoding.ASCII);
            swapWriteMs = phase.ElapsedMilliseconds;
            swTotal.Stop();
            timingLine =
                $"HOTRELOAD phases(ms): read={readMs} parse={parseMs} sema={semaMs} layout={layoutMs} lower={lowerMs} clif={clifWriteMs} aotSpawn={aotSpawnMs} aotCompile={aotCompileMs} plan={planMs} link={linkMs} swapWrite={swapWriteMs} total={swTotal.ElapsedMilliseconds}";
            return 0;
        }
        finally
        {
            if (File.Exists(clifPath))
            {
                File.Delete(clifPath);
            }
            if (File.Exists(objPath))
            {
                File.Delete(objPath);
            }
        }
    }

    var initial = BuildAndSwap(startRunner: true, out _);
    if (initial != 0)
    {
        return initial;
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
            Console.Error.WriteLine($"error: runner exited with code {runner.ExitCode}");
            break;
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

        var exit = BuildAndSwap(startRunner: false, out var timingLine);
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
    plan = new HotStatePlan(string.Empty, string.Empty, string.Empty, string.Empty);
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
        .Where(f => !excludeSpriteFields || !f.Name.StartsWith("state_sprites_", StringComparison.Ordinal))
        .Select(f => (Name: f.Name, Size: f.Size))
        .ToArray();

    if (entries.Length == 0)
    {
        Console.Error.WriteLine("error: --hot-state: state has no persisted fields (all fields were filtered).");
        return false;
    }

    var totalBytes = 0;
    ulong hash = 14695981039346656037UL; // FNV-1a 64 offset basis
    foreach (var (name, size) in entries)
    {
        totalBytes += size;
        var nameBytes = Encoding.UTF8.GetBytes(name);
        foreach (var b in nameBytes)
        {
            hash ^= b;
            hash *= 1099511628211UL;
        }
        hash ^= 0;
        hash *= 1099511628211UL;

        unchecked
        {
            var u = (uint)size;
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
    foreach (var (name, size) in entries)
    {
        map.Append(name);
        map.Append(' ');
        map.Append(size);
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
    foreach (var (name, _) in entries)
    {
        def.Append("  ");
        def.Append(name);
        def.Append(" DATA\n");
    }
    File.WriteAllText(defPath, def.ToString(), Encoding.ASCII);

    plan = new HotStatePlan(mapPath, snapshotPath, defPath, hotExitPath);
    return true;
}

static int WatchFile(string path, string mode, bool includeTests, string moduleName, bool emitIrOnly, string? outputPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, BackendType backend, bool useCraneliftRunner, bool enableHotState, int tickHostFps)
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
        enableHotState &&
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
        _ = ProcessFile(fullPath, mode, includeTests, moduleName, emitIrOnly, outputPath, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, tickHostFps, useCraneliftRunner: useCraneliftRunner, enableHotState: enableHotState);
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
                _ = ProcessFile(fullPath, mode, includeTests, moduleName, emitIrOnly, outputPath, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, tickHostFps, useCraneliftRunner: useCraneliftRunner, enableHotState: enableHotState);
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

static void PrintUsage()
{
    Console.WriteLine("Usage:");
    Console.WriteLine("  stasisc run <file> [--watch] [--hot-state] [--fps <1..240>] [--module <name>] [--with-tests] [--emit-ir] [--backend <llvm|cranelift>] [--graphics] [--graphics-lib <path>]");
    Console.WriteLine("  stasisc test [<file>|--all] [--watch] [--module <name>] [--emit-ir] [--backend <llvm|cranelift>]");
    Console.WriteLine("  stasisc build <file> [--module <name>] [--with-tests] [--out <path>] [--opt-level <0|1|2|3|s|z>] [--lto|--no-lto] [--backend <llvm|cranelift>] [--graphics] [--graphics-lib <path>]");
    Console.WriteLine("  stasisc release <file> [--module <name>] [--out <path>] [--opt-level <0|1|2|3|s|z>] [--lto|--no-lto] [--backend <llvm|cranelift>] [--graphics] [--graphics-lib <path>]");
    Console.WriteLine("  stasisc format <file>");
    Console.WriteLine("Defaults: execute via lli if available, else clang. Use --emit-ir to only write IR to stdout. With no path (or --all), 'test' runs every .stasis file under the working directory. Build/release require clang in PATH. 'release' defaults to -O3 with LTO.");
    Console.WriteLine("Watch: use --watch to re-run on file changes (run/test only).");
    Console.WriteLine("Hot state: use --hot-state (Cranelift run only) to restore and save the global 'state' across runs, enabling simple stateful restart experiments.");
    Console.WriteLine("Tick hosting: if your program defines `function tick()`, the runner will call `main()` once then call `tick()` at `--fps` (host paced) and can hot-swap between ticks in --watch + --hot-state mode.");
    Console.WriteLine("Graphics: use --graphics to enable SDL2/OpenGL graphics runtime. Specify --graphics-lib to override library path.");
    Console.WriteLine("Backend: use --backend to select code generation backend. Defaults to 'cranelift' for run/test/build (when available) and 'llvm' for release; Cranelift is experimental.");
    Console.WriteLine("Cranelift: run/test uses the native DLL runner when available (stasis_runner.exe). Set STASIS_CRANELIFT_RUNNER_EXE to override.");
}

static int RunAllTestsInDirectoryParallel(string[] files, bool includeTests, string moduleName, bool emitIrOnly, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, BackendType backend, bool useCraneliftRunner, bool allowReachabilityFallback, bool useLowerLock = true, int lowerDegree = 1) =>
    RunAllTestsInDirectoryParallelAsync(files, includeTests, moduleName, emitIrOnly, optLevel, enableLto, enableGraphics, graphicsLibPath, backend, useCraneliftRunner, allowReachabilityFallback, useLowerLock, lowerDegree).GetAwaiter().GetResult();

static async Task<int> RunAllTestsInDirectoryParallelAsync(string[] files, bool includeTests, string moduleName, bool emitIrOnly, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, BackendType backend, bool useCraneliftRunner, bool allowReachabilityFallback, bool useLowerLock, int lowerDegree)
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
            var prep = PrepareForLower(file, emitIrOnly, backend);
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
            var result = LowerPrepared(item, includeTests, moduleName, emitIrOnly, effectiveGraphics, backend, useLowerLock, allowReachabilityFallback);
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

static PrepareResult PrepareForLower(string path, bool emitIrOnly, BackendType backend)
{
    var stopwatch = Stopwatch.StartNew();
    var diagnostics = new List<Diagnostic>();
    try
    {
    var source = LoadSourceWithImports(path, out var importDiagnostics, out var importSource);
    if (importDiagnostics.Count > 0)
    {
        PrintDiagnostics(importDiagnostics, importSource, path);
        return new PrepareResult(null, new CompileResult(path, importSource, false, false, backend, null, null, importDiagnostics, emitIrOnly, stopwatch.ElapsedMilliseconds));
    }
        var usesGraphics = DetectsGraphicsUsage(source);
        var parse = Parser.Parse(source);
        diagnostics.AddRange(parse.Diagnostics);
        var hasTests = parse.CompilationUnit.Declarations.OfType<TestDeclarationSyntax>().Any();

        if (parse.Diagnostics.Count > 0 || (!hasTests && !emitIrOnly))
        {
            return new PrepareResult(null, new CompileResult(path, source, hasTests, usesGraphics, backend, null, null, diagnostics, emitIrOnly, stopwatch.ElapsedMilliseconds));
        }

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        diagnostics.AddRange(sema.Diagnostics);
        if (sema.Diagnostics.Count > 0)
        {
            return new PrepareResult(null, new CompileResult(path, source, hasTests, usesGraphics, backend, null, null, diagnostics, emitIrOnly, stopwatch.ElapsedMilliseconds));
        }

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        stopwatch.Stop();
        return new PrepareResult(new PreparedForLower(path, source, parse.CompilationUnit, sema, layout, hasTests, usesGraphics, stopwatch.ElapsedMilliseconds), null);
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


static CompileResult LowerPrepared(PreparedForLower prep, bool includeTests, string moduleName, bool emitIrOnly, bool enableGraphics, BackendType backend, bool useLowerLock, bool allowReachabilityFallback)
{
    var stopwatch = Stopwatch.StartNew();
    var diagnostics = new List<Diagnostic>();
    string? tempArtifact = null;
    string? irForOutput = null;

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
                return new CompileResult(prep.FilePath, prep.Source, prep.HasTests, enableGraphics, backend, tempArtifact, irForOutput, diagnostics, emitIrOnly, prep.PrepMilliseconds + stopwatch.ElapsedMilliseconds);
            }

            tempArtifact = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.clif");
            File.WriteAllText(tempArtifact, result.Ir);

            return new CompileResult(prep.FilePath, prep.Source, prep.HasTests, enableGraphics, backend, tempArtifact, irForOutput, diagnostics, emitIrOnly, prep.PrepMilliseconds + stopwatch.ElapsedMilliseconds);
        }
        else
        {
            var lowerer = new ModuleLowerer();
            var lowerOptions = enableGraphics
                ? new LowerOptions(IncludeTests: includeTests, EmitTestHarness: includeTests, HeadlessGraphics: false)
                : (includeTests ? LowerOptions.Default : LowerOptions.Production);
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
                return new CompileResult(prep.FilePath, prep.Source, prep.HasTests, enableGraphics, backend, tempArtifact, irForOutput, diagnostics, emitIrOnly, prep.PrepMilliseconds + stopwatch.ElapsedMilliseconds);
            }

            tempArtifact = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.ll");
            File.WriteAllText(tempArtifact, lower.Ir);

            return new CompileResult(prep.FilePath, prep.Source, prep.HasTests, enableGraphics, backend, tempArtifact, irForOutput, diagnostics, emitIrOnly, prep.PrepMilliseconds + stopwatch.ElapsedMilliseconds);
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
        if (!TryFindCraneliftAot(out var aotTool))
        {
            Console.Error.WriteLine("error: stasis-cranelift-aot not found. Build it with `cargo build -p stasis-cranelift-aot` (in tools/cranelift-aot) or set STASIS_CRANELIFT_AOT.");
        }
        else
        {
            if (useCraneliftRunner)
            {
                executeExit = ExecuteClifWithRunner("test", result.ArtifactPath, optLevel, enableLto, result.UsesGraphics, graphicsLibPath, aotTool, moduleName, hotStatePlan: null, tickHostFps: null, out _, out _, out _, out _);
            }
            else
            {
                var tempObj = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.obj");
                try
                {
                    var aotExit = RunProcess(aotTool, $"--input \"{result.ArtifactPath}\" --output \"{tempObj}\" --target x86_64-pc-windows-msvc");
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
    else
    {
        executeExit = Execute("test", result.ArtifactPath, optLevel, enableLto, result.UsesGraphics, graphicsLibPath);
    }
    testStopwatch.Stop();
    var total = result.CompileMilliseconds + testStopwatch.ElapsedMilliseconds;
    Console.WriteLine($"Total time={total}ms");

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
        if (line.Contains("test", StringComparison.Ordinal))
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
    long CompileMilliseconds);

sealed record PreparedForLower(
    string FilePath,
    string Source,
    CompilationUnitSyntax CompilationUnit,
    SemanticResult Sema,
    LayoutPlan Layout,
    bool HasTests,
    bool UsesGraphics,
    long PrepMilliseconds);

sealed record PrepareResult(PreparedForLower? Prepared, CompileResult? Result);
