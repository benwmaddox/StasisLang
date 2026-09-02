#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    FunctionKw,
    Identifier,
    Integer,
    StringLiteral,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Colon,
    Comma,
    Semicolon,
    Other,
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexerDiagnostic {
    pub message: String,
    pub offset: usize,
}

pub fn lex(source: &str) -> Result<Vec<Token>, String> {
    lex_with_diagnostic(source).map_err(|error| error.message)
}

pub fn lex_with_diagnostic(source: &str) -> Result<Vec<Token>, LexerDiagnostic> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'"' {
            let start = i;
            i += 1;
            let mut closed = false;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    i += 1;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                return Err(LexerDiagnostic {
                    message: "unterminated string literal".to_string(),
                    offset: start,
                });
            }
            tokens.push(Token {
                kind: TokenKind::StringLiteral,
                start,
                end: i.min(bytes.len()),
            });
            continue;
        }
        if b.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Integer,
                start,
                end: i,
            });
            continue;
        }
        if is_identifier_start(b) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_identifier_continue(bytes[i]) {
                i += 1;
            }
            let text = &source[start..i];
            let kind = if text == "function" {
                TokenKind::FunctionKw
            } else {
                TokenKind::Identifier
            };
            tokens.push(Token {
                kind,
                start,
                end: i,
            });
            continue;
        }
        let (kind, width) = match b {
            b'(' => (TokenKind::LParen, 1usize),
            b')' => (TokenKind::RParen, 1usize),
            b'{' => (TokenKind::LBrace, 1usize),
            b'}' => (TokenKind::RBrace, 1usize),
            b':' => (TokenKind::Colon, 1usize),
            b',' => (TokenKind::Comma, 1usize),
            b';' => (TokenKind::Semicolon, 1usize),
            _ => (TokenKind::Other, 1usize),
        };
        tokens.push(Token {
            kind,
            start: i,
            end: i + width,
        });
        i += width;
    }
    tokens.push(Token {
        kind: TokenKind::Eof,
        start: bytes.len(),
        end: bytes.len(),
    });
    Ok(tokens)
}

pub(crate) fn is_inside_backtick_literal(source: &str, offset: usize) -> bool {
    let bytes = &source.as_bytes()[..offset.min(source.len())];
    let mut cursor = 0usize;
    let mut in_string = false;
    let mut in_comment = false;
    let mut in_backtick = false;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if in_comment {
            if byte == b'\n' {
                in_comment = false;
            }
        } else if in_string {
            if byte == b'\\' {
                cursor = cursor.saturating_add(1);
            } else if byte == b'"' {
                in_string = false;
            }
        } else if !in_backtick && byte == b'/' && bytes.get(cursor + 1) == Some(&b'/') {
            in_comment = true;
            cursor = cursor.saturating_add(1);
        } else if !in_backtick && byte == b'"' {
            in_string = true;
        } else if byte == b'`' {
            in_backtick = !in_backtick;
        }
        cursor = cursor.saturating_add(1);
    }
    in_backtick
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}
