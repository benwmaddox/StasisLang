using Stasis.Compiler;
using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;

var args = Environment.GetCommandLineArgs().Skip(1).ToList();
if (args.Count == 0 || args.Contains("--help"))
{
    PrintUsage();
    return;
}

var includeTests = args.Remove("--with-tests");
var moduleNameIndex = args.IndexOf("--module");
string moduleName = "module";
if (moduleNameIndex >= 0 && moduleNameIndex + 1 < args.Count)
{
    moduleName = args[moduleNameIndex + 1];
    args.RemoveAt(moduleNameIndex + 1);
    args.RemoveAt(moduleNameIndex);
}

if (args.Count == 0)
{
    PrintUsage();
    return;
}

var path = args[0];
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
    Console.WriteLine("Usage: stasisc <file> [--with-tests] [--module <name>] [--help]");
    Console.WriteLine("  Emits LLVM IR to stdout. Defaults to production lowering (tests omitted).");
}

static void PrintDiagnostics(IEnumerable<Diagnostic> diagnostics)
{
    foreach (var d in diagnostics)
    {
        Console.Error.WriteLine($"error: {d.Message} @ {d.Span.Start}");
    }
}
