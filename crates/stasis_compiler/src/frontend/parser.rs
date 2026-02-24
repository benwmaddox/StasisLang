use std::ops::Range;

use crate::frontend::lexer::{lex, Token, TokenKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedParam {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFunctionSignature {
    pub name: String,
    pub params: Vec<ParsedParam>,
    pub return_type_name: String,
    pub signature_range: Range<usize>,
    pub body_range: Range<usize>,
}

pub fn parse_top_level_functions(source: &str) -> Result<Vec<ParsedFunctionSignature>, String> {
    let tokens = lex(source)?;
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < tokens.len() {
        if tokens[cursor].kind != TokenKind::FunctionKw {
            cursor += 1;
            continue;
        }
        let signature_start = tokens[cursor].start;
        cursor += 1;
        let name = token_text(source, expect(&tokens, cursor, TokenKind::Identifier)?).to_string();
        cursor += 1;
        expect(&tokens, cursor, TokenKind::LParen)?;
        cursor += 1;
        let mut params = Vec::new();
        while tokens
            .get(cursor)
            .is_some_and(|token| token.kind != TokenKind::RParen)
        {
            let param_name =
                token_text(source, expect(&tokens, cursor, TokenKind::Identifier)?).to_string();
            cursor += 1;
            expect(&tokens, cursor, TokenKind::Colon)?;
            cursor += 1;
            let type_name =
                token_text(source, expect(&tokens, cursor, TokenKind::Identifier)?).to_string();
            cursor += 1;
            params.push(ParsedParam {
                name: param_name,
                type_name,
            });
            if tokens
                .get(cursor)
                .is_some_and(|token| token.kind == TokenKind::Comma)
            {
                cursor += 1;
            }
        }
        expect(&tokens, cursor, TokenKind::RParen)?;
        cursor += 1;

        let mut return_type_name = "void".to_string();
        if tokens
            .get(cursor)
            .is_some_and(|token| token.kind == TokenKind::Colon)
        {
            cursor += 1;
            return_type_name =
                token_text(source, expect(&tokens, cursor, TokenKind::Identifier)?).to_string();
            cursor += 1;
        }
        let signature_end = tokens
            .get(cursor)
            .map_or(source.len(), |token| token.start)
            .min(source.len());
        let body_start_token = expect(&tokens, cursor, TokenKind::LBrace)?;
        cursor += 1;
        let body_end_token_index = find_matching_rbrace(&tokens, cursor, 1)?;
        let body_end_token = tokens[body_end_token_index];
        let body_range = body_start_token.start..body_end_token.end;
        out.push(ParsedFunctionSignature {
            name,
            params,
            return_type_name,
            signature_range: signature_start..signature_end,
            body_range,
        });
        cursor = body_end_token_index + 1;
    }
    Ok(out)
}

fn token_text<'a>(source: &'a str, token: Token) -> &'a str {
    &source[token.start..token.end]
}

fn expect(tokens: &[Token], cursor: usize, kind: TokenKind) -> Result<Token, String> {
    let token = tokens
        .get(cursor)
        .copied()
        .ok_or_else(|| format!("unexpected end of token stream, expected {kind:?}"))?;
    if token.kind != kind {
        return Err(format!(
            "expected token {kind:?} but found {:?}",
            token.kind
        ));
    }
    Ok(token)
}

fn find_matching_rbrace(tokens: &[Token], start: usize, mut depth: usize) -> Result<usize, String> {
    let mut cursor = start;
    while cursor < tokens.len() {
        match tokens[cursor].kind {
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => {
                depth -= 1;
                if depth == 0 {
                    return Ok(cursor);
                }
            }
            TokenKind::Eof => break,
            _ => {}
        }
        cursor += 1;
    }
    Err("missing closing '}' for function body".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_function_signature_and_body_range() {
        let source = "function main(): i32 { return 0; }\n";
        let parsed = parse_top_level_functions(source).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "main");
        assert_eq!(parsed[0].return_type_name, "i32");
        assert_eq!(parsed[0].params.len(), 0);
        assert_eq!(&source[parsed[0].body_range.clone()], "{ return 0; }");
    }

    #[test]
    fn handles_nested_braces_inside_function_body() {
        let source = "function main(): i32 { if (1) { return 1; } return 0; }\n";
        let parsed = parse_top_level_functions(source).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert!(
            source[parsed[0].body_range.clone()].contains("return 0"),
            "expected full outer body capture"
        );
    }
}
