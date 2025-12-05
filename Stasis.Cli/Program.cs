using Stasis.Compiler;
using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;

var argv = Environment.GetCommandLineArgs().Skip(1).ToList();
if (argv.Count == 0 || argv.Contains("--help"))
{
    PrintUsage();
    return;
}

var mode = "run";
if (argv[0].Equals("run", StringComparison.OrdinalIgnoreCase) || argv[0].Equals("test", StringComparison.OrdinalIgnoreCase))
{
    mode = argv[0].ToLowerInvariant();
    argv.RemoveAt(0);
}

var includeTests = argv.Remove("--with-tests") || mode == "test";
var moduleNameIndex = argv.IndexOf("--module");
string moduleName = "module";
if (moduleNameIndex >= 0 && moduleNameIndex + 1 < argv.Count)
{
    moduleName = argv[moduleNameIndex + 1];
    argv.RemoveAt(moduleNameIndex + 1);
    argv.RemoveAt(moduleNameIndex);
}

if (argv.Count == 0)
{
    PrintUsage();
    return;
}

var path = argv[0];
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
}

Console.WriteLine(lower.Ir);
if (lower.Diagnostics.Count > 0)
{
    Environment.Exit(1);
}

static void PrintUsage()
{
    Console.WriteLine("Usage:");
    Console.WriteLine("  stasisc run <file> [--module <name>] [--with-tests]");
    Console.WriteLine("  stasisc test <file> [--module <name>]");
    Console.WriteLine("Emits LLVM IR to stdout. Defaults to production lowering (tests omitted) unless using 'test' or --with-tests.");
}

static void PrintDiagnostics(IEnumerable<Diagnostic> diagnostics)
{
    foreach (var d in diagnostics)
    {
        Console.Error.WriteLine($"error: {d.Message} @ {d.Span.Start}");
    }
}
