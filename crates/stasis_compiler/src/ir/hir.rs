use crate::backend::emit::SimpleStmt;

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionHIR {
    pub blocks: Vec<Block>,
    pub(crate) statements: Vec<SimpleStmt>,
    pub(crate) debug_statements: Vec<DebugStatement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DebugStatement {
    pub(crate) source_offset: u32,
    pub(crate) children: Vec<DebugStatement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub source: String,
}
