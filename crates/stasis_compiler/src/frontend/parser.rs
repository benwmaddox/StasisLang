use std::ops::Range;

use crate::frontend::lexer::{lex, lex_with_diagnostic, Token, TokenKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParserDiagnostic {
    pub message: String,
    pub start: usize,
    pub end: usize,
    pub symbol: String,
}

impl From<String> for ParserDiagnostic {
    fn from(message: String) -> Self {
        Self {
            message,
            start: 0,
            end: 0,
            symbol: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedParam {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLocalBinding {
    pub function_name: String,
    pub name: String,
    pub name_range: Range<usize>,
    pub type_name: String,
    pub visibility_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLocalDeclaration {
    pub function_name: String,
    pub name: String,
    pub name_range: Range<usize>,
    pub visibility_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFunctionSignature {
    pub name: String,
    pub annotations: Vec<ParsedFunctionAnnotation>,
    pub params: Vec<ParsedParam>,
    pub return_type_name: String,
    pub signature_range: Range<usize>,
    pub body_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFunctionAnnotation {
    pub name: String,
    pub has_parentheses: bool,
    pub arguments: Vec<ParsedFunctionAnnotationArgument>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFunctionAnnotationArgument {
    pub kind: ParsedFunctionAnnotationArgumentKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedFunctionAnnotationArgumentKind {
    Integer,
    String,
    Identifier,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedExternFunctionDeclaration {
    pub name: String,
    pub name_range: Range<usize>,
    pub symbol_name: String,
    pub explicit_symbol: bool,
    pub annotations: Vec<ParsedFunctionAnnotation>,
    pub params: Vec<ParsedParam>,
    pub return_type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedField {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedStructDefinition {
    pub name: String,
    pub fields: Vec<ParsedField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedStructDefinitionRange {
    pub name: String,
    pub definition_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEnumDefinition {
    pub name: String,
    pub variants: Vec<ParsedEnumVariant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEnumVariant {
    pub name: String,
    pub value: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedGlobalDefinition {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedGlobalBlockDefinition {
    pub name: String,
    pub fields: Vec<ParsedField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedConstDefinition {
    pub name: String,
    pub type_name: String,
    pub value_text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedTypeLayout {
    pub enums: Vec<ParsedEnumDefinition>,
    pub structs: Vec<ParsedStructDefinition>,
    pub globals: Vec<ParsedGlobalDefinition>,
    pub global_blocks: Vec<ParsedGlobalBlockDefinition>,
    pub constants: Vec<ParsedConstDefinition>,
}

pub fn parse_typed_local_bindings(source: &str) -> Result<Vec<ParsedLocalBinding>, String> {
    let tokens = lex(source)?;
    let mut bindings = Vec::new();
    for declaration in parse_local_declarations(source)? {
        let Some(name_index) = tokens
            .iter()
            .position(|token| token.start == declaration.name_range.start)
        else {
            continue;
        };
        if !tokens
            .get(name_index + 1)
            .is_some_and(|token| token.kind == TokenKind::Colon)
        {
            continue;
        }
        let (type_name, _) = parse_type_name(source, &tokens, name_index + 2)?;
        bindings.push(ParsedLocalBinding {
            function_name: declaration.function_name,
            name: declaration.name,
            name_range: declaration.name_range,
            type_name,
            visibility_range: declaration.visibility_range,
        });
    }
    bindings.sort_by_key(|binding| {
        (
            binding.function_name.clone(),
            binding.name.clone(),
            binding.type_name.clone(),
            binding.visibility_range.start,
            binding.visibility_range.end,
        )
    });
    bindings.dedup();
    Ok(bindings)
}

pub fn parse_local_declarations(source: &str) -> Result<Vec<ParsedLocalDeclaration>, String> {
    let tokens = lex(source)?;
    let mut declarations = Vec::new();
    for function in parse_top_level_functions(source)? {
        let scope_ranges = lexical_scope_ranges(&tokens, function.body_range.clone());
        let loop_ranges = for_scope_ranges(source, &tokens, function.body_range.clone());
        declarations.extend(foreach_local_declarations(source, &tokens, &function));
        let mut cursor = tokens.partition_point(|token| token.start < function.body_range.start);
        while let Some(token) = tokens.get(cursor).copied() {
            if token.start >= function.body_range.end || token.kind == TokenKind::Eof {
                break;
            }
            if token.kind != TokenKind::Identifier || token_text(source, token) != "let" {
                cursor += 1;
                continue;
            }
            let Some(name) = tokens.get(cursor + 1).copied() else {
                break;
            };
            if name.kind != TokenKind::Identifier {
                cursor += 1;
                continue;
            }
            if declarations
                .iter()
                .any(|declaration| declaration.name_range == (name.start..name.end))
            {
                cursor += 2;
                continue;
            }
            let scope_end = scope_ranges
                .iter()
                .chain(loop_ranges.iter())
                .filter(|range| range.start <= token.start && token.end <= range.end)
                .min_by_key(|range| range.end.saturating_sub(range.start))
                .map(|range| range.end)
                .unwrap_or(function.body_range.end);
            let Some(visible_from) = tokens[cursor + 2..]
                .iter()
                .take_while(|candidate| candidate.start < scope_end)
                .find(|candidate| candidate.kind == TokenKind::Semicolon)
                .map(|semicolon| semicolon.end)
            else {
                cursor += 2;
                continue;
            };
            declarations.push(ParsedLocalDeclaration {
                function_name: function.name.clone(),
                name: token_text(source, name).to_string(),
                name_range: name.start..name.end,
                visibility_range: visible_from..scope_end,
            });
            cursor += 2;
        }
    }
    declarations.sort_by_key(|declaration| declaration.name_range.start);
    declarations.dedup_by_key(|declaration| declaration.name_range.clone());
    Ok(declarations)
}

fn foreach_local_declarations(
    source: &str,
    tokens: &[Token],
    function: &ParsedFunctionSignature,
) -> Vec<ParsedLocalDeclaration> {
    let mut declarations = Vec::new();
    let mut cursor = tokens.partition_point(|token| token.start < function.body_range.start);
    while let Some(token) = tokens.get(cursor).copied() {
        if token.start >= function.body_range.end || token.kind == TokenKind::Eof {
            break;
        }
        if token.kind != TokenKind::Identifier || token_text(source, token) != "foreach" {
            cursor += 1;
            continue;
        }
        let Some(header_open_index) = tokens
            .get(cursor + 1)
            .filter(|token| token.kind == TokenKind::LParen)
            .map(|_| cursor + 1)
        else {
            cursor += 1;
            continue;
        };
        let Some(header_close_index) = matching_token_index(
            tokens,
            header_open_index,
            TokenKind::LParen,
            TokenKind::RParen,
        ) else {
            cursor += 1;
            continue;
        };
        let Some(body_open_index) = tokens
            .get(header_close_index + 1)
            .filter(|token| token.kind == TokenKind::LBrace)
            .map(|_| header_close_index + 1)
        else {
            cursor += 1;
            continue;
        };
        let Some(body_close_index) = matching_token_index(
            tokens,
            body_open_index,
            TokenKind::LBrace,
            TokenKind::RBrace,
        ) else {
            cursor += 1;
            continue;
        };
        let header = &tokens[header_open_index + 1..header_close_index];
        if !header.first().is_some_and(|token| {
            token.kind == TokenKind::Identifier && token_text(source, *token) == "let"
        }) {
            cursor += 1;
            continue;
        }
        let binding_tokens = header
            .iter()
            .copied()
            .skip(1)
            .take_while(|token| {
                token.kind != TokenKind::Identifier || token_text(source, *token) != "in"
            })
            .filter(|token| token.kind == TokenKind::Identifier)
            .collect::<Vec<_>>();
        for binding in binding_tokens.into_iter().take(2) {
            declarations.push(ParsedLocalDeclaration {
                function_name: function.name.clone(),
                name: token_text(source, binding).to_string(),
                name_range: binding.start..binding.end,
                visibility_range: tokens[body_open_index].start..tokens[body_close_index].end,
            });
        }
        cursor = body_close_index + 1;
    }
    declarations
}

pub fn completion_expected_type(source: &str, cursor: usize) -> Result<Option<String>, String> {
    let cursor = cursor.min(source.len());
    let tokens = lex(source)?;
    let Some(colon_index) = tokens
        .iter()
        .enumerate()
        .take_while(|(_, token)| token.start < cursor)
        .filter(|(_, token)| token.kind == TokenKind::Colon)
        .map(|(index, _)| index)
        .last()
    else {
        return Ok(None);
    };
    let (type_name, next) = parse_type_name(source, &tokens, colon_index + 1)?;
    let has_assignment = tokens[colon_index + 1..]
        .iter()
        .take_while(|token| token.start < cursor)
        .skip(next.saturating_sub(colon_index + 1))
        .any(|token| token.kind == TokenKind::Other && token_text(source, *token) == "=");
    Ok(has_assignment.then_some(type_name))
}

fn lexical_scope_ranges(tokens: &[Token], body_range: Range<usize>) -> Vec<Range<usize>> {
    let mut starts = Vec::new();
    let mut ranges = Vec::new();
    for token in tokens
        .iter()
        .filter(|token| body_range.start <= token.start && token.end <= body_range.end)
    {
        match token.kind {
            TokenKind::LBrace => starts.push(token.start),
            TokenKind::RBrace => {
                if let Some(start) = starts.pop() {
                    ranges.push(start..token.end);
                }
            }
            _ => {}
        }
    }
    ranges
}

fn for_scope_ranges(source: &str, tokens: &[Token], body_range: Range<usize>) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut cursor = tokens.partition_point(|token| token.start < body_range.start);
    while let Some(token) = tokens.get(cursor).copied() {
        if token.start >= body_range.end || token.kind == TokenKind::Eof {
            break;
        }
        if token.kind == TokenKind::Identifier && token_text(source, token) == "for" {
            let Some(header_open) = tokens
                .get(cursor + 1)
                .filter(|token| token.kind == TokenKind::LParen)
            else {
                cursor += 1;
                continue;
            };
            let Some(header_close_index) =
                matching_token_index(tokens, cursor + 1, TokenKind::LParen, TokenKind::RParen)
            else {
                cursor += 1;
                continue;
            };
            let Some(body_open_index) = tokens
                .get(header_close_index + 1)
                .filter(|token| token.kind == TokenKind::LBrace)
                .map(|_| header_close_index + 1)
            else {
                cursor += 1;
                continue;
            };
            if let Some(body_close_index) = matching_token_index(
                tokens,
                body_open_index,
                TokenKind::LBrace,
                TokenKind::RBrace,
            ) {
                ranges.push(header_open.start..tokens[body_close_index].end);
            }
        }
        cursor += 1;
    }
    ranges
}

fn matching_token_index(
    tokens: &[Token],
    open_index: usize,
    open_kind: TokenKind,
    close_kind: TokenKind,
) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open_index) {
        if token.kind == open_kind {
            depth = depth.saturating_add(1);
        } else if token.kind == close_kind {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTestDeclaration {
    pub display_name: String,
    pub generated_function_name: String,
    pub declaration_range: Range<usize>,
    pub body_range: Range<usize>,
}

pub fn parse_top_level_test_declarations(
    source: &str,
) -> Result<Vec<ParsedTestDeclaration>, String> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut depth = 0usize;
    while cursor < bytes.len() {
        if let Some(next) = skip_comment_or_string(source, cursor) {
            cursor = next;
            continue;
        }
        let byte = bytes[cursor];
        if byte == b'{' {
            depth = depth.saturating_add(1);
            cursor += 1;
            continue;
        }
        if byte == b'}' {
            depth = depth.saturating_sub(1);
            cursor += 1;
            continue;
        }
        if depth == 0 && starts_with_keyword(source, cursor, "test") {
            let declaration_start = cursor;
            cursor += "test".len();
            cursor = skip_ascii_whitespace_and_comments(source, cursor);

            if bytes.get(cursor).copied() != Some(b'`') {
                return Err(format!(
                    "test declaration missing backtick name near '{}'",
                    snippet_from(source, declaration_start)
                ));
            }
            let name_start = cursor + 1;
            let mut name_end = name_start;
            while name_end < bytes.len() && bytes[name_end] != b'`' {
                name_end += 1;
            }
            if name_end >= bytes.len() {
                return Err("unterminated test name (missing closing backtick)".to_string());
            }
            let display_name = source[name_start..name_end].to_string();
            cursor = name_end + 1;
            cursor = skip_ascii_whitespace_and_comments(source, cursor);

            if bytes.get(cursor).copied() != Some(b'(') {
                return Err(format!(
                    "test '{}' missing parameter list '()'",
                    display_name
                ));
            }
            let params_close = find_matching_delimiter(source, cursor, b'(', b')')
                .ok_or_else(|| format!("test '{}' missing ')'", display_name))?;
            let params = source[cursor + 1..params_close].trim();
            if !params.is_empty() {
                return Err(format!(
                    "test '{}' must not declare parameters",
                    display_name
                ));
            }
            cursor = params_close + 1;
            cursor = skip_ascii_whitespace_and_comments(source, cursor);

            if bytes.get(cursor).copied() != Some(b':') {
                return Err(format!(
                    "test '{}' missing ': bool' return type",
                    display_name
                ));
            }
            cursor += 1;
            cursor = skip_ascii_whitespace_and_comments(source, cursor);
            let (return_type, after_return_type) = parse_identifier(source, cursor)?;
            if return_type != "bool" {
                return Err(format!(
                    "test '{}' return type must be bool, found '{}'",
                    display_name, return_type
                ));
            }
            cursor = skip_ascii_whitespace_and_comments(source, after_return_type);

            if bytes.get(cursor).copied() != Some(b'{') {
                return Err(format!("test '{}' missing body block", display_name));
            }
            let body_start = cursor;
            let body_close = find_matching_delimiter(source, body_start, b'{', b'}')
                .ok_or_else(|| format!("test '{}' missing closing '}}'", display_name))?;
            let body_end = body_close + 1;
            let declaration_end = body_end;
            out.push(ParsedTestDeclaration {
                display_name,
                generated_function_name: format!("__stasis_test_{}", out.len()),
                declaration_range: declaration_start..declaration_end,
                body_range: body_start..body_end,
            });
            cursor = declaration_end;
            continue;
        }
        cursor += 1;
    }
    Ok(out)
}

pub fn rewrite_top_level_test_declarations(
    source: &str,
) -> Result<(String, Vec<ParsedTestDeclaration>), String> {
    let declarations = parse_top_level_test_declarations(source)?;
    if declarations.is_empty() {
        return Ok((source.to_string(), Vec::new()));
    }
    let mut rewritten = String::with_capacity(source.len() + declarations.len() * 32);
    let mut cursor = 0usize;
    for declaration in &declarations {
        if declaration.declaration_range.start < cursor
            || declaration.declaration_range.end > source.len()
        {
            return Err("invalid test declaration bounds during rewrite".to_string());
        }
        rewritten.push_str(&source[cursor..declaration.declaration_range.start]);
        let body = &source[declaration.body_range.clone()];
        rewritten.push_str(&format!(
            "function {}(): bool {}",
            declaration.generated_function_name, body
        ));
        cursor = declaration.declaration_range.end;
    }
    rewritten.push_str(&source[cursor..]);
    Ok((rewritten, declarations))
}

pub fn parse_top_level_functions(source: &str) -> Result<Vec<ParsedFunctionSignature>, String> {
    parse_top_level_functions_with_diagnostic(source).map_err(|error| error.message)
}

pub fn parse_top_level_functions_with_diagnostic(
    source: &str,
) -> Result<Vec<ParsedFunctionSignature>, ParserDiagnostic> {
    parse_top_level_functions_impl(source)
}

fn parse_top_level_functions_impl(
    source: &str,
) -> Result<Vec<ParsedFunctionSignature>, ParserDiagnostic> {
    let tokens = lex_with_diagnostic(source)
        .map_err(|error| lexer_error_context(source, error.message, error.offset))?;
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < tokens.len() {
        if tokens[cursor].kind != TokenKind::FunctionKw {
            cursor += 1;
            continue;
        }
        let function_token_index = cursor;
        let signature_start = tokens[cursor].start;
        cursor += 1;
        let (next_cursor, _, annotations) = parse_function_annotations(source, &tokens, cursor)
            .map_err(|message| {
                contextual_parser_error(
                    source,
                    &tokens,
                    function_token_index,
                    cursor,
                    None,
                    message,
                )
            })?;
        cursor = next_cursor;
        let name = token_text(
            source,
            expect(&tokens, cursor, TokenKind::Identifier).map_err(|message| {
                contextual_parser_error(
                    source,
                    &tokens,
                    function_token_index,
                    cursor,
                    None,
                    message,
                )
            })?,
        )
        .to_string();
        cursor += 1;
        expect(&tokens, cursor, TokenKind::LParen).map_err(|message| {
            contextual_parser_error(
                source,
                &tokens,
                function_token_index,
                cursor,
                Some(&name),
                message,
            )
        })?;
        cursor += 1;
        let mut params = Vec::new();
        while tokens
            .get(cursor)
            .is_some_and(|token| token.kind != TokenKind::RParen)
        {
            let param_name = token_text(
                source,
                expect(&tokens, cursor, TokenKind::Identifier).map_err(|message| {
                    contextual_parser_error(
                        source,
                        &tokens,
                        function_token_index,
                        cursor,
                        Some(&name),
                        message,
                    )
                })?,
            )
            .to_string();
            cursor += 1;
            expect(&tokens, cursor, TokenKind::Colon).map_err(|message| {
                contextual_parser_error(
                    source,
                    &tokens,
                    function_token_index,
                    cursor,
                    Some(&name),
                    message,
                )
            })?;
            cursor += 1;
            let (type_name, next_cursor) =
                parse_type_name(source, &tokens, cursor).map_err(|message| {
                    contextual_parser_error(
                        source,
                        &tokens,
                        function_token_index,
                        cursor,
                        Some(&name),
                        message,
                    )
                })?;
            cursor = next_cursor;
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
        expect(&tokens, cursor, TokenKind::RParen).map_err(|message| {
            contextual_parser_error(
                source,
                &tokens,
                function_token_index,
                cursor,
                Some(&name),
                message,
            )
        })?;
        cursor += 1;

        let mut return_type_name = "void".to_string();
        if tokens
            .get(cursor)
            .is_some_and(|token| token.kind == TokenKind::Colon)
        {
            cursor += 1;
            let (parsed_return_type, next_cursor) = parse_type_name(source, &tokens, cursor)
                .map_err(|message| {
                    contextual_parser_error(
                        source,
                        &tokens,
                        function_token_index,
                        cursor,
                        Some(&name),
                        message,
                    )
                })?;
            return_type_name = parsed_return_type;
            cursor = next_cursor;
        }

        if tokens
            .get(cursor)
            .is_some_and(|token| token.kind == TokenKind::Semicolon)
        {
            cursor += 1;
            continue;
        }

        let signature_end = tokens
            .get(cursor)
            .map_or(source.len(), |token| token.start)
            .min(source.len());
        let body_start_token = expect(&tokens, cursor, TokenKind::LBrace).map_err(|message| {
            contextual_parser_error(
                source,
                &tokens,
                function_token_index,
                cursor,
                Some(&name),
                message,
            )
        })?;
        cursor += 1;
        let body_end_token_index =
            find_matching_rbrace(&tokens, cursor, 1).map_err(|message| ParserDiagnostic {
                message,
                start: signature_start,
                end: source.len(),
                symbol: name.clone(),
            })?;
        let body_end_token = tokens[body_end_token_index];
        let body_range = body_start_token.start..body_end_token.end;
        out.push(ParsedFunctionSignature {
            name,
            annotations,
            params,
            return_type_name,
            signature_range: signature_start..signature_end,
            body_range,
        });
        cursor = body_end_token_index + 1;
    }
    Ok(out)
}

fn contextual_parser_error(
    source: &str,
    tokens: &[Token],
    function_token_index: usize,
    cursor: usize,
    symbol: Option<&str>,
    message: String,
) -> ParserDiagnostic {
    let token = tokens
        .get(cursor)
        .or_else(|| tokens.get(function_token_index))
        .copied()
        .unwrap_or(Token {
            kind: TokenKind::Eof,
            start: source.len(),
            end: source.len(),
        });
    ParserDiagnostic {
        message,
        start: token.start,
        end: token.end.max(token.start + 1).min(source.len()),
        symbol: symbol
            .map(str::to_string)
            .unwrap_or_else(|| function_name_hint(source, tokens, function_token_index)),
    }
}

fn function_name_hint(source: &str, tokens: &[Token], function_token_index: usize) -> String {
    let mut after_at = false;
    for (index, token) in tokens.iter().enumerate().skip(function_token_index + 1) {
        if token.kind == TokenKind::FunctionKw || token.kind == TokenKind::LBrace {
            break;
        }
        if token_is_other_char(source, *token, b'@') {
            after_at = true;
            continue;
        }
        if token.kind == TokenKind::Identifier {
            if after_at {
                after_at = false;
                continue;
            }
            if tokens
                .get(index + 1)
                .is_some_and(|next| next.kind == TokenKind::LParen)
            {
                return token_text(source, *token).to_string();
            }
        }
    }
    String::new()
}

pub(crate) fn lexer_error_context(
    source: &str,
    message: String,
    offset: usize,
) -> ParserDiagnostic {
    let mut symbol = String::new();
    let limit = offset.min(source.len());
    let bytes = source.as_bytes();
    let mut index = 0usize;
    while index < limit {
        if bytes[index] == b'/' && index + 1 < limit && bytes[index + 1] == b'/' {
            index += 2;
            while index < limit && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if bytes[index] == b'/' && index + 1 < limit && bytes[index + 1] == b'*' {
            index += 2;
            while index < limit {
                if bytes[index] == b'*' && index + 1 < limit && bytes[index + 1] == b'/' {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        if bytes[index] == b'"' {
            index += 1;
            while index < limit {
                if bytes[index] == b'\\' {
                    index = (index + 2).min(limit);
                } else if bytes[index] == b'"' {
                    index += 1;
                    break;
                } else {
                    index += 1;
                }
            }
            continue;
        }
        if is_identifier_char(bytes[index]) && !bytes[index].is_ascii_digit() {
            let start = index;
            index += 1;
            while index < limit && is_identifier_char(bytes[index]) {
                index += 1;
            }
            if &source[start..index] == "function" {
                // A declaration may carry annotations before its name. Clear the
                // previous declaration first; an incomplete annotation must not
                // make a lexer failure appear to belong to that prior function.
                symbol.clear();
                let mut name_index = index;
                while name_index < limit && bytes[name_index].is_ascii_whitespace() {
                    name_index += 1;
                }
                let mut annotations_complete = true;
                while name_index < limit && bytes[name_index] == b'@' {
                    name_index += 1;
                    while name_index < limit && bytes[name_index].is_ascii_whitespace() {
                        name_index += 1;
                    }
                    if name_index >= limit
                        || !is_identifier_char(bytes[name_index])
                        || bytes[name_index].is_ascii_digit()
                    {
                        annotations_complete = false;
                        break;
                    }
                    while name_index < limit && is_identifier_char(bytes[name_index]) {
                        name_index += 1;
                    }
                    while name_index < limit && bytes[name_index].is_ascii_whitespace() {
                        name_index += 1;
                    }
                    if name_index < limit && bytes[name_index] == b'(' {
                        let mut depth = 0usize;
                        while name_index < limit {
                            match bytes[name_index] {
                                b'"' => {
                                    name_index += 1;
                                    while name_index < limit {
                                        if bytes[name_index] == b'\\' {
                                            name_index = (name_index + 2).min(limit);
                                        } else if bytes[name_index] == b'"' {
                                            name_index += 1;
                                            break;
                                        } else {
                                            name_index += 1;
                                        }
                                    }
                                }
                                b'(' => {
                                    depth += 1;
                                    name_index += 1;
                                }
                                b')' if depth > 0 => {
                                    depth -= 1;
                                    name_index += 1;
                                    if depth == 0 {
                                        break;
                                    }
                                }
                                _ => name_index += 1,
                            }
                        }
                        if depth != 0 {
                            annotations_complete = false;
                            break;
                        }
                    }
                    while name_index < limit && bytes[name_index].is_ascii_whitespace() {
                        name_index += 1;
                    }
                }
                if annotations_complete
                    && name_index < limit
                    && is_identifier_char(bytes[name_index])
                    && !bytes[name_index].is_ascii_digit()
                {
                    let name_start = name_index;
                    name_index += 1;
                    while name_index < limit && is_identifier_char(bytes[name_index]) {
                        name_index += 1;
                    }
                    symbol = source[name_start..name_index].to_string();
                }
            }
            continue;
        }
        index += 1;
    }
    let start = offset.min(source.len());
    ParserDiagnostic {
        message,
        start,
        end: source.len().max(start + 1),
        symbol,
    }
}

pub fn parse_top_level_struct_definitions(
    source: &str,
) -> Result<Vec<ParsedStructDefinitionRange>, String> {
    let tokens = lex(source)?;
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut depth = 0usize;
    while cursor < tokens.len() {
        let token = tokens[cursor];
        match token.kind {
            TokenKind::LBrace => {
                depth = depth.saturating_add(1);
                cursor += 1;
                continue;
            }
            TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
                cursor += 1;
                continue;
            }
            TokenKind::Identifier if depth == 0 && token_text(source, token) == "struct" => {
                let (parsed, next_cursor) = parse_struct_definition_range(source, &tokens, cursor)?;
                out.push(parsed);
                cursor = next_cursor;
                continue;
            }
            _ => {}
        }
        cursor += 1;
    }
    Ok(out)
}

pub fn parse_top_level_extern_functions(
    source: &str,
) -> Result<Vec<ParsedExternFunctionDeclaration>, String> {
    let tokens = lex(source)?;
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < tokens.len() {
        let mut has_extern_prefix = false;
        if tokens
            .get(cursor)
            .copied()
            .is_some_and(|token| token.kind == TokenKind::Identifier)
            && token_text(source, tokens[cursor]) == "extern"
            && tokens
                .get(cursor + 1)
                .is_some_and(|token| token.kind == TokenKind::FunctionKw)
        {
            has_extern_prefix = true;
            cursor += 1;
        }

        if tokens
            .get(cursor)
            .is_none_or(|token| token.kind != TokenKind::FunctionKw)
        {
            cursor += 1;
            continue;
        }

        cursor += 1;
        let (after_annotations, annotation_symbol, annotations) =
            parse_function_annotations(source, &tokens, cursor)?;
        cursor = after_annotations;
        let name_token = expect(&tokens, cursor, TokenKind::Identifier)?;
        let name = token_text(source, name_token).to_string();
        let name_range = name_token.start..name_token.end;
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
            let (type_name, next_cursor) = parse_type_name(source, &tokens, cursor)?;
            cursor = next_cursor;
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
            let (parsed_return_type, next_cursor) = parse_type_name(source, &tokens, cursor)?;
            return_type_name = parsed_return_type;
            cursor = next_cursor;
        }

        if tokens
            .get(cursor)
            .is_some_and(|token| token.kind == TokenKind::Semicolon)
        {
            if has_extern_prefix || annotation_symbol.is_some() {
                let explicit_symbol = annotation_symbol.is_some();
                let symbol_name = annotation_symbol.unwrap_or_else(|| name.clone());
                out.push(ParsedExternFunctionDeclaration {
                    name,
                    name_range,
                    symbol_name,
                    explicit_symbol,
                    annotations,
                    params,
                    return_type_name,
                });
            }
            cursor += 1;
            continue;
        }

        if tokens
            .get(cursor)
            .is_some_and(|token| token.kind == TokenKind::LBrace)
        {
            cursor += 1;
            let body_end_token_index = find_matching_rbrace(&tokens, cursor, 1)?;
            cursor = body_end_token_index + 1;
            continue;
        }

        cursor += 1;
    }
    Ok(out)
}

pub fn parse_top_level_type_layout(source: &str) -> Result<ParsedTypeLayout, String> {
    let tokens = lex(source)?;
    let mut out = ParsedTypeLayout::default();
    let mut cursor = 0usize;
    let mut depth = 0usize;
    while cursor < tokens.len() {
        let token = tokens[cursor];
        match token.kind {
            TokenKind::LBrace => {
                depth = depth.saturating_add(1);
                cursor += 1;
                continue;
            }
            TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
                cursor += 1;
                continue;
            }
            TokenKind::Identifier if depth == 0 => {
                let keyword = token_text(source, token);
                if keyword == "struct" {
                    let (parsed, next_cursor) = parse_struct_definition(source, &tokens, cursor)?;
                    out.structs.push(parsed);
                    cursor = next_cursor;
                    continue;
                }
                if keyword == "enum" {
                    let (parsed, next_cursor) = parse_enum_definition(source, &tokens, cursor)?;
                    out.enums.push(parsed);
                    cursor = next_cursor;
                    continue;
                }
                if keyword == "global" {
                    let (parsed, next_cursor) = parse_global_definition(source, &tokens, cursor)?;
                    match parsed {
                        ParsedGlobalTopLevel::TypedGlobal(global) => out.globals.push(global),
                        ParsedGlobalTopLevel::GlobalBlock(block) => out.global_blocks.push(block),
                    }
                    cursor = next_cursor;
                    continue;
                }
                if keyword == "const" {
                    let (parsed, next_cursor) = parse_const_definition(source, &tokens, cursor)?;
                    out.constants.push(parsed);
                    cursor = next_cursor;
                    continue;
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedGlobalTopLevel {
    TypedGlobal(ParsedGlobalDefinition),
    GlobalBlock(ParsedGlobalBlockDefinition),
}

fn parse_struct_definition(
    source: &str,
    tokens: &[Token],
    cursor: usize,
) -> Result<(ParsedStructDefinition, usize), String> {
    let name_token = expect(tokens, cursor + 1, TokenKind::Identifier)?;
    let body_open = expect(tokens, cursor + 2, TokenKind::LBrace)?;
    let (fields, mut next_cursor) = parse_braced_fields(source, tokens, body_open, cursor + 2)?;
    if tokens
        .get(next_cursor)
        .is_some_and(|token| token.kind == TokenKind::Semicolon)
    {
        next_cursor += 1;
    }
    Ok((
        ParsedStructDefinition {
            name: token_text(source, name_token).to_string(),
            fields,
        },
        next_cursor,
    ))
}

fn parse_struct_definition_range(
    source: &str,
    tokens: &[Token],
    cursor: usize,
) -> Result<(ParsedStructDefinitionRange, usize), String> {
    let name_token = expect(tokens, cursor + 1, TokenKind::Identifier)?;
    let body_open = expect(tokens, cursor + 2, TokenKind::LBrace)?;
    let (_, mut next_cursor) = parse_braced_fields(source, tokens, body_open, cursor + 2)?;
    if tokens
        .get(next_cursor)
        .is_some_and(|token| token.kind == TokenKind::Semicolon)
    {
        next_cursor += 1;
    }
    let definition_end = tokens
        .get(next_cursor.saturating_sub(1))
        .map_or(source.len(), |token| token.end)
        .min(source.len());
    Ok((
        ParsedStructDefinitionRange {
            name: token_text(source, name_token).to_string(),
            definition_range: tokens[cursor].start..definition_end,
        },
        next_cursor,
    ))
}

fn parse_enum_definition(
    source: &str,
    tokens: &[Token],
    cursor: usize,
) -> Result<(ParsedEnumDefinition, usize), String> {
    let name_token = expect(tokens, cursor + 1, TokenKind::Identifier)?;
    let name = token_text(source, name_token).to_string();
    let open_cursor = cursor + 2;
    let open_token = expect(tokens, open_cursor, TokenKind::LBrace)?;
    if open_token.kind != TokenKind::LBrace {
        return Err("internal parser error: expected '{' token for enum block".to_string());
    }
    let mut variants = Vec::new();
    let mut next_cursor = open_cursor + 1;
    while tokens
        .get(next_cursor)
        .is_some_and(|token| token.kind != TokenKind::RBrace)
    {
        let variant = expect(tokens, next_cursor, TokenKind::Identifier)?;
        let variant_name = token_text(source, variant).to_string();
        next_cursor += 1;
        let mut explicit_value = None;
        if tokens
            .get(next_cursor)
            .copied()
            .is_some_and(|token| token_is_other_char(source, token, b'='))
        {
            next_cursor += 1;
            let mut sign: i32 = 1;
            if tokens
                .get(next_cursor)
                .copied()
                .is_some_and(|token| token_is_other_char(source, token, b'-'))
            {
                sign = -1;
                next_cursor += 1;
            }
            let value_token = expect(tokens, next_cursor, TokenKind::Integer)?;
            let value_text = token_text(source, value_token);
            let value = value_text
                .parse::<i32>()
                .map_err(|error| format!("invalid enum discriminant '{value_text}': {error}"))?;
            explicit_value = Some(value.saturating_mul(sign));
            next_cursor += 1;
        }
        if tokens
            .get(next_cursor)
            .is_some_and(|token| token.kind == TokenKind::Comma)
        {
            next_cursor += 1;
        } else if !tokens
            .get(next_cursor)
            .is_some_and(|token| token.kind == TokenKind::RBrace)
        {
            return Err(format!(
                "enum variant '{variant_name}' must be followed by a comma"
            ));
        }
        variants.push(ParsedEnumVariant {
            name: variant_name,
            value: explicit_value,
        });
    }
    expect(tokens, next_cursor, TokenKind::RBrace)?;
    next_cursor += 1;
    if tokens
        .get(next_cursor)
        .is_some_and(|token| token.kind == TokenKind::Semicolon)
    {
        next_cursor += 1;
    }
    Ok((ParsedEnumDefinition { name, variants }, next_cursor))
}

fn parse_global_definition(
    source: &str,
    tokens: &[Token],
    cursor: usize,
) -> Result<(ParsedGlobalTopLevel, usize), String> {
    let name_token = expect(tokens, cursor + 1, TokenKind::Identifier)?;
    let name = token_text(source, name_token).to_string();
    let mut next_cursor = cursor + 2;
    let Some(next_token) = tokens.get(next_cursor).copied() else {
        return Err(format!("incomplete global declaration near '{name}'"));
    };
    if next_token.kind == TokenKind::Colon {
        next_cursor += 1;
        let (type_name, after_type) = parse_type_name(source, tokens, next_cursor)?;
        next_cursor = after_type;
        expect(tokens, next_cursor, TokenKind::Semicolon)?;
        next_cursor += 1;
        return Ok((
            ParsedGlobalTopLevel::TypedGlobal(ParsedGlobalDefinition { name, type_name }),
            next_cursor,
        ));
    }
    if next_token.kind == TokenKind::LBrace {
        let (fields, mut after_block) =
            parse_braced_fields(source, tokens, next_token, next_cursor)?;
        if tokens
            .get(after_block)
            .is_some_and(|token| token.kind == TokenKind::Semicolon)
        {
            after_block += 1;
        }
        return Ok((
            ParsedGlobalTopLevel::GlobalBlock(ParsedGlobalBlockDefinition { name, fields }),
            after_block,
        ));
    }
    Err(format!(
        "unsupported global declaration near '{}': expected ':' or '{{'",
        name
    ))
}

fn parse_const_definition(
    source: &str,
    tokens: &[Token],
    cursor: usize,
) -> Result<(ParsedConstDefinition, usize), String> {
    let name_token = expect(tokens, cursor + 1, TokenKind::Identifier)?;
    let name = token_text(source, name_token).to_string();
    let mut next_cursor = cursor + 2;
    expect(tokens, next_cursor, TokenKind::Colon)?;
    next_cursor += 1;
    let (type_name, after_type) = parse_type_name(source, tokens, next_cursor)?;
    next_cursor = after_type;
    let equals = tokens
        .get(next_cursor)
        .copied()
        .ok_or_else(|| format!("incomplete const declaration near '{name}'"))?;
    if !token_is_other_char(source, equals, b'=') {
        return Err(format!(
            "const declaration for '{}' must include '=' initializer",
            name
        ));
    }
    next_cursor += 1;
    let value_start = tokens
        .get(next_cursor)
        .map_or(equals.end, |token| token.start);
    let mut semicolon_cursor = next_cursor;
    while tokens
        .get(semicolon_cursor)
        .is_some_and(|token| token.kind != TokenKind::Semicolon)
    {
        if tokens
            .get(semicolon_cursor)
            .is_some_and(|token| token.kind == TokenKind::Eof)
        {
            return Err(format!("const declaration for '{}' is missing ';'", name));
        }
        semicolon_cursor += 1;
    }
    let semicolon = expect(tokens, semicolon_cursor, TokenKind::Semicolon)?;
    let value_text = source
        .get(value_start..semicolon.start)
        .unwrap_or_default()
        .trim()
        .to_string();
    next_cursor = semicolon_cursor + 1;
    Ok((
        ParsedConstDefinition {
            name,
            type_name,
            value_text,
        },
        next_cursor,
    ))
}

fn parse_braced_fields(
    source: &str,
    tokens: &[Token],
    open_token: Token,
    open_cursor: usize,
) -> Result<(Vec<ParsedField>, usize), String> {
    if open_token.kind != TokenKind::LBrace {
        return Err("internal parser error: expected '{' token for field block".to_string());
    }
    let mut fields = Vec::new();
    let mut cursor = open_cursor + 1;
    while tokens
        .get(cursor)
        .is_some_and(|token| token.kind != TokenKind::RBrace)
    {
        let field_name_token = expect(tokens, cursor, TokenKind::Identifier)?;
        let field_name = token_text(source, field_name_token).to_string();
        cursor += 1;
        expect(tokens, cursor, TokenKind::Colon)?;
        cursor += 1;
        let (type_name, next_cursor) = parse_type_name(source, tokens, cursor)?;
        cursor = next_cursor;
        expect(tokens, cursor, TokenKind::Semicolon)?;
        cursor += 1;
        fields.push(ParsedField {
            name: field_name,
            type_name,
        });
    }
    expect(tokens, cursor, TokenKind::RBrace)?;
    cursor += 1;
    Ok((fields, cursor))
}

fn parse_function_annotations(
    source: &str,
    tokens: &[Token],
    mut cursor: usize,
) -> Result<(usize, Option<String>, Vec<ParsedFunctionAnnotation>), String> {
    let mut extern_symbol: Option<String> = None;
    let mut annotations = Vec::new();
    while tokens
        .get(cursor)
        .copied()
        .is_some_and(|token| token_is_other_char(source, token, b'@'))
    {
        cursor += 1;
        let name = expect(tokens, cursor, TokenKind::Identifier)?;
        let annotation_name = token_text(source, name);
        cursor += 1;
        let has_parentheses = tokens
            .get(cursor)
            .is_some_and(|token| token.kind == TokenKind::LParen);
        let mut arguments = Vec::new();
        if has_parentheses {
            if annotation_name == "extern" {
                let parsed = parse_extern_symbol_annotation(source, tokens, cursor)?;
                if parsed.is_some() {
                    extern_symbol = parsed;
                }
            }
            let next_cursor = skip_parenthesized_tokens(tokens, cursor)?;
            if annotation_name == "effects" {
                arguments = parse_effect_annotation_arguments(
                    source,
                    &tokens[cursor + 1..next_cursor - 1],
                )?;
                cursor = next_cursor;
                annotations.push(ParsedFunctionAnnotation {
                    name: annotation_name.to_string(),
                    has_parentheses,
                    arguments,
                });
                continue;
            }
            let mut expects_argument = true;
            for token in &tokens[cursor + 1..next_cursor - 1] {
                if token.kind == TokenKind::Comma {
                    if expects_argument {
                        return Err(format!(
                            "annotation '@{annotation_name}' has an empty argument"
                        ));
                    }
                    expects_argument = true;
                    continue;
                }
                if !expects_argument {
                    return Err(format!(
                        "annotation '@{annotation_name}' expects ',' between arguments"
                    ));
                }
                arguments.push(ParsedFunctionAnnotationArgument {
                    kind: match token.kind {
                        TokenKind::Integer => ParsedFunctionAnnotationArgumentKind::Integer,
                        TokenKind::StringLiteral => ParsedFunctionAnnotationArgumentKind::String,
                        TokenKind::Identifier => ParsedFunctionAnnotationArgumentKind::Identifier,
                        _ => ParsedFunctionAnnotationArgumentKind::Other,
                    },
                    text: token_text(source, *token).to_string(),
                });
                expects_argument = false;
            }
            if expects_argument && !arguments.is_empty() {
                return Err(format!(
                    "annotation '@{annotation_name}' has a trailing comma"
                ));
            }
            cursor = next_cursor;
        }
        annotations.push(ParsedFunctionAnnotation {
            name: annotation_name.to_string(),
            has_parentheses,
            arguments,
        });
    }
    Ok((cursor, extern_symbol, annotations))
}

fn parse_effect_annotation_arguments(
    source: &str,
    tokens: &[Token],
) -> Result<Vec<ParsedFunctionAnnotationArgument>, String> {
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let mut arguments = Vec::new();
    let mut start = 0usize;
    for end in 0..=tokens.len() {
        if end < tokens.len() && tokens[end].kind != TokenKind::Comma {
            continue;
        }
        if start == end {
            return Err("annotation '@effects' has an empty argument".to_string());
        }
        let first = tokens[start];
        let last = tokens[end - 1];
        arguments.push(ParsedFunctionAnnotationArgument {
            kind: if start + 1 == end && first.kind == TokenKind::Identifier {
                ParsedFunctionAnnotationArgumentKind::Identifier
            } else {
                ParsedFunctionAnnotationArgumentKind::Other
            },
            text: source[first.start..last.end].to_string(),
        });
        start = end + 1;
    }
    Ok(arguments)
}

fn parse_extern_symbol_annotation(
    source: &str,
    tokens: &[Token],
    open_paren_cursor: usize,
) -> Result<Option<String>, String> {
    let Some(first) = tokens.get(open_paren_cursor + 1).copied() else {
        return Ok(None);
    };
    if first.kind == TokenKind::RParen {
        return Ok(None);
    }
    if first.kind != TokenKind::StringLiteral {
        return Err("extern annotation expects string symbol literal".to_string());
    }
    let literal_text = token_text(source, first);
    let symbol = parse_string_literal_text(literal_text)?;
    Ok(Some(symbol))
}

pub(crate) fn parse_string_literal_text(literal_text: &str) -> Result<String, String> {
    let bytes = literal_text.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'"' || *bytes.last().unwrap_or(&0) != b'"' {
        return Err(format!("invalid string literal token '{}'", literal_text));
    }
    let mut out = String::new();
    let mut index = 1usize;
    while index + 1 < bytes.len() {
        let byte = bytes[index];
        if byte == b'\\' {
            let Some(escaped) = bytes.get(index + 1).copied() else {
                return Err("unterminated escape sequence in string literal".to_string());
            };
            let decoded = match escaped {
                b'\\' => '\\',
                b'"' => '"',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                b'0' => '\0',
                other => {
                    return Err(format!(
                        "unsupported escape sequence '\\{}' in string literal",
                        other as char
                    ))
                }
            };
            out.push(decoded);
            index += 2;
            continue;
        }
        let Some(next_char) = literal_text[index..].chars().next() else {
            return Err("invalid UTF-8 boundary in string literal".to_string());
        };
        out.push(next_char);
        index += next_char.len_utf8();
    }
    Ok(out)
}

fn skip_parenthesized_tokens(tokens: &[Token], open_cursor: usize) -> Result<usize, String> {
    if tokens
        .get(open_cursor)
        .is_none_or(|token| token.kind != TokenKind::LParen)
    {
        return Err("internal parser error: expected '(' for annotation".to_string());
    }
    let mut depth = 1i32;
    let mut cursor = open_cursor + 1;
    while cursor < tokens.len() {
        match tokens[cursor].kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => {
                depth -= 1;
                if depth == 0 {
                    return Ok(cursor + 1);
                }
            }
            TokenKind::Eof => break,
            _ => {}
        }
        cursor += 1;
    }
    Err("missing closing ')' in function annotation".to_string())
}

fn parse_type_name(
    source: &str,
    tokens: &[Token],
    cursor: usize,
) -> Result<(String, usize), String> {
    let base = expect(tokens, cursor, TokenKind::Identifier)?;
    let mut next = cursor + 1;
    let mut end = base.end;
    while let Some(open) = tokens.get(next).copied() {
        if !token_is_other_char(source, open, b'[') {
            break;
        }
        let mut depth = 1i32;
        let mut scan = next + 1;
        while scan < tokens.len() {
            let token = tokens[scan];
            if token_is_other_char(source, token, b'[') {
                depth += 1;
            } else if token_is_other_char(source, token, b']') {
                depth -= 1;
                if depth == 0 {
                    end = token.end;
                    next = scan + 1;
                    break;
                }
            } else if token.kind == TokenKind::Eof {
                return Err("missing closing ']' in type annotation".to_string());
            }
            scan += 1;
        }
        if depth != 0 {
            return Err("missing closing ']' in type annotation".to_string());
        }
    }
    Ok((source[base.start..end].to_string(), next))
}

fn parse_identifier(source: &str, start: usize) -> Result<(&str, usize), String> {
    let bytes = source.as_bytes();
    if start >= bytes.len() {
        return Err("expected identifier but reached end of source".to_string());
    }
    let first = bytes[start];
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return Err(format!(
            "expected identifier near '{}'",
            snippet_from(source, start)
        ));
    }
    let mut end = start + 1;
    while end < bytes.len() {
        let byte = bytes[end];
        if byte.is_ascii_alphanumeric() || byte == b'_' {
            end += 1;
            continue;
        }
        break;
    }
    Ok((&source[start..end], end))
}

fn starts_with_keyword(source: &str, cursor: usize, keyword: &str) -> bool {
    let Some(tail) = source.get(cursor..) else {
        return false;
    };
    if !tail.starts_with(keyword) {
        return false;
    }
    let before_ok = if cursor == 0 {
        true
    } else {
        !is_identifier_char(source.as_bytes()[cursor - 1])
    };
    if !before_ok {
        return false;
    }
    let end = cursor + keyword.len();
    if end >= source.len() {
        return true;
    }
    !is_identifier_char(source.as_bytes()[end])
}

fn is_identifier_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn skip_ascii_whitespace_and_comments(source: &str, mut cursor: usize) -> usize {
    let bytes = source.as_bytes();
    loop {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor + 1 < bytes.len() && bytes[cursor] == b'/' && bytes[cursor + 1] == b'/' {
            cursor += 2;
            while cursor < bytes.len() && bytes[cursor] != b'\n' {
                cursor += 1;
            }
            continue;
        }
        if cursor + 1 < bytes.len() && bytes[cursor] == b'/' && bytes[cursor + 1] == b'*' {
            cursor += 2;
            while cursor + 1 < bytes.len() {
                if bytes[cursor] == b'*' && bytes[cursor + 1] == b'/' {
                    cursor += 2;
                    break;
                }
                cursor += 1;
            }
            continue;
        }
        return cursor;
    }
}

fn skip_comment_or_string(source: &str, cursor: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if cursor >= bytes.len() {
        return None;
    }
    if cursor + 1 < bytes.len() && bytes[cursor] == b'/' && bytes[cursor + 1] == b'/' {
        let mut next = cursor + 2;
        while next < bytes.len() && bytes[next] != b'\n' {
            next += 1;
        }
        return Some(next);
    }
    if cursor + 1 < bytes.len() && bytes[cursor] == b'/' && bytes[cursor + 1] == b'*' {
        let mut next = cursor + 2;
        while next + 1 < bytes.len() {
            if bytes[next] == b'*' && bytes[next + 1] == b'/' {
                return Some(next + 2);
            }
            next += 1;
        }
        return Some(bytes.len());
    }
    if bytes[cursor] == b'"' {
        let mut next = cursor + 1;
        while next < bytes.len() {
            if bytes[next] == b'\\' {
                next = next.saturating_add(2);
                continue;
            }
            if bytes[next] == b'"' {
                return Some(next + 1);
            }
            next += 1;
        }
        return Some(bytes.len());
    }
    None
}

fn find_matching_delimiter(source: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    if start >= bytes.len() || bytes[start] != open {
        return None;
    }
    let mut depth = 0usize;
    let mut cursor = start;
    while cursor < bytes.len() {
        if let Some(next) = skip_comment_or_string(source, cursor) {
            cursor = next;
            continue;
        }
        let byte = bytes[cursor];
        if byte == open {
            depth += 1;
        } else if byte == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(cursor);
            }
        }
        cursor += 1;
    }
    None
}

fn snippet_from(source: &str, start: usize) -> String {
    source
        .get(start..)
        .unwrap_or("")
        .chars()
        .take(64)
        .collect::<String>()
}

fn token_text<'a>(source: &'a str, token: Token) -> &'a str {
    &source[token.start..token.end]
}

fn token_is_other_char(source: &str, token: Token, byte: u8) -> bool {
    token.kind == TokenKind::Other
        && token.end == token.start + 1
        && source
            .as_bytes()
            .get(token.start)
            .copied()
            .is_some_and(|value| value == byte)
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
    fn parses_typed_locals_without_treating_foreach_bindings_as_typed() {
        let source = r#"
function move_player(player: Player, delta: f32): f32 {
    let speed: f32 = delta;
    foreach (let enemy in state.enemies) {
        let damage: i32 = 1;
    }
    return speed;
}
function reset(): void {
    let player: Player;
}
"#;
        let bindings = parse_typed_local_bindings(source).expect("typed locals");
        assert_eq!(
            bindings
                .iter()
                .map(|binding| (
                    binding.function_name.as_str(),
                    binding.name.as_str(),
                    binding.type_name.as_str(),
                ))
                .collect::<Vec<_>>(),
            vec![
                ("move_player", "damage", "i32"),
                ("move_player", "speed", "f32"),
                ("reset", "player", "Player"),
            ]
        );
        let damage = bindings
            .iter()
            .find(|binding| binding.name == "damage")
            .expect("nested binding");
        assert!(damage.visibility_range.end < source.find("return speed").expect("return"));
    }

    #[test]
    fn typed_for_initializer_is_visible_only_through_loop_body() {
        let source = r#"
function tick(): i32 {
    for (let index: i32 = 0; index < 2; index += 1) {
        let inside: i32 = index;
    }
    let after: i32 = 3;
    return after;
}
"#;
        let bindings = parse_typed_local_bindings(source).expect("typed locals");
        let after_start = source.find("let after").expect("after binding");
        for name in ["index", "inside"] {
            let binding = bindings
                .iter()
                .find(|binding| binding.name == name)
                .expect("loop binding");
            assert!(binding.visibility_range.end < after_start);
        }
    }

    #[test]
    fn local_declaration_visibility_excludes_initializers_and_ends_with_foreach() {
        let source = r#"
function tick(): i32 {
    let value = value + 1;
    foreach (let enemy, index in enemies) {
        value += enemy + index;
    }
    return value;
}
"#;
        let declarations = parse_local_declarations(source).expect("local declarations");
        let value = declarations
            .iter()
            .find(|declaration| declaration.name == "value")
            .expect("value declaration");
        assert!(value.visibility_range.start > source.find("value + 1").expect("initializer"));

        let after_loop = source.find("return value").expect("after loop");
        for name in ["enemy", "index"] {
            let declaration = declarations
                .iter()
                .find(|declaration| declaration.name == name)
                .expect("foreach declaration");
            assert!(declaration.visibility_range.start <= source.find("value +=").expect("body"));
            assert!(declaration.visibility_range.end < after_loop);
        }
    }

    #[test]
    fn completion_expected_type_uses_typed_binding_before_cursor() {
        let source = "let next_score: i32 = sco";
        assert_eq!(
            completion_expected_type(source, source.len()).expect("expected type"),
            Some("i32".to_string())
        );
        assert_eq!(
            completion_expected_type(":palette sco", 12).expect("command"),
            None
        );
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

    #[test]
    fn supports_function_inline_annotation_before_name() {
        let source = "function @inline fast_path(): i32 { return 1; }\n";
        let parsed = parse_top_level_functions(source).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "fast_path");
        assert_eq!(parsed[0].return_type_name, "i32");
    }

    #[test]
    fn supports_function_annotation_with_arguments_and_extern_declaration() {
        let source = "function @extern(\"stasis_gfx_cache_text\") gfx_cache_text(font: i32, text: string): i32;\nfunction main(): i32 { return 0; }\n";
        let parsed = parse_top_level_functions(source).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "main");
    }

    #[test]
    fn supports_array_type_annotations_in_params_and_return_type() {
        let source = "function copy_ascii(src: ascii[], dst: ascii[]): i32[120] { return 0; }\n";
        let parsed = parse_top_level_functions(source).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].params.len(), 2);
        assert_eq!(parsed[0].params[0].type_name, "ascii[]");
        assert_eq!(parsed[0].params[1].type_name, "ascii[]");
        assert_eq!(parsed[0].return_type_name, "i32[120]");
    }

    #[test]
    fn parses_top_level_structs_globals_and_global_blocks() {
        let source = "struct Enemy { hp: i32; speed: f32; }\nglobal state: Enemy;\nglobal State { score: i32; first_enemy: Enemy; }\nfunction main(): i32 { return State.score; }\n";
        let parsed = parse_top_level_type_layout(source).expect("parse");
        assert_eq!(parsed.enums.len(), 0);
        assert_eq!(parsed.structs.len(), 1);
        assert_eq!(parsed.structs[0].name, "Enemy");
        assert_eq!(parsed.structs[0].fields.len(), 2);
        assert_eq!(parsed.structs[0].fields[0].name, "hp");
        assert_eq!(parsed.structs[0].fields[0].type_name, "i32");
        assert_eq!(parsed.globals.len(), 1);
        assert_eq!(parsed.globals[0].name, "state");
        assert_eq!(parsed.globals[0].type_name, "Enemy");
        assert_eq!(parsed.global_blocks.len(), 1);
        assert_eq!(parsed.global_blocks[0].name, "State");
        assert_eq!(parsed.global_blocks[0].fields.len(), 2);
        assert_eq!(parsed.global_blocks[0].fields[1].name, "first_enemy");
        assert_eq!(parsed.global_blocks[0].fields[1].type_name, "Enemy");
    }

    #[test]
    fn parses_top_level_struct_definition_ranges() {
        let source =
            "struct Enemy {\n    hp: i32;\n    speed: f32;\n}\nfunction main(): i32 { return 0; }\n";
        let parsed = parse_top_level_struct_definitions(source).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "Enemy");
        assert_eq!(
            &source[parsed[0].definition_range.clone()],
            "struct Enemy {\n    hp: i32;\n    speed: f32;\n}"
        );
    }

    #[test]
    fn parses_array_type_annotations_in_struct_and_global_fields() {
        let source = "struct Buffer { samples: f32[512]; }\nglobal audio: Buffer;\nglobal debug_text: ascii[64];\n";
        let parsed = parse_top_level_type_layout(source).expect("parse");
        assert_eq!(parsed.structs.len(), 1);
        assert_eq!(parsed.structs[0].fields[0].type_name, "f32[512]");
        assert_eq!(parsed.globals.len(), 2);
        assert_eq!(parsed.globals[0].type_name, "Buffer");
        assert_eq!(parsed.globals[1].type_name, "ascii[64]");
    }

    #[test]
    fn parses_top_level_constants_with_type_and_initializer_text() {
        let source = "const GAME_H: f32 = 624.0;\nconst BENCH_WARMUP_FRAMES: i32 = 200;\n";
        let parsed = parse_top_level_type_layout(source).expect("parse");
        assert_eq!(parsed.constants.len(), 2);
        assert_eq!(parsed.constants[0].name, "GAME_H");
        assert_eq!(parsed.constants[0].type_name, "f32");
        assert_eq!(parsed.constants[0].value_text, "624.0");
        assert_eq!(parsed.constants[1].name, "BENCH_WARMUP_FRAMES");
        assert_eq!(parsed.constants[1].type_name, "i32");
        assert_eq!(parsed.constants[1].value_text, "200");
    }

    #[test]
    fn parses_top_level_enums_with_variants() {
        let source = "enum BrickType { Basic, Armored, Reflector, }\n";
        let parsed = parse_top_level_type_layout(source).expect("parse");
        assert_eq!(parsed.enums.len(), 1);
        assert_eq!(parsed.enums[0].name, "BrickType");
        assert_eq!(
            parsed.enums[0]
                .variants
                .iter()
                .map(|variant| variant.name.clone())
                .collect::<Vec<_>>(),
            vec![
                "Basic".to_string(),
                "Armored".to_string(),
                "Reflector".to_string()
            ]
        );
    }

    #[test]
    fn parses_top_level_enum_variant_explicit_values() {
        let source = "enum Scancode { A = 4, Return = 40, Escape = 41, }\n";
        let parsed = parse_top_level_type_layout(source).expect("parse");
        assert_eq!(parsed.enums.len(), 1);
        assert_eq!(parsed.enums[0].name, "Scancode");
        assert_eq!(parsed.enums[0].variants.len(), 3);
        assert_eq!(parsed.enums[0].variants[0].name, "A");
        assert_eq!(parsed.enums[0].variants[0].value, Some(4));
        assert_eq!(parsed.enums[0].variants[1].name, "Return");
        assert_eq!(parsed.enums[0].variants[1].value, Some(40));
        assert_eq!(parsed.enums[0].variants[2].name, "Escape");
        assert_eq!(parsed.enums[0].variants[2].value, Some(41));
    }

    #[test]
    fn accepts_an_optional_final_enum_comma() {
        for source in [
            "enum Phase { Ready, Running }\n",
            "enum Phase { Ready, Running, }\n",
        ] {
            let parsed = parse_top_level_type_layout(source).expect("parse enum");
            assert_eq!(parsed.enums[0].variants.len(), 2);
        }
    }

    #[test]
    fn rejects_missing_commas_between_enum_variants() {
        let source = "enum Phase { Ready Running, }\n";
        let error = parse_top_level_type_layout(source).expect_err("missing enum comma must fail");
        assert!(
            error.contains("must be followed by a comma"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn parses_extern_keyword_function_declaration() {
        let source =
            "extern function host_cli_arg_count(): i32;\nfunction main(): i32 { return 0; }\n";
        let parsed = parse_top_level_extern_functions(source).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "host_cli_arg_count");
        assert_eq!(parsed[0].symbol_name, "host_cli_arg_count");
        assert!(!parsed[0].explicit_symbol);
        assert_eq!(parsed[0].return_type_name, "i32");
        assert_eq!(&source[parsed[0].name_range.clone()], "host_cli_arg_count");
    }

    #[test]
    fn parses_annotated_extern_function_declaration() {
        let source = "function @extern(\"stasis_gfx_cache_text\") gfx_cache_text(font: i32, text: string): i32;\n";
        let parsed = parse_top_level_extern_functions(source).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "gfx_cache_text");
        assert_eq!(parsed[0].symbol_name, "stasis_gfx_cache_text");
        assert!(parsed[0].explicit_symbol);
        assert_eq!(parsed[0].params.len(), 2);
        assert_eq!(parsed[0].params[1].type_name, "string");
        assert_eq!(&source[parsed[0].name_range.clone()], "gfx_cache_text");
    }

    #[test]
    fn preserves_name_spans_for_multiple_extern_declarations() {
        let source = "extern function first(): void;\n".to_string()
            + "function @extern(\"second_symbol\") second(value: i32): void;\n";
        let parsed = parse_top_level_extern_functions(&source).expect("parse");
        assert_eq!(parsed.len(), 2);
        assert_eq!(&source[parsed[0].name_range.clone()], "first");
        assert_eq!(&source[parsed[1].name_range.clone()], "second");
        assert!(parsed[0].name_range.end < parsed[1].name_range.start);
    }

    #[test]
    fn missing_final_function_brace_owns_parser_span_and_symbol() {
        let source = "function first(): void { return; }\n\nfunction final_hook(): void {\n";
        let error = parse_top_level_functions_with_diagnostic(source)
            .expect_err("missing final brace must fail");
        assert_eq!(error.message, "missing closing '}' for function body");
        assert_eq!(error.symbol, "final_hook");
        assert_eq!(
            &source[error.start..source.find("function final_hook").unwrap() + 8],
            "function"
        );
        assert_eq!(error.end, source.len());
    }

    #[test]
    fn malformed_later_function_owns_parser_span_and_symbol() {
        let source = "function first(): void {}\nfunction second(: i32): void {}\n";
        let error = parse_top_level_functions_with_diagnostic(source)
            .expect_err("later malformed function must fail");
        assert_eq!(error.symbol, "second");
        assert_eq!(&source[error.start..error.end], ":");
        assert_eq!(source[..error.start].matches('\n').count() + 1, 2);
    }

    #[test]
    fn later_function_missing_parameter_colon_keeps_active_context() {
        let source = "function first(): void {}\nfunction second(value i32): void {}\n";
        let error = parse_top_level_functions_with_diagnostic(source)
            .expect_err("missing parameter colon must fail");
        assert_eq!(error.symbol, "second");
        assert_eq!(&source[error.start..error.end], "i32");
    }

    #[test]
    fn malformed_later_annotation_keeps_active_context() {
        let source = "function first(): void {}\nfunction @effects(state,) second(): void {}\n";
        let error = parse_top_level_functions_with_diagnostic(source)
            .expect_err("malformed annotation must fail");
        assert_eq!(error.symbol, "second");
        assert!(error.start > source.find("function second").unwrap_or(0));
        assert!(error.end > error.start);
    }

    #[test]
    fn lexer_failure_in_later_function_keeps_best_parser_context() {
        let source = "function first(): void {}\nfunction second(): void { \"unterminated\n";
        let error = parse_top_level_functions_with_diagnostic(source)
            .expect_err("unterminated later string must fail");
        assert_eq!(error.symbol, "second");
        assert_eq!(error.start, source.find("\"unterminated").unwrap());
        assert_eq!(error.end, source.len());
    }

    #[test]
    fn lexer_failure_uses_preceding_function_not_later_text() {
        let source = "function first(): void { \"unterminated\nfunction later(): void {}\n";
        let error = parse_top_level_functions_with_diagnostic(source)
            .expect_err("unterminated earlier string must fail");
        assert_eq!(error.symbol, "first");
        assert_eq!(error.start, source.find("\"unterminated").unwrap());
    }

    #[test]
    fn lexer_failure_ignores_function_text_inside_string() {
        let source = "function first(): void {}\n\"unterminated function fake(): void {}\n";
        let error = parse_top_level_functions_with_diagnostic(source)
            .expect_err("unterminated string must fail");
        assert_eq!(error.symbol, "first");
        assert!(!error.symbol.contains("fake"));
    }

    #[test]
    fn lexer_failure_ignores_function_text_inside_block_comment() {
        let source = concat!(
            "function first(): void {}\n",
            "/* function fake(): void {} */\n",
            "function actual(): void { \"unterminated\n",
        );
        let error = parse_top_level_functions_with_diagnostic(source)
            .expect_err("unterminated string after block comment must fail");
        assert_eq!(error.symbol, "actual");
        assert_eq!(error.start, source.find("\"unterminated").unwrap());
        assert_eq!(error.end, source.len());
    }

    #[test]
    fn lexer_failure_in_annotated_function_does_not_retain_prior_symbol() {
        let source = "function first(): void {}\nfunction @extern(\"unterminated\n";
        let error = parse_top_level_functions_with_diagnostic(source)
            .expect_err("unterminated annotated string must fail");
        assert!(
            error.symbol.is_empty(),
            "unexpected symbol: {}",
            error.symbol
        );
        assert_eq!(error.start, source.find("\"unterminated").unwrap());
    }

    #[test]
    fn preserves_utf8_in_string_literals() {
        assert_eq!(
            parse_string_literal_text("\"héllo 世界 ✓\"").expect("parse UTF-8 literal"),
            "héllo 世界 ✓"
        );
    }

    #[test]
    fn parses_top_level_test_declarations() {
        let source = "global x: i32;\ntest `alpha`(): bool { return true; }\nfunction main(): i32 { return 0; }\n";
        let parsed = parse_top_level_test_declarations(source).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].display_name, "alpha");
        assert_eq!(parsed[0].generated_function_name, "__stasis_test_0");
        assert_eq!(&source[parsed[0].body_range.clone()], "{ return true; }");
    }

    #[test]
    fn rewrites_top_level_test_declarations_to_functions() {
        let source = "test `alpha`(): bool { return true; }\n";
        let (rewritten, parsed) = rewrite_top_level_test_declarations(source).expect("rewrite");
        assert_eq!(parsed.len(), 1);
        assert!(rewritten.contains("function __stasis_test_0(): bool { return true; }"));
        assert!(!rewritten.contains("test `alpha`(): bool"));
    }

    #[test]
    fn rejects_non_bool_test_return_type() {
        let source = "test `alpha`(): i32 { return 1; }";
        let error = parse_top_level_test_declarations(source).expect_err("expected error");
        assert!(error.contains("return type must be bool"));
    }

    #[test]
    fn ignores_nested_test_keyword_inside_function_body() {
        let source =
            "function demo(): i32 { /* test `inner`(): bool { return false; } */ return 0; }\n";
        let parsed = parse_top_level_test_declarations(source).expect("parse");
        assert_eq!(parsed.len(), 0);
    }

    #[test]
    fn parses_internal_annotations_on_functions_and_externs() {
        let source = concat!(
            "function @internal helper(): i32 { return 1; }\n",
            "function @internal @extern(\"host_raw\") host_raw(): i32;\n",
            "extern function @internal legacy_host_raw(): i32;\n",
        );
        let functions = parse_top_level_functions(source).expect("functions");
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].annotations[0].name, "internal");
        let externs = parse_top_level_extern_functions(source).expect("externs");
        assert_eq!(externs.len(), 2);
        assert!(externs.iter().all(|external| external
            .annotations
            .iter()
            .any(|annotation| annotation.name == "internal")));
        assert_eq!(externs[0].symbol_name, "host_raw");
        assert_eq!(externs[1].symbol_name, "legacy_host_raw");
    }
}
