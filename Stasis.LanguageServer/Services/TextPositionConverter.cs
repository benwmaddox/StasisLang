namespace Stasis.LanguageServer.Services;

using OmniSharp.Extensions.LanguageServer.Protocol.Models;

public static class TextPositionConverter
{
    public static int PositionToOffset(string text, Position position)
    {
        if (position.Line < 0 || position.Character < 0)
        {
            return 0;
        }

        int offset = 0;
        int line = 0;
        int lineStart = 0;

        while (offset < text.Length)
        {
            var ch = text[offset];
            if (ch == '\r' || ch == '\n')
            {
                if (line == position.Line)
                {
                    var lineLength = offset - lineStart;
                    var clamped = Math.Min(position.Character, lineLength);
                    return lineStart + clamped;
                }

                if (ch == '\r' && offset + 1 < text.Length && text[offset + 1] == '\n')
                {
                    offset += 2;
                }
                else
                {
                    offset += 1;
                }

                line++;
                lineStart = offset;
                continue;
            }

            offset += 1;
        }

        if (line == position.Line)
        {
            var lineLength = offset - lineStart;
            var clamped = Math.Min(position.Character, lineLength);
            return lineStart + clamped;
        }

        return text.Length;
    }

    public static Position OffsetToPosition(string text, int offset)
    {
        if (offset < 0)
        {
            offset = 0;
        }
        if (offset > text.Length)
        {
            offset = text.Length;
        }

        int line = 0;
        int character = 0;
        int i = 0;

        while (i < offset && i < text.Length)
        {
            var ch = text[i];
            if (ch == '\r')
            {
                if (i + 1 < offset && i + 1 < text.Length && text[i + 1] == '\n')
                {
                    i += 2;
                }
                else
                {
                    i += 1;
                }

                line++;
                character = 0;
                continue;
            }

            if (ch == '\n')
            {
                i += 1;
                line++;
                character = 0;
                continue;
            }

            i += 1;
            character++;
        }

        return new Position(line, character);
    }
}
