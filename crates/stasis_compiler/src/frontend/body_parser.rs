use crate::frontend::types::{TypeTable, TYPE_ID_BOOL};
use crate::ir::hir::{
    eval_const_i64, AssignOp, AssignTarget, ComparisonOp, ConversionKind, DebugStatement,
    ParsedSimpleStatements, SimpleCondition, SimpleExpr, SimpleStmt,
};

pub(crate) fn parse_simple_statements_with_debug(
    block_text: &str,
    type_table: &mut TypeTable,
) -> Result<ParsedSimpleStatements, String> {
    parse_simple_statements_with_debug_at(block_text, type_table, 0)
}

fn parse_simple_statements_with_debug_at(
    block_text: &str,
    type_table: &mut TypeTable,
    block_offset: usize,
) -> Result<ParsedSimpleStatements, String> {
    let leading = block_text.len() - block_text.trim_start().len();
    let trimmed = block_text.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err("expected function body block enclosed in '{...}'".to_string());
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    let inner_offset = block_offset
        .checked_add(leading)
        .and_then(|offset| offset.checked_add(1))
        .ok_or_else(|| "statement source offset overflow".to_string())?;
    let mut statements = Vec::new();
    let mut debug_statements = Vec::new();
    let mut cursor = 0usize;
    while cursor < inner.len() {
        cursor = skip_ascii_whitespace_and_comments(inner, cursor);
        if cursor >= inner.len() {
            break;
        }
        if starts_with_keyword(inner, cursor, "let") {
            let let_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[let_start..semicolon].trim();
            let statement = parse_let_statement(statement_text, type_table)?;
            statements.push(statement);
            debug_statements.push(debug_statement(inner_offset, let_start, Vec::new())?);
            cursor = semicolon + 1;
            continue;
        }
        if starts_with_keyword(inner, cursor, "return") {
            let return_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[return_start..semicolon].trim();
            let statement = parse_return_statement(statement_text)?;
            statements.push(statement);
            debug_statements.push(debug_statement(inner_offset, return_start, Vec::new())?);
            cursor = semicolon + 1;
            continue;
        }
        if starts_with_keyword(inner, cursor, "continue") {
            let continue_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[continue_start..semicolon].trim();
            let statement = parse_continue_statement(statement_text)?;
            statements.push(statement);
            debug_statements.push(debug_statement(inner_offset, continue_start, Vec::new())?);
            cursor = semicolon + 1;
            continue;
        }
        if starts_with_keyword(inner, cursor, "for") {
            let (statement, debug, next_cursor) =
                parse_for_statement_at(inner, cursor, type_table, inner_offset)?;
            statements.push(statement);
            debug_statements.push(debug);
            cursor = next_cursor;
            continue;
        }
        if starts_with_keyword(inner, cursor, "foreach") {
            let (statement, debug, next_cursor) =
                parse_foreach_statement_at(inner, cursor, type_table, inner_offset)?;
            statements.push(statement);
            debug_statements.push(debug);
            cursor = next_cursor;
            continue;
        }
        if starts_with_keyword(inner, cursor, "if") {
            let (statement, debug, next_cursor) =
                parse_if_statement_at(inner, cursor, type_table, inner_offset)?;
            statements.push(statement);
            debug_statements.push(debug);
            cursor = next_cursor;
            continue;
        }
        if starts_with_keyword(inner, cursor, "while") {
            return Err(format!(
                "unsupported statement in function body near '{}'",
                snippet_from(inner, cursor)
            ));
        }
        if looks_like_from_conversion_statement(inner, cursor) {
            let start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[start..semicolon].trim();
            let statement = parse_from_conversion_statement(statement_text)?;
            statements.push(statement);
            debug_statements.push(debug_statement(inner_offset, start, Vec::new())?);
            cursor = semicolon + 1;
            continue;
        }
        if looks_like_assignment(inner, cursor) {
            let assignment_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[assignment_start..semicolon].trim();
            let statement = parse_assignment_statement(statement_text)?;
            statements.push(statement);
            debug_statements.push(debug_statement(inner_offset, assignment_start, Vec::new())?);
            cursor = semicolon + 1;
            continue;
        }
        if looks_like_call_statement(inner, cursor) {
            let call_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[call_start..semicolon].trim();
            let statement = parse_call_statement(statement_text)?;
            statements.push(statement);
            debug_statements.push(debug_statement(inner_offset, call_start, Vec::new())?);
            cursor = semicolon + 1;
            continue;
        }
        return Err(format!(
            "unsupported statement in function body near '{}'",
            snippet_from(inner, cursor)
        ));
    }
    Ok(ParsedSimpleStatements {
        statements,
        debug_statements,
    })
}

fn debug_statement(
    base: usize,
    relative: usize,
    children: Vec<DebugStatement>,
) -> Result<DebugStatement, String> {
    Ok(DebugStatement {
        source_offset: u32::try_from(base.saturating_add(relative))
            .map_err(|_| "statement source offset exceeds u32".to_string())?,
        children,
    })
}

pub(crate) fn parse_let_statement(
    statement_text: &str,
    type_table: &mut TypeTable,
) -> Result<SimpleStmt, String> {
    let after_let = statement_text
        .strip_prefix("let")
        .ok_or_else(|| format!("invalid let statement '{statement_text}'"))?;
    let mut cursor = skip_ascii_whitespace(after_let, 0);
    let (name, next) = parse_identifier(after_let, cursor)?;
    cursor = skip_ascii_whitespace(after_let, next);
    let (type_id, expression) = match after_let.as_bytes().get(cursor).copied() {
        Some(b':') => {
            cursor += 1;
            cursor = skip_ascii_whitespace(after_let, cursor);
            let (type_name, initializer) =
                split_type_annotation_and_initializer(after_let, cursor)?;
            let resolved_type_id = type_table.resolve_or_intern(type_name).map_err(|_| {
                format!(
                    "unsupported let type '{}' in statement '{}'",
                    type_name, statement_text
                )
            })?;
            let expression = if let Some(expression_text) = initializer {
                parse_value_expression(expression_text)?
            } else if resolved_type_id == TYPE_ID_BOOL {
                SimpleExpr::Bool(false)
            } else if type_table.is_integer(resolved_type_id) {
                SimpleExpr::Int(0)
            } else {
                SimpleExpr::Float(0.0)
            };
            (Some(resolved_type_id), expression)
        }
        Some(b'=') => {
            cursor += 1;
            let expression_text = after_let[cursor..].trim();
            if expression_text.is_empty() {
                return Err(format!(
                    "missing expression in let statement '{}'",
                    statement_text
                ));
            }
            (None, parse_value_expression(expression_text)?)
        }
        _ => {
            return Err(format!(
                "invalid let statement '{}': expected ':' type annotation or '=' inferred initializer",
                statement_text
            ));
        }
    };
    Ok(SimpleStmt::Let {
        name: name.to_string(),
        type_id,
        expression,
    })
}

pub(crate) fn split_type_annotation_and_initializer<'a>(
    source: &'a str,
    type_start: usize,
) -> Result<(&'a str, Option<&'a str>), String> {
    if type_start >= source.len() {
        return Err("missing type annotation in let statement".to_string());
    }
    let bytes = source.as_bytes();
    let mut cursor = type_start;
    while cursor < bytes.len() {
        if bytes[cursor] == b'=' {
            let type_name = source[type_start..cursor].trim();
            if type_name.is_empty() {
                return Err("missing type annotation in let statement".to_string());
            }
            let initializer = source[cursor + 1..].trim();
            if initializer.is_empty() {
                return Err("missing expression in let statement".to_string());
            }
            return Ok((type_name, Some(initializer)));
        }
        cursor += 1;
    }
    let type_name = source[type_start..].trim();
    if type_name.is_empty() {
        return Err("missing type annotation in let statement".to_string());
    }
    Ok((type_name, None))
}

pub(crate) fn parse_assignment_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    let mut cursor = skip_ascii_whitespace(statement_text, 0);
    let (target, next) = parse_assignment_target(statement_text, cursor)?;
    cursor = skip_ascii_whitespace(statement_text, next);
    let (op, op_width) = if statement_text
        .as_bytes()
        .get(cursor..cursor + 2)
        .is_some_and(|bytes| bytes == b"+=")
    {
        (AssignOp::Add, 2)
    } else if statement_text
        .as_bytes()
        .get(cursor..cursor + 2)
        .is_some_and(|bytes| bytes == b"-=")
    {
        (AssignOp::Sub, 2)
    } else if statement_text
        .as_bytes()
        .get(cursor..cursor + 2)
        .is_some_and(|bytes| bytes == b"*=")
    {
        (AssignOp::Mul, 2)
    } else if statement_text
        .as_bytes()
        .get(cursor..cursor + 2)
        .is_some_and(|bytes| bytes == b"/=")
    {
        (AssignOp::Div, 2)
    } else if statement_text
        .as_bytes()
        .get(cursor..cursor + 2)
        .is_some_and(|bytes| bytes == b"%=")
    {
        (AssignOp::Mod, 2)
    } else if statement_text
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| *byte == b'=')
    {
        (AssignOp::Set, 1)
    } else {
        return Err(format!(
            "unsupported assignment operator in statement '{}'",
            statement_text
        ));
    };
    cursor += op_width;
    let expression_text = statement_text[cursor..].trim();
    if expression_text.is_empty() {
        return Err(format!(
            "missing expression in assignment statement '{}'",
            statement_text
        ));
    }
    Ok(SimpleStmt::Assign {
        target,
        op,
        expression: parse_value_expression(expression_text)?,
    })
}

pub(crate) fn parse_assignment_target(
    source: &str,
    cursor: usize,
) -> Result<(AssignTarget, usize), String> {
    let (first, mut next) = parse_identifier(source, cursor)?;
    let mut collection_path = first.to_string();
    let mut index_expr: Option<SimpleExpr> = None;
    let mut suffix = String::new();

    loop {
        next = skip_ascii_whitespace(source, next);
        let Some(byte) = source.as_bytes().get(next).copied() else {
            break;
        };
        if byte == b'.' {
            next += 1;
            next = skip_ascii_whitespace(source, next);
            let (segment, after_segment) = parse_identifier(source, next)?;
            if index_expr.is_none() {
                collection_path.push('.');
                collection_path.push_str(segment);
            } else {
                if !suffix.is_empty() {
                    suffix.push('.');
                }
                suffix.push_str(segment);
            }
            next = after_segment;
            continue;
        }
        if byte == b'[' {
            if index_expr.is_some() {
                return Err(format!(
                    "multiple index segments are unsupported in assignment target near '{}'",
                    snippet_from(source, next)
                ));
            }
            let close = find_matching_delimiter(source, next, b'[', b']').ok_or_else(|| {
                format!(
                    "missing closing ']' in assignment target near '{}'",
                    snippet_from(source, next)
                )
            })?;
            let index_text = source[next + 1..close].trim();
            if index_text.is_empty() {
                return Err(format!(
                    "empty index expression in assignment target near '{}'",
                    snippet_from(source, next)
                ));
            }
            index_expr = Some(parse_simple_expression(index_text)?);
            if let Some(const_i64) = eval_const_i64(index_expr.as_ref().expect("index expr set")) {
                if const_i64 < 0 {
                    return Err(
                        "negative collection indices are unsupported (use .length/.max_length)"
                            .to_string(),
                    );
                }
            }
            next = close + 1;
            continue;
        }
        break;
    }

    if let Some(index) = index_expr {
        Ok((
            AssignTarget::IndexedPath {
                collection_path,
                index,
                suffix,
            },
            next,
        ))
    } else {
        Ok((assign_target_from_path(collection_path), next))
    }
}

pub(crate) fn parse_from_conversion_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    let trimmed = statement_text.trim();
    let marker_i32 = ".from_i32(";
    let marker_f32 = ".from_f32(";
    let marker_f64 = ".from_f64(";
    let (marker_pos, marker, kind) = if let Some(pos) = trimmed.find(marker_i32) {
        (pos, marker_i32, ConversionKind::FromI32)
    } else if let Some(pos) = trimmed.find(marker_f32) {
        (pos, marker_f32, ConversionKind::FromF32)
    } else if let Some(pos) = trimmed.find(marker_f64) {
        (pos, marker_f64, ConversionKind::FromF64)
    } else {
        return Err(format!(
            "unsupported conversion statement '{}': expected from_i32, from_f32, or from_f64",
            statement_text
        ));
    };

    let target_text = trimmed[..marker_pos].trim();
    if target_text.is_empty() {
        return Err(format!(
            "missing conversion target in statement '{}'",
            statement_text
        ));
    }

    let open = marker_pos + marker.len() - 1;
    let close = find_matching_delimiter(trimmed, open, b'(', b')')
        .ok_or_else(|| format!("missing ')' in conversion statement '{statement_text}'"))?;
    let arg_text = trimmed[open + 1..close].trim();
    if arg_text.is_empty() {
        return Err(format!(
            "missing source expression in conversion statement '{}'",
            statement_text
        ));
    }
    let source = parse_simple_expression(arg_text)?;
    let trailing = trimmed[close + 1..].trim();
    if !trailing.is_empty() {
        return Err(format!(
            "unexpected trailing tokens in conversion statement '{}'",
            statement_text
        ));
    }
    let (target, next) = parse_assignment_target(target_text, 0)?;
    if skip_ascii_whitespace(target_text, next) != target_text.len() {
        return Err(format!(
            "unsupported conversion target '{}' in statement '{}'",
            target_text, statement_text
        ));
    }
    Ok(SimpleStmt::Convert {
        target,
        kind,
        source,
    })
}

pub(crate) fn parse_call_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    let expression = parse_value_expression(statement_text)?;
    if matches!(expression, SimpleExpr::Call { .. }) {
        Ok(SimpleStmt::Expr(expression))
    } else {
        Err(format!(
            "unsupported expression statement '{}': expected call expression",
            statement_text
        ))
    }
}

pub(crate) fn parse_return_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    let after_return = statement_text
        .strip_prefix("return")
        .ok_or_else(|| format!("invalid return statement '{statement_text}'"))?;
    let expression_text = after_return.trim();
    if expression_text.is_empty() {
        return Ok(SimpleStmt::ReturnVoid);
    }
    Ok(SimpleStmt::Return(parse_value_expression(expression_text)?))
}

pub(crate) fn parse_continue_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    if statement_text.trim() != "continue" {
        return Err(format!(
            "invalid continue statement '{}': expected bare continue",
            statement_text
        ));
    }
    Ok(SimpleStmt::Continue)
}

fn parse_for_statement_at(
    source: &str,
    start: usize,
    type_table: &mut TypeTable,
    source_offset: usize,
) -> Result<(SimpleStmt, DebugStatement, usize), String> {
    let mut cursor = start + "for".len();
    cursor = skip_ascii_whitespace_and_comments(source, cursor);
    cursor = expect_byte(source, cursor, b'(', "'(' after for")?;
    let header_open = cursor - 1;
    let header_close = find_matching_delimiter(source, header_open, b'(', b')')
        .ok_or_else(|| "missing ')' for for-header".to_string())?;
    let header = source[header_open + 1..header_close].trim();
    let header_parts = split_for_header(header)?;
    let init_text = header_parts[0].trim();
    let condition_text = header_parts[1].trim();
    let step_text = header_parts[2].trim();
    if init_text.is_empty() || condition_text.is_empty() || step_text.is_empty() {
        return Err(format!(
            "for header must include init, condition, and step: '{}'",
            header
        ));
    }

    let init = parse_for_control_segment(init_text, type_table)?;
    let condition = parse_simple_condition(condition_text)?;
    let step = parse_for_control_segment(step_text, type_table)?;

    cursor = skip_ascii_whitespace_and_comments(source, header_close + 1);
    cursor = expect_byte(source, cursor, b'{', "'{' after for header")?;
    let body_open = cursor - 1;
    let body_close = find_matching_delimiter(source, body_open, b'{', b'}')
        .ok_or_else(|| "missing '}' for for body".to_string())?;
    let body_block = &source[body_open..=body_close];
    let parsed_body = parse_simple_statements_with_debug_at(
        body_block,
        type_table,
        source_offset.saturating_add(body_open),
    )?;
    let next_cursor = body_close + 1;

    let debug = debug_statement(source_offset, start, parsed_body.debug_statements)?;
    Ok((
        SimpleStmt::For {
            init: Box::new(init),
            condition,
            step: Box::new(step),
            body_statements: parsed_body.statements,
        },
        debug,
        next_cursor,
    ))
}

pub(crate) fn parse_for_control_segment(
    segment_text: &str,
    type_table: &mut TypeTable,
) -> Result<SimpleStmt, String> {
    let trimmed = segment_text.trim();
    if trimmed.is_empty() {
        return Ok(SimpleStmt::Noop);
    }
    if starts_with_keyword(trimmed, 0, "let") {
        return parse_let_statement(trimmed, type_table);
    }
    if trimmed.contains(".from_i32(")
        || trimmed.contains(".from_f32(")
        || trimmed.contains(".from_f64(")
    {
        return parse_from_conversion_statement(trimmed);
    }
    if looks_like_assignment(trimmed, 0) {
        return parse_assignment_statement(trimmed);
    }
    if let Ok(call_statement) = parse_call_statement(trimmed) {
        return Ok(call_statement);
    }
    Err(format!(
        "unsupported for-loop control segment '{}'",
        trimmed
    ))
}

fn parse_foreach_statement_at(
    source: &str,
    start: usize,
    type_table: &mut TypeTable,
    source_offset: usize,
) -> Result<(SimpleStmt, DebugStatement, usize), String> {
    let mut cursor = start + "foreach".len();
    cursor = skip_ascii_whitespace_and_comments(source, cursor);
    cursor = expect_byte(source, cursor, b'(', "'(' after foreach")?;
    let header_open = cursor - 1;
    let header_close = find_matching_delimiter(source, header_open, b'(', b')')
        .ok_or_else(|| "missing ')' for foreach-header".to_string())?;
    let header = source[header_open + 1..header_close].trim();
    if !starts_with_keyword(header, 0, "let") {
        return Err(format!(
            "foreach header must start with 'let': '{}'",
            header
        ));
    }
    let header_body = header
        .strip_prefix("let")
        .ok_or_else(|| format!("invalid foreach header '{}'", header))?;
    let mut header_cursor = skip_ascii_whitespace(header_body, 0);
    let (first_identifier, next) = parse_identifier(header_body, header_cursor)?;
    header_cursor = skip_ascii_whitespace(header_body, next);

    let mut item_name = first_identifier.to_string();
    let mut index_name: Option<String> = None;
    if header_body.as_bytes().get(header_cursor).copied() == Some(b',') {
        header_cursor += 1;
        header_cursor = skip_ascii_whitespace(header_body, header_cursor);
        let (second_identifier, next) = parse_identifier(header_body, header_cursor)?;
        item_name = first_identifier.to_string();
        index_name = Some(second_identifier.to_string());
        header_cursor = skip_ascii_whitespace(header_body, next);
    }
    if !starts_with_keyword(header_body, header_cursor, "in") {
        return Err(format!(
            "foreach header must include 'in <collection>' segment: '{}'",
            header
        ));
    }
    header_cursor += "in".len();
    header_cursor = skip_ascii_whitespace(header_body, header_cursor);
    let (collection_path, next) = parse_identifier_path(header_body, header_cursor)?;
    header_cursor = skip_ascii_whitespace(header_body, next);
    if header_cursor != header_body.len() {
        return Err(format!(
            "unexpected trailing tokens in foreach header '{}'",
            header
        ));
    }

    cursor = skip_ascii_whitespace_and_comments(source, header_close + 1);
    cursor = expect_byte(source, cursor, b'{', "'{' after foreach header")?;
    let body_open = cursor - 1;
    let body_close = find_matching_delimiter(source, body_open, b'{', b'}')
        .ok_or_else(|| "missing '}' for foreach body".to_string())?;
    let body_block = &source[body_open..=body_close];
    let parsed_body = parse_simple_statements_with_debug_at(
        body_block,
        type_table,
        source_offset.saturating_add(body_open),
    )?;
    let next_cursor = body_close + 1;

    let debug = debug_statement(source_offset, start, parsed_body.debug_statements)?;
    Ok((
        SimpleStmt::Foreach {
            item_name,
            index_name,
            collection_path,
            body_statements: parsed_body.statements,
        },
        debug,
        next_cursor,
    ))
}

fn parse_if_statement_at(
    source: &str,
    start: usize,
    type_table: &mut TypeTable,
    source_offset: usize,
) -> Result<(SimpleStmt, DebugStatement, usize), String> {
    let mut cursor = start + "if".len();
    cursor = skip_ascii_whitespace_and_comments(source, cursor);
    cursor = expect_byte(source, cursor, b'(', "'(' after if")?;
    let condition_open = cursor - 1;
    let condition_close = find_matching_delimiter(source, condition_open, b'(', b')')
        .ok_or_else(|| "missing ')' for if condition".to_string())?;
    let condition_text = source[condition_open + 1..condition_close].trim();
    if condition_text.is_empty() {
        return Err("if condition expression cannot be empty".to_string());
    }
    let condition = parse_simple_condition(condition_text)?;

    cursor = skip_ascii_whitespace_and_comments(source, condition_close + 1);
    cursor = expect_byte(source, cursor, b'{', "'{' after if condition")?;
    let then_open = cursor - 1;
    let then_close = find_matching_delimiter(source, then_open, b'{', b'}')
        .ok_or_else(|| "missing '}' for if body".to_string())?;
    let then_block = &source[then_open..=then_close];
    let parsed_then = parse_simple_statements_with_debug_at(
        then_block,
        type_table,
        source_offset.saturating_add(then_open),
    )?;
    let mut next_cursor = then_close + 1;
    let mut else_statements: Option<Vec<SimpleStmt>> = None;
    let mut children = parsed_then.debug_statements;

    let else_cursor = skip_ascii_whitespace_and_comments(source, next_cursor);
    if starts_with_keyword(source, else_cursor, "else") {
        let mut cursor = else_cursor + "else".len();
        cursor = skip_ascii_whitespace_and_comments(source, cursor);
        if starts_with_keyword(source, cursor, "if") {
            let (else_if_statement, else_if_debug, after_else_if) =
                parse_if_statement_at(source, cursor, type_table, source_offset)?;
            else_statements = Some(vec![else_if_statement]);
            children.push(else_if_debug);
            next_cursor = after_else_if;
        } else {
            cursor = expect_byte(source, cursor, b'{', "'{' after else")?;
            let else_open = cursor - 1;
            let else_close = find_matching_delimiter(source, else_open, b'{', b'}')
                .ok_or_else(|| "missing '}' for else body".to_string())?;
            let else_block = &source[else_open..=else_close];
            let parsed_else = parse_simple_statements_with_debug_at(
                else_block,
                type_table,
                source_offset.saturating_add(else_open),
            )?;
            else_statements = Some(parsed_else.statements);
            children.extend(parsed_else.debug_statements);
            next_cursor = else_close + 1;
        }
    }

    let debug = debug_statement(source_offset, start, children)?;
    Ok((
        SimpleStmt::If {
            condition,
            then_statements: parsed_then.statements,
            else_statements,
        },
        debug,
        next_cursor,
    ))
}

pub(crate) fn parse_simple_condition(condition_text: &str) -> Result<SimpleCondition, String> {
    parse_or_condition(condition_text.trim())
}

pub(crate) fn parse_or_condition(condition_text: &str) -> Result<SimpleCondition, String> {
    let parts = split_top_level_condition(condition_text, b"||");
    if parts.len() == 1 {
        return parse_and_condition(parts[0]);
    }
    let mut cursor = parts.into_iter();
    let first = cursor
        .next()
        .ok_or_else(|| format!("invalid logical-or condition '{}'", condition_text))?;
    let mut out = parse_and_condition(first)?;
    for part in cursor {
        let rhs = parse_and_condition(part)?;
        out = SimpleCondition::Or(Box::new(out), Box::new(rhs));
    }
    Ok(out)
}

pub(crate) fn parse_and_condition(condition_text: &str) -> Result<SimpleCondition, String> {
    let parts = split_top_level_condition(condition_text, b"&&");
    if parts.len() == 1 {
        return parse_not_condition(parts[0]);
    }
    let mut cursor = parts.into_iter();
    let first = cursor
        .next()
        .ok_or_else(|| format!("invalid logical-and condition '{}'", condition_text))?;
    let mut out = parse_not_condition(first)?;
    for part in cursor {
        let rhs = parse_not_condition(part)?;
        out = SimpleCondition::And(Box::new(out), Box::new(rhs));
    }
    Ok(out)
}

pub(crate) fn parse_not_condition(condition_text: &str) -> Result<SimpleCondition, String> {
    let trimmed = condition_text.trim();
    if trimmed.is_empty() {
        return Err("condition expression cannot be empty".to_string());
    }
    if let Some(rest) = trimmed.strip_prefix('!') {
        let inner = parse_not_condition(rest)?;
        return Ok(SimpleCondition::Not(Box::new(inner)));
    }
    parse_condition_atom(trimmed)
}

pub(crate) fn parse_condition_atom(condition_text: &str) -> Result<SimpleCondition, String> {
    let trimmed = condition_text.trim();
    if trimmed.is_empty() {
        return Err("condition expression cannot be empty".to_string());
    }
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        if let Some(close_index) = find_matching_delimiter(trimmed, 0, b'(', b')') {
            if close_index == trimmed.len() - 1 {
                let inner = &trimmed[1..trimmed.len() - 1];
                return parse_or_condition(inner.trim());
            }
        }
    }
    if let Some((op, position, width)) = find_condition_operator(trimmed) {
        let lhs_text = trimmed[..position].trim();
        let rhs_text = trimmed[position + width..].trim();
        if lhs_text.is_empty() || rhs_text.is_empty() {
            return Err(format!(
                "invalid if condition '{}': both sides of comparison are required",
                trimmed
            ));
        }
        return Ok(SimpleCondition::Comparison {
            lhs: parse_simple_expression(lhs_text)?,
            op,
            rhs: parse_simple_expression(rhs_text)?,
        });
    }
    Ok(SimpleCondition::Expr(parse_simple_expression(trimmed)?))
}

pub(crate) fn split_top_level_condition<'a>(condition_text: &'a str, op: &[u8; 2]) -> Vec<&'a str> {
    let bytes = condition_text.as_bytes();
    let mut parts: Vec<&'a str> = Vec::new();
    let mut depth = 0i32;
    let mut segment_start = 0usize;
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        if in_string {
            if escaped {
                escaped = false;
                index += 1;
                continue;
            }
            if bytes[index] == b'\\' {
                escaped = true;
                index += 1;
                continue;
            }
            if bytes[index] == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            index += 2;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        match bytes[index] {
            b'"' => {
                in_string = true;
                index += 1;
                continue;
            }
            b'(' => {
                depth += 1;
                index += 1;
                continue;
            }
            b')' => {
                depth -= 1;
                index += 1;
                continue;
            }
            _ => {}
        }
        if depth == 0
            && index + 1 < bytes.len()
            && bytes[index] == op[0]
            && bytes[index + 1] == op[1]
        {
            parts.push(condition_text[segment_start..index].trim());
            segment_start = index + 2;
            index += 2;
            continue;
        }
        index += 1;
    }
    parts.push(condition_text[segment_start..].trim());
    parts
}

pub(crate) fn find_condition_operator(
    condition_text: &str,
) -> Option<(ComparisonOp, usize, usize)> {
    let bytes = condition_text.as_bytes();
    let mut depth = 0i32;
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        if in_string {
            if escaped {
                escaped = false;
                index += 1;
                continue;
            }
            if bytes[index] == b'\\' {
                escaped = true;
                index += 1;
                continue;
            }
            if bytes[index] == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            index += 2;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        match bytes[index] {
            b'"' => in_string = true,
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'=' | b'!' | b'<' | b'>' if depth == 0 => {
                if index + 1 < bytes.len() {
                    match (bytes[index], bytes[index + 1]) {
                        (b'=', b'=') => return Some((ComparisonOp::Eq, index, 2)),
                        (b'!', b'=') => return Some((ComparisonOp::Ne, index, 2)),
                        (b'<', b'=') => return Some((ComparisonOp::Le, index, 2)),
                        (b'>', b'=') => return Some((ComparisonOp::Ge, index, 2)),
                        _ => {}
                    }
                }
                match bytes[index] {
                    b'<' => return Some((ComparisonOp::Lt, index, 1)),
                    b'>' => return Some((ComparisonOp::Gt, index, 1)),
                    _ => {}
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

pub(crate) fn skip_ascii_whitespace(source: &str, mut cursor: usize) -> usize {
    while cursor < source.len() && source.as_bytes()[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

pub(crate) fn skip_ascii_whitespace_and_comments(source: &str, mut cursor: usize) -> usize {
    loop {
        cursor = skip_ascii_whitespace(source, cursor);
        let bytes = source.as_bytes();
        if cursor + 1 < bytes.len() && bytes[cursor] == b'/' && bytes[cursor + 1] == b'/' {
            cursor += 2;
            while cursor < bytes.len() && bytes[cursor] != b'\n' {
                cursor += 1;
            }
            continue;
        }
        if cursor + 1 < bytes.len() && bytes[cursor] == b'/' && bytes[cursor + 1] == b'*' {
            cursor += 2;
            let mut closed = false;
            while cursor + 1 < bytes.len() {
                if bytes[cursor] == b'*' && bytes[cursor + 1] == b'/' {
                    cursor += 2;
                    closed = true;
                    break;
                }
                cursor += 1;
            }
            if !closed {
                return bytes.len();
            }
            continue;
        }
        return cursor;
    }
}

pub(crate) fn starts_with_keyword(source: &str, cursor: usize, keyword: &str) -> bool {
    let Some(tail) = source.get(cursor..) else {
        return false;
    };
    if !tail.starts_with(keyword) {
        return false;
    }
    let end = cursor + keyword.len();
    if end >= source.len() {
        return true;
    }
    !source.as_bytes()[end].is_ascii_alphanumeric() && source.as_bytes()[end] != b'_'
}

pub(crate) fn looks_like_assignment(source: &str, cursor: usize) -> bool {
    let bytes = source.as_bytes();
    if cursor >= bytes.len() {
        return false;
    }
    if !bytes[cursor].is_ascii_alphabetic() && bytes[cursor] != b'_' {
        return false;
    }
    let mut index = cursor;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => paren_depth += 1,
            b')' => {
                paren_depth -= 1;
                if paren_depth < 0 {
                    return false;
                }
            }
            b'[' => bracket_depth += 1,
            b']' => {
                bracket_depth -= 1;
                if bracket_depth < 0 {
                    return false;
                }
            }
            b';' if paren_depth == 0 && bracket_depth == 0 => return false,
            b'=' if paren_depth == 0 && bracket_depth == 0 => {
                if index + 1 < bytes.len() && bytes[index + 1] == b'=' {
                    return false;
                }
                return true;
            }
            _ => {}
        }
        index += 1;
    }
    false
}

pub(crate) fn looks_like_from_conversion_statement(source: &str, cursor: usize) -> bool {
    let Ok(semicolon) = find_statement_terminator(source, cursor) else {
        return false;
    };
    let tail = source.get(cursor..semicolon).unwrap_or_default().trim();
    let Some(dot_pos) = tail.find(".from_") else {
        return false;
    };
    if dot_pos == 0 {
        return false;
    }
    let prefix = tail[..dot_pos].trim();
    if prefix.is_empty() {
        return false;
    }
    let first = prefix.as_bytes()[0];
    if !first.is_ascii_alphabetic() && first != b'_' {
        return false;
    }
    let method_tail = &tail[dot_pos..];
    method_tail.starts_with(".from_i32(")
        || method_tail.starts_with(".from_f32(")
        || method_tail.starts_with(".from_f64(")
}

pub(crate) fn looks_like_call_statement(source: &str, cursor: usize) -> bool {
    let Ok(semicolon) = find_statement_terminator(source, cursor) else {
        return false;
    };
    let statement_text = source.get(cursor..semicolon).unwrap_or_default().trim();
    if statement_text.is_empty() {
        return false;
    }
    let Ok(expression) = parse_simple_expression(statement_text) else {
        return false;
    };
    matches!(expression, SimpleExpr::Call { .. })
}

pub(crate) fn split_for_header(header: &str) -> Result<[String; 3], String> {
    let mut parts: Vec<String> = Vec::new();
    let bytes = header.as_bytes();
    let mut depth = 0i32;
    let mut segment_start = 0usize;
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        if in_string {
            if escaped {
                escaped = false;
                index += 1;
                continue;
            }
            if bytes[index] == b'\\' {
                escaped = true;
                index += 1;
                continue;
            }
            if bytes[index] == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            index += 2;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        match bytes[index] {
            b'"' => in_string = true,
            b'(' => depth += 1,
            b')' => depth -= 1,
            b';' if depth == 0 => {
                parts.push(header[segment_start..index].to_string());
                segment_start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    parts.push(header[segment_start..].to_string());
    if parts.len() != 3 {
        return Err(format!(
            "for header must contain exactly 3 segments separated by ';': '{}'",
            header
        ));
    }
    Ok([parts.remove(0), parts.remove(0), parts.remove(0)])
}

pub(crate) fn find_statement_terminator(source: &str, start: usize) -> Result<usize, String> {
    let bytes = source.as_bytes();
    let mut paren_depth = 0i32;
    let mut brace_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut index = start;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        if in_string {
            let byte = bytes[index];
            if escaped {
                escaped = false;
                index += 1;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                index += 1;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            index += 2;
            let mut closed = false;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    closed = true;
                    break;
                }
                index += 1;
            }
            if !closed {
                return Err(format!(
                    "unterminated block comment near '{}'",
                    snippet_from(source, start)
                ));
            }
            continue;
        }
        match bytes[index] {
            b'"' => in_string = true,
            b'(' => paren_depth += 1,
            b')' => paren_depth -= 1,
            b'{' => brace_depth += 1,
            b'}' => brace_depth -= 1,
            b'[' => bracket_depth += 1,
            b']' => bracket_depth -= 1,
            b';' if paren_depth == 0 && brace_depth == 0 && bracket_depth == 0 => return Ok(index),
            _ => {}
        }
        index += 1;
    }
    Err(format!(
        "missing ';' terminator near '{}'",
        snippet_from(source, start)
    ))
}

pub(crate) fn find_matching_delimiter(
    source: &str,
    open_index: usize,
    open: u8,
    close: u8,
) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(open_index).copied() != Some(open) {
        return None;
    }
    let mut depth = 0i32;
    let mut index = open_index;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        if in_string {
            let byte = bytes[index];
            if escaped {
                escaped = false;
                index += 1;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                index += 1;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            index += 2;
            let mut closed = false;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    closed = true;
                    break;
                }
                index += 1;
            }
            if !closed {
                return None;
            }
            continue;
        }
        let byte = bytes[index];
        if byte == b'"' {
            in_string = true;
        } else if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

pub(crate) fn expect_byte(
    source: &str,
    cursor: usize,
    expected: u8,
    context: &str,
) -> Result<usize, String> {
    if cursor >= source.len() || source.as_bytes()[cursor] != expected {
        return Err(format!(
            "expected {} near '{}'",
            context,
            snippet_from(source, cursor)
        ));
    }
    Ok(cursor + 1)
}

pub(crate) fn parse_identifier(source: &str, cursor: usize) -> Result<(&str, usize), String> {
    let bytes = source.as_bytes();
    if cursor >= bytes.len() {
        return Err("expected identifier but reached end of statement".to_string());
    }
    let start_byte = bytes[cursor];
    if !start_byte.is_ascii_alphabetic() && start_byte != b'_' {
        return Err(format!(
            "expected identifier near '{}'",
            snippet_from(source, cursor)
        ));
    }
    let mut end = cursor + 1;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    Ok((&source[cursor..end], end))
}

pub(crate) fn parse_identifier_path(
    source: &str,
    cursor: usize,
) -> Result<(String, usize), String> {
    let (first, mut next) = parse_identifier(source, cursor)?;
    let mut path = first.to_string();
    loop {
        next = skip_ascii_whitespace(source, next);
        if source.as_bytes().get(next).copied() != Some(b'.') {
            break;
        }
        next += 1;
        next = skip_ascii_whitespace(source, next);
        let (segment, after_segment) = parse_identifier(source, next)?;
        path.push('.');
        path.push_str(segment);
        next = after_segment;
    }
    Ok((path, next))
}

pub(crate) fn assign_target_from_path(path: String) -> AssignTarget {
    if path.contains('.') {
        AssignTarget::GlobalPath(path)
    } else {
        AssignTarget::Local(path)
    }
}

pub(crate) fn snippet_from(source: &str, cursor: usize) -> String {
    source
        .get(cursor..)
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect()
}

pub(crate) fn parse_simple_expression(expression: &str) -> Result<SimpleExpr, String> {
    let tokens = tokenize_simple_expression(expression)?;
    let mut parser = ExprParser {
        tokens: &tokens,
        cursor: 0,
    };
    let parsed = parser.parse_precedence(0)?;
    if parser.cursor != parser.tokens.len() {
        return Err(format!(
            "unexpected trailing tokens in expression '{}'",
            expression
        ));
    }
    Ok(parsed)
}

pub(crate) fn parse_value_expression(expression: &str) -> Result<SimpleExpr, String> {
    match parse_simple_expression(expression) {
        Ok(parsed) => Ok(parsed),
        Err(primary_error) => {
            if !looks_like_condition_expression(expression) {
                return Err(primary_error);
            }
            match parse_simple_condition(expression) {
                Ok(condition) => Ok(SimpleExpr::Condition(Box::new(condition))),
                Err(_) => Err(primary_error),
            }
        }
    }
}

pub(crate) fn looks_like_condition_expression(expression: &str) -> bool {
    let bytes = expression.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'<' || byte == b'>' {
            return true;
        }
        if byte == b'=' && index + 1 < bytes.len() && bytes[index + 1] == b'=' {
            return true;
        }
        if byte == b'!' {
            if index + 1 < bytes.len() && bytes[index + 1] == b'=' {
                return true;
            }
            return true;
        }
        if byte == b'&' && index + 1 < bytes.len() && bytes[index + 1] == b'&' {
            return true;
        }
        if byte == b'|' && index + 1 < bytes.len() && bytes[index + 1] == b'|' {
            return true;
        }
        index += 1;
    }
    false
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ExprToken {
    Int(i64),
    Float(f64),
    StringLiteral(String),
    Identifier(String),
    Op(char),
    Comma,
    Dot,
    LBracket,
    RBracket,
    LParen,
    RParen,
}

pub(crate) fn tokenize_simple_expression(expression: &str) -> Result<Vec<ExprToken>, String> {
    let bytes = expression.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if byte == b'/' && index + 1 < bytes.len() {
            let next = bytes[index + 1];
            if next == b'/' {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
                continue;
            }
            if next == b'*' {
                index += 2;
                let mut closed = false;
                while index + 1 < bytes.len() {
                    if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                        index += 2;
                        closed = true;
                        break;
                    }
                    index += 1;
                }
                if !closed {
                    return Err(format!(
                        "unterminated block comment in expression '{}'",
                        expression
                    ));
                }
                continue;
            }
        }
        if byte == b'"' {
            index += 1;
            let mut literal = String::new();
            let mut closed = false;
            while index < bytes.len() {
                let current = bytes[index];
                if current == b'\\' {
                    index += 1;
                    if index >= bytes.len() {
                        return Err(format!(
                            "unterminated escape sequence in string literal '{}'",
                            expression
                        ));
                    }
                    let escaped = bytes[index];
                    let decoded = match escaped {
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'0' => '\0',
                        b'\\' => '\\',
                        b'"' => '"',
                        _ => {
                            return Err(format!(
                                "unsupported escape sequence '\\{}' in expression '{}'",
                                escaped as char, expression
                            ))
                        }
                    };
                    literal.push(decoded);
                    index += 1;
                    continue;
                }
                if current == b'"' {
                    index += 1;
                    closed = true;
                    break;
                }
                let Some(next_char) = expression[index..].chars().next() else {
                    return Err(format!(
                        "unterminated string literal in expression '{}'",
                        expression
                    ));
                };
                literal.push(next_char);
                index += next_char.len_utf8();
            }
            if !closed {
                return Err(format!(
                    "unterminated string literal in expression '{}'",
                    expression
                ));
            }
            tokens.push(ExprToken::StringLiteral(literal));
            continue;
        }
        if byte.is_ascii_digit() {
            let start = index;
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            if index < bytes.len()
                && bytes[index] == b'.'
                && index + 1 < bytes.len()
                && bytes[index + 1].is_ascii_digit()
            {
                index += 1;
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    index += 1;
                }
                let text = &expression[start..index];
                let value = text
                    .parse::<f64>()
                    .map_err(|error| format!("invalid float literal '{text}': {error}"))?;
                tokens.push(ExprToken::Float(value));
            } else {
                let text = &expression[start..index];
                let value = text
                    .parse::<i64>()
                    .map_err(|error| format!("invalid integer literal '{text}': {error}"))?;
                tokens.push(ExprToken::Int(value));
            }
            continue;
        }
        if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = index;
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            tokens.push(ExprToken::Identifier(expression[start..index].to_string()));
            continue;
        }
        match byte {
            b'+' | b'-' | b'*' | b'/' | b'%' => {
                tokens.push(ExprToken::Op(byte as char));
                index += 1;
            }
            b',' => {
                tokens.push(ExprToken::Comma);
                index += 1;
            }
            b'.' => {
                tokens.push(ExprToken::Dot);
                index += 1;
            }
            b'(' => {
                tokens.push(ExprToken::LParen);
                index += 1;
            }
            b')' => {
                tokens.push(ExprToken::RParen);
                index += 1;
            }
            b'[' => {
                tokens.push(ExprToken::LBracket);
                index += 1;
            }
            b']' => {
                tokens.push(ExprToken::RBracket);
                index += 1;
            }
            _ => {
                return Err(format!(
                    "unsupported token '{}' in return expression '{}'",
                    byte as char, expression
                ));
            }
        }
    }
    Ok(tokens)
}

pub(crate) struct ExprParser<'a> {
    tokens: &'a [ExprToken],
    cursor: usize,
}

impl ExprParser<'_> {
    fn parse_precedence(&mut self, min_precedence: u8) -> Result<SimpleExpr, String> {
        let mut lhs = self.parse_primary()?;
        while let Some((operator, precedence)) = self.peek_binary_operator() {
            if precedence < min_precedence {
                break;
            }
            self.cursor += 1;
            let rhs = self.parse_precedence(precedence + 1)?;
            lhs = SimpleExpr::Binary {
                lhs: Box::new(lhs),
                op: operator,
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_primary(&mut self) -> Result<SimpleExpr, String> {
        let token = self
            .tokens
            .get(self.cursor)
            .ok_or_else(|| "unexpected end of expression".to_string())?
            .clone();
        self.cursor += 1;
        match token {
            ExprToken::Int(value) => Ok(SimpleExpr::Int(value)),
            ExprToken::Float(value) => Ok(SimpleExpr::Float(value)),
            ExprToken::StringLiteral(value) => Ok(SimpleExpr::StringLiteral(value)),
            ExprToken::Identifier(name) => {
                if name == "true" {
                    return Ok(SimpleExpr::Bool(true));
                }
                if name == "false" {
                    return Ok(SimpleExpr::Bool(false));
                }
                if matches!(self.tokens.get(self.cursor), Some(ExprToken::LParen)) {
                    self.cursor += 1;
                    let mut args = Vec::new();
                    if !matches!(self.tokens.get(self.cursor), Some(ExprToken::RParen)) {
                        loop {
                            args.push(self.parse_precedence(0)?);
                            if matches!(self.tokens.get(self.cursor), Some(ExprToken::Comma)) {
                                self.cursor += 1;
                                continue;
                            }
                            break;
                        }
                    }
                    match self.tokens.get(self.cursor) {
                        Some(ExprToken::RParen) => {
                            self.cursor += 1;
                            Ok(SimpleExpr::Call { target: name, args })
                        }
                        _ => Err("expected ')' after call arguments".to_string()),
                    }
                } else {
                    self.parse_identifier_access_chain(name)
                }
            }
            ExprToken::Op('-') => {
                let rhs = self.parse_primary()?;
                let lhs = match rhs {
                    SimpleExpr::Float(_) => SimpleExpr::Float(0.0),
                    _ => SimpleExpr::Int(0),
                };
                Ok(SimpleExpr::Binary {
                    lhs: Box::new(lhs),
                    op: '-',
                    rhs: Box::new(rhs),
                })
            }
            ExprToken::Op('+') => self.parse_primary(),
            ExprToken::LParen => {
                let expr = self.parse_precedence(0)?;
                match self.tokens.get(self.cursor) {
                    Some(ExprToken::RParen) => {
                        self.cursor += 1;
                        Ok(expr)
                    }
                    _ => Err("expected ')' in expression".to_string()),
                }
            }
            other => Err(format!("unexpected token {other:?} in expression")),
        }
    }

    fn parse_identifier_access_chain(&mut self, first: String) -> Result<SimpleExpr, String> {
        let mut collection_path = first;
        let mut index_expr: Option<SimpleExpr> = None;
        let mut suffix = String::new();
        loop {
            if matches!(self.tokens.get(self.cursor), Some(ExprToken::Dot)) {
                if let (Some(ExprToken::Identifier(method)), Some(ExprToken::LParen)) = (
                    self.tokens.get(self.cursor + 1).cloned(),
                    self.tokens.get(self.cursor + 2),
                ) {
                    self.cursor += 3;
                    let receiver = if let Some(index) = index_expr {
                        SimpleExpr::IndexedPath {
                            collection_path,
                            index: Box::new(index),
                            suffix,
                        }
                    } else {
                        SimpleExpr::Identifier(collection_path)
                    };
                    let mut args = vec![receiver];
                    if !matches!(self.tokens.get(self.cursor), Some(ExprToken::RParen)) {
                        loop {
                            args.push(self.parse_precedence(0)?);
                            if matches!(self.tokens.get(self.cursor), Some(ExprToken::Comma)) {
                                self.cursor += 1;
                                continue;
                            }
                            break;
                        }
                    }
                    return match self.tokens.get(self.cursor) {
                        Some(ExprToken::RParen) => {
                            self.cursor += 1;
                            Ok(SimpleExpr::Call {
                                target: method,
                                args,
                            })
                        }
                        _ => Err("expected ')' after receiver call arguments".to_string()),
                    };
                }
                self.cursor += 1;
                let Some(ExprToken::Identifier(segment)) = self.tokens.get(self.cursor).cloned()
                else {
                    return Err("expected identifier after '.' in expression path".to_string());
                };
                self.cursor += 1;
                if index_expr.is_none() {
                    collection_path.push('.');
                    collection_path.push_str(&segment);
                } else {
                    if !suffix.is_empty() {
                        suffix.push('.');
                    }
                    suffix.push_str(&segment);
                }
                continue;
            }
            if matches!(self.tokens.get(self.cursor), Some(ExprToken::LBracket)) {
                if index_expr.is_some() {
                    return Err(
                        "multiple index segments are unsupported in expression path".to_string()
                    );
                }
                self.cursor += 1;
                let expression = self.parse_precedence(0)?;
                if let Some(const_i64) = eval_const_i64(&expression) {
                    if const_i64 < 0 {
                        return Err(
                            "negative collection indices are unsupported (use .length/.max_length)"
                                .to_string(),
                        );
                    }
                }
                match self.tokens.get(self.cursor) {
                    Some(ExprToken::RBracket) => {
                        self.cursor += 1;
                        index_expr = Some(expression);
                    }
                    _ => return Err("expected ']' in expression path".to_string()),
                }
                continue;
            }
            break;
        }
        if let Some(index) = index_expr {
            Ok(SimpleExpr::IndexedPath {
                collection_path,
                index: Box::new(index),
                suffix,
            })
        } else {
            Ok(SimpleExpr::Identifier(collection_path))
        }
    }

    fn peek_binary_operator(&self) -> Option<(char, u8)> {
        let ExprToken::Op(op) = self.tokens.get(self.cursor)? else {
            return None;
        };
        let precedence = match *op {
            '*' | '/' | '%' => 20,
            '+' | '-' => 10,
            _ => return None,
        };
        Some((*op, precedence))
    }
}
