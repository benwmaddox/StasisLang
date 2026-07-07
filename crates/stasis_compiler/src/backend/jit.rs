use crate::backend::EngineEntrypoints;
use crate::compiler::{CompileReport, CompileResult, Compiler, FunctionId, FunctionMeta};
use crate::frontend::indexer::hash_text;
use crate::frontend::types::{
    TypeCategory, TypeTable, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_I32, TYPE_ID_VOID,
};
use crate::ir::hir::FunctionHIR;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, Module};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
#[cfg(test)]
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;

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
    artifact_index: HashMap<FunctionId, usize>,
    modules: Vec<JITModule>,
    runtime_libraries: Vec<stasis_dynload::Library>,
    runtime_symbol_cache: BTreeMap<String, usize>,
    source_disk_probe_cache: BTreeMap<String, SourceDiskProbe>,
    import_parse_cache: BTreeMap<String, ImportParseCacheEntry>,
    compile_analysis_cache: Option<CompileAnalysisCache>,
    required_emit_roots: Vec<String>,
    #[cfg(test)]
    _test_guard: MutexGuard<'static, ()>,
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
        #[cfg(test)]
        let _test_guard = acquire_jit_process_test_guard();
        #[cfg(test)]
        {
            // Many unit tests manipulate process-global JIT tables. Keep them isolated and
            // deterministic by clearing under the test guard.
            stasis_dynload::clear_jit_i32_global_table();
            stasis_dynload::clear_jit_f32_global_table();
            stasis_dynload::clear_jit_f64_global_table();
            stasis_dynload::clear_jit_i32_array_global_table();
            stasis_dynload::clear_jit_f32_array_global_table();
            stasis_dynload::clear_jit_f64_array_global_table();
            stasis_dynload::clear_jit_string_literal_table();
            stasis_dynload::clear_registered_global_memory();
        }

        Self {
            compiler: Compiler::new(),
            next_slot: 0,
            next_symbol_seq: 0,
            artifacts: Vec::new(),
            artifact_index: HashMap::new(),
            modules: Vec::new(),
            runtime_libraries: Vec::new(),
            runtime_symbol_cache: BTreeMap::new(),
            source_disk_probe_cache: BTreeMap::new(),
            import_parse_cache: BTreeMap::new(),
            compile_analysis_cache: None,
            required_emit_roots: Vec::new(),
            #[cfg(test)]
            _test_guard,
        }
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.compiler.upsert_file(path, content);
    }

    pub fn set_required_emit_roots(&mut self, roots: &[String]) {
        self.required_emit_roots.clear();
        self.required_emit_roots.extend_from_slice(roots);
    }

    pub fn refresh_imported_sources_from_disk(&mut self, root_source_path: &str) -> bool {
        let tracked: Vec<(String, u64)> = self
            .compiler
            .files()
            .iter()
            .filter(|file| file.path != root_source_path)
            .map(|file| (file.path.clone(), file.hash))
            .collect();
        let tracked_paths: BTreeSet<String> =
            tracked.iter().map(|(path, _)| path.clone()).collect();
        self.source_disk_probe_cache
            .retain(|path, _| tracked_paths.contains(path));

        let mut changed = false;
        for (path, known_hash) in tracked {
            let disk_path = Path::new(&path);
            let Some(probe) = probe_disk_source(disk_path) else {
                continue;
            };
            if self.source_disk_probe_cache.get(&path) == Some(&probe) {
                continue;
            }

            let Ok(content) = std::fs::read_to_string(disk_path) else {
                continue;
            };
            let disk_hash = hash_text(&content);
            self.source_disk_probe_cache.insert(path.clone(), probe);
            if disk_hash != known_hash {
                self.compiler.upsert_file(path, content);
                changed = true;
            }
        }
        changed
    }

    pub fn compile(&mut self) -> CompileResult<CompileReport> {
        stasis_dynload::clear_jit_string_literal_table();
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
        let files_fingerprint = compute_files_fingerprint(self.compiler.files());
        let cache_miss = self
            .compile_analysis_cache
            .as_ref()
            .is_none_or(|cache| cache.files_fingerprint != files_fingerprint);
        let mut force_reemit_reachable = false;
        if cache_miss {
            let extern_signatures =
                collect_supported_extern_call_signatures(self.compiler.files(), &mut type_table)
                    .map_err(crate::compiler::CompileError::Backend)?;
            let (resolved_extern_signatures, extern_symbol_addresses) = self
                .resolve_extern_call_signatures(&extern_signatures)
                .map_err(crate::compiler::CompileError::Backend)?;
            let next_cache = build_compile_analysis_cache_from_resolved_externs(
                self.compiler.files(),
                self.compiler.functions(),
                &mut type_table,
                files_fingerprint,
                resolved_extern_signatures,
                extern_symbol_addresses,
            )
            .map_err(crate::compiler::CompileError::Backend)?;
            if let Some(previous_cache) = self.compile_analysis_cache.as_ref() {
                force_reemit_reachable =
                    compile_analysis_requires_reemit(previous_cache, &next_cache);
            }
            self.compile_analysis_cache = Some(next_cache);
        }
        let analysis = self.compile_analysis_cache.as_ref().ok_or_else(|| {
            crate::compiler::CompileError::Invariant(
                "jit compile analysis cache missing after refresh".to_string(),
            )
        })?;
        seed_fixed_collection_max_length_headers(&analysis.global_path_types, &type_table)
            .map_err(crate::compiler::CompileError::Backend)?;
        let emit_function_ids = select_emit_function_ids(
            self.compiler.functions(),
            self.artifacts(),
            &self.required_emit_roots,
            force_reemit_reachable,
        );
        let mut next_slot = self.next_slot;
        let mut next_symbol_seq = self.next_symbol_seq;
        let mut staged_artifacts = self.artifacts.clone();
        let mut staged_modules: Vec<JITModule> = Vec::new();
        let emit = self
            .compiler
            .emit_pass_for_ids_with(&emit_function_ids, &mut |meta, hir| {
                let symbol = format!("jit_fn_{}_{}", meta.id, next_symbol_seq);
                next_symbol_seq = next_symbol_seq.saturating_add(1);
                let (module, code_ptr) = compile_function_to_jit_module(
                    meta,
                    hir,
                    &symbol,
                    &analysis.call_signatures,
                    &mut type_table,
                    &analysis.global_path_types,
                    &analysis.constant_values,
                    &analysis.collection_infos,
                    &analysis.named_struct_field_types,
                    &analysis.extern_symbol_addresses,
                )?;
                let slot = next_slot;
                next_slot = next_slot.saturating_add(1);
                staged_modules.push(module);
                staged_artifacts.retain(|artifact| artifact.function_id != meta.id);
                staged_artifacts.push(JitArtifact {
                    function_id: meta.id,
                    slot,
                    body_hash: meta.body_hash,
                    code_ptr,
                });
                Ok(())
            })?;
        self.next_slot = next_slot;
        self.next_symbol_seq = next_symbol_seq;
        self.artifacts = staged_artifacts;
        self.modules.extend(staged_modules);
        let reachable = crate::backend::reachability::compute_reachable_function_ids(
            self.compiler.functions(),
            &self.required_emit_roots,
        );
        self.artifacts
            .retain(|artifact| reachable.contains(&artifact.function_id));
        let report = CompileReport { index, emit };
        self.rebuild_artifact_index();
        self.refresh_runtime_dispatch_table();
        Ok(report)
    }

    pub fn artifacts(&self) -> &[JitArtifact] {
        &self.artifacts
    }

    pub fn activate_runtime_dispatch_table(&self) {
        self.refresh_runtime_dispatch_table();
    }

    pub fn artifact_slot_for_function_name(&self, name: &str) -> Option<u32> {
        let function = self
            .compiler
            .functions()
            .iter()
            .find(|function| function.name == name)?;
        self.artifact_for_function_id(function.id)
            .map(|artifact| artifact.slot)
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
            .artifact_for_function_id(function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;
        let raw = stasis_dynload::invoke_noarg_u64(artifact.code_ptr as usize)?;
        Ok((raw as u32) as i32)
    }

    pub fn execute_void_noarg_by_name(&self, name: &str) -> Result<(), String> {
        let function = self
            .compiler
            .functions()
            .iter()
            .find(|function| function.name == name)
            .ok_or_else(|| format!("function '{name}' not found"))?;
        if function.return_type != TYPE_ID_VOID {
            return Err(format!(
                "function '{name}' is not void-returning (type id {})",
                function.return_type
            ));
        }
        if !function.params.is_empty() {
            return Err(format!(
                "function '{name}' is not a no-argument function (param count {})",
                function.params.len()
            ));
        }
        let artifact = self
            .artifact_for_function_id(function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;
        stasis_dynload::invoke_noarg_void(artifact.code_ptr as usize)
    }

    pub fn read_i32_global_path(&self, path: &str) -> i32 {
        stasis_dynload::stasis_jit_global_i32_load(hash_global_path(path))
    }

    pub fn write_i32_global_path(&self, path: &str, value: i32) {
        stasis_dynload::stasis_jit_global_i32_store(hash_global_path(path), value);
    }

    pub fn execute_bool_noarg_by_name(&self, name: &str) -> Result<bool, String> {
        let function = self
            .compiler
            .functions()
            .iter()
            .find(|function| function.name == name)
            .ok_or_else(|| format!("function '{name}' not found"))?;
        if function.return_type != TYPE_ID_BOOL {
            return Err(format!(
                "function '{name}' is not bool-returning (type id {})",
                function.return_type
            ));
        }
        if !function.params.is_empty() {
            return Err(format!(
                "function '{name}' is not a no-argument function (param count {})",
                function.params.len()
            ));
        }
        let artifact = self
            .artifact_for_function_id(function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;
        let raw = stasis_dynload::invoke_noarg_u64(artifact.code_ptr as usize)?;
        Ok((raw as u32) != 0)
    }

    pub fn execute_i32_twoarg_by_name(
        &self,
        name: &str,
        left: i32,
        right: i32,
    ) -> Result<i32, String> {
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
        if function.params.len() != 2 {
            return Err(format!(
                "function '{name}' is not a two-argument function (param count {})",
                function.params.len()
            ));
        }
        let artifact = self
            .artifact_for_function_id(function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;
        stasis_dynload::invoke_i32_i32_to_i32(artifact.code_ptr as usize, left, right)
    }

    pub fn symbol_code_ptrs(&self) -> BTreeMap<String, u64> {
        let mut symbol_code_ptrs = BTreeMap::new();
        let reachable = crate::backend::reachability::compute_reachable_function_ids(
            self.compiler.functions(),
            &self.required_emit_roots,
        );
        for function in self.compiler.functions() {
            if !reachable.contains(&function.id) {
                continue;
            }
            if let Some(artifact) = self.artifact_for_function_id(function.id) {
                symbol_code_ptrs.insert(function.name.clone(), artifact.code_ptr);
            }
        }
        symbol_code_ptrs
    }

    pub fn validate_on_code_swap_signature(&self) -> Result<(), String> {
        let Some(function) = self
            .compiler
            .functions()
            .iter()
            .find(|function| function.name == "on_code_swap")
        else {
            return Ok(());
        };
        if function.return_type != TYPE_ID_VOID || !function.params.is_empty() {
            return Err(
                "invalid on_code_swap signature; expected function on_code_swap(): void"
                    .to_string(),
            );
        }
        Ok(())
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

        Ok(JitEnginePackage {
            tick_code_ptr,
            render_code_ptr,
            on_code_swap_code_ptr,
            symbol_code_ptrs: self.symbol_code_ptrs(),
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
            .artifact_for_function_id(function.id)
            .ok_or_else(|| format!("compiled artifact missing for required entrypoint '{name}'"))?;
        Ok(artifact.code_ptr)
    }

    fn refresh_runtime_dispatch_table(&self) {
        let mut i32_entries = Vec::new();
        let mut f32_entries = Vec::new();
        let mut code_ptr_entries = Vec::new();
        let type_table = self.compiler.types();
        let empty_struct_fields = NamedStructFieldTypeMap::new();
        let named_struct_field_types = self
            .compile_analysis_cache
            .as_ref()
            .map(|cache| &cache.named_struct_field_types)
            .unwrap_or(&empty_struct_fields);
        let reachable = crate::backend::reachability::compute_reachable_function_ids(
            self.compiler.functions(),
            &self.required_emit_roots,
        );
        for function in self.compiler.functions() {
            if !reachable.contains(&function.id) {
                continue;
            }
            let Some(artifact) = self.artifact_for_function_id(function.id) else {
                continue;
            };
            code_ptr_entries.push((function.id, artifact.code_ptr as usize));
            let abi_arity = abi_word_count_for_params(&function.params, named_struct_field_types);
            let Ok(arity) = u8::try_from(abi_arity) else {
                continue;
            };
            if arity > 8 {
                continue;
            }
            if is_i32_abi_compatible_type(function.return_type, type_table)
                && (function
                    .params
                    .iter()
                    .all(|type_id| is_i32_abi_compatible_type(*type_id, type_table))
                    || function
                        .params
                        .iter()
                        .all(|type_id| *type_id == TYPE_ID_F32))
            {
                i32_entries.push((function.id, arity, artifact.code_ptr as usize));
            } else if function.return_type == TYPE_ID_VOID && function.params.is_empty() {
                i32_entries.push((function.id, arity, artifact.code_ptr as usize));
            } else if function.return_type == TYPE_ID_F32
                && (function
                    .params
                    .iter()
                    .all(|type_id| *type_id == TYPE_ID_F32)
                    || (function.params.len() == 1
                        && is_i32_abi_compatible_type(function.params[0], type_table)
                        && abi_arity == 1))
            {
                f32_entries.push((function.id, arity, artifact.code_ptr as usize));
            }
        }
        stasis_dynload::replace_jit_i32_dispatch_table(&i32_entries);
        stasis_dynload::replace_jit_f32_dispatch_table(&f32_entries);
        stasis_dynload::replace_jit_code_ptr_table(&code_ptr_entries);
    }

    fn rebuild_artifact_index(&mut self) {
        self.artifact_index.clear();
        for (index, artifact) in self.artifacts.iter().enumerate() {
            self.artifact_index.insert(artifact.function_id, index);
        }
    }

    fn artifact_for_function_id(&self, function_id: FunctionId) -> Option<&JitArtifact> {
        let index = self.artifact_index.get(&function_id).copied()?;
        self.artifacts.get(index)
    }

    fn resolve_extern_call_signatures(
        &mut self,
        extern_signatures: &[ExternCallSignature],
    ) -> Result<(Vec<ResolvedExternCallSignature>, ExternSymbolAddressMap), String> {
        resolve_extern_call_signatures_with(extern_signatures, |_signature, candidate| {
            self.resolve_host_symbol_address(candidate)
        })
    }

    fn resolve_host_symbol_address(&mut self, symbol: &str) -> Option<usize> {
        if let Some(existing) = self.runtime_symbol_cache.get(symbol).copied() {
            return Some(existing);
        }
        if let Some(address) = builtin_host_symbol_address(symbol) {
            self.runtime_symbol_cache
                .insert(symbol.to_string(), address);
            return Some(address);
        }
        self.ensure_runtime_libraries_loaded();
        for library in &self.runtime_libraries {
            if let Ok(address) = library.symbol_address(symbol) {
                self.runtime_symbol_cache
                    .insert(symbol.to_string(), address);
                return Some(address);
            }
        }
        None
    }

    fn ensure_runtime_libraries_loaded(&mut self) {
        if !self.runtime_libraries.is_empty() {
            return;
        }
        for path in stasis_dynload::runtime_library_candidate_paths() {
            if !path.exists() {
                continue;
            }
            if let Ok(library) = stasis_dynload::Library::load(&path) {
                self.runtime_libraries.push(library);
            }
        }
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
            let Some((source_hash, source)) = self
                .compiler
                .files()
                .iter()
                .find(|file| file.path == path)
                .map(|file| (file.hash, file.content.clone()))
            else {
                continue;
            };
            let imports = self.cached_import_paths_for_source(&path, source_hash, &source);
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

        self.import_parse_cache
            .retain(|path, _| known_paths.contains(path));
        Ok(())
    }

    fn cached_import_paths_for_source(
        &mut self,
        path: &str,
        source_hash: u64,
        source: &str,
    ) -> Vec<String> {
        if let Some(entry) = self.import_parse_cache.get(path) {
            if entry.source_hash == source_hash {
                return entry.import_paths.clone();
            }
        }
        let import_paths = parse_import_paths(source);
        self.import_parse_cache.insert(
            path.to_string(),
            ImportParseCacheEntry {
                source_hash,
                import_paths: import_paths.clone(),
            },
        );
        import_paths
    }
}

impl Default for JitProcess {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
fn acquire_jit_process_test_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

use super::emit::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceDiskProbe {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportParseCacheEntry {
    source_hash: u64,
    import_paths: Vec<String>,
}

fn probe_disk_source(path: &Path) -> Option<SourceDiskProbe> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok();
    Some(SourceDiskProbe {
        len: metadata.len(),
        modified,
    })
}

fn select_emit_function_ids(
    functions: &[FunctionMeta],
    artifacts: &[JitArtifact],
    required_emit_roots: &[String],
    force_reemit_reachable: bool,
) -> Vec<FunctionId> {
    let compiled_body_hashes: HashMap<FunctionId, u64> = artifacts
        .iter()
        .map(|artifact| (artifact.function_id, artifact.body_hash))
        .collect();
    crate::backend::emit::select_emit_function_ids(
        functions,
        required_emit_roots,
        &compiled_body_hashes,
        force_reemit_reachable,
    )
}

fn function_address(function: *const ()) -> usize {
    function as usize
}

fn builtin_host_symbol_address(symbol: &str) -> Option<usize> {
    let address = match symbol {
        "print_i32" | "stasis_jit_print_i32" => {
            function_address(stasis_dynload::stasis_jit_print_i32 as *const ())
        }
        "print_string" | "stasis_jit_print_string" => {
            function_address(stasis_dynload::stasis_jit_print_string as *const ())
        }
        "gfx_load_sprite" | "stasis_gfx_load_sprite" => {
            function_address(stasis_dynload::stasis_jit_gfx_load_sprite as *const ())
        }
        "gfx_dump_bmp" | "stasis_gfx_dump_bmp" => {
            function_address(stasis_dynload::stasis_jit_gfx_dump_bmp as *const ())
        }
        "load_font" | "stasis_load_font" => {
            function_address(stasis_dynload::stasis_jit_load_font as *const ())
        }
        "measure_text" | "stasis_measure_text" => {
            function_address(stasis_dynload::stasis_jit_measure_text as *const ())
        }
        "gfx_cache_text" | "stasis_gfx_cache_text" => {
            function_address(stasis_dynload::stasis_jit_gfx_cache_text as *const ())
        }
        "audio_is_available" | "stasis_audio_is_available" => {
            function_address(stasis_dynload::stasis_jit_audio_is_available as *const ())
        }
        "audio_push_f32_interleaved" | "stasis_audio_push_f32_interleaved" => {
            function_address(stasis_dynload::stasis_jit_audio_push_f32_interleaved as *const ())
        }
        "sin_fast" | "stasis_jit_sin_fast" => {
            function_address(stasis_dynload::stasis_jit_sin_fast as *const ())
        }
        "cos_fast" | "stasis_jit_cos_fast" => {
            function_address(stasis_dynload::stasis_jit_cos_fast as *const ())
        }
        "stasis_jit_global_i32_load" => {
            function_address(stasis_dynload::stasis_jit_global_i32_load as *const ())
        }
        "stasis_jit_global_i32_store" => {
            function_address(stasis_dynload::stasis_jit_global_i32_store as *const ())
        }
        "stasis_jit_global_f32_load" => {
            function_address(stasis_dynload::stasis_jit_global_f32_load as *const ())
        }
        "stasis_jit_global_f32_store" => {
            function_address(stasis_dynload::stasis_jit_global_f32_store as *const ())
        }
        "stasis_jit_global_f64_load" => {
            function_address(stasis_dynload::stasis_jit_global_f64_load as *const ())
        }
        "stasis_jit_global_f64_store" => {
            function_address(stasis_dynload::stasis_jit_global_f64_store as *const ())
        }
        "stasis_jit_collection_i32_load" => {
            function_address(stasis_dynload::stasis_jit_collection_i32_load as *const ())
        }
        "stasis_jit_collection_i32_store" => {
            function_address(stasis_dynload::stasis_jit_collection_i32_store as *const ())
        }
        "sys_memcpy_u8" | "stasis_jit_sys_memcpy_u8" => {
            function_address(stasis_dynload::stasis_jit_sys_memcpy_u8 as *const ())
        }
        "sys_memcpy_i32" | "stasis_jit_sys_memcpy_i32" => {
            function_address(stasis_dynload::stasis_jit_sys_memcpy_i32 as *const ())
        }
        "sys_memcpy_f32" | "stasis_jit_sys_memcpy_f32" => {
            function_address(stasis_dynload::stasis_jit_sys_memcpy_f32 as *const ())
        }
        "sys_memmove_u8" | "stasis_jit_sys_memmove_u8" => {
            function_address(stasis_dynload::stasis_jit_sys_memmove_u8 as *const ())
        }
        "sys_memmove_i32" | "stasis_jit_sys_memmove_i32" => {
            function_address(stasis_dynload::stasis_jit_sys_memmove_i32 as *const ())
        }
        "sys_memmove_f32" | "stasis_jit_sys_memmove_f32" => {
            function_address(stasis_dynload::stasis_jit_sys_memmove_f32 as *const ())
        }
        _ => return None,
    };
    Some(address)
}

fn seed_fixed_collection_max_length_headers(
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> Result<(), String> {
    for (path, type_id) in global_path_types {
        let Some(type_info) = type_table.type_info(*type_id) else {
            continue;
        };
        match type_info.category {
            TypeCategory::AsciiFixed => {
                let Some(payload_bytes) = type_info.layout.payload_size_bytes else {
                    continue;
                };
                let max_length = i32::try_from(payload_bytes).map_err(|_| {
                    format!(
                        "ascii max_length overflow for '{}' (payload bytes {})",
                        path, payload_bytes
                    )
                })?;
                seed_collection_max_length(path, max_length);
            }
            TypeCategory::Utf8Fixed => {
                let Some(payload_bytes) = type_info.layout.payload_size_bytes else {
                    continue;
                };
                let max_length = i32::try_from(payload_bytes).map_err(|_| {
                    format!(
                        "utf8 max_length overflow for '{}' (payload bytes {})",
                        path, payload_bytes
                    )
                })?;
                seed_collection_max_length(path, max_length);
            }
            TypeCategory::ArrayFixed => {
                let Some(max_length) = type_table.fixed_collection_len(*type_id) else {
                    continue;
                };
                seed_collection_max_length(path, max_length);
            }
            _ => {}
        }
    }
    Ok(())
}

fn seed_collection_max_length(path: &str, max_length: i32) {
    let max_length_path = format!("{path}.max_length");
    stasis_dynload::stasis_jit_global_i32_store(hash_global_path(&max_length_path), max_length);
}

fn compile_function_to_jit_module(
    meta: &FunctionMeta,
    hir: &FunctionHIR,
    symbol: &str,
    call_signatures: &CallSignatureMap,
    type_table: &mut TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    extern_symbol_addresses: &ExternSymbolAddressMap,
) -> Result<(JITModule, u64), String> {
    let mut jit_builder = JITBuilder::new(default_libcall_names())
        .map_err(|error| format!("failed to construct JIT builder: {error}"))?;
    jit_builder.symbol(
        "stasis_jit_call_i32_0",
        stasis_dynload::stasis_jit_call_i32_0 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_1",
        stasis_dynload::stasis_jit_call_i32_1 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_2",
        stasis_dynload::stasis_jit_call_i32_2 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_3",
        stasis_dynload::stasis_jit_call_i32_3 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_4",
        stasis_dynload::stasis_jit_call_i32_4 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_5",
        stasis_dynload::stasis_jit_call_i32_5 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_6",
        stasis_dynload::stasis_jit_call_i32_6 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_7",
        stasis_dynload::stasis_jit_call_i32_7 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_8",
        stasis_dynload::stasis_jit_call_i32_8 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_f32_1",
        stasis_dynload::stasis_jit_call_i32_f32_1 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_f32_2",
        stasis_dynload::stasis_jit_call_i32_f32_2 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_f32_3",
        stasis_dynload::stasis_jit_call_i32_f32_3 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_f32_4",
        stasis_dynload::stasis_jit_call_i32_f32_4 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_f32_5",
        stasis_dynload::stasis_jit_call_i32_f32_5 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_f32_6",
        stasis_dynload::stasis_jit_call_i32_f32_6 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_f32_7",
        stasis_dynload::stasis_jit_call_i32_f32_7 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_i32_f32_8",
        stasis_dynload::stasis_jit_call_i32_f32_8 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_f32_0",
        stasis_dynload::stasis_jit_call_f32_0 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_f32_1",
        stasis_dynload::stasis_jit_call_f32_1 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_f32_2",
        stasis_dynload::stasis_jit_call_f32_2 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_f32_3",
        stasis_dynload::stasis_jit_call_f32_3 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_f32_4",
        stasis_dynload::stasis_jit_call_f32_4 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_f32_5",
        stasis_dynload::stasis_jit_call_f32_5 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_f32_6",
        stasis_dynload::stasis_jit_call_f32_6 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_f32_7",
        stasis_dynload::stasis_jit_call_f32_7 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_f32_8",
        stasis_dynload::stasis_jit_call_f32_8 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_call_f32_i32_1",
        stasis_dynload::stasis_jit_call_f32_i32_1 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_print_i32",
        stasis_dynload::stasis_jit_print_i32 as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_print_string",
        stasis_dynload::stasis_jit_print_string as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_lookup_code_ptr",
        stasis_dynload::stasis_jit_lookup_code_ptr as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_sin_fast",
        stasis_dynload::stasis_jit_sin_fast as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_cos_fast",
        stasis_dynload::stasis_jit_cos_fast as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_i32_load",
        stasis_dynload::stasis_jit_global_i32_load as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_i32_store",
        stasis_dynload::stasis_jit_global_i32_store as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_f32_load",
        stasis_dynload::stasis_jit_global_f32_load as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_f32_store",
        stasis_dynload::stasis_jit_global_f32_store as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_f64_load",
        stasis_dynload::stasis_jit_global_f64_load as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_f64_store",
        stasis_dynload::stasis_jit_global_f64_store as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_collection_i32_load",
        stasis_dynload::stasis_jit_collection_i32_load as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_collection_i32_store",
        stasis_dynload::stasis_jit_collection_i32_store as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_i32_array_load",
        stasis_dynload::stasis_jit_global_i32_array_load as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_i32_array_store",
        stasis_dynload::stasis_jit_global_i32_array_store as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_i32_array_ptr",
        stasis_dynload::stasis_jit_global_i32_array_ptr as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_f32_array_load",
        stasis_dynload::stasis_jit_global_f32_array_load as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_f32_array_store",
        stasis_dynload::stasis_jit_global_f32_array_store as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_f32_array_ptr",
        stasis_dynload::stasis_jit_global_f32_array_ptr as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_f64_array_load",
        stasis_dynload::stasis_jit_global_f64_array_load as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_f64_array_store",
        stasis_dynload::stasis_jit_global_f64_array_store as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_f64_array_ptr",
        stasis_dynload::stasis_jit_global_f64_array_ptr as *const u8,
    );
    for (extern_symbol, address) in extern_symbol_addresses {
        if *address == 0 {
            continue;
        }
        jit_builder.symbol(extern_symbol, *address as *const u8);
    }
    compile_function_with_module(
        JITModule::new(jit_builder),
        meta,
        hir,
        symbol,
        SharedCompileBackendMode::Jit,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        |_| Ok(()),
        |_, _| {},
        |mut module, function_id, mut context| {
            module
                .define_function(function_id, &mut context)
                .map_err(|error| format!("failed to define JIT function {symbol}: {error}"))?;
            module.clear_context(&mut context);
            module
                .finalize_definitions()
                .map_err(|error| format!("failed to finalize JIT definitions: {error}"))?;
            let code_ptr = module.get_finalized_function(function_id) as usize as u64;
            Ok((module, code_ptr))
        },
    )
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
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
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
            "function main(): i32 { return helper(1, 2, 3); }\n",
        );
        let error = process.compile().expect_err("expected compile error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("unknown call target 'helper'")
                        || message.contains("unsupported call arity 3"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_two_arg_i32_function_in_memory() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function add_pair(left: i32, right: i32): i32 { return left + right; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_twoarg_by_name("add_pair", 4, 6)
            .expect("execute two-arg function");
        assert_eq!(value, 10);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_global_path_set_and_read() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct State { rng_state: i32; }\nglobal state: State;\nfunction set_seed(): i32 { state.rng_state = 7; return state.rng_state; }\nfunction main(): i32 { return set_seed(); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_call_expression_statement() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global State { value: i32; }\nfunction set_value(): i32 { State.value = 9; return 0; }\nfunction main(): i32 { set_value(); return State.value; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_void_function_body_and_call_statement() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global State { value: i32; }\nfunction set_value(): void { State.value = 12; return; }\nfunction main(): i32 { set_value(); return State.value; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 12);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_void_call_with_mixed_f32_i32_arguments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global State { value: i32; }\nfunction bump(dt: f32, delta: i32): void { if (dt > 0.0) { State.value += delta; } return; }\nfunction main(): i32 { bump(1.0, 5); return State.value; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 5);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_print_string_literal_statement() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { print_string(\"hello\\n\"); print_i32(7); return 1; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_string_constant_identifier_argument() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const UI_FONT_PATH: string = \"../../docs/assets/fonts/dejavu-sans-mono.ttf\";\nfunction consume(path: string): i32 { return 5; }\nfunction main(): i32 { return consume(UI_FONT_PATH); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 5);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_ascii_constant_identifier_argument() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const TITLE: ascii[] = \"play\";\nfunction consume(path: ascii[]): i32 { return 6; }\nfunction main(): i32 { return consume(TITLE); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 6);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_utf8_literal_for_ascii_parameter_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function take_ascii(value: ascii[]): i32 { return 7; }\nfunction main(): i32 { return take_ascii(\"hi\"); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_non_ascii_utf8_string_literal_argument() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function take_text(value: utf8[]): i32 { return 9; }\nfunction main(): i32 { return take_text(\"cafÃƒÂ© Ã¢Ëœâ€¢\"); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_block_comments_between_statements() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n    let value: i32 = 2;\n    /* increment path */\n    value += 5;\n    return value;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_string_literal_with_semicolon_in_call_statement() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n    print_string(\"alpha; beta {x}\");\n    return 1;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_control_flow_comments_near_delimiters() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n\
                let sum: i32 = 0;\n\
                for /* before header */ (let i: i32 = 0; i < 3; i += 1) /* before body */ {\n\
                    sum += i;\n\
                }\n\
                if /* before condition */ (sum == 3) /* before then */ {\n\
                    return 1;\n\
                } else /* between else and body */ {\n\
                    return 0;\n\
                }\n\
            }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_comments_inside_expression_arithmetic() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n\
                let value: i32 = 1 /* plus */ + 2;\n\
                let value2: i32 = value + // trailing comment\n\
                    1;\n\
                return value2;\n\
            }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 4);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_comments_inside_condition_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n\
                let value: i32 = 4;\n\
                if (value /* lhs */ == /* rhs */ 4) { return 1; }\n\
                return 0;\n\
            }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_for_header_comment_with_semicolon() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n\
                let sum: i32 = 0;\n\
                for (let i: i32 = 0 /* ; in comment */; i < 4; i += 1) {\n\
                    sum += i;\n\
                }\n\
                return sum;\n\
            }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 6);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_ignores_logical_operator_text_inside_comments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n\
                let value: i32 = 2;\n\
                if ((value > 0 /* || hidden */) && (value < 3 /* && hidden */)) {\n\
                    return 1;\n\
                }\n\
                return 0;\n\
            }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_print_int_alias_statement() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { print_int(3); return 1; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_print_char_alias_statement() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { print_char(10); return 1; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_global_path_compound_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct State { counter: i32; }\nglobal state: State;\nfunction main(): i32 { state.counter = 10; state.counter -= 3; return state.counter; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_typed_f32_global_path_set_and_read() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Layout { width: f32; }\nglobal state: Layout;\nfunction main(): i32 { state.width = 3.5; let w: f32 = state.width; if (w > 3.0) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_typed_f64_global_path_set_and_read() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Layout { width: f64; }\nglobal state: Layout;\nfunction main(): i32 { state.width = 3.5; let w: f64 = state.width; if (w > 3.0) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_f64_array_set_and_read() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global values: f64[4];\nfunction main(): i32 { values[0] = 3.5; let w: f64 = values[0]; if (w > 3.0) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_f64_return_call_and_conversions() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(value: f64): f64 { return value + 1.0; }\nfunction main(): i32 {\n    let src: i32 = 8;\n    let x: f64 = 0.0;\n    x.from_i32(src);\n    let v: f64 = helper(x);\n    let out: i32 = 0;\n    out.from_f64(v);\n    return out;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_global_path_from_i32_conversion_target() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct State { src: i32; value: f32; }\nglobal state: State;\nfunction main(): i32 { state.src = 8; state.value.from_i32(state.src); let out: i32 = 0; out.from_f32(state.value); return out; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 8);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_collection_handle_rebind_with_set_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global left: i32[4];\nglobal right: i32[4];\nfunction main(): i32 {\n    let view: i32[] = left;\n    view[0] = 65;\n    view = right;\n    view[0] = 66;\n    return left[0] * 100 + right[0];\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 6566);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_typed_ascii_view_let_binding() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global text: ascii[4];\nfunction main(): i32 {\n    let view: ascii[] = text;\n    view[0] = 90;\n    return text[0];\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 90);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_collection_handle_compound_assignment_for_local() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global left: i32[4];\nglobal right: i32[4];\nfunction main(): i32 {\n    let view: i32[] = left;\n    view += right;\n    return 0;\n}\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("collection handle assignment only supports '='"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_indexed_path_from_i32_conversion_target() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Node { value: f32; }\nglobal nodes: Node[COUNT];\nfunction main(): i32 { nodes[1].value.from_i32(7); let out: i32 = 0; out.from_f32(nodes[1].value); return out; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_string_header_length_path() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global hud_line: utf8[64];\nfunction main(): i32 { hud_line.length = 7; return hud_line.length; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_initializes_string_max_length_headers() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global a: ascii[32];\nglobal u: utf8[64];\nfunction main(): i32 {\n    return a.max_length + u.max_length;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 96);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_initializes_fixed_array_max_length_header_and_path() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global values: i32[12];\nfunction main(): i32 {\n    return values.max_length;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 12);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_ascii_copy_truncates_to_destination_capacity() {
        let mut process = JitProcess::new();
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("jit_stdlib_ascii_copy_sample.stasis");
        process.upsert_file(
            sample_path.to_string_lossy().to_string(),
            "import \"src/stdlib/stdlib.stasis\";\nglobal src_text: ascii[8];\nglobal dst_text: ascii[4];\nfunction main(): i32 {\n    src_text[0] = 65;\n    src_text[1] = 66;\n    src_text[2] = 67;\n    src_text[3] = 68;\n    src_text[4] = 69;\n    src_text[5] = 0;\n    ascii_set_len(src_text, 5);\n    ascii_copy(dst_text, src_text);\n    return length(dst_text) * 10 + dst_text[3];\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 30);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_ascii_recount_is_bounded_by_capacity() {
        let mut process = JitProcess::new();
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("jit_stdlib_ascii_recount_sample.stasis");
        process.upsert_file(
            sample_path.to_string_lossy().to_string(),
            "import \"src/stdlib/stdlib.stasis\";\nglobal text: ascii[4];\nfunction main(): i32 {\n    text[0] = 65;\n    text[1] = 66;\n    text[2] = 67;\n    text[3] = 68;\n    ascii_set_len(text, 0);\n    return ascii_recount(text);\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 4);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_utf8_from_ascii_clamps_to_header_capacity() {
        let mut process = JitProcess::new();
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("jit_stdlib_utf8_from_ascii_capacity_sample.stasis");
        process.upsert_file(
            sample_path.to_string_lossy().to_string(),
            "import \"src/stdlib/stdlib.stasis\";\nglobal src_text: ascii[8];\nglobal dst_text: utf8[4];\nfunction main(): i32 {\n    src_text[0] = 65;\n    src_text[1] = 66;\n    src_text[2] = 67;\n    src_text[3] = 68;\n    src_text[4] = 69;\n    src_text[5] = 0;\n    ascii_set_len(src_text, 5);\n    let written: i32 = utf8_from_ascii(dst_text, src_text, 99);\n    return written * 1000 + length_bytes(dst_text) * 100 + length_chars(dst_text) * 10 + dst_text[3];\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 3330);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_ascii_set_len_clamps_to_max_length() {
        let mut process = JitProcess::new();
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("jit_stdlib_ascii_set_len_clamp_sample.stasis");
        process.upsert_file(
            sample_path.to_string_lossy().to_string(),
            "import \"src/stdlib/stdlib.stasis\";\nglobal text: ascii[4];\nfunction main(): i32 {\n    ascii_set_len(text, 99);\n    return length(text);\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 4);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_utf8_set_len_ascii_clamps_to_max_length() {
        let mut process = JitProcess::new();
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("jit_stdlib_utf8_set_len_clamp_sample.stasis");
        process.upsert_file(
            sample_path.to_string_lossy().to_string(),
            "import \"src/stdlib/stdlib.stasis\";\nglobal text: utf8[4];\nfunction main(): i32 {\n    utf8_set_len_ascii(text, 99);\n    return length_bytes(text) * 10 + length_chars(text);\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 44);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_utf8_from_ascii_respects_source_capacity_without_terminator() {
        let mut process = JitProcess::new();
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("jit_stdlib_utf8_from_ascii_src_cap_sample.stasis");
        process.upsert_file(
            sample_path.to_string_lossy().to_string(),
            "import \"src/stdlib/stdlib.stasis\";\nglobal src_text: ascii[4];\nglobal dst_text: utf8[8];\nfunction main(): i32 {\n    src_text[0] = 65;\n    src_text[1] = 66;\n    src_text[2] = 67;\n    src_text[3] = 68;\n    ascii_set_len(src_text, 0);\n    let written: i32 = utf8_from_ascii(dst_text, src_text, 8);\n    return written * 100 + length_bytes(dst_text) * 10 + dst_text[4];\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 440);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_foreach_struct_array_with_index_alias() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal state: Enemy[COUNT];\nfunction main(): i32 {\n    foreach (let enemy, i in state) { enemy.hp = i + 1; }\n    let sum: i32 = 0;\n    foreach (let enemy in state) { sum += enemy.hp; }\n    return sum;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 6);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_foreach_struct_array_without_let_header_style() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal state: Enemy[COUNT];\nfunction main(): i32 {\n    foreach (i, enemy in state) { enemy.hp = i + 1; }\n    let sum: i32 = 0;\n    foreach (enemy in state) { sum += enemy.hp; }\n    return sum;\n}\n",
        );
        let error = process
            .compile()
            .expect_err("compile should reject non-let foreach");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("foreach header must start with 'let'"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_foreach_over_local_fixed_array_parameter() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 4;\nglobal nums: i32[COUNT];\nfunction init(arr: i32[4]): i32 {\n    foreach (let v, i in arr) { arr[i] = i + 1; }\n    let sum: i32 = 0;\n    foreach (let v in arr) { sum += v; }\n    return sum;\n}\nfunction main(): i32 { return init(nums); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 10);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_foreach_over_local_fixed_array_without_let_header_style() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 4;\nglobal nums: i32[COUNT];\nfunction init(arr: i32[4]): i32 {\n    foreach (i, v in arr) { arr[i] = i + 1; }\n    let sum: i32 = 0;\n    foreach (v in arr) { sum += v; }\n    return sum;\n}\nfunction main(): i32 { return init(nums); }\n",
        );
        let error = process
            .compile()
            .expect_err("compile should reject non-let foreach");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("foreach header must start with 'let'"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_foreach_struct_array_f32_fields() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Node { x: f32; }\nglobal nodes: Node[COUNT];\nfunction main(): i32 {\n    foreach (let node, i in nodes) { let fi: f32 = 0.0; fi.from_i32(i); node.x = fi + 1.5; }\n    let total: f32 = 0.0;\n    foreach (let node in nodes) { total = total + node.x; }\n    if (total > 3.9) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_tick_from_stasis_fixture_with_input_snapshot() {
        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("stasis")
            .join("rust_native_tick_input_snapshot.stasis");
        let source = std::fs::read_to_string(&fixture_path)
            .expect("read rust_native_tick_input_snapshot.stasis fixture");

        let mut process = JitProcess::new();
        process.upsert_file(fixture_path.to_string_lossy().to_string(), source);
        process.compile().expect("compile");

        let snapshot_mode_hash = hash_global_path("snapshot_mode");
        stasis_dynload::stasis_jit_global_i32_store(snapshot_mode_hash, 1);

        let tick_value = process
            .execute_i32_noarg_by_name("tick")
            .expect("execute tick");
        assert_eq!(tick_value, 1);

        let pointer_count =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("model_pointer_count"));
        let escape_down =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("model_escape_down"));
        let first_went_down =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("model_first_went_down"));
        let first_x =
            stasis_dynload::stasis_jit_global_f32_load(hash_global_path("model_first_x_px"));
        let latched =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("last_tick_code"));
        assert_eq!(pointer_count, 1);
        assert_eq!(escape_down, 1);
        assert_eq!(first_went_down, 1);
        assert!(first_x > 12.0);
        assert_eq!(latched, 1);

        stasis_dynload::stasis_jit_global_i32_store(snapshot_mode_hash, 2);

        let quit_tick = process
            .execute_i32_noarg_by_name("tick")
            .expect("execute quit tick");
        assert_eq!(quit_tick, -1);
        let quit_latched =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("last_tick_code"));
        assert_eq!(quit_latched, -1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_indexed_i32_array_access() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 4;\nglobal nums: i32[COUNT];\nfunction main(): i32 {\n    nums[0] = 2;\n    nums[1] = 5;\n    let i: i32 = 1;\n    nums[i] += 3;\n    return nums[0] + nums[1];\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 10);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_indexed_i32_array_parameter_access() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nglobal nums: i32[COUNT];\nfunction read_at(arr: i32[], idx: i32): i32 { return arr[idx]; }\nfunction write_at(arr: i32[], idx: i32, value: i32): void { arr[idx] = value; return; }\nfunction main(): i32 {\n    nums[1] = 9;\n    write_at(nums, 2, 11);\n    return read_at(nums, 1) + read_at(nums, 2);\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 20);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_indexed_struct_array_parameter_field_access() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal enemies: Enemy[COUNT];\nfunction mutate(arr: Enemy[3], idx: i32): i32 {\n    arr[idx].hp = 10;\n    arr[idx + 1].hp = arr[idx].hp + 4;\n    return arr[idx + 1].hp;\n}\nfunction main(): i32 { return mutate(enemies, 0); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 14);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_indexed_struct_array_view_parameter_field_access() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal enemies: Enemy[COUNT];\nfunction mutate(arr: Enemy[], idx: i32): i32 {\n    arr[idx].hp = 10;\n    arr[idx + 1].hp = arr[idx].hp + 4;\n    return arr[idx + 1].hp;\n}\nfunction main(): i32 { return mutate(enemies, 0); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 14);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_struct_parameter_field_access() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Pipe { active: bool; }\nglobal pipe: Pipe;\nfunction read_active(p: Pipe): i32 { if (p.active) { return 1; } return 0; }\nfunction main(): i32 { pipe.active = true; return read_active(pipe); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_i32_to_f32_intrinsic_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let x: f32 = i32_to_f32(7); if (x > 6.9) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_continue_in_for_loop() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let count: i32 = 0; for (let i: i32 = 0; i < 5; i += 1) { if (i == 2) { continue; } count += 1; } return count; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 4);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_indexed_struct_element_alias_binding() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Enemy { hp: i32; }\nglobal enemies: Enemy[2];\nfunction bad(arr: Enemy[2]): i32 {\n    let enemy = arr[0];\n    enemy.hp = 10;\n    return arr[0].hp;\n}\nfunction main(): i32 { return bad(enemies); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 10);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_local_indexed_struct_element_in_scalar_context() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Enemy { hp: i32; }\nglobal enemies: Enemy[2];\nfunction bad(arr: Enemy[2]): i32 { return arr[0]; }\nfunction main(): i32 { return bad(enemies); }\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains(
                        "local indexed collection access requires field path for struct elements"
                    ),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_foreach_over_local_struct_array_parameter() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal enemies: Enemy[COUNT];\nfunction sum_fill(arr: Enemy[3]): i32 {\n    foreach (let enemy, i in arr) { enemy.hp = i + 2; }\n    let total: i32 = 0;\n    foreach (let enemy in arr) { total += enemy.hp; }\n    return total;\n}\nfunction main(): i32 { return sum_fill(enemies); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_indexed_struct_field_access() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal state: Enemy[COUNT];\nfunction main(): i32 {\n    let i: i32 = 1;\n    state[i].hp = 7;\n    return state[i].hp;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_indexed_struct_value_copy_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Enemy { hp: i32; speed: f32; }\nglobal enemies: Enemy[COUNT];\nfunction main(): i32 {\n    enemies[0].hp = 11;\n    enemies[0].speed = 2.5;\n    enemies[1] = enemies[0];\n    if (enemies[1].speed > 2.4) { return enemies[1].hp; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 11);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_indexed_struct_value_copy_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Enemy { hp: i32; speed: f32; }\nglobal enemies: Enemy[COUNT];\nfunction copy_local(arr: Enemy[2]): i32 {\n    arr[0].hp = 11;\n    arr[0].speed = 2.5;\n    arr[1] = arr[0];\n    if (arr[1].speed > 2.4) { return arr[1].hp; }\n    return 0;\n}\nfunction main(): i32 { return copy_local(enemies); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 11);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_indexed_struct_value_copy_assignment_for_view_param() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Enemy { hp: i32; speed: f32; }\nglobal enemies: Enemy[COUNT];\nfunction copy_local(arr: Enemy[]): i32 {\n    arr[0].hp = 11;\n    arr[0].speed = 2.5;\n    arr[1] = arr[0];\n    if (arr[1].speed > 2.4) { return arr[1].hp; }\n    return 0;\n}\nfunction main(): i32 { return copy_local(enemies); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 11);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_local_indexed_struct_copy_assignment_for_mismatched_layouts() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct A { hp: i32; }\nstruct B { hp: i32; speed: f32; }\nglobal lhs: A[2];\nglobal rhs: B[2];\nfunction copy(arr_lhs: A[2], arr_rhs: B[2]): i32 { arr_rhs[0] = arr_lhs[0]; return 0; }\nfunction main(): i32 { return copy(lhs, rhs); }\n",
        );
        let error = process
            .compile()
            .expect_err("compile should reject local mismatched indexed struct copy assignment");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains(
                        "struct indexed copy assignment requires matching field layout for 'arr_rhs[...]' and 'arr_lhs[...]'"
                    ),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_local_indexed_struct_copy_compound_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Enemy { hp: i32; }\nglobal enemies: Enemy[2];\nfunction bad(arr: Enemy[2]): i32 { arr[1] += arr[0]; return 0; }\nfunction main(): i32 { return bad(enemies); }\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains(
                        "struct indexed copy assignment only supports '=' for 'arr[...]'"
                    ),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_global_struct_path_value_copy_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Pos { x: f32; y: f32; }\nstruct Enemy { hp: i32; pos: Pos; }\nglobal src: Enemy;\nglobal dst: Enemy;\nfunction main(): i32 {\n    src.hp = 13;\n    src.pos.x = 4.5;\n    src.pos.y = 2.0;\n    dst = src;\n    if (dst.pos.x > 4.4) { return dst.hp; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 13);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_global_block_nested_struct_path_copy_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Enemy { hp: i32; speed: f32; }\nglobal state { src: Enemy; dst: Enemy; }\nfunction main(): i32 {\n    state.src.hp = 9;\n    state.src.speed = 3.25;\n    state.dst = state.src;\n    if (state.dst.speed > 3.2) { return state.dst.hp; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_struct_copy_from_indexed_to_global_path() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Enemy { hp: i32; speed: f32; }\nglobal source: Enemy[COUNT];\nglobal target: Enemy;\nfunction main(): i32 {\n    source[1].hp = 6;\n    source[1].speed = 2.75;\n    target = source[1];\n    if (target.speed > 2.7) { return target.hp; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 6);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_struct_copy_from_global_to_indexed_path() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Enemy { hp: i32; speed: f32; }\nglobal source: Enemy;\nglobal target: Enemy[COUNT];\nfunction main(): i32 {\n    source.hp = 8;\n    source.speed = 4.25;\n    target[1] = source;\n    if (target[1].speed > 4.2) { return target[1].hp; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 8);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_struct_copy_from_indexed_to_global_on_layout_mismatch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 1;\nstruct Src { hp: i32; armor: i32; }\nstruct Dst { hp: i32; }\nglobal source: Src[COUNT];\nglobal target: Dst;\nfunction main(): i32 {\n    target = source[0];\n    return 0;\n}\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains(
                        "struct copy assignment from indexed source requires matching field layout"
                    ),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_struct_copy_from_global_to_indexed_on_layout_mismatch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 1;\nstruct Src { hp: i32; }\nstruct Dst { hp: i32; armor: i32; }\nglobal source: Src;\nglobal target: Dst[COUNT];\nfunction main(): i32 {\n    target[0] = source;\n    return 0;\n}\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains(
                        "struct copy assignment to indexed target requires matching field layout"
                    ),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_evaluates_struct_copy_indices_once_each() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Enemy { hp: i32; speed: f32; }\nglobal enemies: Enemy[COUNT];\nglobal target_calls: i32;\nglobal source_calls: i32;\nfunction next_target(): i32 { target_calls += 1; return 1; }\nfunction next_source(): i32 { source_calls += 1; return 0; }\nfunction main(): i32 {\n    enemies[0].hp = 9;\n    enemies[0].speed = 3.5;\n    enemies[next_target()] = enemies[next_source()];\n    if (enemies[1].speed > 3.4) { return target_calls * 100 + source_calls * 10 + enemies[1].hp; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 119);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_indexed_struct_copy_assignment_for_mismatched_layouts() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 1;\nstruct One { hp: i32; }\nstruct Two { hp: i32; armor: i32; }\nglobal a: One[COUNT];\nglobal b: Two[COUNT];\nfunction main(): i32 {\n    a[0] = b[0];\n    return 0;\n}\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message
                        .contains("struct indexed copy assignment requires matching field layout"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_global_struct_copy_assignment_with_collection_fields() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Player { hp: i32; name: ascii[8]; }\nglobal left: Player;\nglobal right: Player;\nfunction main(): i32 {\n    left = right;\n    return 0;\n}\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains(
                        "struct path copy assignment currently supports scalar fields only"
                    ),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_global_struct_path_copy_compound_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Enemy { hp: i32; }\nglobal a: Enemy;\nglobal b: Enemy;\nfunction main(): i32 {\n    b += a;\n    return 0;\n}\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("struct path copy assignment only supports '='"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_indexed_named_field_assignment_from_enum_variant() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "enum BrickType { Basic, Armored, Reflector }\nconst COUNT: i32 = 2;\nstruct Brick { brick_type: BrickType; hp: i32; }\nglobal bricks: Brick[COUNT];\nfunction place(kind: BrickType): i32 { bricks[0].brick_type = kind; return 0; }\nfunction main(): i32 {\n    let ignored: i32 = place(BrickType.Reflector);\n    if (bricks[0].brick_type == BrickType.Reflector) { return 1; }\n    return ignored;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_named_type_let_binding() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "enum BrickType { Basic, Armored }\nfunction cost(kind: BrickType): i32 { if (kind == BrickType.Armored) { return 2; } return 1; }\nfunction main(): i32 { let t: BrickType = BrickType.Armored; return cost(t); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 2);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_allows_i32_return_from_named_i32_abi_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function as_i32(v: u8): i32 { return v; }\nfunction main(): i32 { let b: u8 = 7; return as_i32(b); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_i32_literal_for_named_i32_abi_parameter() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function as_i32(v: u8): i32 { return v; }\nfunction main(): i32 { return as_i32(7); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_named_i32_abi_binary_arithmetic() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function seed(): u8 { return 60; }\nfunction main(): i32 { let b: u8 = seed(); return b - 48; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 12);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_named_i32_abi_to_f32_coercion_in_binary_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function seed(): u8 { return 2; }\nfunction main(): i32 {\n    let b: u8 = seed();\n    let value: f32 = b + 0.5;\n    if (value > 2.4) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_i32_call_with_five_arguments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function sum5(a: i32, b: i32, c: i32, d: i32, e: i32): i32 { return a + b + c + d + e; }\nfunction main(): i32 { return sum5(1, 2, 3, 4, 5); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 15);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_bool_return_call_with_f32_arguments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function point_in_rect(px: f32, py: f32, rx: f32, ry: f32, rw: f32, rh: f32): bool {\n    if (px < rx) { return false; }\n    if (py < ry) { return false; }\n    if (px > (rx + rw)) { return false; }\n    if (py > (ry + rh)) { return false; }\n    return true;\n}\nfunction main(): i32 {\n    if (point_in_rect(5.0, 6.0, 0.0, 0.0, 10.0, 10.0)) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_f32_return_call_with_four_arguments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function dist2(ax: f32, ay: f32, bx: f32, by: f32): f32 {\n    let dx: f32 = ax - bx;\n    let dy: f32 = ay - by;\n    return (dx * dx) + (dy * dy);\n}\nfunction main(): i32 {\n    let value: f32 = dist2(0.0, 0.0, 3.0, 4.0);\n    if (value > 24.9) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_f32_return_call_with_mixed_arguments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function mix(a: f32, b: i32, c: f32, d: f32): f32 {\n    let bf: f32 = 0.0;\n    bf.from_i32(b);\n    return a + bf + c + d;\n}\nfunction main(): i32 {\n    let value: f32 = mix(1.0, 2, 3.0, 4.0);\n    if (value > 9.9) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_i32_return_call_with_mixed_arguments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function score(base: i32, scale: f32): i32 {\n    let x: i32 = 0;\n    x.from_f32(scale);\n    return base + x;\n}\nfunction main(): i32 { return score(2, 3.0); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 5);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_bool_return_condition_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function fits(size_w: i32, size_h: i32, max_w: i32, max_h: i32): bool {\n    return size_w <= max_w && size_h <= max_h;\n}\nfunction main(): i32 {\n    if (fits(10, 20, 10, 21)) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_sin_fast_intrinsic_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let x: f32 = sin_fast(0.0); if (x < 0.001) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_extern_keyword_function_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "extern function global_i32_load(path_hash: i32): i32;\nfunction main(): i32 { return global_i32_load(123) + 1; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_annotated_extern_function_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function @extern(\"stasis_jit_global_i32_load\") host_load(path_hash: i32): i32;\nfunction main(): i32 { return host_load(456) + 2; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 2);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_sys_memcpy_i32_extern_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "extern function sys_memcpy_i32(dst: i32[], dst_index: i32, src: i32[], src_index: i32, count: i32): void;\nconst COUNT: i32 = 4;\nglobal src: i32[COUNT];\nglobal dst: i32[COUNT];\nfunction main(): i32 { src[0] = 3; src[1] = 5; sys_memcpy_i32(dst, 0, src, 0, 2); return dst[0] + dst[1]; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 8);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_global_block_style_path_set_and_read() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global State { score: i32; }\nfunction main(): i32 { State.score = 7; return State.score; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[test]
    fn jit_process_incremental_compile_emits_only_changed_function() {
        let mut process = JitProcess::new();
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

    #[cfg(windows)]
    #[test]
    fn jit_process_skips_unreachable_invalid_function_body() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function bad(): i32 { return missing(); }\nfunction tick(): i32 { return 1; }\n",
        );
        let report = process.compile().expect("compile");
        assert_eq!(report.emit.emitted_functions, 1);
        let value = process
            .execute_i32_noarg_by_name("tick")
            .expect("execute tick");
        assert_eq!(value, 1);
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
    fn jit_process_supports_i32_let_and_if_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let total: i32 = 5; if (total > 4) { return total + 2; } return 0; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_for_loop_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 0; i < 4; i += 1) { sum += i; } return sum; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_for_loop_with_empty_init_segment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let i: i32 = 0; let sum: i32 = 0; for (; i < 4; i += 1) { sum += i; } return sum; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected missing init segment to be rejected");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("for header must include init, condition, and step"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_supports_for_loop_with_empty_step_segment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let i: i32 = 0; let sum: i32 = 0; for (; i < 4; ) { sum += i; i += 1; } return sum; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected missing step segment to be rejected");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("for header must include init, condition, and step"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_supports_for_loop_call_init_and_conversion_step_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global State { init_calls: i32; }\nfunction mark_init(): void { State.init_calls += 1; return; }\nfunction main(): i32 { let i: f32 = 0.0; let sum: i32 = 0; for (mark_init(); i < 3.0; i.from_i32(sum)) { sum += 1; } return sum + State.init_calls; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 2);
        assert_eq!(report.emit.emitted_functions, 2);
        assert_eq!(process.artifacts().len(), 2);
        assert!(process.artifacts()[0].code_ptr != 0);
        assert!(process.artifacts()[1].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_for_loop_global_init_and_indexed_conversion_step_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Node { value: f32; }\nglobal nodes: Node[COUNT];\nglobal State { snap: f32; }\nfunction main(): i32 { let sum: i32 = 0; for (State.snap.from_i32(0); sum < 2; nodes[1].value.from_i32(sum)) { sum += 1; } let out: i32 = 0; out.from_f32(nodes[1].value); return out; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_inferred_let_and_for_init_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum = 0; for (let i = 0; i < 4; i += 1) { sum += i; } return sum; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_inferred_float_let_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let alpha = 0.5; if (alpha > 0.4) { return 1; } return 0; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_bool_condition_expression_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let ready = true; if (ready) { return 1; } return 0; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_rejects_i32_condition_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value = 1; if (value) { return 1; } return 0; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected condition type error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("condition expression must be bool"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_f32_condition_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let alpha = 1.0; if (alpha) { return 1; } return 0; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected condition type error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("condition expression must be bool"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_i32_for_condition_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let i = 2; for (i -= 0; i; i -= 1) { return 1; } return 0; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected condition type error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("condition expression must be bool"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_let_shadowing_parameter() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(value: i32): i32 { let value: i32 = 1; return value; }\n",
        );
        let error = process.compile().expect_err("expected shadowing error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("let binding 'value' shadows existing variable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_for_init_shadowing_outer_local() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let i: i32 = 4; for (let i: i32 = 0; i < 1; i += 1) { } return i; }\n",
        );
        let error = process.compile().expect_err("expected shadowing error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("let binding 'i' shadows existing variable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_foreach_item_shadowing_outer_local() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nglobal values: i32[COUNT];\nfunction main(): i32 { let value: i32 = 0; foreach (let value in values) { value += 1; } return value; }\n",
        );
        let error = process.compile().expect_err("expected shadowing error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("foreach item binding 'value' shadows existing variable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_foreach_item_and_index_name_collision() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nglobal values: i32[COUNT];\nfunction main(): i32 { foreach (let v, v in values) { } return 0; }\n",
        );
        let error = process.compile().expect_err("expected shadowing error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("foreach index binding 'v' shadows existing variable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_for_loop_missing_init_segment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (; sum < 3; sum += 1) { } return sum; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected for header segment error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("for header must include init, condition, and step"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_for_loop_missing_condition_segment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (sum = 0; ; sum += 1) { } return sum; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected for header segment error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("for header must include init, condition, and step"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_for_loop_missing_step_segment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (sum = 0; sum < 3; ) { } return sum; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected for header segment error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("for header must include init, condition, and step"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_duplicate_parameter_names() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function add(v: i32, v: i32): i32 { return v; }\n",
        );
        let error = process.compile().expect_err("expected shadowing error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("parameter 'v' shadows existing variable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
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

    #[test]
    fn jit_process_supports_spec_compound_assignment_operators() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 20; value -= 3; value *= 2; value /= 7; value %= 4; return value; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_for_loop_logical_condition_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 0; (i < 5) && !(i == 3); i += 1) { sum += i; } return sum; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_if_else_if_else_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 2; if (value == 0) { return 1; } else if (value == 2) { return 5; } else { return 9; } }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_logical_if_condition_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 2; if ((value > 1 && value < 4) || !(value == 2)) { return 11; } return 0; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_rejects_while_loop_keyword() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let i: i32 = 0; while (i < 3) { i += 1; } return i; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected unsupported statement");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("unsupported statement in function body near 'while"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_exposes_symbol_code_ptr_map() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return 2; }\n",
        );
        process.compile().expect("compile");
        let map = process.symbol_code_ptrs();
        assert!(map.contains_key("main"));
        assert!(map.get("main").copied().unwrap_or(0) != 0);
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
    fn jit_process_executes_i32_let_and_if_true_branch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let total: i32 = 5; if (total > 4) { return total + 2; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_i32_let_and_if_false_branch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let total: i32 = 3; if (total > 4) { return total + 2; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 0);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_for_loop_accumulation() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 0; i < 4; i += 1) { sum += i; } return sum; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 6);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_compound_assignment_operators() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 20; value -= 3; value *= 2; value /= 7; value %= 4; return value; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 0);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_if_else_if_branch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 2; if (value == 0) { return 1; } else if (value == 2) { return 5; } else { return 9; } }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 5);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_if_else_fallback_branch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 3; if (value == 0) { return 1; } else if (value == 2) { return 5; } else { return 9; } }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_logical_and_or_not_condition_true() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 2; if ((value > 1 && value < 4) || !(value == 2)) { return 11; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 11);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_logical_and_or_not_condition_false() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 5; if ((value > 1 && value < 4) || !(value == 5)) { return 11; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 0);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_for_loop_with_decrement_step() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 5; i > 0; i -= 2) { sum += i; } return sum; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_for_loop_with_logical_condition() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 0; (i < 5) && !(i == 3); i += 1) { sum += i; } return sum; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 3);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_for_loop_call_init_and_conversion_step() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global State { init_calls: i32; }\nfunction mark_init(): void { State.init_calls += 1; return; }\nfunction main(): i32 { let i: f32 = 0.0; let sum: i32 = 0; for (mark_init(); i < 3.0; i.from_i32(sum)) { sum += 1; } return sum + State.init_calls; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 4);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_for_loop_global_init_and_indexed_conversion_step() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Node { value: f32; }\nglobal nodes: Node[COUNT];\nglobal State { snap: f32; }\nfunction main(): i32 { let sum: i32 = 0; for (State.snap.from_i32(0); sum < 2; nodes[1].value.from_i32(sum)) { sum += 1; } let out: i32 = 0; out.from_f32(nodes[1].value); return out; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 2);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_inferred_let_and_for_init() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum = 0; for (let i = 0; i < 4; i += 1) { sum += i; } return sum; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 6);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_inferred_float_let() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let alpha = 0.5; if (alpha > 0.4) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_bool_condition_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let ready = true; if (ready) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_noarg_call_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 7; }\nfunction main(): i32 { return helper() + 2; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_two_arg_call_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function add_pair(left: i32, right: i32): i32 { return left + right; }\nfunction main(): i32 { return add_pair(2, 3); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 5);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_receiver_style_call_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function damage(enemy: i32, amount: i32): i32 { return enemy - amount; }\nfunction main(): i32 { let enemy: i32 = 10; return enemy.damage(3); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_resolves_same_method_name_by_receiver_type() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function damage(enemy: Enemy, amount: i32): i32 { return amount + 1; }\nfunction damage(player: Player, amount: i32): i32 { return amount + 2; }\nfunction main(enemy: Enemy, player: Player): i32 { return enemy.damage(3) + player.damage(3); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_twoarg_by_name("main", 10, 20)
            .expect("execute in memory");
        assert_eq!(value, 9);
    }

    #[test]
    fn jit_process_rejects_ambiguous_overload_for_same_receiver_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function damage(enemy: Enemy, amount: i32): i32 { return amount + 1; }\nfunction damage(other_enemy: Enemy, amount: i32): i32 { return amount + 2; }\nfunction main(enemy: Enemy): i32 { return enemy.damage(3); }\n",
        );
        let error = process.compile().expect_err("expected ambiguous overload");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("ambiguous overload for call target 'damage'"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
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

    #[cfg(windows)]
    #[test]
    fn jit_process_recompiles_shifted_function_ids_when_artifact_body_hash_mismatch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function f0(): i32 { return 1; }\nfunction f1(): i32 { return 2; }\nfunction main(): i32 { return f1() + 100; }\n",
        );
        let first = process.compile().expect("first compile");
        assert_eq!(first.emit.emitted_functions, 2);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute first"),
            102
        );

        process.upsert_file(
            "sample.stasis",
            "function inserted(): i32 { return 9; }\nfunction f0(): i32 { return 1; }\nfunction f1(): i32 { return 2; }\nfunction main(): i32 { return f1() + 100; }\n",
        );
        let second = process.compile().expect("second compile");
        assert_eq!(
            second.emit.emitted_functions, 2,
            "f1 and main should be re-emitted after function-id shift"
        );
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute second"),
            102
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_keeps_previous_artifacts_on_partial_emit_failure() {
        let mut process = JitProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return 1; }\n");
        process.compile().expect("initial compile");
        let first_main_ptr = process
            .symbol_code_ptrs()
            .get("main")
            .copied()
            .expect("main ptr after initial compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute initial main"),
            1
        );

        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { return helper(); }\nfunction helper(): i32 { return missing(); }\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("unknown call target 'missing'"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }

        let second_main_ptr = process
            .symbol_code_ptrs()
            .get("main")
            .copied()
            .expect("main ptr after failed compile");
        assert_eq!(second_main_ptr, first_main_ptr);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute preserved main"),
            1
        );

        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { return helper(); }\nfunction helper(): i32 { return 5; }\n",
        );
        process.compile().expect("recovery compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute recovered main"),
            5
        );
    }

    #[test]
    fn jit_process_keeps_compile_analysis_cache_for_unchanged_sources() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const VALUE: i32 = 5;\nfunction main(): i32 { return VALUE; }\n",
        );

        process.compile().expect("first compile");
        let first_fingerprint = process
            .compile_analysis_cache
            .as_ref()
            .expect("analysis cache after first compile")
            .files_fingerprint;

        process.compile().expect("second compile");
        let second_fingerprint = process
            .compile_analysis_cache
            .as_ref()
            .expect("analysis cache after second compile")
            .files_fingerprint;

        assert_eq!(
            first_fingerprint, second_fingerprint,
            "unchanged sources should keep the same compile-analysis cache key"
        );
    }

    #[test]
    fn jit_process_rebuilds_compile_analysis_cache_when_dependency_changes() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "import \"stdlib.stasis\";\nfunction main(): i32 { return helper(); }\n",
        );
        process.upsert_file("stdlib.stasis", "function helper(): i32 { return 11; }\n");

        process.compile().expect("first compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute first"),
            11
        );
        let first_fingerprint = process
            .compile_analysis_cache
            .as_ref()
            .expect("analysis cache after first compile")
            .files_fingerprint;

        process.upsert_file("stdlib.stasis", "function helper(): i32 { return 27; }\n");
        process.compile().expect("second compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute second"),
            27
        );
        let second_fingerprint = process
            .compile_analysis_cache
            .as_ref()
            .expect("analysis cache after second compile")
            .files_fingerprint;

        assert_ne!(
            first_fingerprint, second_fingerprint,
            "dependency source change should invalidate compile-analysis cache key"
        );
    }

    #[test]
    fn jit_process_refreshes_import_parse_cache_when_imports_change() {
        let mut process = JitProcess::new();
        process.upsert_file("dep.stasis", "function dep(): i32 { return 1; }\n");
        process.upsert_file(
            "main.stasis",
            "import \"dep.stasis\";\nfunction main(): i32 { return dep(); }\n",
        );
        process.compile().expect("first compile");
        let first_entry = process
            .import_parse_cache
            .get("main.stasis")
            .expect("main import cache entry")
            .clone();
        assert_eq!(
            first_entry.import_paths,
            vec!["dep.stasis".to_string()],
            "expected single cached import"
        );

        process.upsert_file("dep2.stasis", "function dep2(): i32 { return 2; }\n");
        process.upsert_file(
            "main.stasis",
            "import \"dep.stasis\";\nimport \"dep2.stasis\";\nfunction main(): i32 { return dep() + dep2(); }\n",
        );
        process.compile().expect("second compile");
        let second_entry = process
            .import_parse_cache
            .get("main.stasis")
            .expect("main import cache entry after update")
            .clone();
        assert_ne!(
            first_entry.source_hash, second_entry.source_hash,
            "cache hash should refresh when source import set changes"
        );
        assert_eq!(
            second_entry.import_paths,
            vec!["dep.stasis".to_string(), "dep2.stasis".to_string()],
            "expected refreshed cached imports"
        );
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute second"),
            3
        );
    }

    #[test]
    fn jit_process_reemits_reachable_functions_when_imported_constant_changes() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "import \"constants.stasis\";\nfunction main(): i32 { return VALUE; }\n",
        );
        process.upsert_file("constants.stasis", "const VALUE: i32 = 11;\n");

        let first = process.compile().expect("first compile");
        assert_eq!(first.emit.emitted_functions, 1);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute first"),
            11
        );

        process.upsert_file("constants.stasis", "const VALUE: i32 = 27;\n");
        let second = process.compile().expect("second compile");
        assert_eq!(
            second.emit.emitted_functions, 1,
            "main should be re-emitted when imported constants change"
        );
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute second"),
            27
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
