use crate::backend::emit::*;
use crate::backend::{AotOptimizationProfile, EngineEntrypoints};
use crate::compiler::{CompileReport, CompileResult, Compiler, FunctionId, FunctionMeta};
use crate::frontend::types::{TypeCategory, TypeTable, TYPE_ID_I32};
use crate::ir::hir::FunctionHIR;
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_module::{default_libcall_names, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
#[cfg(test)]
use std::sync::{Mutex, OnceLock};
use target_lexicon::Triple;

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
    target: stasis_jit::AotTarget,
    next_object_index: u32,
    artifacts: Vec<AotArtifact>,
    object_bytes: Vec<Vec<u8>>,
    string_literals: BTreeMap<i32, String>,
    collection_max_lengths: BTreeMap<String, i32>,
    compile_analysis_cache: Option<CompileAnalysisCache>,
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
            target: stasis_jit::AotTarget::default(),
            next_object_index: 0,
            artifacts: Vec::new(),
            object_bytes: Vec::new(),
            string_literals: BTreeMap::new(),
            collection_max_lengths: BTreeMap::new(),
            compile_analysis_cache: None,
            required_emit_roots: Vec::new(),
        }
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.compiler.upsert_file(path, content);
    }

    pub fn set_target(&mut self, target: stasis_jit::AotTarget) {
        self.target = target;
    }

    pub fn set_required_emit_roots(&mut self, roots: &[String]) {
        self.required_emit_roots.clear();
        self.required_emit_roots.extend_from_slice(roots);
    }

    pub fn compile(&mut self) -> CompileResult<CompileReport> {
        self.load_import_graph_sources()
            .map_err(crate::compiler::CompileError::Backend)?;
        let index = self.compiler.index_pass()?;
        self.compiler
            .types_mut()
            .ensure_utf8_view_id()
            .map_err(crate::compiler::CompileError::Backend)?;
        self.compiler
            .types_mut()
            .ensure_ascii_view_id()
            .map_err(crate::compiler::CompileError::Backend)?;
        let mut analysis_type_table = self.compiler.types().clone();
        let files_fingerprint = compute_files_fingerprint(self.compiler.files());
        let cache_miss = self
            .compile_analysis_cache
            .as_ref()
            .is_none_or(|cache| cache.files_fingerprint != files_fingerprint);
        let mut force_reemit_reachable = false;
        if cache_miss {
            let next_cache = build_compile_analysis_cache(
                self.compiler.files(),
                self.compiler.functions(),
                &mut analysis_type_table,
                files_fingerprint,
                resolve_preferred_extern_call_signatures,
            )
            .map_err(crate::compiler::CompileError::Backend)?;
            if let Some(previous_cache) = self.compile_analysis_cache.as_ref() {
                force_reemit_reachable =
                    compile_analysis_requires_reemit(previous_cache, &next_cache);
            }
            self.compile_analysis_cache = Some(next_cache);
        }
        *self.compiler.types_mut() = analysis_type_table.clone();
        let analysis = self.compile_analysis_cache.as_ref().ok_or_else(|| {
            crate::compiler::CompileError::Invariant(
                "aot compile analysis cache missing after refresh".to_string(),
            )
        })?;
        self.string_literals.clear();
        for constant in analysis.constant_values.values() {
            if let ConstantValue::String { value, .. } = constant {
                record_string_literal(&mut self.string_literals, value)
                    .map_err(crate::compiler::CompileError::Backend)?;
            }
        }
        self.collection_max_lengths =
            collect_fixed_collection_max_lengths(&analysis.global_path_types, &analysis_type_table)
                .map_err(crate::compiler::CompileError::Backend)?;
        let compiled_body_hashes: HashMap<FunctionId, u64> = self
            .artifacts
            .iter()
            .map(|artifact| (artifact.function_id, artifact.body_hash))
            .collect();
        let emit_function_ids = select_emit_function_ids(
            self.compiler.functions(),
            &self.required_emit_roots,
            &compiled_body_hashes,
            force_reemit_reachable,
        );

        let (
            compiler,
            next_object_index,
            artifacts,
            object_bytes,
            optimization_profile,
            string_literals,
            target,
        ) = (
            &mut self.compiler,
            &mut self.next_object_index,
            &mut self.artifacts,
            &mut self.object_bytes,
            self.optimization_profile,
            &mut self.string_literals,
            self.target.clone(),
        );
        let emit = compiler.emit_pass_for_ids_with(
            &emit_function_ids,
            &mut |meta, hir, lowered_types| {
                // Stable per-function symbols are required so AOT objects can reference each other
                // directly without forcing recompilation of callers on every body change.
                let symbol = format!("aot_fn_{}", meta.id);
                let mut type_table = lowered_types.clone();
                type_table.ensure_utf8_view_id()?;
                type_table.ensure_ascii_view_id()?;
                let bytes = compile_function_to_object_bytes(
                    meta,
                    hir,
                    &symbol,
                    optimization_profile,
                    &analysis.call_signatures,
                    &mut type_table,
                    &analysis.global_path_types,
                    &analysis.constant_values,
                    string_literals,
                    &target,
                    &analysis.collection_infos,
                    &analysis.named_struct_field_types,
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
            },
        )?;

        let reachable = crate::backend::reachability::compute_reachable_function_ids(
            self.compiler.functions(),
            &self.required_emit_roots,
        );
        artifacts.retain(|artifact| reachable.contains(&artifact.function_id));
        compact_active_artifact_storage(artifacts, object_bytes);
        self.next_object_index = u32::try_from(self.object_bytes.len()).unwrap_or(u32::MAX);
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

fn compact_active_artifact_storage(artifacts: &mut [AotArtifact], object_bytes: &mut Vec<Vec<u8>>) {
    let mut remapped_indices: BTreeMap<u32, u32> = BTreeMap::new();
    let mut compacted: Vec<Vec<u8>> = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let original_index = artifact.object_index;
        let next_index = if let Some(index) = remapped_indices.get(&original_index).copied() {
            index
        } else {
            let Some(bytes) = object_bytes.get(original_index as usize).cloned() else {
                continue;
            };
            let index = u32::try_from(compacted.len()).unwrap_or(u32::MAX);
            compacted.push(bytes);
            remapped_indices.insert(original_index, index);
            index
        };
        artifact.object_index = next_index;
    }
    *object_bytes = compacted;
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
    target: &stasis_jit::AotTarget,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> Result<Vec<u8>, String> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", optimization_profile.as_cranelift_opt_level())
        .map_err(|error| format!("failed to configure Cranelift opt level: {error}"))?;
    if target.requires_position_independent_code() {
        flag_builder
            .set("is_pic", "true")
            .map_err(|error| format!("failed to configure position-independent AOT: {error}"))?;
    }
    let flags = settings::Flags::new(flag_builder);
    let isa_builder = match target.object_triple() {
        Some(triple_text) => {
            let triple = Triple::from_str(triple_text).map_err(|error| {
                format!("failed to parse AOT target triple {triple_text}: {error}")
            })?;
            let triple_display = triple.to_string();
            cranelift_codegen::isa::lookup(triple).map_err(|error| {
                format!("failed to construct ISA builder for {triple_display}: {error}")
            })?
        }
        None => cranelift_native::builder()
            .map_err(|error| format!("failed to construct native ISA builder: {error}"))?,
    };
    let isa = isa_builder
        .finish(flags)
        .map_err(|error| format!("failed to finalize native ISA: {error}"))?;

    let builder = ObjectBuilder::new(
        isa,
        "stasis_aot_module".to_string(),
        default_libcall_names(),
    )
    .map_err(|error| format!("failed to construct object builder: {error}"))?;
    compile_function_with_module(
        ObjectModule::new(builder),
        meta,
        hir,
        symbol,
        RuntimeHelperLinkage::Imported,
        SharedCompileBackendMode::AotDirect,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        |statement| record_string_literals_in_stmt(statement, string_literals),
        |_meta, _func| {
            #[cfg(test)]
            maybe_invoke_clif_dump_hook(_meta, _func);
        },
        |mut module, function_id, mut context| {
            module
                .define_function(function_id, &mut context)
                .map_err(|error| format!("failed to define AOT function {symbol}: {error}"))?;
            module.clear_context(&mut context);
            module
                .finish()
                .emit()
                .map_err(|error| format!("failed to emit AOT object bytes: {error}"))
        },
    )
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
    use crate::backend::jit::JitProcess;
    use crate::backend::EngineEntrypoints;
    use object::{Architecture, BinaryFormat, File, Object, ObjectSection, RelocationKind};
    #[cfg(windows)]
    use std::process::Command;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct ParityCorpusCase {
        label: &'static str,
        source: &'static str,
        expected_exit: i32,
        expected_extern_symbols: &'static [(&'static str, &'static str)],
        expected_string_literals: &'static [&'static str],
        expected_collection_max_lengths: &'static [(&'static str, i32)],
        expected_clif_markers: &'static [(&'static str, &'static [&'static str])],
    }

    #[cfg(windows)]
    fn run_linked_i32_noarg_fixture(
        process: &AotProcess,
        function_name: &str,
        label: &str,
        link_config: &stasis_jit::AotLinkConfig,
    ) -> Option<i32> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_fixture_{label}_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let exe_path = temp_root.join(format!("{function_name}_{label}.exe"));
        let link_result =
            process.link_executable_for_i32_noarg_function(function_name, &exe_path, link_config);
        if let Err(ref message) = link_result {
            if message.contains("undefined symbol") {
                eprintln!(
                    "skipping AOT parity fixture '{label}': runtime symbols not available at link time"
                );
                let _ = fs::remove_dir_all(&temp_root);
                return None;
            }
        }
        link_result.expect("link executable");

        let status = Command::new(&exe_path)
            .status()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", exe_path.display()));
        let code = status.code().expect("expected process exit code");
        let _ = fs::remove_dir_all(&temp_root);
        Some(code)
    }

    fn capture_aot_clif_by_function(process: &mut AotProcess) -> BTreeMap<String, String> {
        let captured: Arc<Mutex<BTreeMap<String, String>>> = Arc::new(Mutex::new(BTreeMap::new()));
        let captured_hook = Arc::clone(&captured);
        set_clif_dump_hook(Some(Box::new(move |meta, func| {
            captured_hook
                .lock()
                .expect("lock clif capture")
                .insert(meta.name.clone(), format!("{}", func.display()));
        })));
        let compile_result = process.compile();
        set_clif_dump_hook(None);
        compile_result.expect("aot compile");
        let captured = captured.lock().expect("lock clif capture").clone();
        captured
    }

    fn parity_corpus_cases() -> &'static [ParityCorpusCase] {
        &[
            ParityCorpusCase {
                label: "extern_and_string_literal",
                source: "extern function print_string(value: string): void;\nfunction main(): i32 { print_string(\"alpha; beta {x}\"); return 1; }\n",
                expected_exit: 1,
                expected_extern_symbols: &[("print_string", "stasis_jit_print_string")],
                expected_string_literals: &["alpha; beta {x}"],
                expected_collection_max_lengths: &[],
                expected_clif_markers: &[("main", &["call"])],
            },
            ParityCorpusCase {
                label: "globals_and_collection_view",
                source: "const COUNT: i32 = 3;\nglobal nums: i32[COUNT];\nfunction main(): i32 {\n    nums[1] = 9;\n    nums[2] = 11;\n    return nums[1] + nums[2];\n}\n",
                expected_exit: 20,
                expected_extern_symbols: &[],
                expected_string_literals: &[],
                expected_collection_max_lengths: &[("nums", 3)],
                expected_clif_markers: &[("main", &["call", "iadd"])],
            },
            ParityCorpusCase {
                label: "control_flow_branching",
                source: "function main(): i32 {\n    let sum: i32 = 0;\n    for (let i: i32 = 0; i < 3; i += 1) {\n        sum += i;\n    }\n    if (sum == 3) {\n        return 1;\n    }\n    return 0;\n}\n",
                expected_exit: 1,
                expected_extern_symbols: &[],
                expected_string_literals: &[],
                expected_collection_max_lengths: &[],
                expected_clif_markers: &[("main", &["brif", "jump"])],
            },
            ParityCorpusCase {
                label: "struct_view_abi",
                source: "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal enemies: Enemy[COUNT];\nfunction mutate(arr: Enemy[], idx: i32): i32 {\n    arr[idx].hp = 10;\n    arr[idx + 1].hp = arr[idx].hp + 4;\n    return arr[idx + 1].hp;\n}\nfunction main(): i32 { return mutate(enemies, 0); }\n",
                expected_exit: 14,
                expected_extern_symbols: &[],
                expected_string_literals: &[],
                expected_collection_max_lengths: &[("enemies", 3)],
                expected_clif_markers: &[
                    ("main", &["call"]),
                    ("mutate", &["call fn39", "call fn38", "iadd"]),
                ],
            },
        ]
    }

    fn run_parity_corpus_case(case: &ParityCorpusCase) {
        let mut jit = JitProcess::new();
        jit.upsert_file("sample.stasis", case.source);
        jit.compile()
            .unwrap_or_else(|error| panic!("jit compile {}: {:?}", case.label, error));
        let jit_result = jit
            .execute_i32_noarg_by_name("main")
            .unwrap_or_else(|error| panic!("jit execute {}: {error}", case.label));
        assert_eq!(
            jit_result, case.expected_exit,
            "unexpected JIT result for parity fixture '{}'",
            case.label
        );

        let mut aot = AotProcess::new();
        aot.upsert_file("sample.stasis", case.source);
        let captured_clif = capture_aot_clif_by_function(&mut aot);

        let analysis = aot
            .compile_analysis_cache
            .as_ref()
            .expect("compile analysis cache");
        let resolved_externs: BTreeMap<_, _> = analysis
            .resolved_extern_signatures
            .iter()
            .map(|signature| (signature.name.as_str(), signature.symbol.as_str()))
            .collect();
        for (name, expected_symbol) in case.expected_extern_symbols {
            assert_eq!(
                resolved_externs.get(name).copied(),
                Some(*expected_symbol),
                "unexpected extern symbol mapping for fixture '{}'",
                case.label
            );
        }

        for expected_literal in case.expected_string_literals {
            assert!(
                aot.string_literals()
                    .values()
                    .any(|value| value == expected_literal),
                "missing string literal {:?} in fixture '{}'",
                expected_literal,
                case.label
            );
        }

        for (path, expected_max_length) in case.expected_collection_max_lengths {
            assert_eq!(
                aot.collection_max_lengths().get(*path).copied(),
                Some(*expected_max_length),
                "unexpected collection max_length for fixture '{}'",
                case.label
            );
        }

        for (function_name, markers) in case.expected_clif_markers {
            let clif = captured_clif.get(*function_name).unwrap_or_else(|| {
                panic!("missing captured CLIF for function '{}'", function_name)
            });
            for marker in *markers {
                assert!(
                    clif.contains(marker),
                    "missing CLIF marker '{}' in fixture '{}' function '{}':\n{}",
                    marker,
                    case.label,
                    function_name,
                    clif
                );
            }
        }

        #[cfg(windows)]
        {
            let Some(link_config) = resolve_link_config_for_smoke() else {
                return;
            };
            let Some(aot_result) =
                run_linked_i32_noarg_fixture(&aot, "main", case.label, &link_config)
            else {
                return;
            };
            assert_eq!(
                aot_result, jit_result,
                "AOT/JIT mismatch for parity fixture '{}'",
                case.label
            );
        }
    }

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
    fn aot_process_supports_local_annotation_only_type_during_emit() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let token: local_only = 7; return token; }\n",
        );
        let report = process.compile().expect("aot compile");
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
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
    fn aot_process_compacts_retained_object_bytes_after_recompile() {
        let mut process = AotProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return 1; }\n");
        process.compile().expect("first compile");
        assert_eq!(process.object_bytes.len(), 1);
        assert_eq!(process.artifacts().len(), 1);
        assert_eq!(process.artifacts()[0].object_index, 0);

        process.upsert_file("sample.stasis", "function main(): i32 { return 9; }\n");
        process.compile().expect("second compile");
        assert_eq!(process.object_bytes.len(), 1);
        assert_eq!(process.artifacts().len(), 1);
        assert_eq!(process.artifacts()[0].object_index, 0);
    }

    #[test]
    fn aot_process_rebuilds_string_literal_table_each_compile() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { print_string(\"first\"); return 0; }\n",
        );
        process.compile().expect("first compile");
        assert_eq!(process.string_literals().len(), 1);

        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { print_string(\"second\"); return 0; }\n",
        );
        process.compile().expect("second compile");
        assert_eq!(process.string_literals().len(), 1);
        assert!(process
            .string_literals()
            .values()
            .any(|value| value == "second"));
    }

    #[test]
    fn aot_process_prefers_known_runtime_extern_symbol_over_source_alias() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "extern function print_i32(value: i32): void;\nfunction main(): i32 { print_i32(7); return 0; }\n",
        );

        process.compile().expect("compile");
        let analysis = process
            .compile_analysis_cache
            .as_ref()
            .expect("compile analysis cache");
        assert_eq!(analysis.resolved_extern_signatures.len(), 1);
        assert_eq!(
            analysis.resolved_extern_signatures[0].symbol,
            "stasis_jit_print_i32"
        );
    }

    #[test]
    fn aot_process_accepts_explicit_single_symbol_extern() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function @extern(\"custom_symbol\") custom(value: i32): i32;\nfunction main(): i32 { return custom(7); }\n",
        );

        process.compile().expect("compile");
        let analysis = process
            .compile_analysis_cache
            .as_ref()
            .expect("compile analysis cache");
        assert_eq!(analysis.resolved_extern_signatures.len(), 1);
        assert_eq!(
            analysis.resolved_extern_signatures[0].symbol,
            "custom_symbol"
        );
    }

    #[test]
    fn aot_process_accepts_known_runtime_shim_families() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "extern function sleep_ms(ms: i32): void;\nextern function audio_init(sample_rate: i32, channels: i32, target_latency_frames: i32): bool;\nfunction main(): i32 { sleep_ms(1); if (audio_init(48000, 2, 512)) { return 1; } return 0; }\n",
        );

        process.compile().expect("compile");
        let analysis = process
            .compile_analysis_cache
            .as_ref()
            .expect("compile analysis cache");
        assert_eq!(analysis.resolved_extern_signatures.len(), 2);
        assert_eq!(
            analysis.resolved_extern_signatures[0].symbol,
            "stasis_jit_sleep_ms"
        );
        assert_eq!(
            analysis.resolved_extern_signatures[1].symbol,
            "stasis_jit_audio_init"
        );
    }

    #[test]
    fn aot_process_rejects_nonexplicit_unknown_extern_without_known_runtime_symbol() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "extern function totally_unknown(value: i32): i32;\nfunction main(): i32 { return totally_unknown(7); }\n",
        );

        let error = process.compile().expect_err("compile should fail");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("unresolved extern call target 'totally_unknown'"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn aot_process_rejects_fake_runtime_prefix_extern_without_export_contract_entry() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "extern function gfx_totally_missing(path: string, max_w: i32, max_h: i32): i32;\nfunction main(): i32 { return gfx_totally_missing(\"sprite.bmp\", 8, 8); }\n",
        );

        let error = process.compile().expect_err("compile should fail");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("unresolved extern call target 'gfx_totally_missing'"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn aot_process_reemits_reachable_functions_when_imported_constant_changes() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "main.stasis",
            "import \"constants.stasis\";\nfunction main(): i32 { return VALUE; }\n",
        );
        process.upsert_file("constants.stasis", "const VALUE: i32 = 11;\n");

        let first = process.compile().expect("first compile");
        assert_eq!(first.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);

        process.upsert_file("constants.stasis", "const VALUE: i32 = 27;\n");
        let second = process.compile().expect("second compile");
        assert_eq!(
            second.emit.emitted_functions, 1,
            "main should be re-emitted when imported constants change"
        );
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
    fn aot_process_emits_android_arm64_elf_objects_when_target_is_configured() {
        let mut process = AotProcess::new();
        process.set_target(stasis_jit::AotTarget::android_arm64_default());
        process.upsert_file(
            "sample.stasis",
            "global State { value: i32; }\nfunction helper(): i32 { return State.value; }\nfunction main(): i32 { State.value = 7; return helper(); }\n",
        );
        process.compile().expect("android aot compile");

        let mut relocation_count = 0;
        for bytes in &process.object_bytes {
            let object = File::parse(bytes.as_slice()).expect("parse emitted object");
            assert_eq!(object.format(), BinaryFormat::Elf);
            assert_eq!(object.architecture(), Architecture::Aarch64);
            for section in object.sections() {
                for (_, relocation) in section.relocations() {
                    relocation_count += 1;
                    assert_ne!(
                        relocation.kind(),
                        RelocationKind::Absolute,
                        "Android AOT objects must be position independent"
                    );
                }
            }
        }
        assert!(
            relocation_count > 0,
            "fixture should exercise AOT relocations"
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
    fn aot_process_prefers_runtime_string_shims_for_asset_externs() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "extern function gfx_load_sprite(path: string, max_w: i32, max_h: i32): i32;\nextern function gfx_release_sprite(handle: i32): void;\nextern function load_font(path: string, size: i32): i32;\nextern function measure_text(font: i32, text: string): f32;\nfunction @extern(\"stasis_gfx_cache_text\") gfx_cache_text(font: i32, text: string): i32;\nfunction main(): i32 { gfx_release_sprite(0); return 0; }\n",
        );
        process.compile().expect("compile");

        let analysis = process
            .compile_analysis_cache
            .as_ref()
            .expect("compile analysis cache");
        let resolved: BTreeMap<_, _> = analysis
            .resolved_extern_signatures
            .iter()
            .map(|signature| (signature.name.as_str(), signature.symbol.as_str()))
            .collect();

        assert_eq!(
            resolved.get("gfx_load_sprite").copied(),
            Some("stasis_jit_gfx_load_sprite")
        );
        assert_eq!(
            resolved.get("gfx_release_sprite").copied(),
            Some("stasis_jit_gfx_release_sprite")
        );
        assert_eq!(
            resolved.get("load_font").copied(),
            Some("stasis_jit_load_font")
        );
        assert_eq!(
            resolved.get("measure_text").copied(),
            Some("stasis_jit_measure_text")
        );
        assert_eq!(
            resolved.get("gfx_cache_text").copied(),
            Some("stasis_gfx_cache_text")
        );
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
    fn aot_and_jit_match_internal_call_fixture_results() {
        let Some(link_config) = resolve_link_config_for_smoke() else {
            return;
        };

        let cases = [
            (
                "value_call",
                "function helper(): i32 { return 9; }\nfunction main(): i32 { return helper() + 1; }\n",
            ),
            (
                "void_call_statement",
                "function helper(): void { return; }\nfunction main(): i32 { helper(); return 7; }\n",
            ),
        ];

        for (label, source) in cases {
            let mut jit = JitProcess::new();
            jit.upsert_file("sample.stasis", source);
            jit.compile().expect("jit compile");
            let jit_result = jit
                .execute_i32_noarg_by_name("main")
                .unwrap_or_else(|error| panic!("jit execute {label}: {error}"));

            let mut aot = AotProcess::new();
            aot.upsert_file("sample.stasis", source);
            aot.compile().expect("aot compile");
            let Some(aot_result) = run_linked_i32_noarg_fixture(&aot, "main", label, &link_config)
            else {
                return;
            };

            assert_eq!(
                aot_result, jit_result,
                "AOT/JIT mismatch for internal call fixture '{label}'"
            );
        }
    }

    #[test]
    fn parity_corpus_covers_shared_lowering_shapes() {
        for case in parity_corpus_cases() {
            run_parity_corpus_case(case);
        }
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
                target: stasis_jit::AotTarget::default(),
            });
        }
        for candidate in ["lld-link.exe", "link.exe"] {
            let output = Command::new("where").arg(candidate).output().ok()?;
            if output.status.success() {
                return Some(stasis_jit::AotLinkConfig {
                    linker_path: Some(PathBuf::from(candidate)),
                    runtime_lib_paths: vec![],
                    target: stasis_jit::AotTarget::default(),
                });
            }
        }
        eprintln!("skipping AOT executable smoke test: no Windows linker found");
        None
    }
}
