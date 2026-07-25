#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StateQuery {
    Scalar(ScalarExpression),
    Predicate(PredicateQuery),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PredicateQuery {
    pub(crate) path: String,
    pub(crate) field: String,
    pub(crate) operator: BinaryOperator,
    pub(crate) right: ScalarExpression,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ScalarExpression {
    Value(StateValueReference),
    Negate(Box<ScalarExpression>),
    Binary {
        left: Box<ScalarExpression>,
        operator: BinaryOperator,
        right: Box<ScalarExpression>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StateValueReference {
    Path(String),
    CollectionItem {
        path: String,
        index: i32,
        field: String,
    },
    I32(i32),
    F64(f64),
    Bool(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl BinaryOperator {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::Remainder => "%",
            Self::Equal => "==",
            Self::NotEqual => "!=",
            Self::Less => "<",
            Self::LessEqual => "<=",
            Self::Greater => ">",
            Self::GreaterEqual => ">=",
        }
    }

    fn precedence(self) -> u8 {
        match self {
            Self::Equal
            | Self::NotEqual
            | Self::Less
            | Self::LessEqual
            | Self::Greater
            | Self::GreaterEqual => 1,
            Self::Add | Self::Subtract => 2,
            Self::Multiply | Self::Divide | Self::Remainder => 3,
        }
    }
}

pub(crate) fn parse_state_query(source: &str) -> Result<StateQuery, String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("state query cannot be empty".to_string());
    }
    if let Some((path, predicate)) = source.split_once("[?") {
        if !source.ends_with(']') {
            return Err("state predicate query is missing closing ']'".to_string());
        }
        validate_path(path, "predicate collection path")?;
        let predicate = &predicate[..predicate.len() - 1];
        let (operator_offset, operator) = find_predicate_operator(predicate)
            .ok_or_else(|| "state predicate requires ==, !=, <, <=, >, or >=".to_string())?;
        let field = predicate[..operator_offset].trim();
        validate_path(field, "predicate field")?;
        let right_source = predicate[operator_offset + operator.text().len()..].trim();
        if right_source.is_empty() {
            return Err("state predicate is missing its right-hand expression".to_string());
        }
        let mut parser = ExpressionParser::new(right_source)?;
        let right = parser.parse()?;
        return Ok(StateQuery::Predicate(PredicateQuery {
            path: path.trim().to_string(),
            field: field.to_string(),
            operator,
            right,
        }));
    }
    let mut parser = ExpressionParser::new(source)?;
    Ok(StateQuery::Scalar(parser.parse()?))
}

fn find_predicate_operator(source: &str) -> Option<(usize, BinaryOperator)> {
    let bytes = source.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let remaining = &source[cursor..];
        for (text, operator) in [
            ("==", BinaryOperator::Equal),
            ("!=", BinaryOperator::NotEqual),
            ("<=", BinaryOperator::LessEqual),
            (">=", BinaryOperator::GreaterEqual),
            ("<", BinaryOperator::Less),
            (">", BinaryOperator::Greater),
        ] {
            if remaining.starts_with(text) {
                return Some((cursor, operator));
            }
        }
        cursor += source[cursor..].chars().next()?.len_utf8();
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
enum QueryToken {
    Reference(String),
    I32(i32),
    F64(f64),
    Bool(bool),
    Operator(BinaryOperator),
    Minus,
    LeftParen,
    RightParen,
}

struct ExpressionParser {
    tokens: Vec<QueryToken>,
    cursor: usize,
}

impl ExpressionParser {
    fn new(source: &str) -> Result<Self, String> {
        Ok(Self {
            tokens: tokenize_expression(source)?,
            cursor: 0,
        })
    }

    fn parse(&mut self) -> Result<ScalarExpression, String> {
        let expression = self.parse_precedence(1)?;
        if self.cursor != self.tokens.len() {
            return Err(format!(
                "unexpected token in state expression: {:?}",
                self.tokens[self.cursor]
            ));
        }
        Ok(expression)
    }

    fn parse_precedence(&mut self, minimum: u8) -> Result<ScalarExpression, String> {
        let mut left = self.parse_prefix()?;
        loop {
            let Some(operator) = self.peek_operator() else {
                break;
            };
            let precedence = operator.precedence();
            if precedence < minimum {
                break;
            }
            self.cursor += 1;
            let right = self.parse_precedence(precedence + 1)?;
            left = ScalarExpression::Binary {
                left: Box::new(left),
                operator,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_prefix(&mut self) -> Result<ScalarExpression, String> {
        let token = self
            .tokens
            .get(self.cursor)
            .cloned()
            .ok_or_else(|| "state expression ended before a value".to_string())?;
        self.cursor += 1;
        match token {
            QueryToken::Minus => Ok(ScalarExpression::Negate(Box::new(self.parse_prefix()?))),
            QueryToken::LeftParen => {
                let expression = self.parse_precedence(1)?;
                if self.tokens.get(self.cursor) != Some(&QueryToken::RightParen) {
                    return Err("state expression is missing closing ')'".to_string());
                }
                self.cursor += 1;
                Ok(expression)
            }
            QueryToken::Reference(reference) => {
                Ok(ScalarExpression::Value(parse_value_reference(&reference)?))
            }
            QueryToken::I32(value) => Ok(ScalarExpression::Value(StateValueReference::I32(value))),
            QueryToken::F64(value) => Ok(ScalarExpression::Value(StateValueReference::F64(value))),
            QueryToken::Bool(value) => {
                Ok(ScalarExpression::Value(StateValueReference::Bool(value)))
            }
            QueryToken::Operator(_) | QueryToken::RightParen => {
                Err("state expression expected a scalar value".to_string())
            }
        }
    }

    fn peek_operator(&self) -> Option<BinaryOperator> {
        match self.tokens.get(self.cursor) {
            Some(QueryToken::Operator(operator)) => Some(*operator),
            Some(QueryToken::Minus) => Some(BinaryOperator::Subtract),
            _ => None,
        }
    }
}

fn tokenize_expression(source: &str) -> Result<Vec<QueryToken>, String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
            continue;
        }
        let remaining = &source[cursor..];
        let matched_operator = [
            ("==", BinaryOperator::Equal),
            ("!=", BinaryOperator::NotEqual),
            ("<=", BinaryOperator::LessEqual),
            (">=", BinaryOperator::GreaterEqual),
            ("+", BinaryOperator::Add),
            ("*", BinaryOperator::Multiply),
            ("/", BinaryOperator::Divide),
            ("%", BinaryOperator::Remainder),
            ("<", BinaryOperator::Less),
            (">", BinaryOperator::Greater),
        ]
        .into_iter()
        .find(|(text, _)| remaining.starts_with(text));
        if let Some((text, operator)) = matched_operator {
            tokens.push(QueryToken::Operator(operator));
            cursor += text.len();
            continue;
        }
        match bytes[cursor] {
            b'-' => {
                tokens.push(QueryToken::Minus);
                cursor += 1;
            }
            b'(' => {
                tokens.push(QueryToken::LeftParen);
                cursor += 1;
            }
            b')' => {
                tokens.push(QueryToken::RightParen);
                cursor += 1;
            }
            byte if byte.is_ascii_digit() => {
                let start = cursor;
                cursor += 1;
                let mut decimal = false;
                while cursor < bytes.len()
                    && (bytes[cursor].is_ascii_digit() || (!decimal && bytes[cursor] == b'.'))
                {
                    decimal |= bytes[cursor] == b'.';
                    cursor += 1;
                }
                let number = &source[start..cursor];
                if decimal {
                    tokens.push(QueryToken::F64(number.parse::<f64>().map_err(|error| {
                        format!("invalid f64 literal '{number}' in state query: {error}")
                    })?));
                } else {
                    tokens.push(QueryToken::I32(number.parse::<i32>().map_err(|error| {
                        format!("invalid i32 literal '{number}' in state query: {error}")
                    })?));
                }
            }
            byte if byte == b'_' || byte.is_ascii_alphabetic() => {
                let start = cursor;
                cursor += 1;
                let mut bracket_depth = 0u8;
                while cursor < bytes.len() {
                    let byte = bytes[cursor];
                    if byte == b'[' {
                        bracket_depth = bracket_depth.saturating_add(1);
                    } else if byte == b']' {
                        bracket_depth = bracket_depth.saturating_sub(1);
                    } else if bracket_depth == 0
                        && !(byte == b'_' || byte == b'.' || byte.is_ascii_alphanumeric())
                    {
                        break;
                    }
                    cursor += 1;
                }
                let reference = &source[start..cursor];
                match reference {
                    "true" => tokens.push(QueryToken::Bool(true)),
                    "false" => tokens.push(QueryToken::Bool(false)),
                    _ => tokens.push(QueryToken::Reference(reference.to_string())),
                }
            }
            _ => {
                return Err(format!(
                    "unsupported character '{}' at byte {cursor} in state query",
                    source[cursor..].chars().next().unwrap_or('?')
                ))
            }
        }
    }
    if tokens.is_empty() {
        return Err("state expression cannot be empty".to_string());
    }
    Ok(tokens)
}

fn parse_value_reference(reference: &str) -> Result<StateValueReference, String> {
    let Some(open) = reference.find('[') else {
        validate_path(reference, "state path")?;
        return Ok(StateValueReference::Path(reference.to_string()));
    };
    let close = reference[open + 1..]
        .find(']')
        .map(|offset| open + 1 + offset)
        .ok_or_else(|| format!("indexed state path '{reference}' is missing closing ']'"))?;
    let path = &reference[..open];
    validate_path(path, "collection path")?;
    let index_text = &reference[open + 1..close];
    let index = index_text
        .parse::<i32>()
        .map_err(|error| format!("invalid collection index '{index_text}': {error}"))?;
    if index < 0 {
        return Err(format!("collection index {index} cannot be negative"));
    }
    let field = reference[close + 1..]
        .strip_prefix('.')
        .unwrap_or(&reference[close + 1..]);
    if close + 1 < reference.len() && field.is_empty() {
        return Err(format!(
            "indexed state path '{reference}' is missing its field"
        ));
    }
    if !field.is_empty() {
        validate_path(field, "collection field")?;
    }
    Ok(StateValueReference::CollectionItem {
        path: path.to_string(),
        index,
        field: field.to_string(),
    })
}

fn validate_path(path: &str, label: &str) -> Result<(), String> {
    let path = path.trim();
    if path.is_empty()
        || path.split('.').any(|segment| {
            segment.is_empty()
                || !segment.bytes().enumerate().all(|(index, byte)| {
                    byte == b'_'
                        || byte.is_ascii_alphabetic()
                        || (index > 0 && byte.is_ascii_digit())
                })
        })
    {
        return Err(format!("invalid {label} '{path}'"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        parse_state_query, BinaryOperator, ScalarExpression, StateQuery, StateValueReference,
    };

    #[test]
    fn parses_indexed_precedence_and_predicate_queries() {
        let scalar =
            parse_state_query("state.score + enemies[2].hp * 2").expect("parse scalar expression");
        let StateQuery::Scalar(ScalarExpression::Binary {
            operator, right, ..
        }) = scalar
        else {
            panic!("expected binary scalar query");
        };
        assert_eq!(operator, BinaryOperator::Add);
        assert!(matches!(
            *right,
            ScalarExpression::Binary {
                operator: BinaryOperator::Multiply,
                ..
            }
        ));

        let predicate =
            parse_state_query("enemies[?hp >= state.minimum_hp]").expect("parse predicate query");
        let StateQuery::Predicate(predicate) = predicate else {
            panic!("expected predicate query");
        };
        assert_eq!(predicate.path, "enemies");
        assert_eq!(predicate.field, "hp");
        assert_eq!(predicate.operator, BinaryOperator::GreaterEqual);
        assert!(
            matches!(predicate.right, ScalarExpression::Value(StateValueReference::Path(path)) if path == "state.minimum_hp")
        );
    }

    #[test]
    fn rejects_ambiguous_or_unbounded_query_syntax() {
        assert!(parse_state_query("").is_err());
        assert!(parse_state_query("enemies[?hp]").is_err());
        assert!(parse_state_query("enemies[-1].hp").is_err());
        assert!(parse_state_query("score + system()").is_err());
    }
}
