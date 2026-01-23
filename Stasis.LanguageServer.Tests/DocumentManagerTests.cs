namespace Stasis.LanguageServer.Tests;

using Stasis.LanguageServer.Services;
using Stasis.Compiler;
using Xunit;
using System.IO;

public class DocumentManagerTests
{
    [Fact]
    public void GetOrCreateDocument_CreatesNewDocument()
    {
        // Arrange
        var manager = new DocumentManager();
        var uri = "file:///test.stasis";
        var content = "let x: i32 = 5;";

        // Act
        var doc = manager.GetOrCreateDocument(uri, content);

        // Assert
        Assert.NotNull(doc);
        Assert.Equal(content, doc.Content);
        Assert.Equal(0, doc.Version);
    }

    [Fact]
    public void GetOrCreateDocument_ReturnsSameDocumentOnMultipleCalls()
    {
        // Arrange
        var manager = new DocumentManager();
        var uri = "file:///test.stasis";
        var content = "let x: i32 = 5;";

        // Act
        var doc1 = manager.GetOrCreateDocument(uri, content);
        var doc2 = manager.GetOrCreateDocument(uri, content);

        // Assert
        Assert.Same(doc1, doc2);
    }

    [Fact]
    public void GetDocument_ReturnsNullForNonExistentDocument()
    {
        // Arrange
        var manager = new DocumentManager();
        var uri = "file:///nonexistent.stasis";

        // Act
        var doc = manager.GetDocument(uri);

        // Assert
        Assert.Null(doc);
    }

    [Fact]
    public void GetDocument_ReturnsCachedDocument()
    {
        // Arrange
        var manager = new DocumentManager();
        var uri = "file:///test.stasis";
        var content = "let x: i32 = 5;";
        var created = manager.GetOrCreateDocument(uri, content);

        // Act
        var retrieved = manager.GetDocument(uri);

        // Assert
        Assert.Same(created, retrieved);
    }

    [Fact]
    public void UpdateDocument_UpdatesContent()
    {
        // Arrange
        var manager = new DocumentManager();
        var uri = "file:///test.stasis";
        var initialContent = "let x: i32 = 5;";
        var updatedContent = "let x: i32 = 10;";

        manager.GetOrCreateDocument(uri, initialContent);

        // Act
        manager.UpdateDocument(uri, updatedContent, 1);
        var doc = manager.GetDocument(uri);

        // Assert
        Assert.NotNull(doc);
        Assert.Equal(updatedContent, doc.Content);
        Assert.Equal(1, doc.Version);
    }

    [Fact]
    public void UpdateDocument_ParsesDocument()
    {
        // Arrange
        var manager = new DocumentManager();
        var uri = "file:///test.stasis";
        var content = "let x: i32 = 5;";

        manager.GetOrCreateDocument(uri, "");

        // Act
        manager.UpdateDocument(uri, content, 1);
        var doc = manager.GetDocument(uri);

        // Assert
        Assert.NotNull(doc);
        Assert.NotNull(doc.ParseResult);
        Assert.NotNull(doc.ParseResult.CompilationUnit);
    }

    [Fact]
    public void UpdateDocument_CapturesDiagnostics()
    {
        // Arrange
        var manager = new DocumentManager();
        var uri = "file:///test.stasis";
        var invalidContent = "let x: i32 ="; // Missing initialization

        manager.GetOrCreateDocument(uri, "");

        // Act
        manager.UpdateDocument(uri, invalidContent, 1);
        var doc = manager.GetDocument(uri);

        // Assert
        Assert.NotNull(doc);
        Assert.NotNull(doc.AllDiagnostics);
        Assert.NotEmpty(doc.AllDiagnostics);
    }

    [Fact]
    public void CloseDocument_RemovesDocument()
    {
        // Arrange
        var manager = new DocumentManager();
        var uri = "file:///test.stasis";
        var content = "let x: i32 = 5;";

        manager.GetOrCreateDocument(uri, content);
        Assert.NotNull(manager.GetDocument(uri));

        // Act
        manager.CloseDocument(uri);

        // Assert
        Assert.Null(manager.GetDocument(uri));
    }

    [Fact]
    public void GetAllDocuments_ReturnsAllOpenDocuments()
    {
        // Arrange
        var manager = new DocumentManager();
        var uri1 = "file:///test1.stasis";
        var uri2 = "file:///test2.stasis";

        manager.GetOrCreateDocument(uri1, "let x: i32 = 5;");
        manager.GetOrCreateDocument(uri2, "let y: f32 = 3.14;");

        // Act
        var all = manager.GetAllDocuments();

        // Assert
        Assert.Equal(2, all.Count);
    }

    [Fact]
    public void UpdateDocument_IncludesImportedStructsInSymbolIndex()
    {
        var tempDir = Directory.CreateTempSubdirectory("stasis_lsp_imports_");
        try
        {
            var importedPath = Path.Combine(tempDir.FullName, "types.stasis");
            File.WriteAllText(importedPath, string.Join("\n", new[]
            {
                "struct Foo {",
                "    a: i32;",
                "    b: i32;",
                "}"
            }));

            var entryPath = Path.Combine(tempDir.FullName, "main.stasis");
            var content = string.Join("\n", new[]
            {
                "import \"types.stasis\";",
                "",
                "global foo: Foo;",
                "",
                "function main(): i32 {",
                "    foo.a = 1;",
                "    return 0;",
                "}"
            });

            var uri = new Uri(entryPath).AbsoluteUri;
            var manager = new DocumentManager();
            manager.GetOrCreateDocument(uri, "");
            manager.UpdateDocument(uri, content, 1);
            var doc = manager.GetDocument(uri);

            Assert.NotNull(doc);
            Assert.NotNull(doc!.SymbolIndex);
            var foo = doc.SymbolIndex!.GetStruct("Foo");
            Assert.NotNull(foo);
            Assert.Contains(foo!.Fields, f => f.Name == "a");
            Assert.Contains(foo.Fields, f => f.Name == "b");
        }
        finally
        {
            tempDir.Delete(recursive: true);
        }
    }

    [Fact]
    public void UpdateDocument_ReportsMissingImportsAsDiagnostics()
    {
        var tempDir = Directory.CreateTempSubdirectory("stasis_lsp_import_missing_");
        try
        {
            var entryPath = Path.Combine(tempDir.FullName, "main.stasis");
            var content = "import \"does_not_exist.stasis\";\nfunction main(): i32 { return 0; }";
            var uri = new Uri(entryPath).AbsoluteUri;

            var manager = new DocumentManager();
            manager.GetOrCreateDocument(uri, "");
            manager.UpdateDocument(uri, content, 1);
            var doc = manager.GetDocument(uri);

            Assert.NotNull(doc);
            Assert.Contains(doc!.AllDiagnostics, d => d.Message.Contains("Import not found", StringComparison.Ordinal));
        }
        finally
        {
            tempDir.Delete(recursive: true);
        }
    }

    [Fact]
    public void UpdateDocument_Limits_diagnostics_and_reports_invalid_calls_and_fields()
    {
        var manager = new DocumentManager();
        var uri = "file:///test.stasis";
        var content = string.Join("\n", new[]
        {
            "struct S { a: i32; }",
            "global state: S;",
            "function f(): void {",
            "    state.b = 1;",
            "    missing0();",
            "    missing1();",
            "    missing2();",
            "    missing3();",
            "    missing4();",
            "    missing5();",
            "}"
        });

        manager.GetOrCreateDocument(uri, "");
        manager.UpdateDocument(uri, content, 1);
        var doc = manager.GetDocument(uri);

        Assert.NotNull(doc);
        Assert.Equal(DiagnosticPolicy.MaxErrors, doc!.AllDiagnostics.Count);
        Assert.Contains(doc.AllDiagnostics, d => d.Message.Contains("Unknown field", StringComparison.Ordinal));
        Assert.Contains(doc.AllDiagnostics, d => d.Message.Contains("Unknown function", StringComparison.Ordinal));
        Assert.Contains(doc.AllDiagnostics, d => d.Message.Contains("Hint:", StringComparison.Ordinal));
    }
}
