use crate::backend::{Backend, BackendFunctionData, JitFunction};
use crate::compiler::FunctionMeta;
use crate::ir::hir::FunctionHIR;

#[derive(Debug, Default)]
pub struct JitBackend {
    next_slot: u32,
}

impl JitBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Backend for JitBackend {
    fn compile_function(
        &mut self,
        meta: &FunctionMeta,
        _hir: &FunctionHIR,
    ) -> Result<BackendFunctionData, String> {
        let slot = self.next_slot;
        self.next_slot = self.next_slot.saturating_add(1);
        Ok(BackendFunctionData::Jit(JitFunction {
            slot,
            body_hash: meta.body_hash,
        }))
    }

    fn finalize(&mut self) -> Result<(), String> {
        Ok(())
    }
}
