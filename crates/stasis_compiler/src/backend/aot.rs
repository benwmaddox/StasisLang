use crate::backend::emit::*;
use crate::backend::{AotOptimizationProfile, EngineEntrypoints};
use crate::compiler::{CompileReport, CompileResult, Compiler, FunctionId, FunctionMeta};
use crate::frontend::types::{TypeCategory, TypeTable, TYPE_ID_I32, TYPE_ID_VOID};
use crate::ir::hir::FunctionHIR;
use cranelift_codegen::ir::{types, AbiParam, InstBuilder, Value};
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{default_libcall_names, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

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
    artifacts: Vec<AotArtifact>,
    object_bytes: Vec<Vec<u8>>,
    string_literals: BTreeMap<i32, String>,
    collection_max_lengths: BTreeMap<String, i32>,
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
            artifacts: Vec::new(),
            object_bytes: Vec::new(),
            string_literals: BTreeMap::new(),
            collection_max_lengths: BTreeMap::new(),
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
        self.load_import_graph_sources()
            .map_err(crate::compiler::CompileError::Backend)?;
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
        // AOT objects must be linked against a concrete runtime. For externs we prefer the most
        // "runtime-friendly" symbol candidate (typically `stasis_jit_*` shims) instead of the
        // raw source-level name.
        let resolved_extern_signatures: Vec<ResolvedExternCallSignature> = extern_signatures
            .iter()
            .filter_map(|sig| {
                sig.symbol_candidates
                    .last()
                    .map(|symbol| ResolvedExternCallSignature {
                        name: sig.name.clone(),
                        symbol: symbol.clone(),
                        params: sig.params.clone(),
                        return_type: sig.return_type,
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
        for constant in constant_values.values() {
            if let ConstantValue::String { value, .. } = constant {
                record_string_literal(&mut self.string_literals, value)
                    .map_err(crate::compiler::CompileError::Backend)?;
            }
        }
        let global_path_types =
            collect_global_path_types(self.compiler.files(), &mut type_table, &constant_values)
                .map_err(crate::compiler::CompileError::Backend)?;
        self.collection_max_lengths =
            collect_fixed_collection_max_lengths(&global_path_types, &type_table)
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
            artifacts,
            object_bytes,
            optimization_profile,
            string_literals,
        ) = (
            &mut self.compiler,
            &mut self.next_object_index,
            &mut self.artifacts,
            &mut self.object_bytes,
            self.optimization_profile,
            &mut self.string_literals,
        );
        let emit = compiler.emit_pass_for_ids_with(&emit_function_ids, &mut |meta, hir| {
            // Stable per-function symbols are required so AOT objects can reference each other
            // directly without forcing recompilation of callers on every body change.
            let symbol = format!("aot_fn_{}", meta.id);
            let bytes = compile_function_to_object_bytes(
                meta,
                hir,
                &symbol,
                optimization_profile,
                &call_signatures,
                &mut type_table,
                &global_path_types,
                &constant_values,
                string_literals,
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

    fn load_import_graph_sources(&mut self) -> Result<(), String> {
        let mut known_paths: BTreeSet<String> = self
            .compiler
            .files()
            .iter()
            .map(|file| file.path.clone())
            .collect();
        let mut queue: Vec<String> = self
            .compiler
            .files()
            .iter()
            .map(|file| file.path.clone())
            .collect();

        while let Some(path) = queue.pop() {
            let Some(source) = self
                .compiler
                .files()
                .iter()
                .find(|file| file.path == path)
                .map(|file| file.content.clone())
            else {
                continue;
            };
            let imports = parse_import_paths(&source);
            for import_path in imports {
                let resolved = resolve_import_path(&path, &import_path);
                let normalized = normalize_path_for_compiler_key(&resolved);
                if known_paths.contains(&normalized) {
                    continue;
                }
                let content = std::fs::read_to_string(&resolved).map_err(|error| {
                    format!(
                        "failed to load import '{}' referenced by '{}': {}",
                        import_path, path, error
                    )
                })?;
                self.compiler.upsert_file(normalized.clone(), content);
                known_paths.insert(normalized.clone());
                queue.push(normalized);
            }
        }

        Ok(())
    }

    pub fn artifacts(&self) -> &[AotArtifact] {
        &self.artifacts
    }

    pub fn string_literals(&self) -> &BTreeMap<i32, String> {
        &self.string_literals
    }

    pub fn collection_max_lengths(&self) -> &BTreeMap<String, i32> {
        &self.collection_max_lengths
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
        let entry_artifact = self
            .artifacts
            .iter()
            .find(|artifact| artifact.function_id == function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;

        let stem = output_executable
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("stasis_aot_exe");
        let object_dir = output_executable
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("{stem}_objects"));
        fs::create_dir_all(&object_dir).map_err(|error| {
            format!(
                "failed to create output object directory {}: {error}",
                object_dir.display()
            )
        })?;

        let mut object_paths: Vec<PathBuf> = Vec::new();
        let mut entry_object_path: Option<PathBuf> = None;
        for artifact in &self.artifacts {
            let artifact_function = self
                .compiler
                .functions()
                .iter()
                .find(|function| function.id == artifact.function_id)
                .ok_or_else(|| {
                    format!(
                        "compiled function metadata missing for id {}",
                        artifact.function_id
                    )
                })?;
            let object_bytes = self
                .object_bytes
                .get(artifact.object_index as usize)
                .ok_or_else(|| {
                    format!(
                        "object bytes missing for function '{}' at index {}",
                        artifact_function.name, artifact.object_index
                    )
                })?;
            let object_file_name = format!(
                "{}_{}.obj",
                sanitize_file_token(&artifact_function.name),
                artifact.object_index
            );
            let object_path = object_dir.join(object_file_name);
            fs::write(&object_path, object_bytes).map_err(|error| {
                format!(
                    "failed to write object file {}: {error}",
                    object_path.display()
                )
            })?;
            if artifact.function_id == function.id {
                entry_object_path = Some(object_path.clone());
            }
            object_paths.push(object_path);
        }
        let entry_object_path = entry_object_path
            .ok_or_else(|| format!("entry object path missing for function '{name}'"))?;

        stasis_jit::link_objects_to_executable(
            &object_paths,
            output_executable,
            &entry_artifact.symbol_name,
            link_config,
        )?;
        Ok(entry_object_path)
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
        let manifest = build_engine_bundle_manifest(
            self.optimization_profile,
            entrypoints,
            &manifest_rows,
            &self.string_literals,
            &self.collection_max_lengths,
        );
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
    string_literals: &mut BTreeMap<i32, String>,
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
        append_abi_params_for_type_id(
            &mut context.func.signature.params,
            *param_type,
            type_table,
            named_struct_field_types,
        )?;
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
    let runtime_call_imports = build_runtime_call_import_ids(
        &mut module,
        call_signatures,
        type_table,
        named_struct_field_types,
    )?;

    let mut function_builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut function_builder_context);
        let runtime_call_refs =
            build_runtime_call_refs(&mut module, &runtime_call_imports, builder.func);
        let entry = builder.create_block();
        for param_type in &meta.params {
            if is_struct_view_type(*param_type, named_struct_field_types) {
                for _ in 0..STRUCT_VIEW_ABI_WORDS {
                    builder.append_block_param(entry, types::I32);
                }
            } else {
                builder.append_block_param(entry, clif_type_for_type_id(*param_type, type_table)?);
            }
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
        let mut block_param_cursor = 0usize;
        for (index, name) in meta.param_names.iter().enumerate() {
            let param_type = meta.params[index];
            let (variable, struct_view) =
                if is_struct_view_type(param_type, named_struct_field_types) {
                    let base_value =
                        block_params
                            .get(block_param_cursor)
                            .copied()
                            .ok_or_else(|| {
                                format!(
                                    "missing struct view base parameter {} for function '{}'",
                                    block_param_cursor, meta.name
                                )
                            })?;
                    let index_value = block_params
                        .get(block_param_cursor + 1)
                        .copied()
                        .ok_or_else(|| {
                            format!(
                                "missing struct view index parameter {} for function '{}'",
                                block_param_cursor + 1,
                                meta.name
                            )
                        })?;
                    let len_value = block_params
                        .get(block_param_cursor + 2)
                        .copied()
                        .ok_or_else(|| {
                            format!(
                                "missing struct view len parameter {} for function '{}'",
                                block_param_cursor + 2,
                                meta.name
                            )
                        })?;
                    block_param_cursor += STRUCT_VIEW_ABI_WORDS;

                    let base_var = declare_new_variable(
                        &mut builder,
                        &mut next_variable,
                        base_value,
                        param_type,
                        type_table,
                    )?;
                    let index_var = declare_new_variable(
                        &mut builder,
                        &mut next_variable,
                        index_value,
                        TYPE_ID_I32,
                        type_table,
                    )?;
                    let len_var = declare_new_variable(
                        &mut builder,
                        &mut next_variable,
                        len_value,
                        TYPE_ID_I32,
                        type_table,
                    )?;
                    (base_var, Some(StructViewBinding { index_var, len_var }))
                } else {
                    let value = block_params
                        .get(block_param_cursor)
                        .copied()
                        .ok_or_else(|| {
                            format!(
                                "missing block parameter {} for function '{}'",
                                block_param_cursor, meta.name
                            )
                        })?;
                    block_param_cursor = block_param_cursor.saturating_add(1);
                    (
                        declare_new_variable(
                            &mut builder,
                            &mut next_variable,
                            value,
                            param_type,
                            type_table,
                        )?,
                        None,
                    )
                };
            if values_by_name.contains_key(name) {
                return Err(format!("parameter '{}' shadows existing variable", name));
            }
            values_by_name.insert(
                name.clone(),
                LocalBinding {
                    var: variable,
                    type_id: param_type,
                    struct_view,
                },
            );
        }
        if block_param_cursor != block_params.len() {
            return Err(format!(
                "block parameter count mismatch for function '{}' (consumed {}, found {})",
                meta.name,
                block_param_cursor,
                block_params.len()
            ));
        }

        let empty_foreach_bindings = ForeachBindingMap::new();
        let mut internal_calls = InternalCallMode::AotDirect(AotDirectCallMode {
            module: &mut module,
            self_function_id: meta.id,
            self_clif_func_id: function_id,
            imported_function_ids: std::collections::HashMap::new(),
        });
        let body = extract_function_body(hir)?;
        let mut terminated = false;
        parse_simple_statements_from_block_with(body, type_table, |type_table, statement| {
            if terminated {
                return Ok(());
            }
            record_string_literals_in_stmt(&statement, string_literals)?;
            terminated = emit_simple_statements(
                &mut builder,
                std::slice::from_ref(&statement),
                &mut values_by_name,
                &runtime_call_refs,
                &mut internal_calls,
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

    #[cfg(test)]
    maybe_invoke_clif_dump_hook(meta, &context.func);

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
type ClifDumpHook =
    Box<dyn Fn(&FunctionMeta, &cranelift_codegen::ir::Function) + Send + Sync + 'static>;

#[cfg(test)]
static CLIF_DUMP_HOOK: OnceLock<Mutex<Option<ClifDumpHook>>> = OnceLock::new();

#[cfg(test)]
fn clif_dump_hook() -> &'static Mutex<Option<ClifDumpHook>> {
    CLIF_DUMP_HOOK.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
fn set_clif_dump_hook(hook: Option<ClifDumpHook>) {
    *clif_dump_hook().lock().expect("lock clif dump hook") = hook;
}

#[cfg(test)]
fn maybe_invoke_clif_dump_hook(meta: &FunctionMeta, func: &cranelift_codegen::ir::Function) {
    let guard = clif_dump_hook().lock().expect("lock clif dump hook");
    let Some(hook) = guard.as_ref() else {
        return;
    };
    hook(meta, func);
}

fn record_string_literal(out: &mut BTreeMap<i32, String>, value: &str) -> Result<(), String> {
    let id = hash_string_literal(value);
    if let Some(existing) = out.get(&id) {
        if existing != value {
            return Err(format!(
                "string literal hash collision for id={id}: existing={existing:?} new={value:?}"
            ));
        }
        return Ok(());
    }
    out.insert(id, value.to_string());
    Ok(())
}

fn collect_fixed_collection_max_lengths(
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> Result<BTreeMap<String, i32>, String> {
    let mut out: BTreeMap<String, i32> = BTreeMap::new();
    for (path, type_id) in global_path_types {
        let Some(type_info) = type_table.type_info(*type_id) else {
            continue;
        };
        match type_info.category {
            TypeCategory::AsciiFixed | TypeCategory::Utf8Fixed => {
                let Some(payload_bytes) = type_info.layout.payload_size_bytes else {
                    continue;
                };
                let max_length = i32::try_from(payload_bytes).map_err(|_| {
                    format!(
                        "collection max_length overflow for '{}' (payload bytes {})",
                        path, payload_bytes
                    )
                })?;
                out.insert(path.clone(), max_length);
            }
            TypeCategory::ArrayFixed => {
                let Some(max_length) = type_table.fixed_collection_len(*type_id) else {
                    continue;
                };
                out.insert(path.clone(), max_length);
            }
            _ => {}
        }
    }
    Ok(out)
}

fn record_string_literals_in_assign_target(
    target: &AssignTarget,
    out: &mut BTreeMap<i32, String>,
) -> Result<(), String> {
    match target {
        AssignTarget::Local(_) | AssignTarget::GlobalPath(_) => Ok(()),
        AssignTarget::IndexedPath { index, .. } => record_string_literals_in_expr(index, out),
    }
}

fn record_string_literals_in_condition(
    condition: &SimpleCondition,
    out: &mut BTreeMap<i32, String>,
) -> Result<(), String> {
    match condition {
        SimpleCondition::Comparison { lhs, rhs, .. } => {
            record_string_literals_in_expr(lhs, out)?;
            record_string_literals_in_expr(rhs, out)?;
            Ok(())
        }
        SimpleCondition::Expr(expr) => record_string_literals_in_expr(expr, out),
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            record_string_literals_in_condition(lhs, out)?;
            record_string_literals_in_condition(rhs, out)?;
            Ok(())
        }
        SimpleCondition::Not(inner) => record_string_literals_in_condition(inner, out),
    }
}

fn record_string_literals_in_expr(
    expression: &SimpleExpr,
    out: &mut BTreeMap<i32, String>,
) -> Result<(), String> {
    match expression {
        SimpleExpr::Int(_)
        | SimpleExpr::Float(_)
        | SimpleExpr::Bool(_)
        | SimpleExpr::Identifier(_) => Ok(()),
        SimpleExpr::StringLiteral(value) => record_string_literal(out, value),
        SimpleExpr::Condition(condition) => record_string_literals_in_condition(condition, out),
        SimpleExpr::IndexedPath { index, .. } => record_string_literals_in_expr(index, out),
        SimpleExpr::Call { args, .. } => {
            for arg in args {
                record_string_literals_in_expr(arg, out)?;
            }
            Ok(())
        }
        SimpleExpr::Binary { lhs, rhs, .. } => {
            record_string_literals_in_expr(lhs, out)?;
            record_string_literals_in_expr(rhs, out)?;
            Ok(())
        }
    }
}

fn record_string_literals_in_stmt(
    statement: &SimpleStmt,
    out: &mut BTreeMap<i32, String>,
) -> Result<(), String> {
    match statement {
        SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => Ok(()),
        SimpleStmt::Let { expression, .. } => record_string_literals_in_expr(expression, out),
        SimpleStmt::Assign {
            target, expression, ..
        } => {
            record_string_literals_in_assign_target(target, out)?;
            record_string_literals_in_expr(expression, out)?;
            Ok(())
        }
        SimpleStmt::Convert { target, source, .. } => {
            record_string_literals_in_assign_target(target, out)?;
            record_string_literals_in_expr(source, out)?;
            Ok(())
        }
        SimpleStmt::If {
            condition,
            then_statements,
            else_statements,
        } => {
            record_string_literals_in_condition(condition, out)?;
            for stmt in then_statements {
                record_string_literals_in_stmt(stmt, out)?;
            }
            if let Some(else_statements) = else_statements {
                for stmt in else_statements {
                    record_string_literals_in_stmt(stmt, out)?;
                }
            }
            Ok(())
        }
        SimpleStmt::For {
            init,
            condition,
            step,
            body_statements,
        } => {
            record_string_literals_in_stmt(init, out)?;
            record_string_literals_in_condition(condition, out)?;
            record_string_literals_in_stmt(step, out)?;
            for stmt in body_statements {
                record_string_literals_in_stmt(stmt, out)?;
            }
            Ok(())
        }
        SimpleStmt::Foreach {
            body_statements, ..
        } => {
            for stmt in body_statements {
                record_string_literals_in_stmt(stmt, out)?;
            }
            Ok(())
        }
        SimpleStmt::Expr(expression) | SimpleStmt::Return(expression) => {
            record_string_literals_in_expr(expression, out)
        }
    }
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
    string_literals: &BTreeMap<i32, String>,
    collection_max_lengths: &BTreeMap<String, i32>,
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
    out.push_str("  ],\n");
    out.push_str("  \"string_literals\": [\n");
    let literals_len = string_literals.len();
    for (index, (id, value)) in string_literals.iter().enumerate() {
        let comma = if index + 1 < literals_len { "," } else { "" };
        out.push_str(&format!(
            "    {{\"id\":{},\"value\":\"{}\"}}{}\n",
            id,
            json_escape(value),
            comma
        ));
    }
    out.push_str("  ],\n");
    out.push_str("  \"collection_max_lengths\": [\n");
    let collections_len = collection_max_lengths.len();
    for (index, (path, max_length)) in collection_max_lengths.iter().enumerate() {
        let comma = if index + 1 < collections_len { "," } else { "" };
        out.push_str(&format!(
            "    {{\"path\":\"{}\",\"max_length\":{}}}{}\n",
            json_escape(path),
            max_length,
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
    use std::sync::Arc;
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
    fn aot_process_supports_f64_global_field_set_and_read() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Layout { width: f64; }\nglobal state: Layout;\nfunction main(): i32 { state.width = 3.5; let w: f64 = state.width; if (w > 3.0) { return 1; } return 0; }\n",
        );
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
    fn aot_engine_bundle_manifest_includes_string_literals() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function tick(): void { print_string(\"hello\\n\"); return; }\nfunction render(): void { return; }\nfunction on_code_swap(): void { return; }\n",
        );
        process.compile().expect("compile");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let bundle_dir = std::env::temp_dir().join(format!("stasis_aot_bundle_literals_{stamp}"));
        let bundle = process
            .write_engine_bundle(&EngineEntrypoints::runtime_default(), &bundle_dir)
            .expect("write bundle");

        let manifest = fs::read_to_string(&bundle.manifest_path).expect("read manifest");
        let literal_id = crate::backend::emit::hash_string_literal("hello\n");
        assert!(
            manifest.contains("\"string_literals\""),
            "manifest should include string_literals field"
        );
        assert!(
            manifest.contains(&format!("\"id\":{literal_id}")),
            "manifest should include expected literal id"
        );
        assert!(
            manifest.contains("\"value\":\"hello\\n\""),
            "manifest should include escaped literal value"
        );

        let _ = fs::remove_dir_all(&bundle_dir);
    }

    #[test]
    fn aot_engine_bundle_manifest_includes_collection_max_lengths() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global values: i32[12];\nfunction tick(): void { return; }\nfunction render(): void { return; }\nfunction on_code_swap(): void { return; }\n",
        );
        process.compile().expect("compile");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let bundle_dir =
            std::env::temp_dir().join(format!("stasis_aot_bundle_collections_{stamp}"));
        let bundle = process
            .write_engine_bundle(&EngineEntrypoints::runtime_default(), &bundle_dir)
            .expect("write bundle");

        let manifest = fs::read_to_string(&bundle.manifest_path).expect("read manifest");
        assert!(
            manifest.contains("\"collection_max_lengths\""),
            "manifest should include collection_max_lengths field"
        );
        assert!(
            manifest.contains("\"path\":\"values\"") && manifest.contains("\"max_length\":12"),
            "manifest should include seeded max_length row"
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
    fn aot_process_links_and_executes_internal_i32_call() {
        let Some(link_config) = resolve_link_config_for_smoke() else {
            return;
        };

        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 9; }\nfunction main(): i32 { return helper() + 1; }\n",
        );
        process.compile().expect("compile");
        assert_eq!(
            process.artifacts().len(),
            2,
            "expected both functions emitted"
        );

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_exe_smoke_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let exe_path = temp_root.join("main_smoke_call.exe");
        let link_result =
            process.link_executable_for_i32_noarg_function("main", &exe_path, &link_config);
        if let Err(ref message) = link_result {
            if message.contains("undefined symbol") {
                eprintln!(
                    "skipping AOT internal-call smoke: runtime symbols not available at link time"
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
            Some(10),
            "expected executable to return exit code 10"
        );
        let _ = fs::remove_dir_all(&temp_root);
    }

    #[cfg(windows)]
    #[test]
    fn aot_process_links_and_executes_internal_void_call_statement() {
        let Some(link_config) = resolve_link_config_for_smoke() else {
            return;
        };

        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(): void { return; }\nfunction main(): i32 { helper(); return 7; }\n",
        );
        process.compile().expect("compile");
        assert_eq!(
            process.artifacts().len(),
            2,
            "expected both functions emitted"
        );

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_exe_smoke_voidcall_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let exe_path = temp_root.join("main_smoke_voidcall.exe");
        let link_result =
            process.link_executable_for_i32_noarg_function("main", &exe_path, &link_config);
        if let Err(ref message) = link_result {
            if message.contains("undefined symbol") {
                eprintln!(
                    "skipping AOT internal-void-call smoke: runtime symbols not available at link time"
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
            Some(7),
            "expected executable to return exit code 7"
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

    #[test]
    #[ignore]
    fn dump_perf_balls_bricks_tick_clif_summary() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let source_path = repo_root.join("samples").join("perf_balls_bricks.stasis");
        assert!(
            source_path.exists(),
            "expected perf sample at {}",
            source_path.display()
        );

        let source_text = fs::read_to_string(&source_path).expect("read perf sample");
        let mut process = AotProcess::new();
        process.upsert_file(source_path.display().to_string(), source_text);

        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured_hook = Arc::clone(&captured);
        set_clif_dump_hook(Some(Box::new(move |meta, func| {
            if meta.name == "tick" {
                *captured_hook.lock().expect("lock clif capture") =
                    Some(format!("{}", func.display()));
            }
        })));

        let report = process.compile().expect("compile perf sample");
        assert!(
            report.emit.emitted_functions > 0,
            "expected functions emitted"
        );

        set_clif_dump_hook(None);
        let clif = captured
            .lock()
            .expect("lock clif capture")
            .take()
            .expect("expected tick clif capture");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis();
        let out_path =
            std::env::temp_dir().join(format!("stasis_perf_balls_bricks_tick_{stamp}.clif"));
        fs::write(&out_path, &clif).expect("write clif dump");

        let call_lines = clif.lines().filter(|line| line.contains("call ")).count();
        let call_value_lines = clif.lines().filter(|line| line.contains("= call ")).count();
        let call_void_lines = clif
            .lines()
            .filter(|line| line.trim_start().starts_with("call "))
            .count();
        let load_lines = clif.lines().filter(|line| line.contains("load")).count();
        let store_lines = clif.lines().filter(|line| line.contains("store")).count();

        println!(
            "clif_dump=perf_balls_bricks tick_clif_path={} bytes={} lines={} call_lines={} call_value_lines={} call_void_lines={} load_lines={} store_lines={}",
            out_path.display(),
            clif.len(),
            clif.lines().count(),
            call_lines,
            call_value_lines,
            call_void_lines,
            load_lines,
            store_lines
        );
    }

    #[cfg(windows)]
    fn resolve_link_config_for_smoke() -> Option<stasis_jit::AotLinkConfig> {
        if let Some(explicit) = std::env::var_os("STASIS_AOT_LINKER") {
            let explicit = PathBuf::from(explicit);
            return Some(stasis_jit::AotLinkConfig {
                linker_path: Some(explicit),
                runtime_lib_paths: vec![],
            });
        }
        for candidate in ["lld-link.exe", "link.exe"] {
            let output = Command::new("where").arg(candidate).output().ok()?;
            if output.status.success() {
                return Some(stasis_jit::AotLinkConfig {
                    linker_path: Some(PathBuf::from(candidate)),
                    runtime_lib_paths: vec![],
                });
            }
        }
        eprintln!("skipping AOT executable smoke test: no Windows linker found");
        None
    }
}
