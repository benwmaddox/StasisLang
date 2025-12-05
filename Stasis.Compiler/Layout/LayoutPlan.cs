namespace Stasis.Compiler.Layout;

public sealed record LayoutPlan(IReadOnlyList<GlobalLayout> Globals, int TotalSize);
