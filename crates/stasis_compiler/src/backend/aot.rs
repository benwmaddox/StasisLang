use crate::backend::eval_simple_i32_return_expression;
use crate::compiler::{CompileReport, CompileResult, Compiler, FunctionId, FunctionMeta};
use crate::frontend::types::{TYPE_ID_I32, TYPE_ID_VOID};
use crate::ir::hir::FunctionHIR;
use cranelift_codegen::ir::{types, AbiParam, InstBuilder};
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{default_libcall_names, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AotArtifact {
    pub function_id: FunctionId,
    pub object_index: u32,
    pub body_hash: u64,
    pub symbol_name: String,
    pub object_bytes_len: usize,
}

#[derive(Debug, Default)]
pub struct AotProcess {
    compiler: Compiler,
    next_object_index: u32,
    next_symbol_seq: u64,
    artifacts: Vec<AotArtifact>,
    object_bytes: Vec<Vec<u8>>,
}

impl AotProcess {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.compiler.upsert_file(path, content);
    }

    pub fn compile(&mut self) -> CompileResult<CompileReport> {
        let (compiler, next_object_index, next_symbol_seq, artifacts, object_bytes) = (
            &mut self.compiler,
            &mut self.next_object_index,
            &mut self.next_symbol_seq,
            &mut self.artifacts,
            &mut self.object_bytes,
        );
        compiler.compile_with(|meta, hir| {
            let symbol = format!("aot_fn_{}_{}", meta.id, *next_symbol_seq);
            *next_symbol_seq = next_symbol_seq.saturating_add(1);
            let bytes = compile_function_to_object_bytes(meta, hir, &symbol)?;
            let object_index = *next_object_index;
            *next_object_index = next_object_index.saturating_add(1);
            object_bytes.push(bytes);
            let object_bytes_len = object_bytes.last().map_or(0usize, std::vec::Vec::len);
            artifacts.retain(|artifact| artifact.function_id != meta.id);
            artifacts.push(AotArtifact {
                function_id: meta.id,
                object_index,
                body_hash: meta.body_hash,
                symbol_name: symbol,
                object_bytes_len,
            });
            Ok(())
        })
    }

    pub fn artifacts(&self) -> &[AotArtifact] {
        &self.artifacts
    }

    pub fn link_executable_for_i32_noarg_function(
        &self,
        name: &str,
        output_executable: &Path,
        link_config: &stasis_jit::AotLinkConfig,
    ) -> Result<PathBuf, String> {
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
        if !function.params.is_empty() {
            return Err(format!(
                "function '{name}' has {} parameters; expected 0 for executable entry smoke",
                function.params.len()
            ));
        }
        let artifact = self
            .artifacts
            .iter()
            .find(|artifact| artifact.function_id == function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;
        let object_bytes = self
            .object_bytes
            .get(artifact.object_index as usize)
            .ok_or_else(|| {
                format!(
                    "object bytes missing for function '{name}' at index {}",
                    artifact.object_index
                )
            })?;
        let object_path = output_executable.with_extension("obj");
        if let Some(parent) = object_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create output object directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        fs::write(&object_path, object_bytes).map_err(|error| {
            format!(
                "failed to write object file {}: {error}",
                object_path.display()
            )
        })?;
        stasis_jit::link_objects_to_executable(
            std::slice::from_ref(&object_path),
            output_executable,
            &artifact.symbol_name,
            link_config,
        )?;
        Ok(object_path)
    }
}

fn compile_function_to_object_bytes(
    meta: &FunctionMeta,
    hir: &FunctionHIR,
    symbol: &str,
) -> Result<Vec<u8>, String> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", "none")
        .map_err(|error| format!("failed to configure Cranelift opt level: {error}"))?;
    let flags = settings::Flags::new(flag_builder);
    let isa_builder = cranelift_native::builder()
        .map_err(|error| format!("failed to construct native ISA builder: {error}"))?;
    let isa = isa_builder
        .finish(flags)
        .map_err(|error| format!("failed to finalize native ISA: {error}"))?;

    let builder = ObjectBuilder::new(
        isa,
        "stasis_compiler_trial".to_string(),
        default_libcall_names(),
    )
    .map_err(|error| format!("failed to construct object builder: {error}"))?;
    let mut module = ObjectModule::new(builder);
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
                "unsupported AOT return type id {other} for function {}",
                meta.name
            ));
        }
    }

    let function_id = module
        .declare_function(symbol, Linkage::Export, &context.func.signature)
        .map_err(|error| format!("failed to declare AOT function {symbol}: {error}"))?;

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
        .map_err(|error| format!("failed to define AOT function {symbol}: {error}"))?;
    module.clear_context(&mut context);
    let product = module.finish();
    product
        .emit()
        .map_err(|error| format!("failed to emit AOT object bytes: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn aot_process_runs_full_compile_and_records_objects() {
        let mut process = AotProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return 7; }\n");
        let report = process.compile().expect("aot compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process.artifacts()[0].object_bytes_len > 0);
    }

    #[test]
    fn aot_process_rejects_non_literal_i32_return() {
        let mut process = AotProcess::new();
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
    fn aot_process_incremental_compile_emits_only_changed_function() {
        let mut process = AotProcess::new();
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
    fn aot_process_supports_binary_literal_return_expression() {
        let mut process = AotProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return 4 + 5; }\n");
        let report = process.compile().expect("aot compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].object_bytes_len > 0);
    }

    #[test]
    fn aot_process_supports_void_return_functions() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function on_code_swap(): void { return; }\n",
        );
        let report = process.compile().expect("aot compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process.artifacts()[0].object_bytes_len > 0);
    }

    #[cfg(windows)]
    #[test]
    fn aot_process_links_and_executes_executable_smoke() {
        let Some(link_config) = resolve_link_config_for_smoke() else {
            return;
        };

        let mut process = AotProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return 27; }\n");
        process.compile().expect("compile");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_exe_smoke_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let exe_path = temp_root.join("main_smoke.exe");
        process
            .link_executable_for_i32_noarg_function("main", &exe_path, &link_config)
            .expect("link executable");

        let status = Command::new(&exe_path)
            .status()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", exe_path.display()));
        assert_eq!(
            status.code(),
            Some(27),
            "expected executable to return exit code 27"
        );
        let _ = fs::remove_dir_all(&temp_root);
    }

    #[cfg(windows)]
    #[test]
    fn aot_process_executable_smoke_reflects_incremental_recompile() {
        let Some(link_config) = resolve_link_config_for_smoke() else {
            return;
        };

        let mut process = AotProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return 5; }\n");
        process.compile().expect("first compile");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_exe_smoke_inc_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");

        let exe_first = temp_root.join("main_first.exe");
        process
            .link_executable_for_i32_noarg_function("main", &exe_first, &link_config)
            .expect("link first executable");
        let first_status = Command::new(&exe_first)
            .status()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", exe_first.display()));
        assert_eq!(first_status.code(), Some(5));

        process.upsert_file("sample.stasis", "function main(): i32 { return 9; }\n");
        process.compile().expect("second compile");
        let exe_second = temp_root.join("main_second.exe");
        process
            .link_executable_for_i32_noarg_function("main", &exe_second, &link_config)
            .expect("link second executable");
        let second_status = Command::new(&exe_second)
            .status()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", exe_second.display()));
        assert_eq!(second_status.code(), Some(9));

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[cfg(windows)]
    fn resolve_link_config_for_smoke() -> Option<stasis_jit::AotLinkConfig> {
        if let Some(explicit) = std::env::var_os("STASIS_AOT_LINKER") {
            let explicit = PathBuf::from(explicit);
            return Some(stasis_jit::AotLinkConfig {
                linker_path: Some(explicit),
            });
        }
        for candidate in ["lld-link.exe", "link.exe"] {
            let output = Command::new("where").arg(candidate).output().ok()?;
            if output.status.success() {
                return Some(stasis_jit::AotLinkConfig {
                    linker_path: Some(PathBuf::from(candidate)),
                });
            }
        }
        eprintln!("skipping AOT executable smoke test: no Windows linker found");
        None
    }
}
