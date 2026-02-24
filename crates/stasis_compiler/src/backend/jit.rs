use crate::backend::eval_simple_i32_return_expression;
use crate::backend::EngineEntrypoints;
use crate::compiler::{CompileReport, CompileResult, Compiler, FunctionId, FunctionMeta};
use crate::frontend::types::{TYPE_ID_I32, TYPE_ID_VOID};
use crate::ir::hir::FunctionHIR;
use cranelift_codegen::ir::{types, AbiParam, InstBuilder};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, Linkage, Module};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitArtifact {
    pub function_id: FunctionId,
    pub slot: u32,
    pub body_hash: u64,
    pub code_ptr: u64,
}

pub struct JitProcess {
    compiler: Compiler,
    next_slot: u32,
    next_symbol_seq: u64,
    artifacts: Vec<JitArtifact>,
    modules: Vec<JITModule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitEnginePackage {
    pub tick_code_ptr: u64,
    pub render_code_ptr: u64,
    pub on_code_swap_code_ptr: Option<u64>,
    pub symbol_code_ptrs: BTreeMap<String, u64>,
}

impl JitProcess {
    pub fn new() -> Self {
        Self {
            compiler: Compiler::new(),
            next_slot: 0,
            next_symbol_seq: 0,
            artifacts: Vec::new(),
            modules: Vec::new(),
        }
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.compiler.upsert_file(path, content);
    }

    pub fn compile(&mut self) -> CompileResult<CompileReport> {
        let (compiler, next_slot, next_symbol_seq, artifacts, modules) = (
            &mut self.compiler,
            &mut self.next_slot,
            &mut self.next_symbol_seq,
            &mut self.artifacts,
            &mut self.modules,
        );
        compiler.compile_with(|meta, hir| {
            let symbol = format!("jit_fn_{}_{}", meta.id, *next_symbol_seq);
            *next_symbol_seq = next_symbol_seq.saturating_add(1);
            let (module, code_ptr) = compile_function_to_jit_module(meta, hir, &symbol)?;
            let slot = *next_slot;
            *next_slot = next_slot.saturating_add(1);
            modules.push(module);
            artifacts.retain(|artifact| artifact.function_id != meta.id);
            artifacts.push(JitArtifact {
                function_id: meta.id,
                slot,
                body_hash: meta.body_hash,
                code_ptr,
            });
            Ok(())
        })
    }

    pub fn artifacts(&self) -> &[JitArtifact] {
        &self.artifacts
    }

    pub fn execute_i32_noarg_by_name(&self, name: &str) -> Result<i32, String> {
        let function = self
            .compiler
            .functions()
            .iter()
            .find(|function| function.name == name)
            .ok_or_else(|| format!("function '{name}' not found"))?;
        if function.return_type != TYPE_ID_I32 {
            return Err(format!(
                "function '{name}' is not i32-returning (type id {})",
                function.return_type
            ));
        }
        let artifact = self
            .artifacts
            .iter()
            .find(|artifact| artifact.function_id == function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;
        let raw = stasis_dynload::invoke_noarg_u64(artifact.code_ptr as usize)?;
        Ok((raw as u32) as i32)
    }

    pub fn build_engine_package(
        &self,
        entrypoints: &EngineEntrypoints,
    ) -> Result<JitEnginePackage, String> {
        let tick_code_ptr = self.code_ptr_for_function_name(&entrypoints.tick)?;
        let render_code_ptr = self.code_ptr_for_function_name(&entrypoints.render)?;
        let on_code_swap_code_ptr = if let Some(name) = entrypoints.on_code_swap.as_ref() {
            Some(self.code_ptr_for_function_name(name)?)
        } else {
            None
        };

        let mut symbol_code_ptrs = BTreeMap::new();
        for function in self.compiler.functions() {
            if let Some(artifact) = self
                .artifacts
                .iter()
                .find(|artifact| artifact.function_id == function.id)
            {
                symbol_code_ptrs.insert(function.name.clone(), artifact.code_ptr);
            }
        }

        Ok(JitEnginePackage {
            tick_code_ptr,
            render_code_ptr,
            on_code_swap_code_ptr,
            symbol_code_ptrs,
        })
    }

    fn code_ptr_for_function_name(&self, name: &str) -> Result<u64, String> {
        let function = self
            .compiler
            .functions()
            .iter()
            .find(|function| function.name == name)
            .ok_or_else(|| format!("required engine entrypoint '{name}' not found"))?;
        let artifact = self
            .artifacts
            .iter()
            .find(|artifact| artifact.function_id == function.id)
            .ok_or_else(|| format!("compiled artifact missing for required entrypoint '{name}'"))?;
        Ok(artifact.code_ptr)
    }
}

impl Default for JitProcess {
    fn default() -> Self {
        Self::new()
    }
}

fn compile_function_to_jit_module(
    meta: &FunctionMeta,
    hir: &FunctionHIR,
    symbol: &str,
) -> Result<(JITModule, u64), String> {
    let builder = JITBuilder::new(default_libcall_names())
        .map_err(|error| format!("failed to construct JIT builder: {error}"))?;
    let mut module = JITModule::new(builder);
    let mut context = module.make_context();
    context.func.signature = module.make_signature();
    match meta.return_type {
        TYPE_ID_VOID => {}
        TYPE_ID_I32 => context
            .func
            .signature
            .returns
            .push(AbiParam::new(types::I32)),
        other => {
            return Err(format!(
                "unsupported JIT return type id {other} for function {}",
                meta.name
            ));
        }
    }

    let function_id = module
        .declare_function(symbol, Linkage::Export, &context.func.signature)
        .map_err(|error| format!("failed to declare JIT function {symbol}: {error}"))?;

    let mut function_builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut function_builder_context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        if meta.return_type == TYPE_ID_I32 {
            let value = eval_simple_i32_return_expression(hir)?;
            let literal = builder.ins().iconst(types::I32, value);
            builder.ins().return_(&[literal]);
        } else {
            builder.ins().return_(&[]);
        }
        builder.finalize();
    }

    module
        .define_function(function_id, &mut context)
        .map_err(|error| format!("failed to define JIT function {symbol}: {error}"))?;
    module.clear_context(&mut context);
    module
        .finalize_definitions()
        .map_err(|error| format!("failed to finalize JIT definitions: {error}"))?;
    let code_ptr = module.get_finalized_function(function_id) as usize as u64;
    Ok((module, code_ptr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::EngineEntrypoints;

    #[test]
    fn jit_process_runs_full_compile_and_records_slots() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return 2; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 2);
        assert_eq!(report.emit.emitted_functions, 2);
        assert_eq!(process.artifacts().len(), 2);
        assert!(process
            .artifacts()
            .iter()
            .all(|artifact| artifact.code_ptr != 0));
    }

    #[test]
    fn jit_process_rejects_non_literal_i32_return() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { return helper(); }\n",
        );
        let error = process.compile().expect_err("expected compile error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("expected integer literal")
                        || message.contains("unsupported return expression"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_incremental_compile_emits_only_changed_function() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return 2; }\n",
        );
        let first = process.compile().expect("first compile");
        assert_eq!(first.emit.emitted_functions, 2);
        assert_eq!(process.artifacts().len(), 2);

        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 3; }\nfunction main(): i32 { return 2; }\n",
        );
        let second = process.compile().expect("second compile");
        assert_eq!(second.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 2);
    }

    #[test]
    fn jit_process_supports_binary_literal_return_expression() {
        let mut process = JitProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return 4 + 5; }\n");
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_void_return_functions() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function on_code_swap(): void { return; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_i32_in_memory_for_verification() {
        let mut process = JitProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return -7; }\n");
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, -7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_execution_reflects_incremental_recompile() {
        let mut process = JitProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return 1; }\n");
        process.compile().expect("first compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute first"),
            1
        );

        process.upsert_file("sample.stasis", "function main(): i32 { return 3; }\n");
        process.compile().expect("second compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute second"),
            3
        );
    }

    #[test]
    fn jit_engine_package_exposes_required_entrypoints() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function tick(): void { return; }\nfunction render(): void { return; }\nfunction on_code_swap(): void { return; }\n",
        );
        process.compile().expect("compile");
        let package = process
            .build_engine_package(&EngineEntrypoints::runtime_default())
            .expect("engine package");
        assert_ne!(package.tick_code_ptr, 0);
        assert_ne!(package.render_code_ptr, 0);
        assert_eq!(
            package.on_code_swap_code_ptr.is_some(),
            true,
            "expected on_code_swap pointer"
        );
        assert_eq!(
            package.symbol_code_ptrs.contains_key("tick"),
            true,
            "expected tick in package symbol map"
        );
        assert_eq!(
            package.symbol_code_ptrs.contains_key("render"),
            true,
            "expected render in package symbol map"
        );
    }

    #[test]
    fn jit_engine_package_errors_when_required_entrypoint_missing() {
        let mut process = JitProcess::new();
        process.upsert_file("sample.stasis", "function tick(): void { return; }\n");
        process.compile().expect("compile");
        let error = process
            .build_engine_package(&EngineEntrypoints::runtime_default())
            .expect_err("missing render should fail");
        assert!(
            error.contains("required engine entrypoint 'render' not found"),
            "unexpected message: {error}"
        );
    }
}
