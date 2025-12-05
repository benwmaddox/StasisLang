using LLVMSharp.Interop;
using Stasis.Compiler.IR;
using Stasis.Compiler.Semantic;

namespace Stasis.Compiler.Tests;

public class IrBuilderTests
{
    static IrBuilderTests()
    {
        LlvmNativeLoader.EnsureLoaded();
    }

    [Fact]
    public void Maps_primitives_to_expected_widths()
    {
        using var builder = new LlvmModuleBuilder("types");

        Assert.Equal(LLVMTypeRef.Int8, builder.TypeMapper.Map(new PrimitiveTypeSymbol("u8")));
        Assert.Equal(LLVMTypeRef.Int16, builder.TypeMapper.Map(new PrimitiveTypeSymbol("u16")));
        Assert.Equal(LLVMTypeRef.Int32, builder.TypeMapper.Map(new PrimitiveTypeSymbol("i32")));
        Assert.Equal(LLVMTypeRef.Float, builder.TypeMapper.Map(new PrimitiveTypeSymbol("f32")));
        Assert.Equal(LLVMTypeRef.Double, builder.TypeMapper.Map(new PrimitiveTypeSymbol("f64")));
        Assert.Equal(LLVMTypeRef.Int32, builder.TypeMapper.Map(new PrimitiveTypeSymbol("bool")));
    }

    [Fact]
    public void Creates_global_array()
    {
        using var builder = new LlvmModuleBuilder("globals");
        builder.DefineGlobalArray("temps", LLVMTypeRef.Float, 3);

        var ir = builder.EmitToString();
        Assert.Contains("@temps = internal global [3 x float] zeroinitializer", ir);
    }

    [Fact]
    public void Creates_function_signature()
    {
        using var builder = new LlvmModuleBuilder("funcs");
        var i32 = builder.TypeMapper.Map(new PrimitiveTypeSymbol("i32"));
        builder.DefineFunction("add", i32, i32, i32);

        var ir = builder.EmitToString();
        Assert.Matches("(declare|define) i32 @add\\([^)]*i32[^)]*i32", ir);
    }
}
