use crate::backend::emit::*;
use crate::backend::{AotOptimizationProfile, EngineEntrypoints};
use crate::compiler::{CompileReport, CompileResult, Compiler, FunctionId, FunctionMeta};
use crate::frontend::types::{TypeTable, TYPE_ID_I32, TYPE_ID_VOID};
use crate::ir::hir::FunctionHIR;
use cranelift_codegen::ir::{AbiParam, InstBuilder, Value};
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{default_libcall_names, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::BTreeMap;
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
    optimization_profile: AotOptimizationProfile,
    next_object_index: u32,
    next_symbol_seq: u64,
    artifacts: Vec<AotArtifact>,
    object_bytes: Vec<Vec<u8>>,
    required_emit_roots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AotEngineBundle {
    pub output_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub object_paths_by_function: BTreeMap<String, PathBuf>,
    pub optimization_profile: AotOptimizationProfile,
}

impl AotProcess {
    pub fn new() -> Self {
        Self::with_optimization_profile(AotOptimizationProfile::Speed)
    }

    pub fn with_optimization_profile(optimization_profile: AotOptimizationProfile) -> Self {
        Self {
            compiler: Compiler::new(),
            optimization_profile,
            next_object_index: 0,
            next_symbol_seq: 0,
            artifacts: Vec::new(),
            object_bytes: Vec::new(),
            required_emit_roots: Vec::new(),
        }
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.compiler.upsert_file(path, content);
    }

    pub fn set_required_emit_roots(&mut self, roots: &[String]) {
        self.required_emit_roots.clear();
        self.required_emit_roots.extend_from_slice(roots);
    }

    pub fn compile(&mut self) -> CompileResult<CompileReport> {
        let index = self.compiler.index_pass()?;
        let mut type_table = self.compiler.types().clone();
        type_table
            .ensure_utf8_view_id()
            .map_err(crate::compiler::CompileError::Backend)?;
        type_table
            .ensure_ascii_view_id()
            .map_err(crate::compiler::CompileError::Backend)?;

        let extern_signatures =
            collect_supported_extern_call_signatures(self.compiler.files(), &mut type_table)
                .map_err(crate::compiler::CompileError::Backend)?;
        let resolved_extern_signatures: Vec<ResolvedExternCallSignature> = extern_signatures
            .iter()
            .filter_map(|sig| {
                sig.symbol_candidates.first().map(|symbol| {
                    ResolvedExternCallSignature {
                        name: sig.name.clone(),
                        symbol: symbol.clone(),
                        params: sig.params.clone(),
                        return_type: sig.return_type,
                    }
                })
            })
            .collect();
        let call_signatures = collect_supported_call_signatures(
            self.compiler.functions(),
            &resolved_extern_signatures,
            &type_table,
        );
        let constant_values =
            collect_top_level_constant_values(self.compiler.files(), &mut type_table)
                .map_err(crate::compiler::CompileError::Backend)?;
        let global_path_types =
            collect_global_path_types(self.compiler.files(), &mut type_table, &constant_values)
                .map_err(crate::compiler::CompileError::Backend)?;
        let collection_infos = collect_foreach_collection_infos(
            self.compiler.files(),
            &mut type_table,
            &constant_values,
        )
        .map_err(crate::compiler::CompileError::Backend)?;
        let named_struct_field_types =
            collect_named_struct_field_types(self.compiler.files(), &mut type_table)
                .map_err(crate::compiler::CompileError::Backend)?;

        let reachable = crate::backend::reachability::compute_reachable_function_ids(
            self.compiler.functions(),
            &self.required_emit_roots,
        );
        let compiled_body_hashes: BTreeMap<FunctionId, u64> = self
            .artifacts
            .iter()
            .map(|artifact| (artifact.function_id, artifact.body_hash))
            .collect();
        let emit_function_ids: Vec<FunctionId> = self
            .compiler
            .functions()
            .iter()
            .filter(|function| reachable.contains(&function.id))
            .filter(|function| {
                let compiled_body_hash = compiled_body_hashes.get(&function.id).copied();
                let artifact_matches_body_hash = compiled_body_hash == Some(function.body_hash);
                function.dirty || !artifact_matches_body_hash
            })
            .map(|function| function.id)
            .collect();

        let (
            compiler,
            next_object_index,
            next_symbol_seq,
            artifacts,
            object_bytes,
            optimization_profile,
        ) = (
            &mut self.compiler,
            &mut self.next_object_index,
            &mut self.next_symbol_seq,
            &mut self.artifacts,
            &mut self.object_bytes,
            self.optimization_profile,
        );
        let emit = compiler.emit_pass_for_ids_with(&emit_function_ids, &mut |meta, hir| {
            let symbol = format!("aot_fn_{}_{}", meta.id, *next_symbol_seq);
            *next_symbol_seq = next_symbol_seq.saturating_add(1);
            let bytes = compile_function_to_object_bytes(
                meta,
                hir,
                &symbol,
                optimization_profile,
                &call_signatures,
                &mut type_table,
                &global_path_types,
                &constant_values,
                &collection_infos,
                &named_struct_field_types,
            )?;
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
        })?;

        artifacts.retain(|artifact| reachable.contains(&artifact.function_id));
        Ok(CompileReport { index, emit })
    }

    pub fn artifacts(&self) -> &[AotArtifact] {
        &self.artifacts
    }

    pub fn optimization_profile(&self) -> AotOptimizationProfile {
        self.optimization_profile
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

    pub fn write_engine_bundle(
        &self,
        entrypoints: &EngineEntrypoints,
        output_dir: &Path,
    ) -> Result<AotEngineBundle, String> {
        fs::create_dir_all(output_dir).map_err(|error| {
            format!(
                "failed to create AOT engine bundle directory {}: {error}",
                output_dir.display()
            )
        })?;

        let mut object_paths_by_function: BTreeMap<String, PathBuf> = BTreeMap::new();
        let mut manifest_rows: Vec<(String, String, String)> = Vec::new();
        for artifact in &self.artifacts {
            let function = self
                .compiler
                .functions()
                .iter()
                .find(|function| function.id == artifact.function_id)
                .ok_or_else(|| {
                    format!(
                        "function metadata missing for artifact function id {}",
                        artifact.function_id
                    )
                })?;
            let bytes = self
                .object_bytes
                .get(artifact.object_index as usize)
                .ok_or_else(|| {
                    format!(
                        "object bytes missing for function '{}' at object index {}",
                        function.name, artifact.object_index
                    )
                })?;
            let object_file_name = format!(
                "{}_{}.obj",
                sanitize_file_token(&function.name),
                artifact.object_index
            );
            let object_path = output_dir.join(&object_file_name);
            fs::write(&object_path, bytes).map_err(|error| {
                format!(
                    "failed to write object file {}: {error}",
                    object_path.display()
                )
            })?;
            object_paths_by_function.insert(function.name.clone(), object_path);
            manifest_rows.push((
                function.name.clone(),
                artifact.symbol_name.clone(),
                object_file_name,
            ));
        }

        // Enforce required runtime entrypoints for engine integration.
        ensure_function_in_bundle(&object_paths_by_function, &entrypoints.tick)?;
        ensure_function_in_bundle(&object_paths_by_function, &entrypoints.render)?;
        if let Some(on_code_swap) = entrypoints.on_code_swap.as_ref() {
            ensure_function_in_bundle(&object_paths_by_function, on_code_swap)?;
        }

        let manifest_path = output_dir.join("engine_bundle_manifest.json");
        let manifest =
            build_engine_bundle_manifest(self.optimization_profile, entrypoints, &manifest_rows);
        fs::write(&manifest_path, manifest).map_err(|error| {
            format!(
                "failed to write engine bundle manifest {}: {error}",
                manifest_path.display()
            )
        })?;

        Ok(AotEngineBundle {
            output_dir: output_dir.to_path_buf(),
            manifest_path,
            object_paths_by_function,
            optimization_profile: self.optimization_profile,
        })
    }

    pub fn write_object_files(
        &self,
        output_dir: &Path,
    ) -> Result<BTreeMap<String, (String, PathBuf)>, String> {
        fs::create_dir_all(output_dir).map_err(|error| {
            format!(
                "failed to create AOT object output directory {}: {error}",
                output_dir.display()
            )
        })?;

        let mut out = BTreeMap::new();
        for artifact in &self.artifacts {
            let function = self
                .compiler
                .functions()
                .iter()
                .find(|function| function.id == artifact.function_id)
                .ok_or_else(|| {
                    format!(
                        "function metadata missing for artifact function id {}",
                        artifact.function_id
                    )
                })?;
            let bytes = self
                .object_bytes
                .get(artifact.object_index as usize)
                .ok_or_else(|| {
                    format!(
                        "object bytes missing for function '{}' at object index {}",
                        function.name, artifact.object_index
                    )
                })?;

            let extension = if cfg!(windows) { "obj" } else { "o" };
            let object_file_name = format!(
                "{}_{}.{}",
                sanitize_file_token(&function.name),
                artifact.object_index,
                extension
            );
            let object_path = output_dir.join(object_file_name);
            fs::write(&object_path, bytes).map_err(|error| {
                format!(
                    "failed to write object file {}: {error}",
                    object_path.display()
                )
            })?;
            out.insert(
                function.name.clone(),
                (artifact.symbol_name.clone(), object_path),
            );
        }
        Ok(out)
    }
}

fn compile_function_to_object_bytes(
    meta: &FunctionMeta,
    hir: &FunctionHIR,
    symbol: &str,
    optimization_profile: AotOptimizationProfile,
    call_signatures: &CallSignatureMap,
    type_table: &mut TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> Result<Vec<u8>, String> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", optimization_profile.as_cranelift_opt_level())
        .map_err(|error| format!("failed to configure Cranelift opt level: {error}"))?;
    let flags = settings::Flags::new(flag_builder);
    let isa_builder = cranelift_native::builder()
        .map_err(|error| format!("failed to construct native ISA builder: {error}"))?;
    let isa = isa_builder
        .finish(flags)
        .map_err(|error| format!("failed to finalize native ISA: {error}"))?;

    let builder = ObjectBuilder::new(
        isa,
        "stasis_aot_module".to_string(),
        default_libcall_names(),
    )
    .map_err(|error| format!("failed to construct object builder: {error}"))?;
    let mut module = ObjectModule::new(builder);
    let mut context = module.make_context();
    context.func.signature = module.make_signature();
    for param_type in &meta.params {
        let clif_param_type = clif_type_for_type_id(*param_type, type_table)?;
        context
            .func
            .signature
            .params
            .push(AbiParam::new(clif_param_type));
    }
    if meta.return_type != TYPE_ID_VOID {
        let clif_return_type =
            clif_type_for_type_id(meta.return_type, type_table).map_err(|_| {
                format!(
                    "unsupported AOT return type id {} for function {}",
                    meta.return_type, meta.name
                )
            })?;
        context
            .func
            .signature
            .returns
            .push(AbiParam::new(clif_return_type));
    }

    let function_id = module
        .declare_function(symbol, Linkage::Export, &context.func.signature)
        .map_err(|error| format!("failed to declare AOT function {symbol}: {error}"))?;
    let runtime_call_imports =
        build_runtime_call_import_ids(&mut module, call_signatures, type_table)?;

    let mut function_builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut function_builder_context);
        let runtime_call_refs =
            build_runtime_call_refs(&mut module, &runtime_call_imports, builder.func);
        let entry = builder.create_block();
        for param_type in &meta.params {
            builder.append_block_param(entry, clif_type_for_type_id(*param_type, type_table)?);
        }
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        if meta.param_names.len() != meta.params.len() {
            return Err(format!(
                "parameter metadata mismatch for function '{}' ({} names, {} types)",
                meta.name,
                meta.param_names.len(),
                meta.params.len()
            ));
        }
        let mut values_by_name: BTreeMap<String, LocalBinding> = BTreeMap::new();
        let mut next_variable = 0u32;
        let block_params: Vec<Value> = builder.block_params(entry).to_vec();
        for (index, name) in meta.param_names.iter().enumerate() {
            let Some(value) = block_params.get(index).copied() else {
                return Err(format!(
                    "missing block parameter {} for function '{}'",
                    index, meta.name
                ));
            };
            let variable = declare_new_variable(
                &mut builder,
                &mut next_variable,
                value,
                meta.params[index],
                type_table,
            )?;
            if values_by_name.contains_key(name) {
                return Err(format!("parameter '{}' shadows existing variable", name));
            }
            values_by_name.insert(
                name.clone(),
                LocalBinding {
                    var: variable,
                    type_id: meta.params[index],
                },
            );
        }

        let empty_foreach_bindings = ForeachBindingMap::new();
        let body = extract_function_body(hir)?;
        let mut terminated = false;
        parse_simple_statements_from_block_with(body, type_table, |type_table, statement| {
            if terminated {
                return Ok(());
            }
            terminated = emit_simple_statements(
                &mut builder,
                std::slice::from_ref(&statement),
                &mut values_by_name,
                &runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                &empty_foreach_bindings,
                None,
                meta.return_type,
                &mut next_variable,
            )?;
            Ok(())
        })?;
        if !terminated {
            if meta.return_type == TYPE_ID_VOID {
                builder.ins().return_(&[]);
            } else {
                return Err(format!(
                    "non-void function '{}' must end with a return statement",
                    meta.name
                ));
            }
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

fn ensure_function_in_bundle(
    object_paths_by_function: &BTreeMap<String, PathBuf>,
    function_name: &str,
) -> Result<(), String> {
    if object_paths_by_function.contains_key(function_name) {
        Ok(())
    } else {
        Err(format!(
            "required engine entrypoint '{}' missing from AOT bundle",
            function_name
        ))
    }
}

fn sanitize_file_token(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "fn".to_string()
    } else {
        out
    }
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn build_engine_bundle_manifest(
    optimization_profile: AotOptimizationProfile,
    entrypoints: &EngineEntrypoints,
    rows: &[(String, String, String)],
) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str(&format!(
        "  \"optimization_profile\": \"{}\",\n",
        optimization_profile.as_str()
    ));
    out.push_str("  \"entrypoints\": {\n");
    out.push_str(&format!(
        "    \"tick\": \"{}\",\n",
        json_escape(&entrypoints.tick)
    ));
    out.push_str(&format!(
        "    \"render\": \"{}\"",
        json_escape(&entrypoints.render)
    ));
    if let Some(on_code_swap) = entrypoints.on_code_swap.as_ref() {
        out.push_str(&format!(
            ",\n    \"on_code_swap\": \"{}\"\n",
            json_escape(on_code_swap)
        ));
    } else {
        out.push('\n');
    }
    out.push_str("  },\n");
    out.push_str("  \"functions\": [\n");
    for (index, (name, symbol, object_file)) in rows.iter().enumerate() {
        let comma = if index + 1 < rows.len() { "," } else { "" };
        out.push_str(&format!(
            "    {{\"name\":\"{}\",\"symbol\":\"{}\",\"object\":\"{}\"}}{}\n",
            json_escape(name),
            json_escape(symbol),
            json_escape(object_file),
            comma
        ));
    }
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::EngineEntrypoints;
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
    fn aot_process_rejects_undefined_call_target() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { return helper(); }\n",
        );
        let error = process.compile().expect_err("expected compile error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("unknown call target"),
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
        assert_eq!(first.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);

        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 3; }\nfunction main(): i32 { return 2; }\n",
        );
        let second = process.compile().expect("second compile");
        assert_eq!(second.emit.emitted_functions, 0);
        assert_eq!(process.artifacts().len(), 1);
    }

    #[test]
    fn aot_process_skips_unreachable_invalid_function_body() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function bad(): i32 { return missing(); }\nfunction tick(): i32 { return 1; }\n",
        );
        let report = process.compile().expect("compile");
        assert_eq!(report.index.parsed_functions, 2);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
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

    #[test]
    fn aot_process_defaults_to_speed_optimization_profile() {
        let process = AotProcess::new();
        assert_eq!(
            process.optimization_profile(),
            AotOptimizationProfile::Speed
        );
    }

    #[test]
    fn aot_engine_bundle_writes_manifest_and_required_entrypoints() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function tick(): void { return; }\nfunction render(): void { return; }\nfunction on_code_swap(): void { return; }\n",
        );
        process.compile().expect("compile");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let bundle_dir = std::env::temp_dir().join(format!("stasis_aot_bundle_{stamp}"));
        let bundle = process
            .write_engine_bundle(&EngineEntrypoints::runtime_default(), &bundle_dir)
            .expect("write bundle");
        assert!(bundle.manifest_path.exists(), "manifest should exist");
        assert_eq!(
            bundle.object_paths_by_function.contains_key("tick"),
            true,
            "expected tick object path"
        );
        assert_eq!(
            bundle.object_paths_by_function.contains_key("render"),
            true,
            "expected render object path"
        );
        let manifest = fs::read_to_string(&bundle.manifest_path).expect("read manifest");
        assert!(
            manifest.contains("\"optimization_profile\": \"speed\""),
            "manifest should include speed optimization profile"
        );
        assert!(
            manifest.contains("\"tick\": \"tick\"") && manifest.contains("\"render\": \"render\""),
            "manifest should include required entrypoints"
        );

        let _ = fs::remove_dir_all(&bundle_dir);
    }

    #[test]
    fn aot_engine_bundle_errors_when_required_entrypoint_missing() {
        let mut process = AotProcess::new();
        process.upsert_file("sample.stasis", "function tick(): void { return; }\n");
        process.compile().expect("compile");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let bundle_dir = std::env::temp_dir().join(format!("stasis_aot_bundle_missing_{stamp}"));
        let error = process
            .write_engine_bundle(&EngineEntrypoints::runtime_default(), &bundle_dir)
            .expect_err("missing render should fail");
        assert!(
            error.contains("required engine entrypoint 'render' missing"),
            "unexpected message: {error}"
        );
        let _ = fs::remove_dir_all(&bundle_dir);
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
        let link_result =
            process.link_executable_for_i32_noarg_function("main", &exe_path, &link_config);
        if let Err(ref message) = link_result {
            if message.contains("undefined symbol") {
                eprintln!(
                    "skipping AOT executable smoke: runtime symbols not available at link time"
                );
                let _ = fs::remove_dir_all(&temp_root);
                return;
            }
        }
        link_result.expect("link executable");

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
        let link_result =
            process.link_executable_for_i32_noarg_function("main", &exe_first, &link_config);
        if let Err(ref message) = link_result {
            if message.contains("undefined symbol") {
                eprintln!(
                    "skipping AOT executable incremental smoke: runtime symbols not available"
                );
                let _ = fs::remove_dir_all(&temp_root);
                return;
            }
        }
        link_result.expect("link first executable");
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
