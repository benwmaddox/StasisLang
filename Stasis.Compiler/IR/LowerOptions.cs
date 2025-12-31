namespace Stasis.Compiler.IR;

public sealed record LowerOptions(bool IncludeTests = true, bool EmitTestHarness = true, bool HeadlessGraphics = true, bool AllowReachabilityFallback = true, string? TargetTriple = null)
{
    public static LowerOptions Default { get; } = new();
    public static LowerOptions Production { get; } = new(IncludeTests: false, EmitTestHarness: false, AllowReachabilityFallback: false);
    public static LowerOptions Graphics { get; } = new(HeadlessGraphics: false);
}
