using System.Diagnostics;
using System.Linq;
using System.Runtime.InteropServices;
using Stasis.Compiler;
using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;

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

while (cliArgs.Count > 0)
{
    var arg = cliArgs.Dequeue();
    switch (arg)
    {
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
        case "--emit-ir":
            emitIrOnly = true;
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
    PrintUsage();
    return;
}

if (!File.Exists(path))
{
    Console.Error.WriteLine($"error: file not found: {path}");
    Environment.Exit(1);
}

var source = File.ReadAllText(path);
var parse = Parser.Parse(source);
if (parse.Diagnostics.Count > 0)
{
    PrintDiagnostics(parse.Diagnostics);
    Environment.Exit(1);
}

LlvmNativeLoader.EnsureLoaded();

var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
if (sema.Diagnostics.Count > 0)
{
    PrintDiagnostics(sema.Diagnostics);
    Environment.Exit(1);
}

var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
var lowerer = new ModuleLowerer();
var lowerOptions = includeTests ? LowerOptions.Default : LowerOptions.Production;
var lower = lowerer.LowerToIr(parse.CompilationUnit, sema, layout, moduleName, lowerOptions);
if (lower.Diagnostics.Count > 0)
{
    PrintDiagnostics(lower.Diagnostics);
    Console.WriteLine(lower.Ir);
    Environment.Exit(1);
}

if (emitIrOnly)
{
    Console.WriteLine(lower.Ir);
    Environment.Exit(lower.Diagnostics.Count > 0 ? 1 : 0);
}

var tempLl = Path.Combine(Path.GetTempPath(), $"stasis_{Guid.NewGuid():N}.ll");
File.WriteAllText(tempLl, lower.Ir);

try
{
    var exitCode = Execute(mode, tempLl);
    Environment.Exit(exitCode);
}
finally
{
    if (File.Exists(tempLl))
    {
        File.Delete(tempLl);
    }
}

static int Execute(string mode, string llPath)
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
            var args = BuildClangArgs(llPath, exePath, mode == "test");
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

static string BuildClangArgs(string llPath, string exePath, bool isTest)
{
    var args = new List<string> { $"\"{llPath}\"", "-o", $"\"{exePath}\"" };
    args.Add("-Wno-override-module");
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

static void PrintUsage()
{
    Console.WriteLine("Usage:");
    Console.WriteLine("  stasisc run <file> [--module <name>] [--with-tests] [--emit-ir]");
    Console.WriteLine("  stasisc test <file> [--module <name>] [--emit-ir]");
    Console.WriteLine("Defaults: execute via lli if available, else clang. Use --emit-ir to only write IR to stdout.");
}

static void PrintDiagnostics(IEnumerable<Diagnostic> diagnostics)
{
    foreach (var d in diagnostics)
    {
        Console.Error.WriteLine($"error: {d.Message} @ {d.Span.Start}");
    }
}
