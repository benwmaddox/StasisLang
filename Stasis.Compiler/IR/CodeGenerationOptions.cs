namespace Stasis.Compiler.IR;

/// <summary>
/// Optimization level for code generation.
/// </summary>
public enum OptimizationLevel
{
    /// <summary>No optimization (debug builds).</summary>
    None = 0,
    /// <summary>Basic optimizations.</summary>
    Basic = 1,
    /// <summary>Standard optimizations (default for release).</summary>
    Standard = 2,
    /// <summary>Aggressive optimizations (-O3).</summary>
    Aggressive = 3,
    /// <summary>Optimize for size (-Os).</summary>
    Size = 4,
    /// <summary>Optimize for minimum size (-Oz).</summary>
    MinSize = 5
}

/// <summary>
/// Options for code generation that are backend-agnostic.
/// </summary>
public sealed record CodeGenerationOptions(
    string ModuleName = "module",
    bool IncludeTests = true,
    bool EmitTestHarness = true,
    bool HeadlessGraphics = true,
    OptimizationLevel Optimization = OptimizationLevel.None,
    bool AllowReachabilityFallback = true)
{
    /// <summary>
    /// Default options for debug/development builds.
    /// </summary>
    public static CodeGenerationOptions Debug { get; } = new();

    /// <summary>
    /// Options for production/release builds (no tests, aggressive optimization).
    /// </summary>
    public static CodeGenerationOptions Release { get; } = new(
        IncludeTests: false,
        EmitTestHarness: false,
        Optimization: OptimizationLevel.Aggressive,
        AllowReachabilityFallback: false);

    /// <summary>
    /// Options for graphics-enabled builds.
    /// </summary>
    public static CodeGenerationOptions Graphics { get; } = new(HeadlessGraphics: false);

    /// <summary>
    /// Converts to LowerOptions for backward compatibility.
    /// </summary>
    public LowerOptions ToLowerOptions() => new(IncludeTests, EmitTestHarness, HeadlessGraphics, AllowReachabilityFallback);
}
