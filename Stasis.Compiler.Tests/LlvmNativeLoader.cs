using System.Runtime.InteropServices;

namespace Stasis.Compiler.Tests;

public static class LlvmNativeLoader
{
    public static void EnsureLoaded()
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
}
