using System.Runtime.InteropServices;
using LLVMSharp.Interop;
using Stasis.Compiler.IR;
using Stasis.Compiler.Semantic;

namespace Stasis.Compiler.Tests;

public class IrBuilderTests
{
    static IrBuilderTests()
    {
        EnsureNativeLibLoaded();
    }

    private static void EnsureNativeLibLoaded()
    {
        var nativeName = RuntimeInformation.IsOSPlatform(OSPlatform.Windows)
            ? "libLLVM.dll"
            : RuntimeInformation.IsOSPlatform(OSPlatform.OSX)
                ? "libLLVM.dylib"
                : "libLLVM.so";

        var searchPaths = new List<string>();

        // Allow an explicit override for local runs/CI agents.
        var explicitDir = Environment.GetEnvironmentVariable("LLVM_NATIVE_PATH");
        if (!string.IsNullOrWhiteSpace(explicitDir))
        {
            searchPaths.Add(Path.Combine(explicitDir, nativeName));
        }

        // Try current directory first (in case the native was copied locally).
        searchPaths.Add(Path.Combine(AppContext.BaseDirectory, nativeName));

        var nugetRoot = Environment.GetEnvironmentVariable("NUGET_PACKAGES")
                        ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".nuget", "packages");

        var rid = RuntimeInformation.IsOSPlatform(OSPlatform.Windows)
            ? "win-x64"
            : RuntimeInformation.IsOSPlatform(OSPlatform.OSX)
                ? "osx-x64"
                : "linux-x64";

        // Check both the umbrella libllvm package and the RID-specific runtime package.
        var version = "20.1.2";
        searchPaths.Add(Path.Combine(nugetRoot, "libllvm", version, "runtimes", rid, "native", nativeName));
        searchPaths.Add(Path.Combine(nugetRoot, $"libllvm.runtime.{rid}", version, "runtimes", rid, "native", nativeName));

        // Fall back to any libllvm.runtime.* that matches the RID (helps if RID split is newer/older).
        var runtimePrefix = $"libllvm.runtime.{rid}";
        if (Directory.Exists(nugetRoot))
        {
            foreach (var dir in Directory.EnumerateDirectories(nugetRoot, $"{runtimePrefix}*", SearchOption.TopDirectoryOnly))
            {
                searchPaths.Add(Path.Combine(dir, version, "runtimes", rid, "native", nativeName));
            }
        }

        foreach (var candidate in searchPaths)
        {
            if (File.Exists(candidate))
            {
                NativeLibrary.Load(candidate);
                return;
            }
        }

        throw new DllNotFoundException($"libLLVM native library not found. Checked: {string.Join(", ", searchPaths)}. Set LLVM_NATIVE_PATH or ensure libllvm runtime packages are restored.");
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
