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
    ConstKeyword,
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
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    PercentEqual,
    AmpAmp,
    PipePipe,
    Less,
    Greater,
    Equal,
    EqualEqual,
    Colon,
    Bang,

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon
}
