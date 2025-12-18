using System.Diagnostics;
using System.Text.RegularExpressions;
using Stasis.Compiler;
using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;

var options = Options.Parse(args);
if (options is null)
{
    PrintUsage();
    return 2;
}

LlvmNativeLoader.EnsureLoaded();

var samplesDir = Path.GetFullPath(options.SamplesDir);
if (!Directory.Exists(samplesDir))
{
    Console.Error.WriteLine($"error: samples dir not found: {samplesDir}");
    return 2;
}

var filter = options.FilterRegex is null ? null : new Regex(options.FilterRegex, RegexOptions.IgnoreCase | RegexOptions.Compiled);
var files = Directory.GetFiles(samplesDir, "*.stasis", SearchOption.TopDirectoryOnly)
    .OrderBy(Path.GetFileName)
    .Where(p => filter is null || filter.IsMatch(Path.GetFileName(p)))
    .ToArray();

if (files.Length == 0)
{
    Console.Error.WriteLine($"error: no .stasis files found under {samplesDir}");
    return 2;
}

var backends = options.Backend switch
{
    "llvm" => new[] { BackendType.Llvm },
    "cranelift" => new[] { BackendType.Cranelift },
    "both" => new[] { BackendType.Llvm, BackendType.Cranelift },
    _ => throw new InvalidOperationException($"unknown backend: {options.Backend}")
};

var results = new List<BenchResult>();
foreach (var backend in backends)
{
    foreach (var file in files)
    {
        var name = Path.GetFileName(file);
        var source = File.ReadAllText(file);

        for (var i = 0; i < options.WarmupIterations; i++)
        {
            CompileOnce(source, backend, name);
        }

        var samplesUs = new double[options.Iterations];
        for (var i = 0; i < options.Iterations; i++)
        {
            var sw = Stopwatch.StartNew();
            CompileOnce(source, backend, name);
            sw.Stop();
            samplesUs[i] = sw.ElapsedTicks * 1_000_000.0 / Stopwatch.Frequency;
        }

        results.Add(new BenchResult(backend, name, samplesUs));
    }
}

PrintSummary(results, files.Length, options);
return 0;

static void CompileOnce(string source, BackendType backend, string displayName)
{
    var parse = Parser.Parse(source);
    if (parse.Diagnostics.Count > 0)
    {
        throw new InvalidOperationException($"parse failed for {displayName}: {parse.Diagnostics[0].Message}");
    }

    var semantic = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
    if (semantic.Diagnostics.Count > 0)
    {
        throw new InvalidOperationException($"semantic failed for {displayName}: {semantic.Diagnostics[0].Message}");
    }

    var layout = new LayoutPlanner(parse.CompilationUnit, semantic.Symbols).Plan();

    if (backend == BackendType.Llvm)
    {
        var lower = new ModuleLowerer().LowerToIr(parse.CompilationUnit, semantic, layout, "bench", LowerOptions.Production);
        if (lower.Diagnostics.Count > 0)
        {
            throw new InvalidOperationException($"lower failed for {displayName}: {lower.Diagnostics[0].Message}");
        }
        _ = lower.Ir;
        return;
    }

    var options = new CodeGenerationOptions(
        ModuleName: "bench",
        IncludeTests: false,
        EmitTestHarness: false);

    using var generator = CodeGeneratorFactory.Create(backend, "bench");
    var result = generator.Generate(parse.CompilationUnit, semantic, layout, options);
    if (!result.Success)
    {
        var first = result.Diagnostics.FirstOrDefault()?.Message ?? "(no diagnostics)";
        throw new InvalidOperationException($"codegen failed for {displayName}: {first}");
    }
    _ = result.Ir;
}

static void PrintSummary(List<BenchResult> results, int fileCount, Options options)
{
    Console.WriteLine($"samples={fileCount} iterations={options.Iterations} warmup={options.WarmupIterations}");

    var byBackend = results
        .GroupBy(r => r.Backend)
        .OrderBy(g => g.Key.ToString(), StringComparer.OrdinalIgnoreCase);

    foreach (var group in byBackend)
    {
        var all = group.SelectMany(r => r.SamplesUs).OrderBy(x => x).ToArray();
        var avgUs = all.Length == 0 ? 0 : all.Average();
        var p95Us = all.Length == 0 ? 0 : all[(int)Math.Floor((all.Length - 1) * 0.95)];
        var maxUs = all.Length == 0 ? 0 : all[^1];
        Console.WriteLine($"{group.Key,-10} avg={FormatDuration(avgUs)} p95={FormatDuration(p95Us)} max={FormatDuration(maxUs)} (across all files/iterations)");
    }

    Console.WriteLine();
    Console.WriteLine("slowest (avg per file):");
    foreach (var backendGroup in results.GroupBy(r => r.Backend).OrderBy(g => g.Key.ToString(), StringComparer.OrdinalIgnoreCase))
    {
        var slowest = backendGroup
            .OrderByDescending(r => r.AvgUs)
            .ThenBy(r => r.File, StringComparer.OrdinalIgnoreCase)
            .Take(5);

        foreach (var row in slowest)
        {
            Console.WriteLine($"{row.Backend,-10} {row.File,-30} avg={FormatDuration(row.AvgUs),8} p95={FormatDuration(row.P95Us),8}");
        }
    }
}

static string FormatDuration(double microseconds)
{
    if (microseconds >= 1000.0)
    {
        return $"{microseconds / 1000.0:F2}ms";
    }

    if (microseconds >= 1.0)
    {
        return $"{microseconds:F1}us";
    }

    return $"{microseconds * 1000.0:F1}ns";
}

static void PrintUsage()
{
    Console.WriteLine("Usage:");
    Console.WriteLine("  dotnet run --project tools/bench-backends -- [--backend llvm|cranelift|both] [--samples-dir <dir>] [--filter <regex>] [--iterations <n>] [--warmup <n>]");
}

sealed record Options(string Backend, string SamplesDir, string? FilterRegex, int Iterations, int WarmupIterations)
{
    public static Options? Parse(string[] args)
    {
        var backend = "both";
        var samplesDir = Path.Combine(AppContext.BaseDirectory, "..", "..", "..", "..", "..", "samples");
        string? filter = null;
        var iterations = 20;
        var warmup = 3;

        var q = new Queue<string>(args);
        while (q.Count > 0)
        {
            var arg = q.Dequeue();
            switch (arg)
            {
                case "--backend" when q.Count > 0:
                    backend = q.Dequeue();
                    break;
                case "--samples-dir" when q.Count > 0:
                    samplesDir = q.Dequeue();
                    break;
                case "--filter" when q.Count > 0:
                    filter = q.Dequeue();
                    break;
                case "--iterations" when q.Count > 0 && int.TryParse(q.Dequeue(), out var iters) && iters > 0:
                    iterations = iters;
                    break;
                case "--warmup" when q.Count > 0 && int.TryParse(q.Dequeue(), out var w) && w >= 0:
                    warmup = w;
                    break;
                case "--help":
                case "-h":
                    return null;
                default:
                    Console.Error.WriteLine($"error: unknown arg: {arg}");
                    return null;
            }
        }

        backend = backend.ToLowerInvariant();
        if (backend is not ("llvm" or "cranelift" or "both"))
        {
            Console.Error.WriteLine("error: --backend must be llvm, cranelift, or both");
            return null;
        }

        return new Options(backend, samplesDir, filter, iterations, warmup);
    }
}

sealed record BenchResult(BackendType Backend, string File, double[] SamplesUs)
{
    public double AvgUs => SamplesUs.Length == 0 ? 0 : SamplesUs.Average();
    public double P95Us
    {
        get
        {
            if (SamplesUs.Length == 0) return 0;
            var sorted = SamplesUs.OrderBy(x => x).ToArray();
            return sorted[(int)Math.Floor((sorted.Length - 1) * 0.95)];
        }
    }
}
