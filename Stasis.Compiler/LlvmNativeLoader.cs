using System.Linq;
using System.Runtime.InteropServices;
using LLVMSharp.Interop;

namespace Stasis.Compiler;

public static class LlvmNativeLoader
{
    private static bool _initialized;

    public static void EnsureLoaded()
    {
        if (_initialized)
        {
            return;
        }
        _initialized = true;

        var nativeName = RuntimeInformation.IsOSPlatform(OSPlatform.Windows)
            ? "libLLVM.dll"
            : RuntimeInformation.IsOSPlatform(OSPlatform.OSX)
                ? "libLLVM.dylib"
                : "libLLVM.so";

        var searchPaths = new List<string>();

        var explicitDir = Environment.GetEnvironmentVariable("LLVM_NATIVE_PATH");
        if (!string.IsNullOrWhiteSpace(explicitDir))
        {
            searchPaths.Add(Path.Combine(explicitDir, nativeName));
        }

        searchPaths.Add(Path.Combine(AppContext.BaseDirectory, nativeName));

        var nugetRoot = Environment.GetEnvironmentVariable("NUGET_PACKAGES")
                        ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".nuget", "packages");

        var rid = RuntimeInformation.IsOSPlatform(OSPlatform.Windows)
            ? "win-x64"
            : RuntimeInformation.IsOSPlatform(OSPlatform.OSX)
                ? (RuntimeInformation.ProcessArchitecture == Architecture.Arm64 ? "osx-arm64" : "osx-x64")
                : RuntimeInformation.ProcessArchitecture == Architecture.Arm64
                    ? "linux-arm64"
                    : "linux-x64";

        var version = "20.1.2";
        searchPaths.Add(Path.Combine(nugetRoot, "libllvm", version, "runtimes", rid, "native", nativeName));
        searchPaths.Add(Path.Combine(nugetRoot, $"libllvm.runtime.{rid}", version, "runtimes", rid, "native", nativeName));

        var runtimePrefix = $"libllvm.runtime.{rid}";
        if (Directory.Exists(nugetRoot))
        {
            foreach (var dir in Directory.EnumerateDirectories(nugetRoot, $"{runtimePrefix}*", SearchOption.TopDirectoryOnly))
            {
                searchPaths.Add(Path.Combine(dir, version, "runtimes", rid, "native", nativeName));
            }
        }

        var candidates = searchPaths
            .Where(File.Exists)
            .ToArray();

        NativeLibrary.SetDllImportResolver(typeof(LLVM).Assembly, (libraryName, assembly, searchPath) =>
        {
            if (!string.Equals(libraryName, "libLLVM", StringComparison.OrdinalIgnoreCase) &&
                !string.Equals(libraryName, nativeName, StringComparison.OrdinalIgnoreCase))
            {
                return IntPtr.Zero;
            }

            foreach (var candidate in candidates)
            {
                if (NativeLibrary.TryLoad(candidate, out var handle))
                {
                    return handle;
                }
            }

            return IntPtr.Zero;
        });

        foreach (var candidate in candidates)
        {
            if (NativeLibrary.TryLoad(candidate, out _))
            {
                return;
            }
        }

        if (NativeLibrary.TryLoad(nativeName, out _))
        {
            return;
        }

        throw new DllNotFoundException($"libLLVM native library not found. Checked: {string.Join(", ", searchPaths)}. Set LLVM_NATIVE_PATH or ensure libllvm runtime packages are restored.");
    }
}
