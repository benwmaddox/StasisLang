using System.Diagnostics;
using System.Linq;
using System.Runtime.InteropServices;
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
string? optLevel = null;
var enableLto = false;
var enableGraphics = false;
string? graphicsLibPath = null;

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

if (!File.Exists(path) && !Directory.Exists(path))
{
    Console.Error.WriteLine($"error: file not found: {path}");
    Environment.Exit(1);
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

LlvmNativeLoader.EnsureLoaded();

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

    var overallExit = RunAllTestsInDirectoryParallel(files, includeTests, moduleName, emitIrOnly, optLevel, enableLto, enableGraphics, graphicsLibPath);
    Environment.Exit(overallExit);
}

var singleExit = ProcessFile(path, mode, includeTests, moduleName, emitIrOnly, outputPath, optLevel, enableLto, enableGraphics, graphicsLibPath);
Environment.Exit(singleExit);

static int ProcessFile(string path, string mode, bool includeTests, string moduleName, bool emitIrOnly, string? outputPath, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, bool useLowerLock = true)
{
    var fileStopwatch = System.Diagnostics.Stopwatch.StartNew();
    var tempLl = string.Empty;

    try
    {
        var source = File.ReadAllText(path);
        var parse = Parser.Parse(source);
        if (parse.Diagnostics.Count > 0)
        {
            PrintDiagnostics(parse.Diagnostics, source, path);
            return 1;
        }

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        if (sema.Diagnostics.Count > 0)
        {
            PrintDiagnostics(sema.Diagnostics, source, path);
            return 1;
        }

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
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
        if (lower.Diagnostics.Count > 0)
        {
            PrintDiagnostics(lower.Diagnostics, source, path);
            Console.WriteLine(lower.Ir);
            return 1;
        }

        if (emitIrOnly)
        {
            Console.WriteLine(lower.Ir);
            return lower.Diagnostics.Count > 0 ? 1 : 0;
        }

        tempLl = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.ll");
        File.WriteAllText(tempLl, lower.Ir);

        if (mode == "build" || mode == "release")
        {
            var outPath = outputPath ?? BuildDefaultOutputPath(path);
            var exitCode = BuildExecutable(tempLl, outPath, includeTests, optLevel, enableLto, enableGraphics, graphicsLibPath);
            return exitCode;
        }

        var executeExit = Execute(mode, tempLl, optLevel, enableLto, enableGraphics, graphicsLibPath);
        return executeExit;
    }
    finally
    {
        if (!string.IsNullOrEmpty(tempLl) && File.Exists(tempLl))
        {
            File.Delete(tempLl);
        }

        fileStopwatch.Stop();
        Console.WriteLine($"Total time={fileStopwatch.ElapsedMilliseconds}ms");
    }
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
            var exit = RunProcess(clang, args);
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

            return RunProcess(exePath, string.Empty);
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

        var sdkRoot = GetLatestWindowsSdkLib();
        if (sdkRoot is not null)
        {
            var ucrt = Path.Combine(sdkRoot, "ucrt", "x64");
            var um = Path.Combine(sdkRoot, "um", "x64");
            args.Add($"-L\"{ucrt}\"");
            args.Add($"-L\"{um}\"");
            // When linking static graphics, let clang pick CRT defaults to avoid duplicate ucrt linkage.
            args.Add("-lkernel32");
            if (!linkingStaticGraphics)
            {
                args.Add("-lucrt");
                args.Add("-llegacy_stdio_definitions");
            }
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
        var foundDll = FindGraphicsLibrary();
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
            if (!File.Exists(dest))
            {
                File.Copy(src, dest, overwrite: false);
            }

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
                    if (!File.Exists(depDest))
                    {
                        File.Copy(depSrc, depDest, overwrite: false);
                    }
                }
            }
        }
    }
    catch
    {
        // Best-effort; missing copies will surface as runtime load errors.
    }
}

static string? FindGraphicsLibrary()
{
    // Look for the graphics library in common locations
    var searchPaths = new List<string>();

    // Check relative to the CLI executable
    var exeDir = AppContext.BaseDirectory;
    searchPaths.Add(exeDir);
    searchPaths.Add(Path.Combine(exeDir, "runtime"));

    // Prefer workspace runtime outputs before falling back to cwd root
    var cwd = Directory.GetCurrentDirectory();
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "Release"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "bin"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build", "Debug"));
    searchPaths.Add(Path.Combine(cwd, "runtime", "build"));
    searchPaths.Add(Path.Combine(cwd, "runtime"));
    searchPaths.Add(cwd);

    string[] candidates;
    if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
    {
        candidates = new[]
        {
            "stasis_graphics_static.lib",
            "stasis_graphics.lib",
            "stasis_graphics.dll"
        };
    }
    else if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX))
    {
        candidates = new[]
        {
            "libstasis_graphics_static.a",
            "libstasis_graphics.dylib"
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

static int RunProcess(string fileName, string arguments)
{
    var psi = new ProcessStartInfo
    {
        FileName = fileName,
        Arguments = arguments,
        UseShellExecute = false
    };

    using var proc = Process.Start(psi)!;
    proc.WaitForExit();
    return proc.ExitCode;
}

static int BuildExecutable(string llPath, string outputPath, bool isTest, string? optLevel, bool enableLto, bool enableGraphics = false, string? graphicsLibPath = null)
{
    if (!TryFindTool("clang", out var clang))
    {
        Console.Error.WriteLine("error: build requires clang in PATH.");
        return 1;
    }

    var args = BuildClangArgs(llPath, outputPath, isTest, optLevel, enableLto, enableGraphics, graphicsLibPath);
    var exit = RunProcess(clang, args);
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

static void PrintUsage()
{
    Console.WriteLine("Usage:");
    Console.WriteLine("  stasisc run <file> [--module <name>] [--with-tests] [--emit-ir] [--graphics] [--graphics-lib <path>]");
    Console.WriteLine("  stasisc test [<file>|--all] [--module <name>] [--emit-ir]");
    Console.WriteLine("  stasisc build <file> [--module <name>] [--with-tests] [--out <path>] [--opt-level <0|1|2|3|s|z>] [--lto|--no-lto] [--graphics] [--graphics-lib <path>]");
    Console.WriteLine("  stasisc release <file> [--module <name>] [--out <path>] [--opt-level <0|1|2|3|s|z>] [--lto|--no-lto] [--graphics] [--graphics-lib <path>]");
    Console.WriteLine("  stasisc format <file>");
    Console.WriteLine("Defaults: execute via lli if available, else clang. Use --emit-ir to only write IR to stdout. With no path (or --all), 'test' runs every .stasis file under the working directory. Build/release require clang in PATH. 'release' defaults to -O3 with LTO.");
    Console.WriteLine("Graphics: use --graphics to enable SDL2/OpenGL graphics runtime. Specify --graphics-lib to override library path.");
}

static int RunAllTestsInDirectoryParallel(string[] files, bool includeTests, string moduleName, bool emitIrOnly, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, bool useLowerLock = true, int lowerDegree = 1) =>
    RunAllTestsInDirectoryParallelAsync(files, includeTests, moduleName, emitIrOnly, optLevel, enableLto, enableGraphics, graphicsLibPath, useLowerLock, lowerDegree).GetAwaiter().GetResult();

static async Task<int> RunAllTestsInDirectoryParallelAsync(string[] files, bool includeTests, string moduleName, bool emitIrOnly, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath, bool useLowerLock, int lowerDegree)
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
            var prep = PrepareForLower(file, emitIrOnly);
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
            var result = LowerPrepared(item, includeTests, moduleName, emitIrOnly, enableGraphics, useLowerLock);
            await resultChannel.Writer.WriteAsync(result);
        }
    })).ToArray();

    var exitCode = 0;
    var consumer = Task.Run(async () =>
    {
        await foreach (var result in resultChannel.Reader.ReadAllAsync())
        {
            exitCode = Math.Max(exitCode, ConsumeCompileResult(result, emitIrOnly, optLevel, enableLto, enableGraphics, graphicsLibPath));
        }
    });

    await Task.WhenAll(producers);
    prepChannel.Writer.Complete();
    await Task.WhenAll(lowerWorkers);
    resultChannel.Writer.Complete();
    await consumer;
    return exitCode;
}

static PrepareResult PrepareForLower(string path, bool emitIrOnly)
{
    var stopwatch = Stopwatch.StartNew();
    var diagnostics = new List<Diagnostic>();
    try
    {
        var source = File.ReadAllText(path);
        var parse = Parser.Parse(source);
        diagnostics.AddRange(parse.Diagnostics);
        var hasTests = parse.CompilationUnit.Declarations.OfType<TestDeclarationSyntax>().Any();

        if (parse.Diagnostics.Count > 0 || (!hasTests && !emitIrOnly))
        {
            return new PrepareResult(null, new CompileResult(path, source, hasTests, null, null, diagnostics, emitIrOnly, stopwatch.ElapsedMilliseconds));
        }

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        diagnostics.AddRange(sema.Diagnostics);
        if (sema.Diagnostics.Count > 0)
        {
            return new PrepareResult(null, new CompileResult(path, source, hasTests, null, null, diagnostics, emitIrOnly, stopwatch.ElapsedMilliseconds));
        }

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        stopwatch.Stop();
        return new PrepareResult(new PreparedForLower(path, source, parse.CompilationUnit, sema, layout, hasTests, stopwatch.ElapsedMilliseconds), null);
    }
    finally
    {
        stopwatch.Stop();
    }
}

static CompileResult LowerPrepared(PreparedForLower prep, bool includeTests, string moduleName, bool emitIrOnly, bool enableGraphics, bool useLowerLock)
{
    var stopwatch = Stopwatch.StartNew();
    var diagnostics = new List<Diagnostic>();
    string? tempLl = null;
    string? irForOutput = null;

    try
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
            return new CompileResult(prep.FilePath, prep.Source, prep.HasTests, tempLl, irForOutput, diagnostics, emitIrOnly, prep.PrepMilliseconds + stopwatch.ElapsedMilliseconds);
        }

        tempLl = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.ll");
        File.WriteAllText(tempLl, lower.Ir);

        return new CompileResult(prep.FilePath, prep.Source, prep.HasTests, tempLl, irForOutput, diagnostics, emitIrOnly, prep.PrepMilliseconds + stopwatch.ElapsedMilliseconds);
    }
    finally
    {
        stopwatch.Stop();
    }
}

static int ConsumeCompileResult(CompileResult result, bool emitIrOnly, string? optLevel, bool enableLto, bool enableGraphics, string? graphicsLibPath)
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

    if (!result.HasTests || string.IsNullOrEmpty(result.LlPath))
    {
        return 0;
    }

    Console.WriteLine($"=== {result.FilePath} ===");
    var executeExit = Execute("test", result.LlPath, optLevel, enableLto, enableGraphics, graphicsLibPath);
    testStopwatch.Stop();
    var total = result.CompileMilliseconds + testStopwatch.ElapsedMilliseconds;
    Console.WriteLine($"Total time={total}ms");

    try
    {
        if (File.Exists(result.LlPath))
        {
            File.Delete(result.LlPath);
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

sealed record CompileResult(
    string FilePath,
    string Source,
    bool HasTests,
    string? LlPath,
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
    long PrepMilliseconds);

sealed record PrepareResult(PreparedForLower? Prepared, CompileResult? Result);
