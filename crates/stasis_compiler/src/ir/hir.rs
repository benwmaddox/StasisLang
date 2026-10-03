use crate::frontend::types::TypeId;

/// Backend-independent, parsed function body consumed by analysis and every code generator.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionHIR {
    pub(crate) statements: Vec<SimpleStmt>,
    pub(crate) debug_statements: Vec<DebugStatement>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SimpleStmt {
    Noop,
    Let {
        name: String,
        type_id: Option<TypeId>,
        expression: SimpleExpr,
    },
    Assign {
        target: AssignTarget,
        op: AssignOp,
        expression: SimpleExpr,
    },
    Convert {
        target: AssignTarget,
        kind: ConversionKind,
        source: SimpleExpr,
    },
    If {
        condition: SimpleCondition,
        then_statements: Vec<SimpleStmt>,
        else_statements: Option<Vec<SimpleStmt>>,
    },
    For {
        init: Box<SimpleStmt>,
        condition: SimpleCondition,
        step: Box<SimpleStmt>,
        body_statements: Vec<SimpleStmt>,
    },
    Foreach {
        item_name: String,
        index_name: Option<String>,
        collection_path: String,
        body_statements: Vec<SimpleStmt>,
    },
    Expr(SimpleExpr),
    Continue,
    Return(SimpleExpr),
    ReturnVoid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AssignTarget {
    Local(String),
    GlobalPath(String),
    IndexedPath {
        collection_path: String,
        index: SimpleExpr,
        suffix: String,
        nested_index: Option<SimpleExpr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConversionKind {
    FromI32,
    FromF32,
    FromF64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SimpleCondition {
    Comparison {
        lhs: SimpleExpr,
        op: ComparisonOp,
        rhs: SimpleExpr,
    },
    Expr(SimpleExpr),
    And(Box<SimpleCondition>, Box<SimpleCondition>),
    Or(Box<SimpleCondition>, Box<SimpleCondition>),
    Not(Box<SimpleCondition>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComparisonOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ExprBinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    ShiftLeft,
    ShiftRight,
    BitAnd,
    BitXor,
    BitOr,
}

impl ExprBinaryOp {
    pub(crate) const fn spelling(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::Remainder => "%",
            Self::ShiftLeft => "<<",
            Self::ShiftRight => ">>",
            Self::BitAnd => "&",
            Self::BitXor => "^",
            Self::BitOr => "|",
        }
    }

    /// Larger values bind more tightly in the Pratt expression parser.
    pub(crate) const fn precedence(self) -> u8 {
        match self {
            Self::Multiply | Self::Divide | Self::Remainder => 100,
            Self::Add | Self::Subtract => 90,
            Self::ShiftLeft | Self::ShiftRight => 80,
            Self::BitAnd => 60,
            Self::BitXor => 50,
            Self::BitOr => 40,
        }
    }

    pub(crate) const fn requires_integer_operands(self) -> bool {
        matches!(
            self,
            Self::ShiftLeft | Self::ShiftRight | Self::BitAnd | Self::BitXor | Self::BitOr
        )
    }

    pub(crate) const fn is_shift(self) -> bool {
        matches!(self, Self::ShiftLeft | Self::ShiftRight)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ExprUnaryOp {
    BitwiseNot,
}

impl ExprUnaryOp {
    pub(crate) const fn spelling(self) -> &'static str {
        match self {
            Self::BitwiseNot => "~",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SimpleExpr {
    DefaultValue(TypeId),
    Int(i64),
    Float(f64),
    Bool(bool),
    StringLiteral(String),
    Condition(Box<SimpleCondition>),
    Identifier(String),
    IndexedPath {
        collection_path: String,
        index: Box<SimpleExpr>,
        suffix: String,
        nested_index: Option<Box<SimpleExpr>>,
    },
    Call {
        target: String,
        args: Vec<SimpleExpr>,
    },
    Unary {
        op: ExprUnaryOp,
        operand: Box<SimpleExpr>,
    },
    Binary {
        lhs: Box<SimpleExpr>,
        op: ExprBinaryOp,
        rhs: Box<SimpleExpr>,
    },
}

impl SimpleExpr {
    /// Returns the signed value of an unsuffixed integer literal, including
    /// the parser's `0 - literal` representation for unary negation.
    pub(crate) fn contextual_integer_literal_value(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            Self::Binary {
                lhs,
                op: ExprBinaryOp::Subtract,
                rhs,
            } if matches!(lhs.as_ref(), Self::Int(0)) => {
                rhs.contextual_integer_literal_value()?.checked_neg()
            }
            _ => None,
        }
    }

    /// Whether an expression is made only from integer literals and bitwise
    /// operators, so an enclosing exact integer type can provide its lane.
    /// Shift counts are intentionally independent of the result lane.
    pub(crate) fn is_contextual_integer_expression(&self) -> bool {
        if self.contextual_integer_literal_value().is_some() {
            return true;
        }
        match self {
            Self::Unary {
                op: ExprUnaryOp::BitwiseNot,
                operand,
            } => operand.is_contextual_integer_expression(),
            Self::Binary {
                lhs,
                op: ExprBinaryOp::BitAnd | ExprBinaryOp::BitXor | ExprBinaryOp::BitOr,
                rhs,
            } => lhs.is_contextual_integer_expression() && rhs.is_contextual_integer_expression(),
            Self::Binary {
                lhs,
                op: ExprBinaryOp::ShiftLeft | ExprBinaryOp::ShiftRight,
                ..
            } => lhs.is_contextual_integer_expression(),
            _ => false,
        }
    }
}

pub(crate) fn eval_const_i64(expression: &SimpleExpr) -> Option<i64> {
    match expression {
        SimpleExpr::Int(value) => Some(*value),
        SimpleExpr::Unary {
            op: ExprUnaryOp::BitwiseNot,
            operand,
        } => Some(i64::from(!(eval_const_i64(operand)? as i32))),
        SimpleExpr::Binary { lhs, op, rhs } => {
            let lhs = eval_const_i64(lhs)?;
            let rhs = eval_const_i64(rhs)?;
            match op {
                ExprBinaryOp::Add => lhs.checked_add(rhs),
                ExprBinaryOp::Subtract => lhs.checked_sub(rhs),
                ExprBinaryOp::Multiply => lhs.checked_mul(rhs),
                ExprBinaryOp::Divide if rhs != 0 => lhs.checked_div(rhs),
                ExprBinaryOp::Remainder if rhs != 0 => lhs.checked_rem(rhs),
                ExprBinaryOp::Divide | ExprBinaryOp::Remainder => None,
                ExprBinaryOp::BitAnd => Some(i64::from((lhs as i32) & (rhs as i32))),
                ExprBinaryOp::BitXor => Some(i64::from((lhs as i32) ^ (rhs as i32))),
                ExprBinaryOp::BitOr => Some(i64::from((lhs as i32) | (rhs as i32))),
                ExprBinaryOp::ShiftLeft => {
                    Some(i64::from((lhs as i32).wrapping_shl((rhs as u32) & 31)))
                }
                ExprBinaryOp::ShiftRight => {
                    Some(i64::from((lhs as i32).wrapping_shr((rhs as u32) & 31)))
                }
            }
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedSimpleStatements {
    pub(crate) statements: Vec<SimpleStmt>,
    pub(crate) debug_statements: Vec<DebugStatement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DebugStatement {
    pub(crate) source_offset: u32,
    pub(crate) children: Vec<DebugStatement>,
}
