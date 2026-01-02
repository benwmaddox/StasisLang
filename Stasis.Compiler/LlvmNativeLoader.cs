using System.Runtime.InteropServices;

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
            AppContext.SetSwitch("LLVMSharp.Interop.DisableResolveLibraryHook", true);
            searchPaths.Add(Path.Combine(explicitDir, nativeName));
            PrependLibraryPath(explicitDir);
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

        foreach (var candidate in searchPaths)
        {
            if (File.Exists(candidate))
            {
                NativeLibrary.Load(candidate);
                return;
            }
        }

        if (NativeLibrary.TryLoad(nativeName, out _))
        {
            return;
        }

        throw new DllNotFoundException($"libLLVM native library not found. Checked: {string.Join(", ", searchPaths)}. Set LLVM_NATIVE_PATH or ensure libllvm runtime packages are restored.");
    }

    private static void PrependLibraryPath(string path)
    {
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            PrependEnvPath("PATH", path);
            return;
        }

        if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX))
        {
            PrependEnvPath("DYLD_LIBRARY_PATH", path);
            PrependEnvPath("DYLD_FALLBACK_LIBRARY_PATH", path);
            return;
        }

        PrependEnvPath("LD_LIBRARY_PATH", path);
    }

    private static void PrependEnvPath(string variable, string path)
    {
        var existing = Environment.GetEnvironmentVariable(variable);
        if (string.IsNullOrWhiteSpace(existing))
        {
            Environment.SetEnvironmentVariable(variable, path);
            return;
        }

        var separator = Path.PathSeparator;
        var segments = existing.Split(separator, StringSplitOptions.RemoveEmptyEntries);
        foreach (var segment in segments)
        {
            if (string.Equals(segment, path, StringComparison.OrdinalIgnoreCase))
            {
                return;
            }
        }

        Environment.SetEnvironmentVariable(variable, $"{path}{separator}{existing}");
    }
}
