namespace Stasis.LanguageServer.Tests;

using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using Stasis.LanguageServer.Services;
using Xunit;

public class TextPositionConverterTests
{
    [Fact]
    public void OffsetToPosition_HandlesCrLf()
    {
        var text = "a\r\nb";

        Assert.Equal(new Position(0, 0), TextPositionConverter.OffsetToPosition(text, 0));
        Assert.Equal(new Position(0, 1), TextPositionConverter.OffsetToPosition(text, 1));
        Assert.Equal(new Position(1, 0), TextPositionConverter.OffsetToPosition(text, 3));
        Assert.Equal(new Position(1, 1), TextPositionConverter.OffsetToPosition(text, 4));
    }

    [Fact]
    public void PositionToOffset_HandlesCrLf()
    {
        var text = "a\r\nb";

        Assert.Equal(0, TextPositionConverter.PositionToOffset(text, new Position(0, 0)));
        Assert.Equal(1, TextPositionConverter.PositionToOffset(text, new Position(0, 1)));
        Assert.Equal(3, TextPositionConverter.PositionToOffset(text, new Position(1, 0)));
        Assert.Equal(4, TextPositionConverter.PositionToOffset(text, new Position(1, 1)));
    }

    [Fact]
    public void PositionToOffset_ClampsBeyondLineLength()
    {
        var text = "abc\ndef";

        Assert.Equal(3, TextPositionConverter.PositionToOffset(text, new Position(0, 10)));
        Assert.Equal(7, TextPositionConverter.PositionToOffset(text, new Position(1, 99)));
    }

    [Fact]
    public void PositionToOffset_ClampsBeyondLineLengthWithCrLf()
    {
        var text = "abc\r\ndef";

        Assert.Equal(3, TextPositionConverter.PositionToOffset(text, new Position(0, 10)));
        Assert.Equal(8, TextPositionConverter.PositionToOffset(text, new Position(1, 99)));
    }
}
