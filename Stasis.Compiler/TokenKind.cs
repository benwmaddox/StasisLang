namespace Stasis.Compiler;

public enum TokenKind
{
    EndOfFile,
    Unknown,

    // Identifiers & literals
    Identifier,
    IntegerLiteral,
    U8Literal,
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
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    EqualEqual,
    BangEqual,
    Colon,
    Bang,
    At,

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon
}
