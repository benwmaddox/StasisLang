namespace Stasis.LanguageServer.Services;

using OmniSharp.Extensions.LanguageServer.Protocol.Models;

public static class TextPositionConverter
{
    public static int PositionToOffset(string text, Position position)
    {
        int offset = 0;
        int line = 0;
        int character = 0;

        while (offset < text.Length)
        {
            if (line == position.Line && character == position.Character)
            {
                return offset;
            }

            var ch = text[offset];
            if (ch == '\r')
            {
                if (offset + 1 < text.Length && text[offset + 1] == '\n')
                {
                    offset += 2;
                }
                else
                {
                    offset += 1;
                }

                line++;
                character = 0;
                continue;
            }

            if (ch == '\n')
            {
                offset += 1;
                line++;
                character = 0;
                continue;
            }

            offset += 1;
            character++;
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

