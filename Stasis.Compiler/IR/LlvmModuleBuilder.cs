using LLVMSharp.Interop;
using Stasis.Compiler.Semantic;

namespace Stasis.Compiler.IR;

public sealed class LlvmModuleBuilder : IDisposable
{
    public LLVMContextRef Context { get; }
    public LLVMModuleRef Module { get; }
    public LlvmTypeMapper TypeMapper { get; }

    public LlvmModuleBuilder(string moduleName)
    {
        Context = LLVMContextRef.Create();
        Module = Context.CreateModuleWithName(moduleName);
        TypeMapper = new LlvmTypeMapper(Context);
    }

    public LLVMValueRef DefineGlobalArray(string name, LLVMTypeRef elementType, uint length)
    {
        var arrType = LLVMTypeRef.CreateArray(elementType, length);
        var global = Module.AddGlobal(arrType, name);
        global.Linkage = LLVMLinkage.LLVMInternalLinkage;
        global.Initializer = LLVMValueRef.CreateConstNull(arrType);
        return global;
    }

    public LLVMValueRef DefineGlobalScalar(string name, LLVMTypeRef elementType)
    {
        var global = Module.AddGlobal(elementType, name);
        global.Linkage = LLVMLinkage.LLVMInternalLinkage;
        global.Initializer = LLVMValueRef.CreateConstNull(elementType);
        return global;
    }

    public LLVMValueRef DefineFunction(string name, LLVMTypeRef returnType, params LLVMTypeRef[] paramTypes)
    {
        var funcType = LLVMTypeRef.CreateFunction(returnType, paramTypes, false);
        return Module.AddFunction(name, funcType);
    }

    public string EmitToString() => Module.PrintToString();

    public void Dispose()
    {
        Module.Dispose();
        Context.Dispose();
    }
}
