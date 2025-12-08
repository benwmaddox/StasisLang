using System.Diagnostics;
using System.Linq;
using System.Runtime.InteropServices;
using Stasis.Compiler;
using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;
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

if (runAllInDirectory && mode == "test")
{
    var root = Directory.Exists(path) ? path : Path.GetDirectoryName(path)!;
    var files = Directory.GetFiles(root, "*.stasis", SearchOption.AllDirectories).OrderBy(p => p).ToArray();
    if (files.Length == 0)
    {
        Console.Error.WriteLine($"error: no .stasis files found under {root}");
        Environment.Exit(1);
    }

    var overallExit = 0;
    foreach (var file in files)
    {
        Console.WriteLine($"=== {file} ===");
        overallExit = Math.Max(overallExit, ProcessFile(file, mode, includeTests, moduleName, emitIrOnly, outputPath, optLevel, enableLto));
    }
    Environment.Exit(overallExit);
}

var singleExit = ProcessFile(path, mode, includeTests, moduleName, emitIrOnly, outputPath, optLevel, enableLto);
Environment.Exit(singleExit);

static int ProcessFile(string path, string mode, bool includeTests, string moduleName, bool emitIrOnly, string? outputPath, string? optLevel, bool enableLto)
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

        LlvmNativeLoader.EnsureLoaded();

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        if (sema.Diagnostics.Count > 0)
        {
            PrintDiagnostics(sema.Diagnostics, source, path);
            return 1;
        }

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        var lowerer = new ModuleLowerer();
        var lowerOptions = includeTests ? LowerOptions.Default : LowerOptions.Production;
        var lower = lowerer.LowerToIr(parse.CompilationUnit, sema, layout, moduleName, lowerOptions);
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
            var exitCode = BuildExecutable(tempLl, outPath, includeTests, optLevel, enableLto);
            return exitCode;
        }

        var executeExit = Execute(mode, tempLl, optLevel, enableLto);
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

static int Execute(string mode, string llPath, string? optLevel, bool enableLto)
{
    if (TryFindTool("lli", out var lli))
    {
        return RunProcess(lli, mode == "test" ? $"-entry-function=run_tests \"{llPath}\"" : $"\"{llPath}\"");
    }

    if (TryFindTool("clang", out var clang))
    {
        var exePath = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}" + (RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".exe" : string.Empty));
        try
        {
            var args = BuildClangArgs(llPath, exePath, mode == "test", optLevel, enableLto);
            var exit = RunProcess(clang, args);
            if (exit != 0)
            {
                return exit;
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

static string BuildClangArgs(string llPath, string exePath, bool isTest, string? optLevel, bool enableLto)
{
    var args = new List<string> { $"\"{llPath}\"", "-o", $"\"{exePath}\"" };
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
            args.Add("-lucrt");
            args.Add("-lkernel32");
            args.Add("-llegacy_stdio_definitions");
        }
    }
    else if (isTest)
    {
        args.Add("-Wl,-e,run_tests");
    }

    return string.Join(" ", args);
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

static int BuildExecutable(string llPath, string outputPath, bool isTest, string? optLevel, bool enableLto)
{
    if (!TryFindTool("clang", out var clang))
    {
        Console.Error.WriteLine("error: build requires clang in PATH.");
        return 1;
    }

    var args = BuildClangArgs(llPath, outputPath, isTest, optLevel, enableLto);
    var exit = RunProcess(clang, args);
    if (exit != 0)
    {
        return exit;
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
    Console.WriteLine("  stasisc run <file> [--module <name>] [--with-tests] [--emit-ir]");
    Console.WriteLine("  stasisc test [<file>|--all] [--module <name>] [--emit-ir]");
    Console.WriteLine("  stasisc build <file> [--module <name>] [--with-tests] [--out <path>] [--opt-level <0|1|2|3|s|z>] [--lto|--no-lto]");
    Console.WriteLine("  stasisc release <file> [--module <name>] [--out <path>] [--opt-level <0|1|2|3|s|z>] [--lto|--no-lto]");
    Console.WriteLine("  stasisc format <file>");
    Console.WriteLine("Defaults: execute via lli if available, else clang. Use --emit-ir to only write IR to stdout. With no path (or --all), 'test' runs every .stasis file under the working directory. Build/release require clang in PATH. 'release' defaults to -O3 with LTO.");
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
