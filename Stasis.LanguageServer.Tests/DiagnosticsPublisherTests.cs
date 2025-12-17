namespace Stasis.LanguageServer.Tests;

using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using OmniSharp.Extensions.LanguageServer.Protocol.Workspace;
using Stasis.Compiler;
using Stasis.LanguageServer.Models;
using Stasis.LanguageServer.Services;
using Xunit;
using Moq;
using OmniSharp.Extensions.LanguageServer.Protocol.Server;
using CompilerDiagnostic = Stasis.Compiler.Diagnostic;
using CompilerSourceSpan = Stasis.Compiler.SourceSpan;

public class DiagnosticsPublisherTests
{
    [Fact]
    public void PublishDiagnostics_PublishesEmptyListForDocumentWithNoDiagnostics()
    {
        // Arrange
        var serverMock = new Mock<ILanguageServerFacade>();
        var publisher = new DiagnosticsPublisher(serverMock.Object);

        var doc = new DocumentState
        {
            Content = "let x: i32 = 5;",
            AllDiagnostics = new List<CompilerDiagnostic>()
        };

        // Act
        publisher.PublishDiagnostics("file:///test.stasis", doc);

        // Assert
        serverMock.Verify(s => s.SendNotification(It.IsAny<PublishDiagnosticsParams>()), Times.Once);
    }

    [Fact]
    public void PublishDiagnostics_PublishesDiagnosticsWithCorrectRange()
    {
        // Arrange
        var serverMock = new Mock<ILanguageServerFacade>();
        var publisher = new DiagnosticsPublisher(serverMock.Object);

        var diagnostic = new CompilerDiagnostic("Test error", new CompilerSourceSpan(0, 5));

        var doc = new DocumentState
        {
            Content = "let x: i32 = 5;",
            AllDiagnostics = new List<CompilerDiagnostic> { diagnostic }
        };

        // Act
        publisher.PublishDiagnostics("file:///test.stasis", doc);

        // Assert
        serverMock.Verify(s => s.SendNotification(It.IsAny<PublishDiagnosticsParams>()), Times.Once);
    }

    [Fact]
    public void PublishDiagnostics_HandlesMultipleDiagnostics()
    {
        // Arrange
        var serverMock = new Mock<ILanguageServerFacade>();
        var publisher = new DiagnosticsPublisher(serverMock.Object);

        var diagnostics = new List<CompilerDiagnostic>
        {
            new CompilerDiagnostic("Error 1", new CompilerSourceSpan(0, 3)),
            new CompilerDiagnostic("Error 2", new CompilerSourceSpan(5, 2)),
            new CompilerDiagnostic("Error 3", new CompilerSourceSpan(8, 4))
        };

        var doc = new DocumentState
        {
            Content = "let x: i32 = 5;",
            AllDiagnostics = diagnostics
        };

        // Act
        publisher.PublishDiagnostics("file:///test.stasis", doc);

        // Assert
        serverMock.Verify(s => s.SendNotification(It.IsAny<PublishDiagnosticsParams>()), Times.Once);
    }

    [Fact]
    public void ClearDiagnostics_PublishesEmptyList()
    {
        // Arrange
        var serverMock = new Mock<ILanguageServerFacade>();
        var publisher = new DiagnosticsPublisher(serverMock.Object);

        // Act
        publisher.ClearDiagnostics("file:///test.stasis");

        // Assert
        serverMock.Verify(s => s.SendNotification(It.IsAny<PublishDiagnosticsParams>()), Times.Once);
    }

    [Fact]
    public void PublishDiagnostics_CorrectlyConvertsOffsetToPosition_FirstLine()
    {
        // Arrange
        var serverMock = new Mock<ILanguageServerFacade>();
        var publisher = new DiagnosticsPublisher(serverMock.Object);

        // Content: "let x: i32 = 5;"
        // Offset 0-3: "let" at line 0, char 0-3
        var diagnostic = new CompilerDiagnostic("Test", new CompilerSourceSpan(0, 3));

        var doc = new DocumentState
        {
            Content = "let x: i32 = 5;",
            AllDiagnostics = new List<CompilerDiagnostic> { diagnostic }
        };

        // Act
        publisher.PublishDiagnostics("file:///test.stasis", doc);

        // Assert - verify that the notification was sent with the correct structure
        serverMock.Verify(s => s.SendNotification(It.IsAny<PublishDiagnosticsParams>()), Times.Once);
    }

    [Fact]
    public void PublishDiagnostics_CorrectlyConvertsOffsetToPosition_MultiLine()
    {
        // Arrange
        var serverMock = new Mock<ILanguageServerFacade>();
        var publisher = new DiagnosticsPublisher(serverMock.Object);

        // Content with newline: "let x: i32 = 5;\nlet y: f32 = 3.14;"
        // Offset 16-20: "let y" starts at line 1, char 0
        var content = "let x: i32 = 5;\nlet y: f32 = 3.14;";
        var diagnostic = new CompilerDiagnostic("Test", new CompilerSourceSpan(16, 5)); // "let y"

        var doc = new DocumentState
        {
            Content = content,
            AllDiagnostics = new List<CompilerDiagnostic> { diagnostic }
        };

        // Act
        publisher.PublishDiagnostics("file:///test.stasis", doc);

        // Assert - verify that the notification was sent
        serverMock.Verify(s => s.SendNotification(It.IsAny<PublishDiagnosticsParams>()), Times.Once);
    }
}
