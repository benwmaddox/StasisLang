using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text;
using Stasis.Compiler.Layout;

namespace Stasis.Compiler;

public static class SemanticFingerprint
{
    public static string ComputeFileFingerprint(string source, LayoutPlan layout)
    {
        ArgumentNullException.ThrowIfNull(source);
        ArgumentNullException.ThrowIfNull(layout);

        var lexerResult = Lexer.Lex(source);
        var hasher = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);

        Span<byte> u64 = stackalloc byte[8];
        Span<byte> u32 = stackalloc byte[4];

        BinaryPrimitives.WriteUInt64LittleEndian(u64, ComputeLayoutHash(layout));
        hasher.AppendData(u64);

        foreach (var token in lexerResult.Tokens)
        {
            if (token.Kind == TokenKind.EndOfFile)
            {
                continue;
            }

            BinaryPrimitives.WriteInt32LittleEndian(u32, (int)token.Kind);
            hasher.AppendData(u32);

            if (token.Text.Length > 0)
            {
                hasher.AppendData(Encoding.UTF8.GetBytes(token.Text));
            }

            hasher.AppendData([0]);
        }

        var digest = hasher.GetHashAndReset();
        return Convert.ToHexString(digest).ToLowerInvariant();
    }

    private static ulong ComputeLayoutHash(LayoutPlan layout)
    {
        // FNV-1a 64
        ulong hash = 14695981039346656037UL;

        static void AddBytes(ref ulong h, ReadOnlySpan<byte> bytes)
        {
            foreach (var b in bytes)
            {
                h ^= b;
                h *= 1099511628211UL;
            }
        }

        static void AddInt(ref ulong h, int value)
        {
            unchecked
            {
                var u = (uint)value;
                for (var i = 0; i < 4; i++)
                {
                    h ^= (byte)(u & 0xFF);
                    h *= 1099511628211UL;
                    u >>= 8;
                }
            }
        }

        foreach (var global in layout.Globals)
        {
            AddBytes(ref hash, Encoding.UTF8.GetBytes(global.Name));
            AddInt(ref hash, global.Offset);
            AddInt(ref hash, global.Size);
            AddInt(ref hash, global.Fields.Count);

            foreach (var field in global.Fields)
            {
                AddBytes(ref hash, Encoding.UTF8.GetBytes(field.Name));
                AddInt(ref hash, field.Offset);
                AddInt(ref hash, field.Size);
                AddInt(ref hash, (int)field.Type);
                AddInt(ref hash, field.ArrayCount);
            }
        }

        AddInt(ref hash, layout.TotalSize);
        return hash;
    }
}
