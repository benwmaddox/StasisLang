use crate::backend::emit::SimpleStmt;

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionHIR {
    pub blocks: Vec<Block>,
    pub(crate) statements: Vec<SimpleStmt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub source: String,
}
