namespace Stasis.Compiler;

public enum TokenKind
{
    EndOfFile,
    Unknown,

    // Identifiers & literals
    Identifier,
    IntegerLiteral,
    FloatLiteral,
    StringLiteral,
    BacktickLiteral,
    TrueKeyword,
    FalseKeyword,

    // Keywords
    StructKeyword,
    EnumKeyword,
    GlobalKeyword,
    FunctionKeyword,
    ExportKeyword,
    TestKeyword,
    ReturnKeyword,
    LetKeyword,
    IfKeyword,
    ElseKeyword,
    ForKeyword,
    ForeachKeyword,
    InKeyword,

    // Operators and punctuation
    Dot,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Less,
    Greater,
    Equal,
    EqualEqual,

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon
}
