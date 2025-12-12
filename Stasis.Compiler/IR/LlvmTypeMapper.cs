using LLVMSharp.Interop;
using Stasis.Compiler.Semantic;

namespace Stasis.Compiler.IR;

public sealed class LlvmTypeMapper
{
    private readonly LLVMContextRef _context;

    public LlvmTypeMapper(LLVMContextRef context)
    {
        _context = context;
    }

    public LLVMTypeRef Map(TypeSymbol type) =>
        type switch
        {
            VoidTypeSymbol => LLVMTypeRef.Void,
            PrimitiveTypeSymbol p => MapPrimitive(p.PrimitiveName),
            ArrayTypeSymbol a => a.Size > 0
                ? LLVMTypeRef.CreateArray(Map(a.ElementType), (uint)a.Size)
                : LLVMTypeRef.CreatePointer(Map(a.ElementType), 0),
            NamedTypeSymbol => LLVMTypeRef.Int32, // treat struct/enums as indices into SoA storage
            _ => LLVMTypeRef.Int32
        };

    private LLVMTypeRef MapPrimitive(string name) =>
        name switch
        {
            "bool" => LLVMTypeRef.Int32,
            "u8" => LLVMTypeRef.Int8,
            "u16" => LLVMTypeRef.Int16,
            "u32" => LLVMTypeRef.Int32,
            "i32" => LLVMTypeRef.Int32,
            "f32" => LLVMTypeRef.Float,
            "f64" => LLVMTypeRef.Double,
            "string" => LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), // bare string is a pointer to bytes
            "void" => LLVMTypeRef.Void,
            _ => LLVMTypeRef.Int32
        };
}
