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
}
