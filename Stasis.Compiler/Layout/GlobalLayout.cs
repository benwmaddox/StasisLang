namespace Stasis.Compiler.Layout;

public sealed record GlobalLayout(string Name, int Offset, int Size, IReadOnlyList<FieldLayout> Fields);
