use crate::backend::emit::*;
use crate::backend::program_snapshot::{ProgramArtifactMapping, ProgramFunction, ProgramSnapshot};
use crate::backend::state_layout::{is_named_scalar_state_path, StateLayout};
use crate::backend::{AotOptimizationProfile, EngineEntrypoints};
use crate::compiler::{CompileReport, CompileResult, Compiler, FunctionId, FunctionMeta};
use crate::frontend::types::{
    TypeCategory, TypeId, TypeTable, TYPE_ID_F32, TYPE_ID_F64, TYPE_ID_I32,
};
use crate::identity::SymbolId;
use crate::ir::hir::FunctionHIR;
use cranelift_codegen::ir::{types, AbiParam, InstBuilder};
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{default_libcall_names, DataDescription, DataId, Linkage, Module};
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
    pub symbol_id: SymbolId,
    pub object_index: u32,
    pub body_hash: u64,
    pub symbol_name: String,
    pub object_bytes_len: usize,
}

#[derive(Debug, Clone, Default)]
pub struct AotProcess {
    compiler: Compiler,
    optimization_profile: AotOptimizationProfile,
    target: stasis_jit::AotTarget,
    next_object_index: u32,
    artifacts: Vec<AotArtifact>,
    object_bytes: Vec<Vec<u8>>,
    string_literals: BTreeMap<i32, String>,
    collection_max_lengths: BTreeMap<String, i32>,
    program_snapshot: Option<ProgramSnapshot>,
    last_failed_source_diagnostic: Option<crate::SourceDiagnostic>,
    required_emit_roots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AotEngineBundle {
    pub output_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub object_paths_by_function: BTreeMap<String, PathBuf>,
    pub object_paths_by_function_id: BTreeMap<FunctionId, PathBuf>,
    pub optimization_profile: AotOptimizationProfile,
}

impl AotEngineBundle {
    /// Enumerates every emitted object by its canonical function identity.
    ///
    /// `object_paths_by_function` is only a convenience lookup for unique source
    /// names. Ambiguous overload names are intentionally absent from that map, so
    /// whole-bundle link and package paths must enumerate this FnId-backed map.
    pub fn object_paths(&self) -> impl ExactSizeIterator<Item = &PathBuf> {
        self.object_paths_by_function_id.values()
    }
}

impl AotProcess {
    pub fn new() -> Self {
        Self::with_optimization_profile(AotOptimizationProfile::Speed)
    }

    pub fn set_project_root(&mut self, root: impl Into<String>) -> Result<(), String> {
        self.compiler.set_project_root(root)
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
            program_snapshot: None,
            last_failed_source_diagnostic: None,
            required_emit_roots: Vec::new(),
        }
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.compiler.upsert_file(path, content);
    }

    pub fn set_import_base_dir(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        let path = fs::canonicalize(&path).unwrap_or(path);
        let _ = self
            .compiler
            .set_project_root(path.to_string_lossy().to_string());
    }

    pub fn set_target(&mut self, target: stasis_jit::AotTarget) {
        self.target = target;
    }

    pub fn set_required_emit_roots(&mut self, roots: &[String]) {
        self.required_emit_roots.clear();
        self.required_emit_roots.extend_from_slice(roots);
        self.compiler.set_analysis_required_roots(roots);
    }

    pub fn compile(&mut self) -> CompileResult<CompileReport> {
        // Keep accepted object buffers in place.  A full `self.clone()` duplicates every
        // object blob before we know whether a candidate will be rejected.
        let accepted_compiler = self.compiler.clone();
        let accepted_next_object_index = self.next_object_index;
        let accepted_artifacts = self.artifacts.clone();
        let accepted_object_bytes_len = self.object_bytes.len();
        let accepted_string_literals = self.string_literals.clone();
        let accepted_collection_max_lengths = self.collection_max_lengths.clone();
        let accepted_program_snapshot = self.program_snapshot.clone();
        match self.compile_internal() {
            Ok(report) => {
                self.last_failed_source_diagnostic = None;
                Ok(report)
            }
            Err(error) => {
                let diagnostic = self.compiler.last_source_diagnostic().cloned();
                self.compiler = accepted_compiler;
                self.next_object_index = accepted_next_object_index;
                self.artifacts = accepted_artifacts;
                self.object_bytes.truncate(accepted_object_bytes_len);
                self.string_literals = accepted_string_literals;
                self.collection_max_lengths = accepted_collection_max_lengths;
                self.program_snapshot = accepted_program_snapshot;
                self.last_failed_source_diagnostic = diagnostic;
                Err(error)
            }
        }
    }

    fn compile_internal(&mut self) -> CompileResult<CompileReport> {
        let index = self.compiler.index_pass()?;
        self.validate_host_aliases()
            .map_err(crate::compiler::CompileError::Backend)?;
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
        let snapshot_revision =
            crate::backend::program_snapshot::semantic_revision_with_required_roots(
                files_fingerprint,
                &self.required_emit_roots,
            );
        let snapshot_miss = self
            .program_snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.source_revision() != snapshot_revision);
        let mut force_reemit_reachable = false;
        if snapshot_miss {
            let next_cache = build_compile_analysis_cache(
                self.compiler.files(),
                self.compiler.functions(),
                &mut analysis_type_table,
                snapshot_revision,
                resolve_preferred_extern_call_signatures,
            )
            .map_err(crate::compiler::CompileError::Backend)?;
            if let Some(previous_snapshot) = self.program_snapshot.as_ref() {
                force_reemit_reachable =
                    compile_analysis_requires_reemit(&previous_snapshot.analysis, &next_cache);
            }
            self.program_snapshot = Some(
                ProgramSnapshot::build(
                    snapshot_revision,
                    self.compiler.files(),
                    self.compiler.module_graph(),
                    self.compiler.functions(),
                    &analysis_type_table,
                    self.compiler.data_flow_summaries_shared(),
                    &self.required_emit_roots,
                    next_cache,
                )
                .map_err(crate::compiler::CompileError::Backend)?,
            );
        }
        *self.compiler.types_mut() = analysis_type_table.clone();
        let snapshot = self.program_snapshot.as_ref().ok_or_else(|| {
            crate::compiler::CompileError::Invariant(
                "aot program snapshot missing after refresh".to_string(),
            )
        })?;
        let analysis = &snapshot.analysis;
        self.string_literals = snapshot.literal_table().clone();
        self.collection_max_lengths =
            collect_fixed_collection_max_lengths(&analysis.global_path_types, &analysis_type_table)
                .map_err(crate::compiler::CompileError::Backend)?;
        let direct_storage = build_aot_direct_storage_bindings(
            &analysis.global_path_types,
            &analysis.collection_infos,
            &analysis_type_table,
        )
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
                    &direct_storage,
                )?;
                let object_index = *next_object_index;
                *next_object_index = next_object_index.saturating_add(1);
                object_bytes.push(bytes);
                let object_bytes_len = object_bytes.last().map_or(0usize, std::vec::Vec::len);
                artifacts.retain(|artifact| artifact.function_id != meta.id);
                artifacts.push(AotArtifact {
                    function_id: meta.id,
                    symbol_id: meta.symbol_id.clone(),
                    object_index,
                    body_hash: meta.body_hash,
                    symbol_name: symbol,
                    object_bytes_len,
                });
                Ok(())
            },
        )?;

        let reachable = snapshot.reachable_function_ids().clone();
        artifacts.retain(|artifact| reachable.contains(&artifact.function_id));
        compact_active_artifact_storage(artifacts, object_bytes);
        self.next_object_index = u32::try_from(self.object_bytes.len()).unwrap_or(u32::MAX);
        if let Some(snapshot) = self.program_snapshot.as_mut() {
            snapshot
                .set_artifact_mappings(self.artifacts.iter().map(|artifact| {
                    ProgramArtifactMapping {
                        function_id: artifact.function_id,
                        symbol_id: artifact.symbol_id.clone(),
                        symbol: artifact.symbol_name.clone(),
                        target_path: None,
                        code_pointer: None,
                    }
                }))
                .map_err(crate::compiler::CompileError::Invariant)?;
        }
        Ok(CompileReport { index, emit })
    }

    pub fn state_layout(&self) -> StateLayout {
        self.program_snapshot
            .as_ref()
            .map_or_else(StateLayout::default, |snapshot| {
                snapshot.state_layout().clone()
            })
    }

    pub fn program_snapshot(&self) -> Option<&ProgramSnapshot> {
        self.program_snapshot.as_ref()
    }

    pub fn last_source_diagnostic(&self) -> Option<&crate::SourceDiagnostic> {
        self.last_failed_source_diagnostic
            .as_ref()
            .or_else(|| self.compiler.last_source_diagnostic())
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
        let function = self.unique_function_by_name(name)?;
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
            let object_path = object_dir.join(format!(
                "{}_fn{}_{}.obj",
                sanitize_file_token(&artifact_function.name),
                artifact_function.id,
                artifact.object_index
            ));
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

        let mut link_entry = entry_artifact.symbol_name.clone();
        if let Some((storage_bytes, wrapper_symbol)) =
            self.compile_standalone_storage_object(&entry_artifact.symbol_name)?
        {
            let storage_path = object_dir.join("direct_storage.obj");
            fs::write(&storage_path, storage_bytes).map_err(|error| {
                format!(
                    "failed to write direct storage object {}: {error}",
                    storage_path.display()
                )
            })?;
            object_paths.push(storage_path);
            link_entry = wrapper_symbol;
        }

        stasis_jit::link_objects_to_executable(
            &object_paths,
            output_executable,
            &link_entry,
            link_config,
        )?;
        Ok(entry_object_path)
    }

    fn validate_host_aliases(&self) -> Result<(), String> {
        let required: BTreeSet<&str> = ["main", "tick", "render", "on_code_swap"]
            .into_iter()
            .chain(self.required_emit_roots.iter().map(String::as_str))
            .collect();
        for name in required {
            let count = self
                .compiler
                .functions()
                .iter()
                .filter(|function| function.name == name)
                .count();
            if count > 1 {
                return Err(format!(
                    "host ABI alias '{name}' requires exactly one canonical identity (found {count})"
                ));
            }
        }
        Ok(())
    }

    fn unique_function_by_name(&self, name: &str) -> Result<&ProgramFunction, String> {
        let mut matches = self
            .program_snapshot
            .as_ref()
            .ok_or_else(|| "program has not compiled successfully".to_string())?
            .functions()
            .iter()
            .filter(|function| function.name == name);
        let function = matches
            .next()
            .ok_or_else(|| format!("function '{name}' not found"))?;
        if matches.next().is_some() {
            return Err(format!("function alias '{name}' is ambiguous"));
        }
        Ok(function)
    }

    /// Builds the storage definitions and entry wrapper required when a
    /// standalone executable references program globals directly.
    pub fn compile_standalone_storage_object(
        &self,
        entry_symbol: &str,
    ) -> Result<Option<(Vec<u8>, String)>, String> {
        fn storage_width(type_name: &str) -> Result<usize, String> {
            match type_name {
                "u8" => Ok(1),
                "u16" => Ok(2),
                "bool" | "u32" | "i32" | "f32" => Ok(4),
                "f64" => Ok(8),
                other => Err(format!("unsupported standalone AOT storage type '{other}'")),
            }
        }

        let layout = self.state_layout();
        if layout.scalars.is_empty() && layout.collections.is_empty() {
            return Ok(None);
        }
        let mut flag_builder = settings::builder();
        flag_builder
            .set(
                "opt_level",
                self.optimization_profile.as_cranelift_opt_level(),
            )
            .map_err(|error| format!("failed to configure Cranelift opt level: {error}"))?;
        if self.target.requires_position_independent_code() {
            flag_builder.set("is_pic", "true").map_err(|error| {
                format!("failed to configure position-independent AOT: {error}")
            })?;
        }
        let flags = settings::Flags::new(flag_builder);
        let isa_builder = match self.target.object_triple() {
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
            "stasis_aot_standalone_storage".to_string(),
            default_libcall_names(),
        )
        .map_err(|error| format!("failed to construct storage object builder: {error}"))?;
        let mut module = ObjectModule::new(builder);
        let pointer_type = module.target_config().pointer_type();
        let mut registrations = Vec::new();

        for scalar in &layout.scalars {
            let storage_type_name = scalar.storage_type_name();
            let width = storage_width(storage_type_name)?;
            let mut bytes = vec![0; width];
            if let Some(collection_path) = scalar.path.strip_suffix(".max_length") {
                if let Some(collection) = layout
                    .collections
                    .iter()
                    .find(|collection| collection.path == collection_path)
                {
                    bytes[..4].copy_from_slice(&collection.capacity.to_ne_bytes());
                }
            }
            let symbol = aot_storage_symbol(&scalar.path, "");
            let data_id = define_standalone_storage_data(&mut module, &symbol, bytes)?;
            registrations.push((
                data_id,
                storage_type_name.to_string(),
                hash_global_path(&scalar.path),
                0,
                matches!(storage_type_name, "u8" | "u16").then_some(1),
            ));
        }
        for collection in &layout.collections {
            let len = usize::try_from(collection.capacity).map_err(|_| {
                format!(
                    "negative standalone AOT collection capacity for '{}'",
                    collection.path
                )
            })?;
            for field in &collection.fields {
                let storage_type_name = field.storage_type_name();
                let width = storage_width(storage_type_name)?;
                let size = len.checked_mul(width).ok_or_else(|| {
                    format!(
                        "standalone AOT storage size overflow for '{}.{}'",
                        collection.path, field.field
                    )
                })?;
                let symbol = aot_storage_symbol(&collection.path, &field.field);
                let data_id = define_standalone_storage_data(&mut module, &symbol, vec![0; size])?;
                registrations.push((
                    data_id,
                    storage_type_name.to_string(),
                    hash_global_path(&collection.path),
                    hash_foreach_field_suffix(&field.field),
                    Some(collection.capacity),
                ));
            }
        }

        let mut entry_signature = module.make_signature();
        entry_signature.returns.push(AbiParam::new(types::I32));
        let entry_id = module
            .declare_function(entry_symbol, Linkage::Import, &entry_signature)
            .map_err(|error| format!("failed to declare standalone entry: {error}"))?;
        let wrapper_symbol = "stasis_aot_standalone_entry".to_string();
        let wrapper_id = module
            .declare_function(&wrapper_symbol, Linkage::Export, &entry_signature)
            .map_err(|error| format!("failed to declare standalone wrapper: {error}"))?;

        let mut register_functions = BTreeMap::new();
        for (_, type_name, _, _, len) in &registrations {
            let lane = if type_name == "f32" {
                "f32"
            } else if type_name == "f64" {
                "f64"
            } else if type_name == "u8" {
                "u8"
            } else if type_name == "u16" {
                "u16"
            } else {
                "i32"
            };
            let key = (lane, len.is_some());
            if register_functions.contains_key(&key) {
                continue;
            }
            let symbol = if len.is_some() {
                format!("stasis_jit_register_global_{lane}_array")
            } else {
                format!("stasis_jit_register_global_{lane}_ptr")
            };
            let mut signature = module.make_signature();
            signature.params.push(AbiParam::new(types::I32));
            if len.is_some() {
                signature.params.push(AbiParam::new(types::I32));
            }
            signature.params.push(AbiParam::new(pointer_type));
            if len.is_some() {
                signature.params.push(AbiParam::new(types::I32));
            }
            let function_id = module
                .declare_function(&symbol, Linkage::Import, &signature)
                .map_err(|error| format!("failed to declare '{symbol}': {error}"))?;
            register_functions.insert(key, function_id);
        }

        let mut context = module.make_context();
        context.func.signature = entry_signature;
        let entry_ref = module.declare_func_in_func(entry_id, &mut context.func);
        let register_refs: BTreeMap<_, _> = register_functions
            .into_iter()
            .map(|(key, id)| (key, module.declare_func_in_func(id, &mut context.func)))
            .collect();
        let registration_refs: Vec<_> = registrations
            .into_iter()
            .map(|(data_id, type_name, path_hash, field_hash, len)| {
                (
                    module.declare_data_in_func(data_id, &mut context.func),
                    type_name,
                    path_hash,
                    field_hash,
                    len,
                )
            })
            .collect();
        let mut builder_context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
            let block = builder.create_block();
            builder.switch_to_block(block);
            builder.seal_block(block);
            for (data, type_name, path_hash, field_hash, len) in registration_refs {
                let lane = if type_name == "f32" {
                    "f32"
                } else if type_name == "f64" {
                    "f64"
                } else if type_name == "u8" {
                    "u8"
                } else if type_name == "u16" {
                    "u16"
                } else {
                    "i32"
                };
                let function_ref = register_refs[&(lane, len.is_some())];
                let path = builder.ins().iconst(types::I32, i64::from(path_hash));
                let pointer = builder.ins().global_value(pointer_type, data);
                if let Some(len) = len {
                    let field = builder.ins().iconst(types::I32, i64::from(field_hash));
                    let length = builder.ins().iconst(types::I32, i64::from(len));
                    builder
                        .ins()
                        .call(function_ref, &[path, field, pointer, length]);
                } else {
                    builder.ins().call(function_ref, &[path, pointer]);
                }
            }
            let call = builder.ins().call(entry_ref, &[]);
            let result = builder.inst_results(call)[0];
            builder.ins().return_(&[result]);
            builder.finalize();
        }
        module
            .define_function(wrapper_id, &mut context)
            .map_err(|error| format!("failed to define standalone wrapper: {error}"))?;
        module.clear_context(&mut context);
        let bytes = module
            .finish()
            .emit()
            .map_err(|error| format!("failed to emit standalone AOT storage object: {error}"))?;
        Ok(Some((bytes, wrapper_symbol)))
    }

    pub fn write_engine_bundle(
        &mut self,
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
        let mut ambiguous_aliases = BTreeSet::new();
        let mut object_paths_by_function_id = BTreeMap::new();
        let mut manifest_rows: Vec<(FunctionId, String, String, String, String, u16)> = Vec::new();
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
                "{}_fn{}_{}.{}",
                sanitize_file_token(&function.name),
                function.id,
                artifact.object_index,
                object_file_extension(&self.target)
            );
            let object_path = output_dir.join(&object_file_name);
            fs::write(&object_path, bytes).map_err(|error| {
                format!(
                    "failed to write object file {}: {error}",
                    object_path.display()
                )
            })?;
            object_paths_by_function_id.insert(function.id, object_path.clone());
            if object_paths_by_function.contains_key(&function.name) {
                object_paths_by_function.remove(&function.name);
                ambiguous_aliases.insert(function.name.clone());
            } else if !ambiguous_aliases.contains(&function.name) {
                object_paths_by_function.insert(function.name.clone(), object_path);
            }
            manifest_rows.push((
                function.id,
                function.symbol_id.to_string(),
                function.name.clone(),
                artifact.symbol_name.clone(),
                object_file_name,
                function.return_type,
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

        if let Some(snapshot) = self.program_snapshot.as_mut() {
            let paths = self
                .artifacts
                .iter()
                .filter_map(|artifact| {
                    let function = self
                        .compiler
                        .functions()
                        .iter()
                        .find(|function| function.id == artifact.function_id)?;
                    let path = object_paths_by_function_id.get(&function.id)?;
                    Some((artifact.function_id, path.display().to_string()))
                })
                .collect();
            snapshot.set_artifact_paths(&paths);
        }
        Ok(AotEngineBundle {
            output_dir: output_dir.to_path_buf(),
            manifest_path,
            object_paths_by_function,
            object_paths_by_function_id,
            optimization_profile: self.optimization_profile,
        })
    }

    pub fn write_object_files(
        &mut self,
        output_dir: &Path,
    ) -> Result<BTreeMap<String, (String, PathBuf)>, String> {
        let canonical = self.write_object_files_by_id(output_dir)?;
        let mut aliases = BTreeMap::new();
        for function in self.compiler.functions() {
            let Some(artifact) = canonical.get(&function.id) else {
                continue;
            };
            if aliases
                .insert(function.name.clone(), artifact.clone())
                .is_some()
            {
                return Err(format!(
                    "AOT name alias '{}' is ambiguous; use canonical FnId",
                    function.name
                ));
            }
        }
        Ok(aliases)
    }

    pub fn write_object_files_by_id(
        &mut self,
        output_dir: &Path,
    ) -> Result<BTreeMap<FunctionId, (String, PathBuf)>, String> {
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

            let object_file_name = format!(
                "{}_fn{}_{}.{}",
                sanitize_file_token(&function.name),
                function.id,
                artifact.object_index,
                object_file_extension(&self.target)
            );
            let object_path = output_dir.join(object_file_name);
            fs::write(&object_path, bytes).map_err(|error| {
                format!(
                    "failed to write object file {}: {error}",
                    object_path.display()
                )
            })?;
            out.insert(function.id, (artifact.symbol_name.clone(), object_path));
        }
        if let Some(snapshot) = self.program_snapshot.as_mut() {
            let paths = self
                .artifacts
                .iter()
                .filter_map(|artifact| {
                    let function = self
                        .compiler
                        .functions()
                        .iter()
                        .find(|function| function.id == artifact.function_id)?;
                    let (_, path) = out.get(&function.id)?;
                    Some((artifact.function_id, path.display().to_string()))
                })
                .collect();
            snapshot.set_artifact_paths(&paths);
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

fn object_file_extension(target: &stasis_jit::AotTarget) -> &'static str {
    if matches!(target, stasis_jit::AotTarget::Native) && cfg!(windows) {
        "obj"
    } else {
        "o"
    }
}

fn aot_scalar_lane(type_id: TypeId, type_table: &TypeTable) -> Option<&'static str> {
    if type_table.unsigned_integer_bits(type_id) == Some(8) {
        Some("u8")
    } else if type_table.unsigned_integer_bits(type_id) == Some(16) {
        Some("u16")
    } else if is_i32_abi_compatible_type(type_id, type_table) {
        Some("i32")
    } else if type_id == TYPE_ID_F32 {
        Some("f32")
    } else if type_id == TYPE_ID_F64 {
        Some("f64")
    } else {
        None
    }
}

fn aot_array_lane(
    path: &str,
    type_id: TypeId,
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> Option<&'static str> {
    let text_storage = global_path_types
        .get(path)
        .and_then(|global_type| type_table.type_info(*global_type))
        .is_some_and(|info| {
            matches!(
                info.category,
                TypeCategory::AsciiFixed | TypeCategory::Utf8Fixed
            )
        });
    if text_storage || path == "gfx_cmd_u8" {
        Some("u8")
    } else {
        aot_scalar_lane(type_id, type_table)
    }
}

fn aot_storage_symbol(path: &str, field: &str) -> String {
    if field.is_empty() {
        path.replace('.', "__")
    } else {
        format!("{}__{}", path.replace('.', "__"), field.replace('.', "__"))
    }
}

fn define_standalone_storage_data(
    module: &mut ObjectModule,
    symbol: &str,
    bytes: Vec<u8>,
) -> Result<DataId, String> {
    let data_id = module
        .declare_data(symbol, Linkage::Export, true, false)
        .map_err(|error| format!("failed to declare standalone storage '{symbol}': {error}"))?;
    let mut description = DataDescription::new();
    description.define(bytes.into_boxed_slice());
    module
        .define_data(data_id, &description)
        .map_err(|error| format!("failed to define standalone storage '{symbol}': {error}"))?;
    Ok(data_id)
}

fn build_aot_direct_storage_bindings(
    global_path_types: &GlobalPathTypeMap,
    collection_infos: &CollectionInfoMap,
    type_table: &TypeTable,
) -> Result<DirectStorageBindings, String> {
    let mut bindings = DirectStorageBindings::default();
    for (path, type_id) in global_path_types {
        if collection_infos.contains_key(path) {
            continue;
        }
        if type_table
            .type_info(*type_id)
            .is_some_and(|info| info.category == TypeCategory::Named)
            && !is_named_scalar_state_path(path, *type_id, global_path_types, type_table)
        {
            continue;
        }
        if aot_scalar_lane(*type_id, type_table).is_some() {
            bindings.scalars.insert(
                path.clone(),
                DirectStorageBinding::Symbol(aot_storage_symbol(path, "")),
            );
        }
    }
    for (path, info) in collection_infos {
        if let Some(type_id) = info.element_type {
            let lane =
                aot_array_lane(path, type_id, global_path_types, type_table).ok_or_else(|| {
                    format!("unsupported AOT direct storage element type {type_id} for '{path}'")
                })?;
            bindings.arrays.insert(
                (path.clone(), String::new()),
                crate::backend::emit::DirectArrayStorageBinding {
                    slot: DirectStorageBinding::Symbol(aot_storage_symbol(path, "")),
                    storage_bytes: aot_lane_bytes(lane),
                    static_len: Some(info.len as usize),
                },
            );
        }
        for (field, type_id) in &info.field_types {
            let lane =
                aot_array_lane(path, *type_id, global_path_types, type_table).ok_or_else(|| {
                    format!(
                        "unsupported AOT direct storage field type {type_id} for '{path}.{field}'"
                    )
                })?;
            bindings.arrays.insert(
                (path.clone(), field.clone()),
                crate::backend::emit::DirectArrayStorageBinding {
                    slot: DirectStorageBinding::Symbol(aot_storage_symbol(path, field)),
                    storage_bytes: aot_lane_bytes(lane),
                    static_len: Some(info.len as usize),
                },
            );
        }
    }
    Ok(bindings)
}

fn aot_lane_bytes(lane: &str) -> u8 {
    match lane {
        "u8" => 1,
        "u16" => 2,
        "f64" => 8,
        _ => 4,
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
    target: &stasis_jit::AotTarget,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    direct_storage: &DirectStorageBindings,
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
        Some(direct_storage),
        None,
        false,
        |statement| record_string_literals_in_stmt(statement, string_literals),
        |_meta, _func| {
            #[cfg(test)]
            maybe_invoke_clif_dump_hook(_meta, _func);
        },
        |mut module, function_id, mut context| {
            module
                .define_function(function_id, &mut context)
                .map_err(|error| {
                    format!(
                        "failed to define AOT function {symbol} ({name}): {error:?}",
                        name = meta.name
                    )
                })?;
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
    rows: &[(FunctionId, String, String, String, String, u16)],
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
    for (index, (function_id, symbol_id, name, symbol, object_file, return_type)) in
        rows.iter().enumerate()
    {
        let comma = if index + 1 < rows.len() { "," } else { "" };
        out.push_str(&format!(
            "    {{\"function_id\":{},\"symbol_id\":\"{}\",\"name\":\"{}\",\"symbol\":\"{}\",\"object\":\"{}\",\"return_type\":{}}}{}\n",
            function_id,
            json_escape(symbol_id),
            json_escape(name),
            json_escape(symbol),
            json_escape(object_file),
            return_type,
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
    use object::{
        Architecture, BinaryFormat, File, Object, ObjectSection, ObjectSymbol, RelocationKind,
    };
    #[cfg(windows)]
    use std::process::Command;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    static CLIF_CAPTURE_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn aot_failed_candidate_preserves_accepted_snapshot_and_artifacts() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "main.stasis",
            "global score: i32; function main(): i32 { return score; }",
        );
        process.compile().expect("initial compile");
        let snapshot = process
            .program_snapshot()
            .expect("accepted snapshot")
            .clone();
        let artifacts = process.artifacts.clone();
        let object_bytes = process.object_bytes.clone();

        process.upsert_file(
            "main.stasis",
            "global score: i32; function main(): i32 { return missing(); }",
        );
        process.compile().expect_err("candidate must fail");

        let accepted = process.program_snapshot().expect("preserved snapshot");
        assert_eq!(accepted.source_revision(), snapshot.source_revision());
        assert_eq!(accepted.functions(), snapshot.functions());
        assert_eq!(accepted.layout_digest(), snapshot.layout_digest());
        assert_eq!(accepted.artifact_mappings(), snapshot.artifact_mappings());
        assert_eq!(process.artifacts, artifacts);
        assert_eq!(process.object_bytes, object_bytes);
    }

    #[test]
    fn duplicate_host_alias_rejects_candidate_and_preserves_active_aot_identity() {
        let mut process = AotProcess::new();
        process.upsert_file("src/main.stasis", "function main(): i32 { return 17; }\n");
        process.compile().expect("compile accepted generation");
        let accepted_snapshot = process.program_snapshot().expect("snapshot").clone();
        let accepted_artifacts = process.artifacts().to_vec();

        process.upsert_file(
            "src/duplicate.stasis",
            "function main(): i32 { return 99; }\n",
        );
        let error = process
            .compile()
            .expect_err("duplicate host alias must fail");

        assert!(matches!(
            error,
            crate::compiler::CompileError::Backend(message)
                if message.contains("host ABI alias 'main' requires exactly one canonical identity")
        ));
        assert_eq!(
            process
                .program_snapshot()
                .expect("active snapshot")
                .functions(),
            accepted_snapshot.functions()
        );
        assert_eq!(process.artifacts(), accepted_artifacts);
    }

    #[test]
    fn aot_transaction_restores_accepted_object_buffers_without_cloning() {
        let mut process = AotProcess::new();
        process.upsert_file("main.stasis", "function main(): i32 { return 1; }");
        process.compile().expect("initial compile");
        let addresses = process
            .object_bytes
            .iter()
            .map(|bytes| bytes.as_ptr())
            .collect::<Vec<_>>();
        process.upsert_file("main.stasis", "function main(): i32 { return missing(); }");
        process.compile().expect_err("reject candidate");
        assert_eq!(
            process
                .object_bytes
                .iter()
                .map(|bytes| bytes.as_ptr())
                .collect::<Vec<_>>(),
            addresses,
            "rejected AOT candidates must restore the original object buffers by move, not clone"
        );
    }

    struct ParityCorpusCase {
        label: &'static str,
        source: &'static str,
        expected_exit: i32,
        expected_extern_symbols: &'static [(&'static str, &'static str)],
        expected_string_literals: &'static [&'static str],
        expected_collection_max_lengths: &'static [(&'static str, i32)],
        expected_clif_markers: &'static [(&'static str, &'static [&'static str])],
    }

    const RENDER_TRACE_FIXTURE: &str = concat!(
        include_str!("../../../../samples/render_parity/frame.stasis"),
        include_str!("../../../../samples/render_parity/trace.stasis")
    );

    #[cfg(windows)]
    fn ensure_test_dynload_artifacts(deps_dir: &Path) -> (PathBuf, PathBuf) {
        let find_artifacts = || {
            [
                deps_dir,
                deps_dir.parent().expect("Cargo profile directory"),
            ]
            .into_iter()
            .find_map(|directory| {
                let import_library = directory.join("stasis_dynload.dll.lib");
                let runtime_dll = directory.join("stasis_dynload.dll");
                (import_library.is_file() && runtime_dll.is_file())
                    .then_some((import_library, runtime_dll))
            })
        };
        if let Some(artifacts) = find_artifacts() {
            return artifacts;
        }

        let profile_dir = deps_dir.parent().expect("Cargo profile directory");
        let target_dir = profile_dir.parent().expect("Cargo target directory");
        let mut command = Command::new("cargo");
        command.arg("build").arg("-p").arg("stasis_dynload");
        if profile_dir.file_name().and_then(|name| name.to_str()) == Some("release") {
            command.arg("--release");
        }
        let output = command
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .env("CARGO_TARGET_DIR", target_dir)
            .output()
            .expect("build stasis_dynload test runtime");
        assert!(
            output.status.success(),
            "failed to build stasis_dynload test runtime\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        find_artifacts().expect("stasis_dynload build did not produce DLL and import library")
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
        let mut effective_config = link_config.clone();
        let deps_dir = std::env::current_exe()
            .expect("current test executable")
            .parent()
            .expect("Cargo deps directory")
            .to_path_buf();
        let (import_library, runtime_dll) = ensure_test_dynload_artifacts(&deps_dir);
        effective_config.runtime_lib_paths.push(import_library);
        fs::copy(&runtime_dll, temp_root.join("stasis_dynload.dll"))
            .expect("copy AOT test runtime");
        let link_result = process.link_executable_for_i32_noarg_function(
            function_name,
            &exe_path,
            &effective_config,
        );
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
        let _capture_lock = CLIF_CAPTURE_LOCK.lock().expect("lock CLIF capture");
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
                expected_clif_markers: &[("main", &["load.i32", "store", "iadd"])],
            },
            ParityCorpusCase {
                label: "renderer_command_trace",
                source: RENDER_TRACE_FIXTURE,
                expected_exit: -996_154_394,
                expected_extern_symbols: &[(
                    "native_render_trace",
                    "stasis_jit_render_v2_trace",
                )],
                expected_string_literals: &[],
                expected_collection_max_lengths: &[
                    ("cmd_i32", 34_608),
                    ("cmd_f32", 108_676),
                    ("cmd_u8", 65_536),
                ],
                expected_clif_markers: &[("main", &["call"])],
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
                label: "deterministic_numerics",
                source: include_str!("../../../../samples/deterministic_numerics/main.stasis"),
                expected_exit: 0,
                expected_extern_symbols: &[],
                expected_string_literals: &[],
                expected_collection_max_lengths: &[],
                expected_clif_markers: &[("main", &["icmp", "sdiv"])],
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
                    ("mutate", &["call", "iadd"]),
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

        let analysis = &aot
            .program_snapshot
            .as_ref()
            .expect("program snapshot")
            .analysis;
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
    fn aot_lowers_core_global_storage_without_runtime_calls() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "direct_storage.stasis",
            "struct Enemy { hp: i32; speed: f32; }\nglobal count: i32;\nglobal ratio: f32;\nglobal precise: f64;\nglobal ints: i32[2];\nglobal floats: f32[2];\nglobal doubles: f64[2];\nglobal bytes: u8[3];\nglobal enemies: Enemy[1];\nglobal label: ascii[4];\nfunction write_globals(): void {\n    count = 7;\n    ratio = 1.5;\n    precise = 2.5;\n    ints[0] = 11;\n    floats[1] = 3.5;\n    doubles[0] = 4.5;\n    bytes[2] = 250;\n    foreach (let byte in bytes) { byte += 1; }\n    enemies[0].hp = 13;\n    enemies[0].speed = 6.5;\n    label[0] = 65;\n    ints[8] = 88;\n}\nfunction read_globals(): i32 {\n    if (ints[8] != 0) { return 1; }\n    if (enemies[0].speed < 6.4) { return 2; }\n    return count + ints[0] + bytes[2] + enemies[0].hp + label[0] + label.max_length;\n}\nfunction main(): i32 { write_globals(); return read_globals(); }\n",
        );
        let captured = capture_aot_clif_by_function(&mut process);
        let writer = captured.get("write_globals").expect("writer CLIF");
        let reader = captured.get("read_globals").expect("reader CLIF");
        assert!(
            reader.contains("load.i32"),
            "expected direct loads:\n{reader}"
        );
        assert!(
            reader.contains("load.i8"),
            "expected direct byte loads:\n{reader}"
        );
        assert!(
            writer.contains("store"),
            "expected direct stores:\n{writer}"
        );
        assert!(
            writer.contains("ireduce.i8"),
            "expected byte-width direct stores:\n{writer}"
        );
        for (name, clif) in [("writer", writer), ("reader", reader)] {
            let has_call_instruction = clif.lines().any(|line| {
                let line = line.trim_start();
                line.starts_with("call ") || line.contains(" = call ")
            });
            assert!(
                !has_call_instruction,
                "core AOT global storage emitted a runtime call in {name}:\n{clif}"
            );
        }
    }

    #[test]
    fn aot_dynamic_global_array_fallback_uses_runtime_helper_signature() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "dynamic_global_array.stasis",
            "global values: i32[2];\nfunction read(index: i32, a: i32, b: i32, c: i32): i32 { return values[index]; }\nfunction main(): i32 { values[1] = 9; return read(1, 0, 0, 0); }\n",
        );

        process
            .compile()
            .expect("compile dynamic global array AOT fixture");
        assert!(
            undefined_runtime_symbols(&process).contains("stasis_jit_global_i32_array_load"),
            "dynamic bounds fallback should reference the array-load runtime helper"
        );
    }

    fn undefined_runtime_symbols(process: &AotProcess) -> BTreeSet<String> {
        process
            .object_bytes
            .iter()
            .flat_map(|bytes| {
                let object = File::parse(bytes.as_slice()).expect("parse AOT object");
                object
                    .symbols()
                    .filter(|symbol| symbol.is_undefined())
                    .filter_map(|symbol| symbol.name().ok().map(str::to_string))
                    .filter(|name| name.starts_with("stasis_jit_"))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn pure_aot_function_exposes_no_runtime_helper_symbols() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "pure.stasis",
            "function main(): i32 { let x: f32 = 10.0 + 2.5; if (x == 12.5) { return 0; } return 1; }\n",
        );
        process.compile().expect("compile pure AOT fixture");

        assert_eq!(undefined_runtime_symbols(&process), BTreeSet::new());
    }

    #[test]
    fn print_aot_function_exposes_only_referenced_runtime_helper_symbol() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "print.stasis",
            "function main(): i32 { print_int(7); return 0; }\n",
        );
        process.compile().expect("compile print AOT fixture");

        assert_eq!(
            undefined_runtime_symbols(&process),
            BTreeSet::from(["stasis_jit_print_i32".to_string()])
        );
    }

    #[test]
    fn standalone_aot_storage_defines_and_registers_direct_symbols() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "storage.stasis",
            "global count: i32;\nglobal ints: i32[2];\nglobal bytes: u8[3];\nfunction main(): i32 { count = 1; ints[0] = 2; bytes[0] = 3; return 0; }\n",
        );
        process.compile().expect("compile");
        let (bytes, wrapper) = process
            .compile_standalone_storage_object("aot_fn_0")
            .expect("storage object")
            .expect("storage required");
        let object = File::parse(bytes.as_slice()).expect("parse storage object");
        let symbols: BTreeSet<String> = object
            .symbols()
            .filter_map(|symbol| symbol.name().ok().map(str::to_string))
            .collect();

        assert_eq!(wrapper, "stasis_aot_standalone_entry");
        for expected in [
            "count",
            "ints",
            "bytes",
            "stasis_aot_standalone_entry",
            "stasis_jit_register_global_i32_ptr",
            "stasis_jit_register_global_i32_array",
            "stasis_jit_register_global_u8_array",
            "aot_fn_0",
        ] {
            assert!(
                symbols.contains(expected),
                "standalone storage object missing symbol '{expected}': {symbols:?}"
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
        let analysis = &process
            .program_snapshot
            .as_ref()
            .expect("program snapshot")
            .analysis;
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
        let analysis = &process
            .program_snapshot
            .as_ref()
            .expect("program snapshot")
            .analysis;
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
        let analysis = &process
            .program_snapshot
            .as_ref()
            .expect("program snapshot")
            .analysis;
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
    fn aot_process_accepts_brickout_compatible_audio_asset_api() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "audio_asset_api.stasis",
            "function @extern(\"stasis_jit_audio_load_music\") audio_load_music(path: string): i32;\nfunction @extern(\"stasis_jit_audio_load_effect\") audio_load_effect(path: string): i32;\nfunction @extern(\"stasis_jit_audio_play_music\") audio_play_music(handle: i32, loop: bool, volume: f32): bool;\nfunction @extern(\"stasis_jit_audio_pause_music\") audio_pause_music(handle: i32, paused: bool): void;\nfunction @extern(\"stasis_jit_audio_set_music_volume\") audio_set_music_volume(handle: i32, volume: f32): void;\nfunction @extern(\"stasis_jit_audio_stop_music\") audio_stop_music(handle: i32): void;\nfunction @extern(\"stasis_jit_audio_play_effect\") audio_play_effect(handle: i32, volume: f32): bool;\nfunction main(): i32 { let music: i32 = audio_load_music(\"music.wav\"); let effect: i32 = audio_load_effect(\"effect.wav\"); audio_play_music(music, true, 0.4); audio_pause_music(music, true); audio_set_music_volume(music, 0.2); audio_stop_music(music); audio_play_effect(effect, 0.5); return music + effect; }\n",
        );
        process.compile().expect("aot compile audio asset API");
        let signatures = &process
            .program_snapshot
            .as_ref()
            .expect("program snapshot")
            .analysis
            .resolved_extern_signatures;
        assert_eq!(signatures.len(), 7);
        for symbol in [
            "stasis_jit_audio_load_music",
            "stasis_jit_audio_load_effect",
            "stasis_jit_audio_play_music",
            "stasis_jit_audio_pause_music",
            "stasis_jit_audio_set_music_volume",
            "stasis_jit_audio_stop_music",
            "stasis_jit_audio_play_effect",
        ] {
            assert!(signatures
                .iter()
                .any(|signature| signature.symbol == symbol));
        }
    }

    #[test]
    fn aot_process_compiles_audio_asset_playback_sample() {
        let sample = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../samples/audio_asset_playback/audio_asset_playback.stasis"
        ));
        let source = sample
            .replace("import \"/vendor/stasis/src/stdlib/audio.stasis\";", "")
            .replace("import \"/vendor/stasis/src/stdlib/graphics.stasis\";", "");
        let declarations = r#"
function @extern("stasis_jit_audio_init") audio_init(sample_rate: i32, channels: i32, latency: i32): bool;
function @extern("stasis_jit_audio_is_available") audio_is_available(): bool;
function @extern("stasis_jit_audio_get_sample_rate") audio_get_sample_rate(): i32;
function @extern("stasis_jit_audio_get_channels") audio_get_channels(): i32;
function @extern("stasis_jit_audio_load_music") audio_load_music(path: string): i32;
function @extern("stasis_jit_audio_load_effect") audio_load_effect(path: string): i32;
function @extern("stasis_jit_audio_play_music") audio_play_music(handle: i32, loop: bool, volume: f32): bool;
function @extern("stasis_jit_audio_pause_music") audio_pause_music(handle: i32, paused: bool): void;
function @extern("stasis_jit_audio_set_music_volume") audio_set_music_volume(handle: i32, volume: f32): void;
function @extern("stasis_jit_audio_stop_music") audio_stop_music(handle: i32): void;
function @extern("stasis_jit_audio_play_effect") audio_play_effect(handle: i32, volume: f32): bool;
function init_window(width: i32, height: i32, title: string): bool { return true; }
function begin_frame(): void { return; }
function clear(r: f32, g: f32, b: f32, a: f32): void { return; }
function end_frame(): void { return; }
"#;
        let mut process = AotProcess::new();
        process.upsert_file(
            "samples/audio_asset_playback/audio_asset_playback.stasis",
            format!("{declarations}\n{source}"),
        );
        process.compile().expect("aot compile playback sample");
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
    fn aot_process_emits_ios_arm64_macho_objects_when_target_is_configured() {
        let mut process = AotProcess::new();
        process.set_target(stasis_jit::AotTarget::ios_arm64_default());
        process.upsert_file(
            "sample.stasis",
            "global State { value: i32; }\nfunction helper(): i32 { return State.value; }\nfunction main(): i32 { State.value = 7; return helper(); }\n",
        );
        process.compile().expect("ios aot compile");

        for bytes in &process.object_bytes {
            let object = File::parse(bytes.as_slice()).expect("parse emitted object");
            assert_eq!(object.format(), BinaryFormat::MachO);
            assert_eq!(object.architecture(), Architecture::Aarch64);
        }
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
        let expected_extension = if cfg!(windows) { "obj" } else { "o" };
        assert!(
            bundle
                .object_paths_by_function
                .values()
                .all(|path| path.extension().and_then(|value| value.to_str())
                    == Some(expected_extension)),
            "native engine bundle should keep host object suffixes"
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
    fn aot_engine_bundle_preserves_objects_for_overloaded_function_names() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Sprite { handle: i32; }\nstruct TextRun { handle: i32; }\nglobal sprite: Sprite;\nglobal text: TextRun;\nfunction draw(self: Sprite, value: i32): void { self.handle = value; }\nfunction draw(self: TextRun, value: i32): void { self.handle = value + 7; }\nfunction main(): i32 { sprite.draw(30); text.draw(5); return sprite.handle + text.handle; }\nfunction tick(): void { return; }\nfunction render(): void { return; }\nfunction on_code_swap(): void { return; }\n",
        );
        process.compile().expect("compile overload bundle");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let bundle_dir = std::env::temp_dir().join(format!("stasis_aot_bundle_overloads_{stamp}"));
        let bundle = process
            .write_engine_bundle(&EngineEntrypoints::runtime_default(), &bundle_dir)
            .expect("write overload bundle");
        assert_eq!(
            bundle.object_paths_by_function_id.len(),
            process.artifacts().len(),
            "both draw overload objects must remain in the bundle"
        );
        assert!(!bundle.object_paths_by_function.contains_key("draw"));
        assert_eq!(bundle.object_paths().len(), process.artifacts().len());
        let draw_objects = bundle
            .object_paths()
            .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
            .filter(|name| name.starts_with("draw_fn"))
            .collect::<Vec<_>>();
        assert_eq!(draw_objects.len(), 2);
        assert_ne!(draw_objects[0], draw_objects[1]);

        let _ = fs::remove_dir_all(&bundle_dir);
    }

    #[cfg(windows)]
    #[test]
    fn aot_engine_bundle_links_and_executes_same_named_receiver_methods() {
        let Some(mut link_config) = resolve_link_config_for_smoke() else {
            return;
        };
        let source = "function draw(self: i32, value: i32): i32 { return value; }\nfunction draw(self: f32, value: i32): i32 { return value + 7; }\nfunction main(): i32 { let sprite: i32 = 1; let text: f32 = 1.0; return sprite.draw(30) + text.draw(5); }\nfunction tick(): void { return; }\nfunction render(): void { return; }\nfunction on_code_swap(): void { return; }\n";
        let mut process = AotProcess::new();
        process.upsert_file("sample.stasis", source);
        process.compile().expect("compile receiver overload bundle");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_receiver_bundle_{stamp}"));
        let bundle = process
            .write_engine_bundle(&EngineEntrypoints::runtime_default(), &temp_root)
            .expect("write receiver overload bundle");
        let main_id = process
            .compiler
            .functions()
            .iter()
            .find(|function| function.name == "main")
            .expect("main function")
            .id;
        let main_symbol = process
            .artifacts()
            .iter()
            .find(|artifact| artifact.function_id == main_id)
            .expect("main artifact")
            .symbol_name
            .clone();

        let deps_dir = std::env::current_exe()
            .expect("current test executable")
            .parent()
            .expect("Cargo deps directory")
            .to_path_buf();
        let (import_library, runtime_dll) = ensure_test_dynload_artifacts(&deps_dir);
        link_config.runtime_lib_paths.push(import_library);
        fs::copy(&runtime_dll, temp_root.join("stasis_dynload.dll"))
            .expect("copy AOT test runtime");
        let executable = temp_root.join("receiver_bundle.exe");
        let object_paths = bundle.object_paths().cloned().collect::<Vec<_>>();
        stasis_jit::link_objects_to_executable(
            &object_paths,
            &executable,
            &main_symbol,
            &link_config,
        )
        .expect("link every receiver overload object");
        let status = Command::new(&executable)
            .status()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", executable.display()));
        assert_eq!(status.code(), Some(42));

        let _ = fs::remove_dir_all(&temp_root);
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
    fn aot_process_prefers_runtime_string_shims_for_host_string_externs() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "extern function gfx_load_sprite(path: string, max_w: i32, max_h: i32): i32;\nextern function gfx_release_sprite(handle: i32): void;\nextern function load_font(path: string, size: i32): i32;\nextern function measure_text(font: i32, text: string): f32;\nfunction @extern(\"stasis_gfx_cache_text\") gfx_cache_text(font: i32, text: string): i32;\nextern function storage_load_i32(scope: string, key: string, fallback: i32): i32;\nextern function storage_save_i32(scope: string, key: string, value: i32): bool;\nfunction @extern(\"stasis_jit_storage_load_ascii\") storage_load_ascii(scope: string, key: string, out: ascii[], capacity: i32): i32;\nfunction @extern(\"stasis_jit_storage_save_ascii\") storage_save_ascii(scope: string, key: string, value: ascii[], length: i32): i32;\nfunction @extern(\"stasis_jit_clipboard_load_ascii\") clipboard_load_ascii(out: ascii[], capacity: i32): i32;\nfunction @extern(\"stasis_jit_clipboard_save_ascii\") clipboard_save_ascii(value: ascii[], length: i32): i32;\nfunction main(): i32 { gfx_release_sprite(0); return 0; }\n",
        );
        process.compile().expect("compile");

        let analysis = &process
            .program_snapshot
            .as_ref()
            .expect("program snapshot")
            .analysis;
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
            Some("stasis_jit_gfx_cache_text")
        );
        assert_eq!(
            resolved.get("storage_load_i32").copied(),
            Some("stasis_jit_storage_load_i32")
        );
        assert_eq!(
            resolved.get("storage_save_i32").copied(),
            Some("stasis_jit_storage_save_i32")
        );
        assert_eq!(
            resolved.get("storage_load_ascii").copied(),
            Some("stasis_jit_storage_load_ascii")
        );
        assert_eq!(
            resolved.get("storage_save_ascii").copied(),
            Some("stasis_jit_storage_save_ascii")
        );
        assert_eq!(
            resolved.get("clipboard_load_ascii").copied(),
            Some("stasis_jit_clipboard_load_ascii")
        );
        assert_eq!(
            resolved.get("clipboard_save_ascii").copied(),
            Some("stasis_jit_clipboard_save_ascii")
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
    fn program_snapshot_multifile_jit_and_aot_executable_parity() {
        let Some(link_config) = resolve_link_config_for_smoke() else {
            return;
        };
        let main = include_str!("../../../../tests/program_snapshot_parity_main.stasis");
        let helper = include_str!("../../../../tests/program_snapshot_parity_helper.stasis");
        let mut jit = crate::backend::jit::JitProcess::new();
        jit.upsert_file("main.stasis", main);
        jit.upsert_file("helper.stasis", helper);
        jit.compile().expect("compile JIT fixture");
        assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(7));

        let mut aot = AotProcess::new();
        aot.upsert_file("main.stasis", main);
        aot.upsert_file("helper.stasis", helper);
        aot.compile().expect("compile AOT fixture");
        assert_eq!(
            jit.program_snapshot().expect("JIT snapshot").functions(),
            aot.program_snapshot().expect("AOT snapshot").functions()
        );
        assert_eq!(
            jit.program_snapshot()
                .expect("JIT snapshot")
                .layout_digest(),
            aot.program_snapshot()
                .expect("AOT snapshot")
                .layout_digest()
        );
        if let Some(exit_code) =
            run_linked_i32_noarg_fixture(&aot, "main", "snapshot_parity", &link_config)
        {
            assert_eq!(exit_code, 7);
        }
    }

    #[test]
    fn qualified_same_name_calls_use_identical_jit_and_aot_module_graphs() {
        let main = include_str!("../../../../tests/module_graph/main.stasis");
        let one = include_str!("../../../../tests/module_graph/one.stasis");
        let two = include_str!("../../../../tests/module_graph/two.stasis");
        let files = [
            ("tests/module_graph/main.stasis", main),
            ("tests/module_graph/one.stasis", one),
            ("tests/module_graph/two.stasis", two),
        ];

        let mut jit = crate::backend::jit::JitProcess::new();
        for (path, source) in files {
            jit.upsert_file(path, source);
        }
        jit.compile().expect("qualified JIT compile");
        assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(18));

        let mut aot = AotProcess::new();
        for (path, source) in files {
            aot.upsert_file(path, source);
        }
        aot.compile().expect("qualified AOT compile");
        assert_eq!(
            jit.program_snapshot().unwrap().module_graph(),
            aot.program_snapshot().unwrap().module_graph()
        );

        #[cfg(windows)]
        {
            let link_config = resolve_link_config_for_smoke()
                .expect("qualified module graph acceptance requires a Windows linker");
            let result =
                run_linked_i32_noarg_fixture(&aot, "main", "qualified_module_graph", &link_config)
                    .expect("qualified module graph acceptance requires a linked executable");
            assert_eq!(result, 18);
        }
    }

    #[cfg(windows)]
    #[test]
    fn symbol_identity_multifile_jit_aot_is_root_relative_stable_and_executable() {
        let Some(link_config) = resolve_link_config_for_smoke() else {
            return;
        };
        let main = include_str!("../../../../tests/symbol_identity_main.stasis");
        let helper = include_str!("../../../../tests/symbol_identity_helper.stasis");

        let mut jit = JitProcess::new();
        jit.set_project_root("C:/workspace/game")
            .expect("set JIT project root");
        jit.upsert_file("C:/workspace/game/src/main.stasis", main);
        jit.upsert_file("C:/workspace/game/src/helper.stasis", helper);
        jit.compile().expect("compile rooted JIT fixture");
        assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(11));

        let mut aot = AotProcess::new();
        aot.upsert_file("src/helper.stasis", helper);
        aot.upsert_file("src/main.stasis", main);
        aot.compile().expect("compile relative AOT fixture");
        let jit_ids = jit
            .program_snapshot()
            .expect("JIT snapshot")
            .functions()
            .iter()
            .map(|function| {
                (
                    function.name.clone(),
                    function.id,
                    function.symbol_id.clone(),
                )
            })
            .collect::<BTreeSet<_>>();
        let aot_ids = aot
            .program_snapshot()
            .expect("AOT snapshot")
            .functions()
            .iter()
            .map(|function| {
                (
                    function.name.clone(),
                    function.id,
                    function.symbol_id.clone(),
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(jit_ids, aot_ids);

        let before_edit = jit_ids;
        jit.upsert_file(
            "C:/workspace/game/src/helper.stasis",
            "function helper(): i32 { return 8; }\n",
        );
        jit.compile().expect("compile helper body edit");
        assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(13));
        let after_edit = jit
            .program_snapshot()
            .expect("edited JIT snapshot")
            .functions()
            .iter()
            .map(|function| {
                (
                    function.name.clone(),
                    function.id,
                    function.symbol_id.clone(),
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(before_edit, after_edit);

        if let Some(exit_code) =
            run_linked_i32_noarg_fixture(&aot, "main", "symbol_identity", &link_config)
        {
            assert_eq!(exit_code, 11);
        }
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
    fn aot_and_jit_execute_receiver_overloads_with_different_arities() {
        let Some(link_config) = resolve_link_config_for_smoke() else {
            return;
        };
        let source = "function draw(self: i32, x: f32, alpha: i32): i32 { return self + alpha; }\nfunction draw(self: f32, x: f32, r: f32, g: f32, b: f32, a: f32): i32 { return 7; }\nfunction main(): i32 { let sprite: i32 = 5; let text: f32 = 2.0; return sprite.draw(1.0, 30) + text.draw(2.0, 1.0, 1.0, 1.0, 1.0); }\n";

        let mut jit = JitProcess::new();
        jit.upsert_file("sample.stasis", source);
        jit.compile().expect("JIT compile");
        let jit_result = jit.execute_i32_noarg_by_name("main").expect("JIT execute");

        let mut aot = AotProcess::new();
        aot.upsert_file("sample.stasis", source);
        aot.compile().expect("AOT compile");
        let Some(aot_result) = run_linked_i32_noarg_fixture(
            &aot,
            "main",
            "receiver_overloads_different_arities",
            &link_config,
        ) else {
            return;
        };

        assert_eq!(jit_result, 42);
        assert_eq!(aot_result, jit_result);
    }

    #[test]
    fn aot_process_compiles_nested_struct_receiver_call() {
        let mut process = AotProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Sprite { handle: i32; }\nstruct GameState { aura: Sprite; sprites: Sprite[2]; }\nglobal state: GameState;\nfunction set_handle(self: Sprite, value: i32): void { self.handle = value; }\nfunction main(): i32 { state.aura.set_handle(37); state.sprites[1].set_handle(5); return state.aura.handle + state.sprites[1].handle; }\n",
        );
        let report = process.compile().expect("AOT compile nested receiver");
        assert!(report.emit.emitted_functions >= 2);
    }

    #[cfg(windows)]
    #[test]
    fn aot_process_links_and_executes_nested_struct_receiver_call() {
        let Some(link_config) = resolve_link_config_for_smoke() else {
            return;
        };
        let source = "struct Sprite { handle: i32; }\nstruct GameState { aura: Sprite; sprites: Sprite[2]; }\nglobal state: GameState;\nfunction set_handle(self: Sprite, value: i32): void { self.handle = value; }\nfunction main(): i32 { state.aura.set_handle(37); state.sprites[1].set_handle(5); return state.aura.handle + state.sprites[1].handle; }\n";
        let mut process = AotProcess::new();
        process.upsert_file("sample.stasis", source);
        process.compile().expect("AOT compile nested receiver");
        let result =
            run_linked_i32_noarg_fixture(&process, "main", "nested_struct_receiver", &link_config)
                .expect("nested receiver fixture must link and execute");
        assert_eq!(result, 42);
    }

    #[cfg(windows)]
    #[test]
    fn aot_process_links_and_executes_immediate_axis_layout_sample() {
        let Some(link_config) = resolve_link_config_for_smoke() else {
            return;
        };

        let mut process = AotProcess::new();
        process.set_import_base_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
        process.upsert_file(
            "src/stdlib/ui_axis_layout.stasis",
            include_str!("../../../../src/stdlib/ui_axis_layout.stasis"),
        );
        process.upsert_file(
            "src/stdlib/ui_layout_audit.stasis",
            include_str!("../../../../src/stdlib/ui_layout_audit.stasis"),
        );
        process.upsert_file(
            "samples/immediate_axis_layout/placement.stasis",
            include_str!("../../../../samples/immediate_axis_layout/placement.stasis"),
        );
        process.upsert_file(
            "samples/immediate_axis_layout/verify.stasis",
            include_str!("../../../../samples/immediate_axis_layout/verify.stasis"),
        );
        process
            .compile()
            .expect("compile immediate axis layout sample");
        assert!(
            process.state_layout().scalars.is_empty(),
            "pure AOT placement fixture must not require runtime storage registration"
        );

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_immediate_axis_layout_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let exe_path = temp_root.join("immediate_axis_layout.exe");
        process
            .link_executable_for_i32_noarg_function("main", &exe_path, &link_config)
            .expect("link immediate axis layout without runtime library");

        let status = Command::new(&exe_path)
            .status()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", exe_path.display()));
        assert_eq!(
            status.code(),
            Some(0),
            "axis layout sample assertions failed"
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

    #[cfg(windows)]
    #[test]
    fn direct_call_generation_sample_matches_jit_and_linked_aot() {
        let main_source = include_str!("../../../../samples/direct_call_generation/main.stasis");
        let math_source = include_str!("../../../../samples/direct_call_generation/math.stasis");
        let fixture_root = Path::new("direct_call_generation_fixture");

        let mut jit = JitProcess::new();
        jit.upsert_file(
            fixture_root.join("main.stasis").to_string_lossy(),
            main_source,
        );
        jit.upsert_file(
            fixture_root.join("math.stasis").to_string_lossy(),
            math_source,
        );
        jit.compile().expect("JIT sample compile");
        let jit_result = jit
            .execute_i32_noarg_by_name("main")
            .expect("JIT sample run");

        let mut aot = AotProcess::new();
        aot.upsert_file(
            fixture_root.join("main.stasis").to_string_lossy(),
            main_source,
        );
        aot.upsert_file(
            fixture_root.join("math.stasis").to_string_lossy(),
            math_source,
        );
        aot.compile().expect("AOT sample compile");
        assert_eq!(aot.artifacts().len(), 9);
        assert_eq!(jit_result, 17);
        let Some(link_config) = resolve_link_config_for_smoke() else {
            eprintln!("skipping linked AOT execution: no Windows linker found");
            return;
        };
        let Some(aot_result) =
            run_linked_i32_noarg_fixture(&aot, "main", "direct_call_generation", &link_config)
        else {
            return;
        };

        assert_eq!(aot_result, jit_result);
    }

    #[cfg(windows)]
    #[test]
    fn selective_jit_revision_sequence_matches_full_aot_builds() {
        let Some(link_config) = resolve_link_config_for_smoke() else {
            return;
        };
        let revisions = [
            "function helper(): i32 { return 1; } function main(): i32 { return helper() + 1; }",
            "function helper(): i32 { return 2; } function main(): i32 { return helper() + 1; }",
        ];
        let mut jit = JitProcess::new();
        for (index, source) in revisions.iter().enumerate() {
            if index == 0 {
                jit.upsert_file("revision.stasis", *source);
                jit.compile().expect("initial JIT revision");
            } else {
                let mut candidate = jit.staged_candidate();
                candidate.upsert_file("revision.stasis", *source);
                candidate.compile_staged().expect("selective JIT revision");
                assert_eq!(
                    candidate
                        .generation_metadata()
                        .expect("selective metadata")
                        .emitted_function_ids
                        .len(),
                    2
                );
                jit = candidate;
            }
            let jit_result = jit
                .execute_i32_noarg_by_name("main")
                .expect("execute JIT revision");

            let mut aot = AotProcess::new();
            aot.upsert_file("revision.stasis", *source);
            aot.compile().expect("full AOT revision");
            let Some(aot_result) = run_linked_i32_noarg_fixture(
                &aot,
                "main",
                &format!("selective_revision_{index}"),
                &link_config,
            ) else {
                return;
            };
            assert_eq!(jit_result, (index as i32) + 2);
            assert_eq!(aot_result, jit_result);
        }
    }

    #[cfg(windows)]
    #[test]
    fn aot_and_jit_execute_deterministic_numeric_sample() {
        let Some(link_config) = resolve_link_config_for_smoke() else {
            return;
        };
        let source = include_str!("../../../../samples/deterministic_numerics/main.stasis");
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", source);
        jit.compile().expect("JIT compile");
        let jit_result = jit.execute_i32_noarg_by_name("main").expect("JIT execute");

        let mut aot = AotProcess::new();
        aot.upsert_file("main.stasis", source);
        aot.compile().expect("AOT compile");
        let Some(aot_result) =
            run_linked_i32_noarg_fixture(&aot, "main", "deterministic_numerics", &link_config)
        else {
            return;
        };

        assert_eq!(jit_result, 0);
        assert_eq!(aot_result, jit_result);
    }

    #[test]
    fn parity_corpus_covers_shared_lowering_shapes() {
        for case in parity_corpus_cases() {
            run_parity_corpus_case(case);
        }
    }

    #[test]
    fn deterministic_numeric_lowering_is_width_explicit_on_arm64_targets() {
        let source =
            include_str!("../../../../samples/deterministic_numerics/main.stasis").replacen(
                "function main(): i32",
                "function deterministic_numeric_probe(): i32",
                1,
            ) + "\nfunction main(): i32 { return deterministic_numeric_probe(); }\n";
        for target in [
            stasis_jit::AotTarget::android_arm64_default(),
            stasis_jit::AotTarget::ios_arm64_default(),
        ] {
            let mut process = AotProcess::new();
            process.set_target(target);
            process.upsert_file("main.stasis", source.clone());
            let clif = capture_aot_clif_by_function(&mut process);
            let main = clif
                .get("deterministic_numeric_probe")
                .expect("deterministic numeric probe CLIF");
            for marker in ["load.i8", "load.i16", "ireduce.i8", "ireduce.i16", "sdiv"] {
                assert!(main.contains(marker), "missing {marker} in:\n{main}");
            }
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

    #[cfg(windows)]
    fn resolve_link_config_for_smoke() -> Option<stasis_jit::AotLinkConfig> {
        if let Some(explicit) = std::env::var_os("STASIS_AOT_LINKER") {
            let explicit = PathBuf::from(explicit);
            eprintln!("AOT smoke linker: {}", explicit.display());
            return Some(stasis_jit::AotLinkConfig {
                linker_path: Some(explicit),
                runtime_lib_paths: vec![],
                target: stasis_jit::AotTarget::default(),
            });
        }
        for candidate in ["link.exe", "lld-link.exe"] {
            if let Some(linker_path) = resolve_windows_linker_path(candidate) {
                eprintln!("AOT smoke linker: {}", linker_path.display());
                return Some(stasis_jit::AotLinkConfig {
                    linker_path: Some(linker_path),
                    runtime_lib_paths: vec![],
                    target: stasis_jit::AotTarget::default(),
                });
            }
        }
        eprintln!("skipping AOT executable smoke test: no Windows linker found");
        None
    }

    #[cfg(windows)]
    fn resolve_windows_linker_path(candidate: &str) -> Option<PathBuf> {
        let output = Command::new("where").arg(candidate).output().ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .find(|path| path.is_absolute() && path.is_file())
    }

    #[cfg(windows)]
    #[test]
    fn aot_smoke_linker_prefers_absolute_msvc_path() {
        if std::env::var_os("STASIS_AOT_LINKER").is_some() {
            return;
        }
        let Some(expected_msvc) = resolve_windows_linker_path("link.exe") else {
            return;
        };
        let config = resolve_link_config_for_smoke().expect("MSVC linker config");
        assert_eq!(config.linker_path.as_deref(), Some(expected_msvc.as_path()));
        assert!(expected_msvc.is_absolute());
    }
}
