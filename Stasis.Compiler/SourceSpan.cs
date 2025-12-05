namespace Stasis.Compiler;

public readonly record struct SourceSpan(int Start, int Length)
{
    public int End => Start + Length;
}
