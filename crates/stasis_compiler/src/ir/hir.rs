#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionHIR {
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub source: String,
}
