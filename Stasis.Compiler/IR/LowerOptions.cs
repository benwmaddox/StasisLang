namespace Stasis.Compiler.IR;

public sealed record LowerOptions(bool IncludeTests = true, bool EmitTestHarness = true)
{
    public static LowerOptions Default { get; } = new();
    public static LowerOptions Production { get; } = new(IncludeTests: false, EmitTestHarness: false);
}
