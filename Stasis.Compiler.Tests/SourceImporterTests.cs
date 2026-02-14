using System.IO;
using Xunit;

namespace Stasis.Compiler.Tests;

public class SourceImporterTests
{
    [Fact]
    public void ExpandImports_InlinesRelativeFiles()
    {
        var tempDir = Directory.CreateTempSubdirectory("stasis_imports");
        try
        {
            var entryPath = Path.Combine(tempDir.FullName, "main.stasis");
            var importedPath = Path.Combine(tempDir.FullName, "lib.stasis");
            File.WriteAllText(importedPath, "function helper(): i32 { return 1; }");
            File.WriteAllText(entryPath, "import \"lib.stasis\";\nfunction main(): i32 { return helper(); }");

            var diagnostics = new List<Diagnostic>();
            var source = File.ReadAllText(entryPath);
            var result = SourceImporter.ExpandImports(entryPath, source, diagnostics);

            Assert.Empty(diagnostics);
            Assert.Contains("function helper()", result.ExpandedSource);
            Assert.DoesNotContain("import \"lib.stasis\"", result.ExpandedSource);
        }
        finally
        {
            tempDir.Delete(true);
        }
    }

    [Fact]
    public void ExpandImports_ReportsMissingFile()
    {
        var tempDir = Directory.CreateTempSubdirectory("stasis_imports");
        try
        {
            var entryPath = Path.Combine(tempDir.FullName, "main.stasis");
            File.WriteAllText(entryPath, "import \"missing.stasis\";\nfunction main(): i32 { return 0; }");

            var diagnostics = new List<Diagnostic>();
            var source = File.ReadAllText(entryPath);
            var result = SourceImporter.ExpandImports(entryPath, source, diagnostics);

            Assert.Single(diagnostics);
            Assert.Contains("Import not found", diagnostics[0].Message);
            Assert.Contains("function main()", result.ExpandedSource);
        }
        finally
        {
            tempDir.Delete(true);
        }
    }

    [Fact]
    public void ExpandImports_RejectsStdlibGlobals()
    {
        var tempDir = Directory.CreateTempSubdirectory("stasis_imports");
        try
        {
            var stdlibDir = Path.Combine(tempDir.FullName, "src", "stdlib");
            Directory.CreateDirectory(stdlibDir);
            var entryPath = Path.Combine(tempDir.FullName, "main.stasis");
            var stdlibPath = Path.Combine(stdlibDir, "bad.stasis");
            File.WriteAllText(stdlibPath, "global bad: i32;");
            File.WriteAllText(entryPath, "import \"src/stdlib/bad.stasis\";\nfunction main(): i32 { return 0; }");

            var diagnostics = new List<Diagnostic>();
            var source = File.ReadAllText(entryPath);
            _ = SourceImporter.ExpandImports(entryPath, source, diagnostics);

            Assert.Single(diagnostics);
            Assert.Contains("stdlib files may not declare globals", diagnostics[0].Message);
        }
        finally
        {
            tempDir.Delete(true);
        }
    }

    [Fact]
    public void ExpandImports_FallsBackToPlatformSpecificFile()
    {
        var tempDir = Directory.CreateTempSubdirectory("stasis_imports");
        try
        {
            var platform = OperatingSystem.IsWindows() ? "windows"
                : OperatingSystem.IsLinux() ? "linux"
                : OperatingSystem.IsMacOS() ? "macos"
                : "unknown";

            if (platform == "unknown")
            {
                return;
            }

            var entryPath = Path.Combine(tempDir.FullName, "main.stasis");
            var importedPath = Path.Combine(tempDir.FullName, $"lib.{platform}.stasis");
            File.WriteAllText(importedPath, "function helper(): i32 { return 2; }");
            File.WriteAllText(entryPath, "import \"lib.stasis\";\nfunction main(): i32 { return helper(); }");

            var diagnostics = new List<Diagnostic>();
            var source = File.ReadAllText(entryPath);
            var result = SourceImporter.ExpandImports(entryPath, source, diagnostics);

            Assert.Empty(diagnostics);
            Assert.Contains("function helper()", result.ExpandedSource);
        }
        finally
        {
            tempDir.Delete(true);
        }
    }

    [Fact]
    public void ExpandImports_UsesSourceLoaderForImportedFiles()
    {
        var tempDir = Directory.CreateTempSubdirectory("stasis_imports_overlay");
        try
        {
            var entryPath = Path.Combine(tempDir.FullName, "main.stasis");
            var importedPath = Path.Combine(tempDir.FullName, "live.stasis");
            var source = "import \"live.stasis\";\nfunction main(): i32 { return helper(); }";
            var diagnostics = new List<Diagnostic>();

            string? Loader(string path)
            {
                if (string.Equals(Path.GetFullPath(path), Path.GetFullPath(importedPath), StringComparison.OrdinalIgnoreCase))
                {
                    return "function helper(): i32 { return 3; }";
                }

                return null;
            }

            var result = SourceImporter.ExpandImports(entryPath, source, diagnostics, Loader);

            Assert.Empty(diagnostics);
            Assert.Contains("function helper()", result.ExpandedSource);
            Assert.DoesNotContain("import \"live.stasis\"", result.ExpandedSource);
        }
        finally
        {
            tempDir.Delete(true);
        }
    }

    [Fact]
    public void ExpandImports_UsesOverlayPlatformSpecificFile()
    {
        var tempDir = Directory.CreateTempSubdirectory("stasis_imports_overlay_platform");
        try
        {
            var platform = OperatingSystem.IsWindows() ? "windows"
                : OperatingSystem.IsLinux() ? "linux"
                : OperatingSystem.IsMacOS() ? "macos"
                : "unknown";

            if (platform == "unknown")
            {
                return;
            }

            var entryPath = Path.Combine(tempDir.FullName, "main.stasis");
            var importedPath = Path.Combine(tempDir.FullName, $"lib.{platform}.stasis");
            var source = "import \"lib.stasis\";\nfunction main(): i32 { return helper(); }";
            var diagnostics = new List<Diagnostic>();

            string? Loader(string path)
            {
                if (string.Equals(Path.GetFullPath(path), Path.GetFullPath(importedPath), StringComparison.OrdinalIgnoreCase))
                {
                    return "function helper(): i32 { return 4; }";
                }

                return null;
            }

            var result = SourceImporter.ExpandImports(entryPath, source, diagnostics, Loader);

            Assert.Empty(diagnostics);
            Assert.Contains("function helper()", result.ExpandedSource);
        }
        finally
        {
            tempDir.Delete(true);
        }
    }
}
