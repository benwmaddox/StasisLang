pub mod aot;
pub mod jit;
mod reachability;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineEntrypoints {
    pub tick: String,
    pub render: String,
    pub on_code_swap: Option<String>,
}

impl EngineEntrypoints {
    pub fn runtime_default() -> Self {
        Self {
            tick: "tick".to_string(),
            render: "render".to_string(),
            on_code_swap: Some("on_code_swap".to_string()),
        }
    }
}

impl Default for EngineEntrypoints {
    fn default() -> Self {
        Self::runtime_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AotOptimizationProfile {
    None,
    Speed,
    SpeedAndSize,
}

impl AotOptimizationProfile {
    pub fn as_cranelift_opt_level(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Speed => "speed",
            Self::SpeedAndSize => "speed_and_size",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Speed => "speed",
            Self::SpeedAndSize => "speed_and_size",
        }
    }
}

impl Default for AotOptimizationProfile {
    fn default() -> Self {
        Self::Speed
    }
}

use crate::ir::hir::FunctionHIR;

pub(crate) fn eval_simple_i32_return_expression(hir: &FunctionHIR) -> Result<i64, String> {
    let expression = extract_return_expression(hir)?;
    let tokens = tokenize_expression(&expression)?;
    if tokens.len() == 1 {
        if let ExprToken::Value(value) = tokens[0] {
            return Ok(value);
        }
    }
    if tokens.len() == 3 {
        if let (ExprToken::Value(lhs), ExprToken::Op(op), ExprToken::Value(rhs)) =
            (tokens[0], tokens[1], tokens[2])
        {
            return match op {
                '+' => Ok(lhs + rhs),
                '-' => Ok(lhs - rhs),
                '*' => Ok(lhs * rhs),
                '/' => {
                    if rhs == 0 {
                        Err("division by zero in return expression".to_string())
                    } else {
                        Ok(lhs / rhs)
                    }
                }
                '%' => {
                    if rhs == 0 {
                        Err("modulo by zero in return expression".to_string())
                    } else {
                        Ok(lhs % rhs)
                    }
                }
                _ => Err(format!("unsupported operator '{op}' in return expression")),
            };
        }
    }
    Err(format!(
        "unsupported return expression '{expression}' (supported: literal or literal op literal)"
    ))
}

fn extract_return_expression(hir: &FunctionHIR) -> Result<String, String> {
    let Some(block) = hir.blocks.first() else {
        return Err("function body missing block text".to_string());
    };
    let source = block.source.as_str();
    let return_index = source.find("return").ok_or_else(|| {
        "expected i32 return expression but no return statement found".to_string()
    })?;
    let tail = &source[return_index + "return".len()..];
    let semicolon_index = tail
        .find(';')
        .ok_or_else(|| "expected i32 return expression ending in ';'".to_string())?;
    let expression = tail[..semicolon_index].trim();
    if expression.is_empty() {
        return Err("expected i32 return expression but expression was empty".to_string());
    }
    Ok(expression.to_string())
}

fn tokenize_expression(expression: &str) -> Result<Vec<ExprToken>, String> {
    let bytes = expression.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    let mut expect_value = true;

    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }

        if expect_value {
            let mut sign: i64 = 1;
            if bytes[index] == b'-' {
                sign = -1;
                index += 1;
            } else if bytes[index] == b'+' {
                index += 1;
            }
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            if index == start {
                return Err(format!(
                    "expected integer literal in return expression near '{}'",
                    &expression[start.min(expression.len())..]
                ));
            }
            let literal_text = &expression[start..index];
            let value = literal_text.parse::<i64>().map_err(|error| {
                format!("invalid integer literal '{literal_text}' in return expression: {error}")
            })?;
            tokens.push(ExprToken::Value(sign * value));
            expect_value = false;
            continue;
        }

        let op = bytes[index] as char;
        if !matches!(op, '+' | '-' | '*' | '/' | '%') {
            return Err(format!("unexpected operator '{op}' in return expression"));
        }
        tokens.push(ExprToken::Op(op));
        index += 1;
        expect_value = true;
    }

    if expect_value {
        return Err(format!(
            "incomplete return expression '{expression}', expected trailing literal"
        ));
    }

    Ok(tokens)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExprToken {
    Value(i64),
    Op(char),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::hir::{Block, FunctionHIR};

    fn hir(body: &str) -> FunctionHIR {
        FunctionHIR {
            blocks: vec![Block {
                source: body.to_string(),
            }],
        }
    }

    #[test]
    fn eval_simple_i32_return_expression_supports_literal_and_binary() {
        assert_eq!(
            eval_simple_i32_return_expression(&hir("{ return 9; }")).expect("literal"),
            9
        );
        assert_eq!(
            eval_simple_i32_return_expression(&hir("{ return 2+3; }")).expect("binary"),
            5
        );
        assert_eq!(
            eval_simple_i32_return_expression(&hir("{ return -4 * 3; }")).expect("binary"),
            -12
        );
    }

    #[test]
    fn eval_simple_i32_return_expression_rejects_unsupported_shapes() {
        let error =
            eval_simple_i32_return_expression(&hir("{ return lhs + rhs; }")).expect_err("error");
        assert!(
            error.contains("expected integer literal"),
            "unexpected message: {error}"
        );
    }
}
