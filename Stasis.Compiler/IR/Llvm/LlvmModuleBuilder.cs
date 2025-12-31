using System.Runtime.InteropServices;
using LLVMSharp.Interop;
using Stasis.Compiler.Semantic;

namespace Stasis.Compiler.IR.Llvm;

public sealed class LlvmModuleBuilder : IDisposable
{
    private readonly bool exportGlobals;
    public LLVMContextRef Context { get; }
    public LLVMModuleRef Module { get; }
    public LlvmTypeMapper TypeMapper { get; }

    public LlvmModuleBuilder(string moduleName, string? targetTriple = null, bool exportGlobals = false)
    {
        this.exportGlobals = exportGlobals;
        Context = LLVMContextRef.Create();
        Module = Context.CreateModuleWithName(moduleName);
        var module = Module;
        module.Target = string.IsNullOrWhiteSpace(targetTriple) ? GetHostTriple() : targetTriple;
        Module = module;
        TypeMapper = new LlvmTypeMapper(Context);
    }

    public LLVMValueRef DefineGlobalArray(string name, LLVMTypeRef elementType, uint length)
    {
        var arrType = LLVMTypeRef.CreateArray(elementType, length);
        var global = Module.AddGlobal(arrType, name);
        global.Linkage = exportGlobals ? LLVMLinkage.LLVMExternalLinkage : LLVMLinkage.LLVMInternalLinkage;
        global.Initializer = LLVMValueRef.CreateConstNull(arrType);
        return global;
    }

    public LLVMValueRef DefineGlobalScalar(string name, LLVMTypeRef elementType)
    {
        var global = Module.AddGlobal(elementType, name);
        global.Linkage = exportGlobals ? LLVMLinkage.LLVMExternalLinkage : LLVMLinkage.LLVMInternalLinkage;
        global.Initializer = LLVMValueRef.CreateConstNull(elementType);
        return global;
    }

    public LLVMValueRef DefineConstantScalar(string name, LLVMTypeRef elementType, LLVMValueRef initializer)
    {
        var global = Module.AddGlobal(elementType, name);
        global.Linkage = exportGlobals ? LLVMLinkage.LLVMExternalLinkage : LLVMLinkage.LLVMInternalLinkage;
        global.IsGlobalConstant = true;
        global.Initializer = initializer;
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

    private static string GetHostTriple()
    {
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            return RuntimeInformation.OSArchitecture switch
            {
                Architecture.Arm64 => "aarch64-pc-windows-msvc",
                _ => "x86_64-pc-windows-msvc"
            };
        }

        if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX))
        {
            return RuntimeInformation.OSArchitecture switch
            {
                Architecture.Arm64 => "aarch64-apple-darwin",
                _ => "x86_64-apple-darwin"
            };
        }

        return RuntimeInformation.OSArchitecture switch
        {
            Architecture.Arm64 => "aarch64-unknown-linux-gnu",
            _ => "x86_64-pc-linux-gnu"
        };
    }
}
