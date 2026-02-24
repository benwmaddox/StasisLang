pub mod aot;
pub mod jit;

use crate::compiler::FunctionMeta;
use crate::ir::hir::FunctionHIR;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendFunctionData {
    None,
    Jit(JitFunction),
    Aot(AotFunction),
}

impl Default for BackendFunctionData {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitFunction {
    pub slot: u32,
    pub body_hash: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AotFunction {
    pub object_index: u32,
    pub body_hash: u64,
}

pub trait Backend {
    fn compile_function(
        &mut self,
        meta: &FunctionMeta,
        hir: &FunctionHIR,
    ) -> Result<BackendFunctionData, String>;

    fn finalize(&mut self) -> Result<(), String>;
}
