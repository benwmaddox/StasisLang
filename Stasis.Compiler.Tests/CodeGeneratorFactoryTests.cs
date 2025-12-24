using Stasis.Compiler.IR;
using Xunit;

namespace Stasis.Compiler.Tests;

public class CodeGeneratorFactoryTests
{
    [Fact]
    public void GetDefaultBackend_UsesCraneliftForDebug()
    {
        var backend = CodeGeneratorFactory.GetDefaultBackend(isRelease: false);
        Assert.Equal(BackendType.Cranelift, backend);
    }

    [Fact]
    public void GetDefaultBackend_UsesLlvmForRelease()
    {
        var backend = CodeGeneratorFactory.GetDefaultBackend(isRelease: true);
        Assert.Equal(BackendType.Llvm, backend);
    }
}
