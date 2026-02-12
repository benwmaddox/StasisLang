namespace Stasis.Compiler.Layout;

public enum FieldType
{
    Unknown,
    Bool,
    U8,
    U16,
    U32,
    I32,
    F32,
    F64,
    String
}

public sealed record FieldLayout(string Name, int Offset, int Size, FieldType Type = FieldType.Unknown, int ArrayCount = 1);
