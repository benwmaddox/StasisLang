#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    FunctionKw,
    Identifier,
    Integer,
    StringLiteral,
    BacktickLiteral,
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
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let start = i;
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 >= bytes.len() {
                return Err(LexerDiagnostic {
                    message: "unterminated block comment".to_string(),
                    offset: start,
                });
            }
            i += 2;
            continue;
        }
        if b == b'"' || b == b'`' {
            let start = i;
            i += 1;
            let mut closed = false;
            while i < bytes.len() {
                if b == b'"' && bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == b {
                    i += 1;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                return Err(LexerDiagnostic {
                    message: if b == b'`' {
                        "unterminated test name (missing closing backtick)"
                    } else {
                        "unterminated string literal"
                    }
                    .to_string(),
                    offset: start,
                });
            }
            tokens.push(Token {
                kind: if b == b'`' {
                    TokenKind::BacktickLiteral
                } else {
                    TokenKind::StringLiteral
                },
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

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_comments_are_skipped_before_quotes_and_keywords() {
        for comment in [
            "/* invokes `update */",
            "/* \" global const struct enum function test import { //\n actual */",
            "/**/",
            "/* outer /* non-nesting */",
        ] {
            let source = format!("{comment}function actual(): i32 {{ return 1; }}");
            let tokens = lex(&source).expect("comment must be opaque");
            assert_eq!(tokens[0].kind, TokenKind::FunctionKw);
            assert_eq!(tokens[0].start, comment.len());
            assert_eq!(
                tokens
                    .iter()
                    .filter(|token| token.kind == TokenKind::FunctionKw)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn unterminated_block_comments_report_the_opening_offset() {
        for source in ["  /*", "  /**", "  /* ` \" function"] {
            assert_eq!(
                lex_with_diagnostic(source).unwrap_err(),
                LexerDiagnostic {
                    message: "unterminated block comment".to_string(),
                    offset: 2,
                }
            );
        }
    }

    #[test]
    fn quoted_regions_are_opaque_and_preserve_byte_ranges() {
        for (source, kind) in [
            (
                r#""global const struct enum function test import from as \" ` { } // /*""#,
                TokenKind::StringLiteral,
            ),
            (
                r#"`global const struct enum function test import from as " \ { } // /* text`"#,
                TokenKind::BacktickLiteral,
            ),
        ] {
            let tokens = lex(source).unwrap();
            assert_eq!(tokens.len(), 2);
            assert_eq!(
                tokens[0],
                Token {
                    kind,
                    start: 0,
                    end: source.len()
                }
            );
            assert_eq!(tokens[1].kind, TokenKind::Eof);
        }
        // Backticks have no escape syntax; a backslash does not hide the closer.
        assert_eq!(
            lex(r#"`name\`"#).unwrap()[0].kind,
            TokenKind::BacktickLiteral
        );
    }

    #[test]
    fn unterminated_quoted_regions_report_the_opening_offset() {
        for (source, message) in [
            ("  \"global", "unterminated string literal"),
            ("  \"global\\", "unterminated string literal"),
            (
                "  `global \" //",
                "unterminated test name (missing closing backtick)",
            ),
        ] {
            assert_eq!(
                lex_with_diagnostic(source).unwrap_err(),
                LexerDiagnostic {
                    message: message.to_string(),
                    offset: 2,
                }
            );
        }
    }
}
