use crate::backend::{AotFunction, Backend, BackendFunctionData};
use crate::compiler::FunctionMeta;
use crate::ir::hir::FunctionHIR;

#[derive(Debug, Default)]
pub struct AotBackend {
    next_object_index: u32,
}

impl AotBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Backend for AotBackend {
    fn compile_function(
        &mut self,
        meta: &FunctionMeta,
        _hir: &FunctionHIR,
    ) -> Result<BackendFunctionData, String> {
        let object_index = self.next_object_index;
        self.next_object_index = self.next_object_index.saturating_add(1);
        Ok(BackendFunctionData::Aot(AotFunction {
            object_index,
            body_hash: meta.body_hash,
        }))
    }

    fn finalize(&mut self) -> Result<(), String> {
        Ok(())
    }
}
