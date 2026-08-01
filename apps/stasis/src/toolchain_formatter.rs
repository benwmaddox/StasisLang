const INDENT_WIDTH: usize = 4;
pub(crate) const LINE_WIDTH: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Word,
    Number,
    String,
    Backtick,
    LineComment,
    BlockComment,
    Symbol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    kind: TokenKind,
    text: String,
    newline_before: bool,
    blank_before: bool,
}

impl Token {
    fn is_symbol(&self, expected: &str) -> bool {
        self.kind == TokenKind::Symbol && self.text == expected
    }

    fn is_word(&self, expected: &str) -> bool {
        self.kind == TokenKind::Word && self.text == expected
    }

    fn is_comment(&self) -> bool {
        matches!(self.kind, TokenKind::LineComment | TokenKind::BlockComment)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BraceKind {
    Block,
    Enum,
}

#[derive(Debug, Clone, Copy)]
struct ParenContext {
    wrapped: bool,
    for_header: bool,
}

#[derive(Debug, Default)]
struct Writer {
    output: String,
    line: String,
    indent: usize,
    extra_indent_once: usize,
}

impl Writer {
    fn column(&self) -> usize {
        if self.line.is_empty() {
            (self.indent + self.extra_indent_once) * INDENT_WIDTH
        } else {
            self.line.chars().count()
        }
    }

    fn line_is_empty(&self) -> bool {
        self.line.is_empty()
    }

    fn ensure_indent(&mut self) {
        if self.line.is_empty() {
            let count = (self.indent + self.extra_indent_once) * INDENT_WIDTH;
            self.line.push_str(&" ".repeat(count));
            self.extra_indent_once = 0;
        }
    }

    fn write(&mut self, text: &str) {
        self.ensure_indent();
        self.line.push_str(text);
    }

    fn space(&mut self) {
        if !self.line.is_empty() && !self.line.ends_with(' ') {
            self.line.push(' ');
        }
    }

    fn trim_line_end(&mut self) {
        let trimmed = self.line.trim_end_matches([' ', '\t']).len();
        self.line.truncate(trimmed);
    }

    fn newline(&mut self) {
        self.trim_line_end();
        if !self.line.is_empty() {
            self.output.push_str(&self.line);
        }
        self.output.push('\n');
        self.line.clear();
        self.extra_indent_once = 0;
    }

    fn blank_line(&mut self) {
        if !self.line.is_empty() {
            self.newline();
        }
        while self.output.ends_with("\n\n\n") {
            self.output.pop();
        }
        if !self.output.is_empty() && !self.output.ends_with("\n\n") {
            self.output.push('\n');
        }
    }

    fn newline_if_needed(&mut self) {
        if !self.line.is_empty() {
            self.newline();
        }
    }

    fn write_multiline_comment(&mut self, text: &str) {
        let mut lines = text.split('\n');
        if let Some(first) = lines.next() {
            self.write(first.trim_end_matches('\r'));
        }
        for line in lines {
            self.newline();
            self.line.push_str(line.trim_end_matches('\r'));
        }
    }

    fn finish(mut self) -> String {
        self.newline_if_needed();
        self.output
            .truncate(self.output.trim_end_matches('\n').len());
        self.output.push('\n');
        self.output
    }
}

pub(crate) fn format_source(source: &str) -> Result<String, String> {
    let tokens = scan(source)?;
    let formatted = render(&tokens)?;
    let formatted_tokens = scan(&formatted)?;
    let original_significant = significant_tokens(&tokens);
    let formatted_significant = significant_tokens(&formatted_tokens);
    if original_significant != formatted_significant {
        return Err("formatter changed the Stasis token stream".to_string());
    }
    if compiler_tokens(source)? != compiler_tokens(&formatted)? {
        return Err("formatter changed the compiler token stream".to_string());
    }
    Ok(formatted)
}

fn compiler_tokens(
    source: &str,
) -> Result<Vec<(stasis_compiler::frontend::lexer::TokenKind, &str)>, String> {
    stasis_compiler::frontend::lexer::lex(source).map(|tokens| {
        tokens
            .into_iter()
            .map(|token| (token.kind, &source[token.start..token.end]))
            .collect()
    })
}

fn significant_tokens(tokens: &[Token]) -> Vec<(TokenKind, &str)> {
    tokens
        .iter()
        .map(|token| (token.kind, token.text.as_str()))
        .collect()
}

fn scan(source: &str) -> Result<Vec<Token>, String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = 0usize;
    let mut newline_count = 0usize;

    while cursor < bytes.len() {
        if bytes[cursor].is_ascii_whitespace() {
            if bytes[cursor] == b'\n' {
                newline_count += 1;
                cursor += 1;
            } else if bytes[cursor] == b'\r' {
                newline_count += 1;
                cursor += 1;
                if cursor < bytes.len() && bytes[cursor] == b'\n' {
                    cursor += 1;
                }
            } else {
                cursor += 1;
            }
            continue;
        }

        let start = cursor;
        let kind;
        if bytes[cursor] == b'/' && cursor + 1 < bytes.len() && bytes[cursor + 1] == b'/' {
            cursor += 2;
            while cursor < bytes.len() && !matches!(bytes[cursor], b'\r' | b'\n') {
                cursor += 1;
            }
            kind = TokenKind::LineComment;
        } else if bytes[cursor] == b'/' && cursor + 1 < bytes.len() && bytes[cursor + 1] == b'*' {
            cursor += 2;
            let mut closed = false;
            while cursor + 1 < bytes.len() {
                if bytes[cursor] == b'*' && bytes[cursor + 1] == b'/' {
                    cursor += 2;
                    closed = true;
                    break;
                }
                cursor += utf8_width(bytes[cursor]);
            }
            if !closed {
                return Err("unterminated block comment".to_string());
            }
            kind = TokenKind::BlockComment;
        } else if bytes[cursor] == b'"' {
            cursor += 1;
            let mut closed = false;
            while cursor < bytes.len() {
                if bytes[cursor] == b'\\' {
                    cursor += 1;
                    if cursor < bytes.len() {
                        cursor += utf8_width(bytes[cursor]);
                    }
                    continue;
                }
                if bytes[cursor] == b'"' {
                    cursor += 1;
                    closed = true;
                    break;
                }
                cursor += utf8_width(bytes[cursor]);
            }
            if !closed {
                return Err("unterminated string literal".to_string());
            }
            kind = TokenKind::String;
        } else if bytes[cursor] == b'`' {
            cursor += 1;
            let mut closed = false;
            while cursor < bytes.len() {
                if bytes[cursor] == b'`' {
                    cursor += 1;
                    closed = true;
                    break;
                }
                cursor += utf8_width(bytes[cursor]);
            }
            if !closed {
                return Err("unterminated backtick literal".to_string());
            }
            kind = TokenKind::Backtick;
        } else if is_word_start(bytes[cursor]) {
            cursor += 1;
            while cursor < bytes.len() && is_word_continue(bytes[cursor]) {
                cursor += 1;
            }
            kind = TokenKind::Word;
        } else if bytes[cursor].is_ascii_digit() {
            cursor += 1;
            while cursor < bytes.len() {
                if bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_' {
                    cursor += 1;
                } else if bytes[cursor] == b'.'
                    && cursor + 1 < bytes.len()
                    && bytes[cursor + 1].is_ascii_digit()
                {
                    cursor += 1;
                } else if matches!(bytes[cursor], b'+' | b'-')
                    && matches!(bytes[cursor - 1], b'e' | b'E')
                    && cursor + 1 < bytes.len()
                    && bytes[cursor + 1].is_ascii_digit()
                {
                    cursor += 1;
                } else {
                    break;
                }
            }
            kind = TokenKind::Number;
        } else {
            let width = matched_operator_width(&bytes[cursor..]);
            cursor += width;
            kind = TokenKind::Symbol;
        }

        let text = source
            .get(start..cursor)
            .ok_or_else(|| "formatter encountered an invalid UTF-8 boundary".to_string())?
            .to_string();
        tokens.push(Token {
            kind,
            text,
            newline_before: newline_count > 0,
            blank_before: newline_count > 1,
        });
        newline_count = 0;
    }

    Ok(tokens)
}

fn utf8_width(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if byte < 0xE0 {
        2
    } else if byte < 0xF0 {
        3
    } else {
        4
    }
}

fn is_word_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_word_continue(byte: u8) -> bool {
    is_word_start(byte) || byte.is_ascii_digit()
}

fn matched_operator_width(bytes: &[u8]) -> usize {
    if bytes.len() >= 2 {
        let pair = &bytes[..2];
        if matches!(
            pair,
            b"+="
                | b"-="
                | b"*="
                | b"/="
                | b"%="
                | b"=="
                | b"!="
                | b"<="
                | b">="
                | b"&&"
                | b"||"
                | b"->"
                | b"::"
        ) {
            return 2;
        }
    }
    utf8_width(bytes[0])
}

fn render(tokens: &[Token]) -> Result<String, String> {
    let mut writer = Writer::default();
    let mut braces = Vec::<BraceKind>::new();
    let mut parens = Vec::<ParenContext>::new();
    let mut bracket_depth = 0usize;
    let mut last_significant: Option<&Token> = None;
    let mut last_was_unary = false;
    let mut force_space_after_comment = false;
    let mut enum_member_started = false;
    let mut top_level_kind: Option<String> = None;
    let mut top_level_has_enum = false;

    let mut index = 0usize;
    while index < tokens.len() {
        let token = &tokens[index];

        if token.blank_before && !writer.line_is_empty() && token.is_comment() {
            writer.blank_line();
        } else if token.blank_before
            && !token.is_symbol("}")
            && writer.line_is_empty()
            && !writer.output.ends_with("\n\n")
            && !writer.output.is_empty()
        {
            writer.blank_line();
        }

        if token.kind == TokenKind::LineComment {
            let attached = !token.newline_before && !writer.line_is_empty();
            if !writer.line_is_empty() {
                writer.space();
            }
            writer.write(&token.text);
            writer.newline();
            if attached
                && braces.is_empty()
                && last_significant
                    .is_some_and(|previous| previous.is_symbol(";") || previous.is_symbol("}"))
            {
                if !keeps_import_group(tokens, index + 1, top_level_kind.as_deref()) {
                    writer.blank_line();
                    top_level_kind = None;
                    top_level_has_enum = false;
                }
            } else if last_significant.is_some_and(is_operator) {
                writer.extra_indent_once = 1;
            }
            force_space_after_comment = false;
            index += 1;
            continue;
        }

        if token.kind == TokenKind::BlockComment {
            let starts_line = writer.line_is_empty();
            let closes_line = last_significant.is_some_and(|previous| {
                (previous.is_symbol(";")
                    && !parens.last().is_some_and(|context| context.for_header))
                    || previous.is_symbol("{")
                    || previous.is_symbol("}")
            });
            if !writer.line_is_empty() {
                writer.space();
            }
            if token.text.contains(['\r', '\n']) {
                writer.write_multiline_comment(&token.text);
            } else {
                writer.write(&token.text);
            }
            if starts_line || closes_line || token.text.contains(['\r', '\n']) {
                writer.newline();
                if closes_line
                    && braces.is_empty()
                    && last_significant
                        .is_some_and(|previous| previous.is_symbol(";") || previous.is_symbol("}"))
                {
                    if !keeps_import_group(tokens, index + 1, top_level_kind.as_deref()) {
                        writer.blank_line();
                        top_level_kind = None;
                        top_level_has_enum = false;
                    }
                }
            } else {
                force_space_after_comment = true;
            }
            index += 1;
            continue;
        }

        if braces.last() == Some(&BraceKind::Enum)
            && parens.is_empty()
            && bracket_depth == 0
            && token.kind == TokenKind::Word
            && enum_member_started
            && !last_significant.is_some_and(|previous| previous.is_symbol("="))
        {
            writer.newline_if_needed();
        }

        if braces.is_empty() && parens.is_empty() && bracket_depth == 0 && top_level_kind.is_none()
        {
            if token.kind == TokenKind::Word {
                top_level_kind = Some(token.text.clone());
            }
        }
        if braces.is_empty() && parens.is_empty() && bracket_depth == 0 && token.is_word("enum") {
            top_level_has_enum = true;
        }

        if token.is_symbol("{") {
            if !writer.line_is_empty() {
                writer.space();
            }
            writer.write("{");
            writer.indent += 1;
            let kind = if top_level_has_enum && braces.is_empty() {
                enum_member_started = false;
                BraceKind::Enum
            } else {
                BraceKind::Block
            };
            braces.push(kind);
            if !has_attached_line_comment(tokens, index + 1) {
                writer.newline();
            }
            last_significant = Some(token);
            last_was_unary = false;
            force_space_after_comment = false;
            index += 1;
            continue;
        }

        if token.is_symbol("}") {
            writer.newline_if_needed();
            writer.indent = writer.indent.saturating_sub(1);
            let closed = braces
                .pop()
                .ok_or_else(|| "unmatched closing brace".to_string())?;
            writer.write("}");
            if closed == BraceKind::Enum {
                enum_member_started = false;
            }
            let next = next_significant(tokens, index + 1);
            if !has_attached_comment(tokens, index + 1) {
                if next.is_some_and(|next| next.is_word("else")) {
                    writer.space();
                } else if !next.is_some_and(|next| next.is_symbol(";") || next.is_symbol(",")) {
                    if braces.is_empty() {
                        writer.blank_line();
                        top_level_kind = None;
                        top_level_has_enum = false;
                    } else {
                        writer.newline();
                    }
                }
            }
            last_significant = Some(token);
            last_was_unary = false;
            force_space_after_comment = false;
            index += 1;
            continue;
        }

        if token.is_symbol("(") {
            if needs_space_before(last_significant, token, false, last_was_unary)
                || force_space_after_comment
            {
                writer.space();
            }
            writer.write("(");
            let for_header = last_significant.is_some_and(|previous| previous.is_word("for"));
            let wrapped = !for_header && should_wrap_parens(tokens, index, writer.column());
            parens.push(ParenContext {
                wrapped,
                for_header,
            });
            if wrapped {
                writer.indent += 1;
                writer.newline();
            }
            last_significant = Some(token);
            last_was_unary = false;
            force_space_after_comment = false;
            index += 1;
            continue;
        }

        if token.is_symbol(")") {
            let context = parens
                .pop()
                .ok_or_else(|| "unmatched closing parenthesis".to_string())?;
            if context.wrapped {
                writer.newline_if_needed();
                writer.indent = writer.indent.saturating_sub(1);
            }
            writer.write(")");
            last_significant = Some(token);
            last_was_unary = false;
            force_space_after_comment = false;
            index += 1;
            continue;
        }

        if token.is_symbol("[") {
            writer.write("[");
            bracket_depth += 1;
            last_significant = Some(token);
            last_was_unary = false;
            force_space_after_comment = false;
            index += 1;
            continue;
        }

        if token.is_symbol("]") {
            if bracket_depth == 0 {
                return Err("unmatched closing bracket".to_string());
            }
            bracket_depth -= 1;
            writer.write("]");
            last_significant = Some(token);
            last_was_unary = false;
            force_space_after_comment = false;
            index += 1;
            continue;
        }

        if token.is_symbol(";") {
            writer.write(";");
            if parens.last().is_some_and(|context| context.for_header) {
                writer.space();
            } else if !has_attached_comment(tokens, index + 1) {
                writer.newline();
                if braces.is_empty() {
                    let next_kind = next_top_level_word(tokens, index + 1);
                    let keep_grouped = matches!(top_level_kind.as_deref(), Some("import"))
                        && next_kind.as_deref() == Some("import");
                    if !keep_grouped {
                        writer.blank_line();
                    }
                    top_level_kind = None;
                    top_level_has_enum = false;
                }
            }
            last_significant = Some(token);
            last_was_unary = false;
            force_space_after_comment = false;
            index += 1;
            continue;
        }

        if token.is_symbol(",") {
            writer.write(",");
            if parens.last().is_some_and(|context| context.wrapped) {
                writer.newline();
            } else {
                writer.space();
            }
            last_significant = Some(token);
            last_was_unary = false;
            force_space_after_comment = false;
            index += 1;
            continue;
        }

        if matches!(token.text.as_str(), "&&" | "||")
            && parens.last().is_some_and(|context| context.wrapped)
            && !writer.line_is_empty()
        {
            writer.newline();
        }

        let unary = is_unary_operator(token, last_significant);
        if needs_space_before(last_significant, token, unary, last_was_unary)
            || force_space_after_comment
        {
            writer.space();
        }
        writer.write(&token.text);

        if braces.last() == Some(&BraceKind::Enum)
            && parens.is_empty()
            && bracket_depth == 0
            && token.kind == TokenKind::Word
        {
            enum_member_started = true;
        }

        last_significant = Some(token);
        last_was_unary = unary;
        force_space_after_comment = false;
        index += 1;
    }

    if !braces.is_empty() {
        return Err("unmatched opening brace".to_string());
    }
    if !parens.is_empty() {
        return Err("unmatched opening parenthesis".to_string());
    }
    if bracket_depth != 0 {
        return Err("unmatched opening bracket".to_string());
    }
    Ok(writer.finish())
}

fn next_significant(tokens: &[Token], start: usize) -> Option<&Token> {
    tokens
        .get(start..)?
        .iter()
        .find(|token| !token.is_comment())
}

fn has_attached_comment(tokens: &[Token], index: usize) -> bool {
    tokens
        .get(index)
        .is_some_and(|token| token.is_comment() && !token.newline_before)
}

fn has_attached_line_comment(tokens: &[Token], index: usize) -> bool {
    tokens
        .get(index)
        .is_some_and(|token| token.kind == TokenKind::LineComment && !token.newline_before)
}

fn next_top_level_word(tokens: &[Token], start: usize) -> Option<String> {
    next_significant(tokens, start)
        .filter(|token| token.kind == TokenKind::Word)
        .map(|token| token.text.clone())
}

fn keeps_import_group(tokens: &[Token], start: usize, top_level_kind: Option<&str>) -> bool {
    top_level_kind == Some("import")
        && next_top_level_word(tokens, start).as_deref() == Some("import")
}

fn is_unary_operator(token: &Token, previous: Option<&Token>) -> bool {
    if token.kind != TokenKind::Symbol || !matches!(token.text.as_str(), "!" | "-" | "+") {
        return false;
    }
    previous.is_none_or(|previous| {
        previous.is_symbol("(")
            || previous.is_symbol("[")
            || previous.is_symbol("{")
            || previous.is_symbol(",")
            || previous.is_symbol(";")
            || is_operator(previous)
            || previous.is_word("return")
    })
}

fn is_operator(token: &Token) -> bool {
    token.kind == TokenKind::Symbol
        && matches!(
            token.text.as_str(),
            "=" | "+="
                | "-="
                | "*="
                | "/="
                | "%="
                | "=="
                | "!="
                | "<"
                | "<="
                | ">"
                | ">="
                | "+"
                | "-"
                | "*"
                | "/"
                | "%"
                | "&&"
                | "||"
                | "|"
                | "&"
                | "->"
        )
}

fn needs_space_before(
    previous: Option<&Token>,
    current: &Token,
    current_is_unary: bool,
    previous_was_unary: bool,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    if current.is_symbol("(") {
        return previous.is_word("if") || previous.is_word("for") || previous.is_word("foreach");
    }
    if matches!(current.text.as_str(), ")" | "]" | "," | ";" | ":" | ".") {
        return false;
    }
    if matches!(previous.text.as_str(), "(" | "[" | "." | "@") {
        return false;
    }
    if current.is_symbol("[") || current.is_symbol("@") {
        return current.is_symbol("@") && previous.kind == TokenKind::Word;
    }
    if current_is_unary {
        return previous.kind == TokenKind::Word
            || previous.kind == TokenKind::Number
            || is_operator(previous);
    }
    if previous_was_unary {
        return false;
    }
    if is_operator(current) || is_operator(previous) {
        return true;
    }
    if previous.is_symbol(":") || previous.is_symbol(",") {
        return true;
    }
    if previous.is_symbol("}") && current.is_word("else") {
        return true;
    }
    if previous.is_symbol(")") && current.kind == TokenKind::Word {
        return true;
    }
    matches!(
        (previous.kind, current.kind),
        (
            TokenKind::Word | TokenKind::Number | TokenKind::String | TokenKind::Backtick,
            TokenKind::Word | TokenKind::Number | TokenKind::String | TokenKind::Backtick
        )
    )
}

fn should_wrap_parens(tokens: &[Token], open_index: usize, current_column: usize) -> bool {
    let Some(close_index) = matching_close(tokens, open_index, "(", ")") else {
        return false;
    };
    if tokens[open_index + 1..close_index]
        .iter()
        .any(|token| token.kind == TokenKind::LineComment || token.text.contains(['\r', '\n']))
    {
        return true;
    }
    let estimated = tokens[open_index + 1..=close_index]
        .iter()
        .map(|token| token.text.chars().count() + 1)
        .sum::<usize>();
    current_column + estimated > LINE_WIDTH
}

fn matching_close(tokens: &[Token], open_index: usize, open: &str, close: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open_index) {
        if token.is_symbol(open) {
            depth += 1;
        } else if token.is_symbol(close) {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{format_source, LINE_WIDTH};
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn formats_declarations_blocks_and_spacing() {
        let source = "struct Player{health :i32;position:Vec2;} enum Screen{Menu Playing} global state :Player; function damage (self:Player,amount:i32):void {if(amount<=0){return;}else{self.health-=amount;}}";
        let expected = "struct Player {\n    health: i32;\n    position: Vec2;\n}\n\nenum Screen {\n    Menu\n    Playing\n}\n\nglobal state: Player;\n\nfunction damage(self: Player, amount: i32): void {\n    if (amount <= 0) {\n        return;\n    } else {\n        self.health -= amount;\n    }\n}\n";
        assert_eq!(format_source(source).expect("format"), expected);
    }

    #[test]
    fn preserves_comments_strings_and_for_header_semicolons() {
        let source = "// header\nfunction main():i32{/* before */let value:i32=1/* plus */+2;for/* header */(let i:i32=0/* ; */;i<2;i+=1){value+=i;}print_string(\"{;}//\");return value;// tail\n}\n";
        let formatted = format_source(source).expect("format");
        assert!(formatted.contains("// header"));
        assert!(formatted.contains("/* before */"));
        assert!(formatted.contains("/* ; */"));
        assert!(formatted.contains("for /* header */ (let i: i32 = 0 /* ; */; i < 2; i += 1) {"));
        assert!(formatted.contains("print_string(\"{;}//\");"));
        assert!(formatted.contains("return value; // tail"));
        assert_eq!(format_source(&formatted).expect("reformat"), formatted);
    }

    #[test]
    fn wraps_long_parameter_lists_at_the_soft_limit() {
        let names = (0..18)
            .map(|index| format!("parameter_{index}: i32"))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!("function calculate({names}):i32{{return parameter_0;}}");
        let formatted = format_source(&source).expect("format");
        assert!(formatted.starts_with("function calculate(\n"));
        assert!(formatted.contains("    parameter_0: i32,\n"));
        assert!(formatted.contains("    parameter_17: i32\n): i32 {"));
        assert!(formatted
            .lines()
            .all(|line| line.chars().count() <= LINE_WIDTH));
        assert_eq!(format_source(&formatted).expect("reformat"), formatted);
    }

    #[test]
    fn normalizes_tabs_blank_lines_and_line_endings() {
        let source = "function main(): i32 {\r\n\treturn 0;   \r\n\r\n\r\n}\r\n\r\n";
        let expected = "function main(): i32 {\n    return 0;\n}\n";
        assert_eq!(format_source(source).expect("format"), expected);
    }

    #[test]
    fn formats_annotations_unary_values_and_attached_comments() {
        let source = "function @extern(\"native_tick\")tick(value:i32):i32; enum Phase{Menu// initial\nPlaying} function main():i32{/* first\n * second\n */let value:i32=2*-3;return value;} // entry\n";
        let expected = "function @extern(\"native_tick\") tick(value: i32): i32;\n\nenum Phase {\n    Menu // initial\n    Playing\n}\n\nfunction main(): i32 {\n    /* first\n * second\n */\n    let value: i32 = 2 * -3;\n    return value;\n} // entry\n";
        assert_eq!(format_source(source).expect("format"), expected);
        assert_eq!(format_source(expected).expect("reformat"), expected);
    }

    #[test]
    fn keeps_commented_imports_in_one_group() {
        let source = "import\"a.stasis\";// first\nimport \"b.stasis\";/* second */\nfunction main():i32{return 0;}";
        let expected = "import \"a.stasis\"; // first\nimport \"b.stasis\"; /* second */\n\nfunction main(): i32 {\n    return 0;\n}\n";
        assert_eq!(format_source(source).expect("format"), expected);
        assert_eq!(format_source(expected).expect("reformat"), expected);
    }

    #[test]
    fn formats_repository_stasis_corpus_idempotently() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        for relative in [
            "src",
            "samples",
            "tests/stasis",
            "tools/fixtures",
            "mobile/android/app/src/main/assets",
        ] {
            collect_stasis_files(&repository.join(relative), &mut files);
        }
        assert!(!files.is_empty(), "expected repository Stasis fixtures");
        for path in files {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
            let formatted = format_source(&source)
                .unwrap_or_else(|error| panic!("failed to format {}: {error}", path.display()));
            assert_eq!(
                format_source(&formatted).expect("reformat corpus file"),
                formatted,
                "formatter was not idempotent for {}",
                path.display()
            );
        }
    }

    fn collect_stasis_files(root: &Path, files: &mut Vec<PathBuf>) {
        if !root.is_dir() {
            return;
        }
        for entry in fs::read_dir(root).expect("read corpus directory") {
            let entry = entry.expect("read corpus entry");
            let file_type = entry.file_type().expect("read corpus file type");
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                collect_stasis_files(&path, files);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "stasis")
            {
                files.push(path);
            }
        }
    }
}
