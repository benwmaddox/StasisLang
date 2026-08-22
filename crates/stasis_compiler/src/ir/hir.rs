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
    },
    Call {
        target: String,
        args: Vec<SimpleExpr>,
    },
    Binary {
        lhs: Box<SimpleExpr>,
        op: char,
        rhs: Box<SimpleExpr>,
    },
}

pub(crate) fn eval_const_i64(expression: &SimpleExpr) -> Option<i64> {
    match expression {
        SimpleExpr::Int(value) => Some(*value),
        SimpleExpr::Binary { lhs, op, rhs } => {
            let lhs = eval_const_i64(lhs)?;
            let rhs = eval_const_i64(rhs)?;
            match *op {
                '+' => lhs.checked_add(rhs),
                '-' => lhs.checked_sub(rhs),
                '*' => lhs.checked_mul(rhs),
                '/' if rhs != 0 => lhs.checked_div(rhs),
                '%' if rhs != 0 => lhs.checked_rem(rhs),
                '/' | '%' => None,
                _ => None,
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
