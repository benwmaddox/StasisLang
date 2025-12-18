using Stasis.Compiler.Semantic;

namespace Stasis.Compiler.IR.Cranelift;

/// <summary>
/// Maps Stasis types to Cranelift types.
/// Cranelift uses a different type system than LLVM.
/// </summary>
public sealed class CraneliftTypeMapper
{
    /// <summary>
    /// Cranelift type representation.
    /// </summary>
    public enum ClifType
    {
        I8,
        I16,
        I32,
        I64,
        F32,
        F64,
        B1,     // Boolean
        R64,    // Reference (pointer)
    }

    /// <summary>
    /// Maps a Stasis type to a Cranelift type.
    /// </summary>
    public ClifType Map(TypeSymbol type) =>
        type switch
        {
            VoidTypeSymbol => throw new InvalidOperationException("Cannot map void to Cranelift type"),
            PrimitiveTypeSymbol p => MapPrimitive(p.PrimitiveName),
            ArrayTypeSymbol => ClifType.R64, // Arrays are pointers
            NamedTypeSymbol => ClifType.I32, // Struct/enum indices
            _ => ClifType.I32
        };

    /// <summary>
    /// Gets the size in bytes for a Cranelift type.
    /// </summary>
    public static int GetTypeSize(ClifType type) =>
        type switch
        {
            ClifType.I8 or ClifType.B1 => 1,
            ClifType.I16 => 2,
            ClifType.I32 => 4,
            ClifType.I64 or ClifType.R64 or ClifType.F64 => 8,
            ClifType.F32 => 4,
            _ => 4
        };

    private static ClifType MapPrimitive(string name) =>
        name switch
        {
            "bool" => ClifType.I32,  // Use i32 for bool like LLVM
            "u8" => ClifType.I8,
            "u16" => ClifType.I16,
            "u32" => ClifType.I32,
            "i32" => ClifType.I32,
            "f32" => ClifType.F32,
            "f64" => ClifType.F64,
            "string" => ClifType.R64, // String is a pointer
            "void" => throw new InvalidOperationException("Cannot map void to Cranelift type"),
            _ => ClifType.I32
        };
}
