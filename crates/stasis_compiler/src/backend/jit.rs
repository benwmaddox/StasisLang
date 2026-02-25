use crate::backend::EngineEntrypoints;
use crate::compiler::{
    CompileReport, CompileResult, Compiler, FunctionId, FunctionMeta, SourceFile,
};
use crate::frontend::parser::{
    parse_top_level_extern_functions, parse_top_level_type_layout, ParsedExternFunctionDeclaration,
    ParsedField,
};
use crate::frontend::types::{
    TypeCategory, TypeId, TypeTable, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_I32, TYPE_ID_VOID,
};
use crate::ir::hir::FunctionHIR;
use cranelift_codegen::ir::{
    condcodes::{FloatCC, IntCC},
    immediates::Ieee32,
    types, AbiParam, FuncRef, InstBuilder, Value,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, FuncId, Linkage, Module};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
    runtime_libraries: Vec<stasis_dynload::Library>,
    runtime_symbol_cache: BTreeMap<String, usize>,
    compile_analysis_cache: Option<CompileAnalysisCache>,
    required_emit_roots: Vec<String>,
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
            runtime_libraries: Vec::new(),
            runtime_symbol_cache: BTreeMap::new(),
            compile_analysis_cache: None,
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
        stasis_dynload::clear_jit_string_literal_table();
        load_import_graph_sources(&mut self.compiler)
            .map_err(crate::compiler::CompileError::Backend)?;
        let index = self.compiler.index_pass()?;
        let mut type_table = self.compiler.types().clone();
        type_table
            .resolve_or_intern("string")
            .map_err(crate::compiler::CompileError::Backend)?;
        type_table
            .resolve_or_intern("ascii[]")
            .map_err(crate::compiler::CompileError::Backend)?;
        let files_fingerprint = compute_files_fingerprint(self.compiler.files());
        let cache_miss = self
            .compile_analysis_cache
            .as_ref()
            .is_none_or(|cache| cache.files_fingerprint != files_fingerprint);
        if cache_miss {
            let extern_signatures =
                collect_supported_extern_call_signatures(self.compiler.files(), &mut type_table)
                    .map_err(crate::compiler::CompileError::Backend)?;
            let (resolved_extern_signatures, extern_symbol_addresses) = self
                .resolve_extern_call_signatures(&extern_signatures)
                .map_err(crate::compiler::CompileError::Backend)?;
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
            self.compile_analysis_cache = Some(CompileAnalysisCache {
                files_fingerprint,
                call_signatures,
                global_path_types,
                constant_values,
                collection_infos,
                extern_symbol_addresses,
            });
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
        );
        let (next_slot, next_symbol_seq, artifacts, modules) = (
            &mut self.next_slot,
            &mut self.next_symbol_seq,
            &mut self.artifacts,
            &mut self.modules,
        );
        let emit = self
            .compiler
            .emit_pass_for_ids_with(&emit_function_ids, &mut |meta, hir| {
                let symbol = format!("jit_fn_{}_{}", meta.id, *next_symbol_seq);
                *next_symbol_seq = next_symbol_seq.saturating_add(1);
                let (module, code_ptr) = compile_function_to_jit_module(
                    meta,
                    hir,
                    &symbol,
                    &analysis.call_signatures,
                    &type_table,
                    &analysis.global_path_types,
                    &analysis.constant_values,
                    &analysis.collection_infos,
                    &analysis.extern_symbol_addresses,
                )?;
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
            })?;
        let report = CompileReport { index, emit };
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
        self.artifacts
            .iter()
            .find(|artifact| artifact.function_id == function.id)
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
            .artifacts
            .iter()
            .find(|artifact| artifact.function_id == function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;
        let raw = stasis_dynload::invoke_noarg_u64(artifact.code_ptr as usize)?;
        Ok((raw as u32) as i32)
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
            .artifacts
            .iter()
            .find(|artifact| artifact.function_id == function.id)
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
            .artifacts
            .iter()
            .find(|artifact| artifact.function_id == function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;
        stasis_dynload::invoke_i32_i32_to_i32(artifact.code_ptr as usize, left, right)
    }

    pub fn symbol_code_ptrs(&self) -> BTreeMap<String, u64> {
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
        symbol_code_ptrs
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
            .artifacts
            .iter()
            .find(|artifact| artifact.function_id == function.id)
            .ok_or_else(|| format!("compiled artifact missing for required entrypoint '{name}'"))?;
        Ok(artifact.code_ptr)
    }

    fn refresh_runtime_dispatch_table(&self) {
        let mut i32_entries = Vec::new();
        let mut f32_entries = Vec::new();
        let mut code_ptr_entries = Vec::new();
        let type_table = self.compiler.types();
        for function in self.compiler.functions() {
            let Ok(arity) = u8::try_from(function.params.len()) else {
                continue;
            };
            if arity > 8 {
                continue;
            }
            let Some(artifact) = self
                .artifacts
                .iter()
                .find(|artifact| artifact.function_id == function.id)
            else {
                continue;
            };
            code_ptr_entries.push((function.id, artifact.code_ptr as usize));
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
                        && is_i32_abi_compatible_type(function.params[0], type_table)))
            {
                f32_entries.push((function.id, arity, artifact.code_ptr as usize));
            }
        }
        stasis_dynload::replace_jit_i32_dispatch_table(&i32_entries);
        stasis_dynload::replace_jit_f32_dispatch_table(&f32_entries);
        stasis_dynload::replace_jit_code_ptr_table(&code_ptr_entries);
    }

    fn resolve_extern_call_signatures(
        &mut self,
        extern_signatures: &[ExternCallSignature],
    ) -> Result<(Vec<ResolvedExternCallSignature>, ExternSymbolAddressMap), String> {
        let mut resolved = Vec::with_capacity(extern_signatures.len());
        let mut symbol_addresses: ExternSymbolAddressMap = BTreeMap::new();
        for signature in extern_signatures {
            let mut selected: Option<(String, usize)> = None;
            for candidate in &signature.symbol_candidates {
                if let Some(address) = self.resolve_host_symbol_address(candidate) {
                    selected = Some((candidate.clone(), address));
                    break;
                }
            }
            let Some((symbol, address)) = selected else {
                return Err(format!(
                    "unresolved extern call target '{}' with candidates {:?}",
                    signature.name, signature.symbol_candidates
                ));
            };
            symbol_addresses.insert(symbol.clone(), address);
            resolved.push(ResolvedExternCallSignature {
                name: signature.name.clone(),
                params: signature.params.clone(),
                return_type: signature.return_type,
                symbol,
            });
        }
        Ok((resolved, symbol_addresses))
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
        for path in runtime_library_candidate_paths() {
            if !path.exists() {
                continue;
            }
            if let Ok(library) = stasis_dynload::Library::load(&path) {
                self.runtime_libraries.push(library);
            }
        }
    }
}

impl Default for JitProcess {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
struct CallSignature {
    function_id: Option<FunctionId>,
    extern_symbol: Option<String>,
    params: Vec<TypeId>,
    return_type: TypeId,
}

#[derive(Debug, Clone)]
struct ExternCallSignature {
    name: String,
    symbol_candidates: Vec<String>,
    params: Vec<TypeId>,
    return_type: TypeId,
}

#[derive(Debug, Clone)]
struct ResolvedExternCallSignature {
    name: String,
    symbol: String,
    params: Vec<TypeId>,
    return_type: TypeId,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ExternImportKey {
    symbol: String,
    params: Vec<TypeId>,
    return_type: TypeId,
}

type CallSignatureMap = BTreeMap<String, Vec<CallSignature>>;
type GlobalPathTypeMap = BTreeMap<String, TypeId>;
type ConstantValueMap = BTreeMap<String, ConstantValue>;
type CollectionInfoMap = BTreeMap<String, ForeachCollectionInfo>;
type ForeachBindingMap = BTreeMap<String, ForeachBinding>;
type ExternSymbolAddressMap = BTreeMap<String, usize>;

#[derive(Debug, Clone)]
struct CompileAnalysisCache {
    files_fingerprint: u64,
    call_signatures: CallSignatureMap,
    global_path_types: GlobalPathTypeMap,
    constant_values: ConstantValueMap,
    collection_infos: CollectionInfoMap,
    extern_symbol_addresses: ExternSymbolAddressMap,
}

#[derive(Debug, Clone, PartialEq)]
enum ConstantValue {
    I32 { value: i32, type_id: TypeId },
    F32(f32),
    Bool(bool),
    String { value: String, type_id: TypeId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ForeachCollectionInfo {
    len: i32,
    element_type: Option<TypeId>,
    field_types: BTreeMap<String, TypeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ForeachBinding {
    collection_handle: ForeachCollectionHandle,
    index_var: Variable,
    element_type: Option<TypeId>,
    field_types: BTreeMap<String, TypeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForeachCollectionHandle {
    PathHash(i32),
    LocalVar(Variable),
}

fn collect_supported_call_signatures(
    functions: &[FunctionMeta],
    extern_signatures: &[ResolvedExternCallSignature],
    type_table: &TypeTable,
) -> CallSignatureMap {
    let mut map: CallSignatureMap = BTreeMap::new();
    for function in functions {
        if !is_supported_call_lane_type(function.return_type, type_table, true) {
            continue;
        }
        if function.params.len() > 8 {
            continue;
        }
        if !function
            .params
            .iter()
            .copied()
            .all(|param| is_supported_call_lane_type(param, type_table, false))
        {
            continue;
        }
        map.entry(function.name.clone())
            .or_default()
            .push(CallSignature {
                function_id: Some(function.id),
                extern_symbol: None,
                params: function.params.clone(),
                return_type: function.return_type,
            });
    }
    for signature in extern_signatures {
        if !is_supported_call_lane_type(signature.return_type, type_table, true) {
            continue;
        }
        if !signature
            .params
            .iter()
            .copied()
            .all(|param| is_supported_call_lane_type(param, type_table, false))
        {
            continue;
        }
        map.entry(signature.name.clone())
            .or_default()
            .push(CallSignature {
                function_id: None,
                extern_symbol: Some(signature.symbol.clone()),
                params: signature.params.clone(),
                return_type: signature.return_type,
            });
    }
    map
}

fn is_supported_call_lane_type(type_id: TypeId, type_table: &TypeTable, allow_void: bool) -> bool {
    if allow_void && type_id == TYPE_ID_VOID {
        return true;
    }
    type_id == TYPE_ID_F32 || is_i32_abi_compatible_type(type_id, type_table)
}

fn select_emit_function_ids(
    functions: &[FunctionMeta],
    artifacts: &[JitArtifact],
    required_emit_roots: &[String],
) -> Vec<FunctionId> {
    let mut reachable = collect_reachable_function_ids(functions, required_emit_roots);
    let reachable_names: BTreeSet<String> = functions
        .iter()
        .filter(|function| reachable.contains(&function.id))
        .map(|function| function.name.clone())
        .collect();
    for function in functions {
        if reachable_names.contains(&function.name) {
            reachable.insert(function.id);
        }
    }
    let compiled: BTreeSet<FunctionId> = artifacts
        .iter()
        .map(|artifact| artifact.function_id)
        .collect();
    functions
        .iter()
        .filter(|function| {
            reachable.contains(&function.id) && (function.dirty || !compiled.contains(&function.id))
        })
        .map(|function| function.id)
        .collect()
}

fn collect_reachable_function_ids(
    functions: &[FunctionMeta],
    required_emit_roots: &[String],
) -> BTreeSet<FunctionId> {
    let mut roots: Vec<FunctionId> = Vec::new();
    for root_name in ["tick", "main", "render", "on_code_swap"] {
        roots.extend(
            functions
                .iter()
                .filter(|function| function.name == root_name)
                .map(|function| function.id),
        );
    }
    for root_name in required_emit_roots {
        roots.extend(
            functions
                .iter()
                .filter(|function| function.name == *root_name)
                .map(|function| function.id),
        );
    }
    if roots.is_empty() {
        return functions.iter().map(|function| function.id).collect();
    }

    let mut reachable: BTreeSet<FunctionId> = BTreeSet::new();
    let mut stack = roots;
    while let Some(function_id) = stack.pop() {
        if !reachable.insert(function_id) {
            continue;
        }
        let Some(function) = functions.get(function_id as usize) else {
            continue;
        };
        for dependency in &function.dependencies {
            stack.push(*dependency);
        }
    }
    reachable
}

fn collect_supported_extern_call_signatures(
    files: &[SourceFile],
    type_table: &mut TypeTable,
) -> Result<Vec<ExternCallSignature>, String> {
    let mut out = Vec::new();
    for file in files {
        let declarations = parse_top_level_extern_functions(&file.content).map_err(|error| {
            format!(
                "failed parsing extern declarations in {}: {error}",
                file.path
            )
        })?;
        for declaration in declarations {
            let signature = build_extern_call_signature(type_table, declaration)?;
            out.push(signature);
        }
    }
    Ok(out)
}

fn build_extern_call_signature(
    type_table: &mut TypeTable,
    declaration: ParsedExternFunctionDeclaration,
) -> Result<ExternCallSignature, String> {
    let mut params = Vec::with_capacity(declaration.params.len());
    for param in &declaration.params {
        let type_id = type_table.resolve_or_intern(&param.type_name)?;
        params.push(type_id);
    }
    let return_type = type_table.resolve_or_intern(&declaration.return_type_name)?;
    Ok(ExternCallSignature {
        name: declaration.name,
        symbol_candidates: build_extern_symbol_candidates(
            &declaration.symbol_name,
            declaration.explicit_symbol,
        ),
        params,
        return_type,
    })
}

fn build_extern_symbol_candidates(symbol_name: &str, explicit_symbol: bool) -> Vec<String> {
    let mut out = Vec::new();
    if symbol_name.is_empty() {
        return out;
    }
    out.push(symbol_name.to_string());
    if explicit_symbol {
        return out;
    }
    if !symbol_name.starts_with("stasis_") {
        out.push(format!("stasis_{symbol_name}"));
    }
    if !symbol_name.starts_with("stasis_jit_") {
        out.push(format!("stasis_jit_{symbol_name}"));
    }
    if symbol_name == "time" {
        out.push("stasis_get_time_ms".to_string());
    } else if symbol_name == "time_us" {
        out.push("stasis_get_time_us".to_string());
    }
    out
}

fn is_i32_abi_compatible_type(type_id: TypeId, type_table: &TypeTable) -> bool {
    if type_id == TYPE_ID_I32 || type_id == TYPE_ID_BOOL {
        return true;
    }
    let Some(type_info) = type_table.type_info(type_id) else {
        return false;
    };
    matches!(
        type_info.category,
        TypeCategory::Named
            | TypeCategory::ArrayFixed
            | TypeCategory::ArrayView
            | TypeCategory::AsciiFixed
            | TypeCategory::AsciiView
            | TypeCategory::Utf8Fixed
            | TypeCategory::Utf8View
    )
}

fn is_collection_handle_type(type_id: TypeId, type_table: &TypeTable) -> bool {
    let Some(type_info) = type_table.type_info(type_id) else {
        return false;
    };
    matches!(
        type_info.category,
        TypeCategory::ArrayFixed
            | TypeCategory::ArrayView
            | TypeCategory::AsciiFixed
            | TypeCategory::AsciiView
            | TypeCategory::Utf8Fixed
            | TypeCategory::Utf8View
    )
}

fn is_i32_scalar_lane_type(type_id: TypeId, type_table: &TypeTable) -> bool {
    type_id != TYPE_ID_BOOL && is_i32_abi_compatible_type(type_id, type_table)
}

fn is_i32_numeric_type(type_id: TypeId, type_table: &TypeTable) -> bool {
    if type_id == TYPE_ID_I32 {
        return true;
    }
    let Some(type_info) = type_table.type_info(type_id) else {
        return false;
    };
    matches!(type_info.category, TypeCategory::Named)
}

fn are_assignment_types_compatible(
    target_type: TypeId,
    expression_type: TypeId,
    type_table: &TypeTable,
) -> bool {
    if target_type == expression_type {
        return true;
    }
    is_i32_abi_compatible_type(target_type, type_table)
        && is_i32_abi_compatible_type(expression_type, type_table)
}

fn load_import_graph_sources(compiler: &mut Compiler) -> Result<(), String> {
    let mut known_paths: BTreeSet<String> = compiler
        .files()
        .iter()
        .map(|file| file.path.clone())
        .collect();
    let mut queue: Vec<String> = compiler
        .files()
        .iter()
        .map(|file| file.path.clone())
        .collect();

    while let Some(path) = queue.pop() {
        let Some(source) = compiler
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
            compiler.upsert_file(normalized.clone(), content);
            known_paths.insert(normalized.clone());
            queue.push(normalized);
        }
    }

    Ok(())
}

fn parse_import_paths(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("import") {
            continue;
        }
        let mut chars = trimmed.chars();
        let mut first_quote_index: Option<usize> = None;
        let mut quote_char: Option<char> = None;
        for (index, ch) in chars.by_ref().enumerate() {
            if ch == '"' || ch == '\'' {
                first_quote_index = Some(index);
                quote_char = Some(ch);
                break;
            }
        }
        let Some(start) = first_quote_index else {
            continue;
        };
        let Some(delim) = quote_char else {
            continue;
        };
        let rest = &trimmed[start + 1..];
        if let Some(end) = rest.find(delim) {
            let path = rest[..end].trim();
            if !path.is_empty() {
                out.push(path.to_string());
            }
        }
    }
    out
}

fn resolve_import_path(base_file: &str, import_path: &str) -> PathBuf {
    let import = Path::new(import_path);
    if import.is_absolute() {
        return import.to_path_buf();
    }
    let base = Path::new(base_file);
    let parent = base.parent().unwrap_or_else(|| Path::new("."));
    parent.join(import)
}

fn normalize_path_for_compiler_key(path: &Path) -> String {
    match std::fs::canonicalize(path) {
        Ok(canonical) => canonical.to_string_lossy().to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

fn compute_files_fingerprint(files: &[SourceFile]) -> u64 {
    let mut entries: Vec<(&str, u64)> = files
        .iter()
        .map(|file| (file.path.as_str(), file.hash))
        .collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));

    let mut hash: u64 = 1469598103934665603;
    for (path, file_hash) in entries {
        for byte in path.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(1099511628211);
        }
        for byte in file_hash.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(1099511628211);
        }
    }
    hash
}

fn runtime_library_candidate_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(configured) = std::env::var_os("STASIS_RUNTIME_DLL_PATH") {
        out.push(PathBuf::from(configured));
    }
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    out.push(
        repo_root
            .join("runtime")
            .join("build")
            .join("bin")
            .join("Release")
            .join("stasis_graphics.dll"),
    );
    out.push(
        repo_root
            .join("runtime")
            .join("build")
            .join("bin")
            .join("Debug")
            .join("stasis_graphics.dll"),
    );
    out
}

fn builtin_host_symbol_address(symbol: &str) -> Option<usize> {
    let address = match symbol {
        "print_i32" | "stasis_jit_print_i32" => stasis_dynload::stasis_jit_print_i32 as usize,
        "print_string" | "stasis_jit_print_string" => {
            stasis_dynload::stasis_jit_print_string as usize
        }
        "sin_fast" | "stasis_jit_sin_fast" => stasis_dynload::stasis_jit_sin_fast as usize,
        "cos_fast" | "stasis_jit_cos_fast" => stasis_dynload::stasis_jit_cos_fast as usize,
        "stasis_jit_global_i32_load" => stasis_dynload::stasis_jit_global_i32_load as usize,
        "stasis_jit_global_i32_store" => stasis_dynload::stasis_jit_global_i32_store as usize,
        "stasis_jit_global_f32_load" => stasis_dynload::stasis_jit_global_f32_load as usize,
        "stasis_jit_global_f32_store" => stasis_dynload::stasis_jit_global_f32_store as usize,
        "sys_memcpy_u8" | "stasis_sys_memcpy_u8" | "stasis_jit_sys_memcpy_u8" => {
            stasis_dynload::stasis_jit_sys_memcpy_u8 as usize
        }
        "sys_memcpy_i32" | "stasis_sys_memcpy_i32" | "stasis_jit_sys_memcpy_i32" => {
            stasis_dynload::stasis_jit_sys_memcpy_i32 as usize
        }
        "sys_memcpy_f32" | "stasis_sys_memcpy_f32" | "stasis_jit_sys_memcpy_f32" => {
            stasis_dynload::stasis_jit_sys_memcpy_f32 as usize
        }
        "sys_memmove_u8" | "stasis_sys_memmove_u8" | "stasis_jit_sys_memmove_u8" => {
            stasis_dynload::stasis_jit_sys_memmove_u8 as usize
        }
        "sys_memmove_i32" | "stasis_sys_memmove_i32" | "stasis_jit_sys_memmove_i32" => {
            stasis_dynload::stasis_jit_sys_memmove_i32 as usize
        }
        "sys_memmove_f32" | "stasis_sys_memmove_f32" | "stasis_jit_sys_memmove_f32" => {
            stasis_dynload::stasis_jit_sys_memmove_f32 as usize
        }
        _ => return None,
    };
    Some(address)
}

fn collect_global_path_types(
    files: &[SourceFile],
    type_table: &mut TypeTable,
    constant_values: &ConstantValueMap,
) -> Result<GlobalPathTypeMap, String> {
    let mut struct_fields_by_name: BTreeMap<String, Vec<ParsedField>> = BTreeMap::new();
    let mut typed_globals: BTreeMap<String, String> = BTreeMap::new();
    let mut global_blocks: BTreeMap<String, Vec<ParsedField>> = BTreeMap::new();

    for file in files {
        let parsed = parse_top_level_type_layout(&file.content).map_err(|error| {
            format!(
                "failed parsing top-level type layout in {}: {error}",
                file.path
            )
        })?;
        for parsed_struct in parsed.structs {
            if let Some(existing) = struct_fields_by_name.get(&parsed_struct.name) {
                if existing != &parsed_struct.fields {
                    return Err(format!(
                        "conflicting struct definition for '{}'",
                        parsed_struct.name
                    ));
                }
            } else {
                struct_fields_by_name.insert(parsed_struct.name, parsed_struct.fields);
            }
        }
        for global in parsed.globals {
            if global_blocks.contains_key(&global.name) {
                return Err(format!(
                    "conflicting global declarations for '{}'",
                    global.name
                ));
            }
            if let Some(existing) = typed_globals.get(&global.name) {
                if existing != &global.type_name {
                    return Err(format!(
                        "conflicting global type for '{}': '{}' vs '{}'",
                        global.name, existing, global.type_name
                    ));
                }
            } else {
                typed_globals.insert(global.name, global.type_name);
            }
        }
        for global_block in parsed.global_blocks {
            if typed_globals.contains_key(&global_block.name) {
                return Err(format!(
                    "conflicting global declarations for '{}'",
                    global_block.name
                ));
            }
            if let Some(existing) = global_blocks.get(&global_block.name) {
                if existing != &global_block.fields {
                    return Err(format!(
                        "conflicting global block definition for '{}'",
                        global_block.name
                    ));
                }
            } else {
                global_blocks.insert(global_block.name, global_block.fields);
            }
        }
    }

    let mut out = GlobalPathTypeMap::new();
    for (global_name, type_name) in typed_globals {
        expand_global_type_paths(
            &global_name,
            &type_name,
            &struct_fields_by_name,
            type_table,
            constant_values,
            &mut out,
            &mut Vec::new(),
        )?;
    }
    for (global_name, fields) in global_blocks {
        for field in fields {
            let path = format!("{global_name}.{}", field.name);
            expand_global_type_paths(
                &path,
                &field.type_name,
                &struct_fields_by_name,
                type_table,
                constant_values,
                &mut out,
                &mut Vec::new(),
            )?;
        }
    }
    Ok(out)
}

fn expand_global_type_paths(
    path: &str,
    type_name: &str,
    struct_fields_by_name: &BTreeMap<String, Vec<ParsedField>>,
    type_table: &mut TypeTable,
    constant_values: &ConstantValueMap,
    out: &mut GlobalPathTypeMap,
    visiting_structs: &mut Vec<String>,
) -> Result<(), String> {
    let trimmed = type_name.trim();
    let Some(fields) = struct_fields_by_name.get(trimmed) else {
        let resolved = resolve_global_path_type_id(trimmed, type_table, constant_values)?;
        out.insert(path.to_string(), resolved);
        if let Some(type_info) = type_table.type_info(resolved) {
            match type_info.category {
                TypeCategory::ArrayFixed | TypeCategory::ArrayView => {
                    out.insert(format!("{path}.length"), TYPE_ID_I32);
                    out.insert(format!("{path}.max_length"), TYPE_ID_I32);
                }
                TypeCategory::AsciiFixed | TypeCategory::AsciiView => {
                    out.insert(format!("{path}.length"), TYPE_ID_I32);
                    out.insert(format!("{path}.byte_length"), TYPE_ID_I32);
                    out.insert(format!("{path}.max_length"), TYPE_ID_I32);
                }
                TypeCategory::Utf8Fixed | TypeCategory::Utf8View => {
                    out.insert(format!("{path}.length"), TYPE_ID_I32);
                    out.insert(format!("{path}.byte_length"), TYPE_ID_I32);
                    out.insert(format!("{path}.char_length"), TYPE_ID_I32);
                    out.insert(format!("{path}.max_length"), TYPE_ID_I32);
                }
                _ => {}
            }
        }
        return Ok(());
    };
    if visiting_structs.iter().any(|existing| existing == trimmed) {
        return Err(format!(
            "recursive struct path expansion is unsupported for '{}'",
            trimmed
        ));
    }
    visiting_structs.push(trimmed.to_string());
    for field in fields {
        let child_path = format!("{path}.{}", field.name);
        expand_global_type_paths(
            &child_path,
            &field.type_name,
            struct_fields_by_name,
            type_table,
            constant_values,
            out,
            visiting_structs,
        )?;
    }
    visiting_structs.pop();
    Ok(())
}

fn resolve_global_path_type_id(
    type_name: &str,
    type_table: &mut TypeTable,
    constant_values: &ConstantValueMap,
) -> Result<TypeId, String> {
    let trimmed = type_name.trim();
    if let Some((element_type, extent_text)) = parse_array_type_parts(trimmed) {
        if extent_text.is_empty() || extent_text.bytes().all(|byte| byte.is_ascii_digit()) {
            return type_table.resolve_or_intern(trimmed);
        }
        let resolved_extent =
            resolve_fixed_array_extent(extent_text, constant_values).ok_or_else(|| {
                format!(
                    "unsupported array extent '{}' in type '{}'",
                    extent_text, type_name
                )
            })?;
        if resolved_extent < 0 {
            return Err(format!(
                "negative array extent {} in type '{}'",
                resolved_extent, type_name
            ));
        }
        let canonical = format!("{}[{}]", element_type.trim(), resolved_extent);
        return type_table.resolve_or_intern(&canonical);
    }
    type_table.resolve_or_intern(trimmed)
}

fn primitive_global_type_id(type_name: &str) -> Option<TypeId> {
    match type_name {
        "i32" => Some(TYPE_ID_I32),
        "f32" => Some(TYPE_ID_F32),
        "bool" => Some(TYPE_ID_BOOL),
        _ => None,
    }
}

fn collect_top_level_constant_values(
    files: &[SourceFile],
    type_table: &mut TypeTable,
) -> Result<ConstantValueMap, String> {
    let mut out = ConstantValueMap::new();
    for file in files {
        let parsed = parse_top_level_type_layout(&file.content).map_err(|error| {
            format!(
                "failed parsing top-level constants in {}: {error}",
                file.path
            )
        })?;
        for parsed_enum in parsed.enums {
            let enum_type_id = type_table.resolve(&parsed_enum.name).unwrap_or(TYPE_ID_I32);
            let mut next_value: i32 = 0;
            for variant in parsed_enum.variants {
                let value = if let Some(explicit) = variant.value {
                    explicit
                } else {
                    next_value
                };
                let path = format!("{}.{}", parsed_enum.name, variant.name);
                let value = ConstantValue::I32 {
                    value,
                    type_id: enum_type_id,
                };
                if let Some(existing) = out.get(&path) {
                    if existing != &value {
                        return Err(format!("conflicting enum variant constant for '{}'", path));
                    }
                } else {
                    out.insert(path, value);
                }
                next_value = next_value.checked_add(1).ok_or_else(|| {
                    format!(
                        "enum '{}' value overflow while assigning discriminants",
                        parsed_enum.name
                    )
                })?;
            }
        }
        for constant in parsed.constants {
            let Some(value) = parse_top_level_constant_literal(
                &constant.name,
                &constant.type_name,
                &constant.value_text,
                type_table,
            )?
            else {
                continue;
            };
            if let Some(existing) = out.get(&constant.name) {
                if existing != &value {
                    return Err(format!(
                        "conflicting constant definition for '{}'",
                        constant.name
                    ));
                }
            } else {
                out.insert(constant.name, value);
            }
        }
    }
    Ok(out)
}

fn parse_top_level_constant_literal(
    name: &str,
    type_name: &str,
    value_text: &str,
    type_table: &mut TypeTable,
) -> Result<Option<ConstantValue>, String> {
    let initializer = value_text.trim();
    if initializer.is_empty() {
        return Err(format!("constant '{}' initializer cannot be empty", name));
    }
    let type_name = type_name.trim();
    match type_name {
        "i32" => {
            let value = initializer.parse::<i32>().map_err(|error| {
                format!("invalid i32 initializer for constant '{}': {error}", name)
            })?;
            Ok(Some(ConstantValue::I32 {
                value,
                type_id: TYPE_ID_I32,
            }))
        }
        "f32" => {
            let value = initializer.parse::<f32>().map_err(|error| {
                format!("invalid f32 initializer for constant '{}': {error}", name)
            })?;
            Ok(Some(ConstantValue::F32(value)))
        }
        "bool" => match initializer {
            "true" => Ok(Some(ConstantValue::Bool(true))),
            "false" => Ok(Some(ConstantValue::Bool(false))),
            other => Err(format!(
                "invalid bool initializer '{}' for constant '{}'",
                other, name
            )),
        },
        "string" | "utf8[]" | "ascii[]" => {
            let value = parse_constant_string_initializer(name, initializer)?;
            let type_id = type_table.resolve_or_intern(type_name).map_err(|error| {
                format!(
                    "invalid type '{}' for constant '{}': {error}",
                    type_name, name
                )
            })?;
            let literal_id = hash_string_literal(&value);
            stasis_dynload::upsert_jit_string_literal(literal_id, &value);
            Ok(Some(ConstantValue::String { value, type_id }))
        }
        _ => Ok(None),
    }
}

fn parse_constant_string_initializer(name: &str, initializer: &str) -> Result<String, String> {
    let tokens = tokenize_simple_expression(initializer).map_err(|error| {
        format!(
            "invalid string initializer for constant '{}': {error}",
            name
        )
    })?;
    if tokens.len() != 1 {
        return Err(format!(
            "constant '{}' string initializer must be a single literal",
            name
        ));
    }
    match &tokens[0] {
        ExprToken::StringLiteral(value) => Ok(value.clone()),
        _ => Err(format!(
            "constant '{}' string initializer must be a string literal",
            name
        )),
    }
}

fn collect_foreach_collection_infos(
    files: &[SourceFile],
    type_table: &mut TypeTable,
    constant_values: &ConstantValueMap,
) -> Result<CollectionInfoMap, String> {
    let mut struct_fields_by_name: BTreeMap<String, Vec<ParsedField>> = BTreeMap::new();
    let mut typed_globals: BTreeMap<String, String> = BTreeMap::new();
    let mut global_blocks: BTreeMap<String, Vec<ParsedField>> = BTreeMap::new();

    for file in files {
        let parsed = parse_top_level_type_layout(&file.content).map_err(|error| {
            format!(
                "failed parsing top-level type layout in {}: {error}",
                file.path
            )
        })?;
        for parsed_struct in parsed.structs {
            if let Some(existing) = struct_fields_by_name.get(&parsed_struct.name) {
                if existing != &parsed_struct.fields {
                    return Err(format!(
                        "conflicting struct definition for '{}'",
                        parsed_struct.name
                    ));
                }
            } else {
                struct_fields_by_name.insert(parsed_struct.name, parsed_struct.fields);
            }
        }
        for global in parsed.globals {
            if global_blocks.contains_key(&global.name) {
                return Err(format!(
                    "conflicting global declarations for '{}'",
                    global.name
                ));
            }
            if let Some(existing) = typed_globals.get(&global.name) {
                if existing != &global.type_name {
                    return Err(format!(
                        "conflicting global type for '{}': '{}' vs '{}'",
                        global.name, existing, global.type_name
                    ));
                }
            } else {
                typed_globals.insert(global.name, global.type_name);
            }
        }
        for global_block in parsed.global_blocks {
            if typed_globals.contains_key(&global_block.name) {
                return Err(format!(
                    "conflicting global declarations for '{}'",
                    global_block.name
                ));
            }
            if let Some(existing) = global_blocks.get(&global_block.name) {
                if existing != &global_block.fields {
                    return Err(format!(
                        "conflicting global block definition for '{}'",
                        global_block.name
                    ));
                }
            } else {
                global_blocks.insert(global_block.name, global_block.fields);
            }
        }
    }

    let mut out = CollectionInfoMap::new();
    for (global_name, type_name) in typed_globals {
        collect_foreach_collections_from_type(
            &global_name,
            &type_name,
            &struct_fields_by_name,
            type_table,
            constant_values,
            &mut out,
            &mut Vec::new(),
        )?;
    }
    for (global_name, fields) in global_blocks {
        for field in fields {
            let path = format!("{global_name}.{}", field.name);
            collect_foreach_collections_from_type(
                &path,
                &field.type_name,
                &struct_fields_by_name,
                type_table,
                constant_values,
                &mut out,
                &mut Vec::new(),
            )?;
        }
    }
    Ok(out)
}

fn collect_foreach_collections_from_type(
    path: &str,
    type_name: &str,
    struct_fields_by_name: &BTreeMap<String, Vec<ParsedField>>,
    type_table: &mut TypeTable,
    constant_values: &ConstantValueMap,
    out: &mut CollectionInfoMap,
    visiting_structs: &mut Vec<String>,
) -> Result<(), String> {
    let trimmed = type_name.trim();
    if let Some((element_type_name, extent_text)) = parse_array_type_parts(trimmed) {
        if extent_text.is_empty() {
            return Ok(());
        }
        let len = resolve_fixed_array_extent(extent_text, constant_values).ok_or_else(|| {
            format!(
                "foreach collection '{}' has unsupported array extent '{}'",
                path, extent_text
            )
        })?;
        if len < 0 {
            return Err(format!(
                "foreach collection '{}' has negative array extent {}",
                path, len
            ));
        }
        let collection = build_collection_info_for_element_type(
            element_type_name,
            struct_fields_by_name,
            type_table,
            visiting_structs,
        )?;
        let info = ForeachCollectionInfo {
            len,
            element_type: collection.element_type,
            field_types: collection.field_types,
        };
        if let Some(existing) = out.get(path) {
            if existing != &info {
                return Err(format!(
                    "conflicting foreach collection info for '{}'",
                    path
                ));
            }
        } else {
            out.insert(path.to_string(), info);
        }
        return Ok(());
    }
    if primitive_global_type_id(trimmed).is_some() {
        return Ok(());
    }
    let Some(fields) = struct_fields_by_name.get(trimmed) else {
        return Ok(());
    };
    if visiting_structs.iter().any(|existing| existing == trimmed) {
        return Err(format!(
            "recursive struct path expansion is unsupported for '{}'",
            trimmed
        ));
    }
    visiting_structs.push(trimmed.to_string());
    for field in fields {
        let child_path = format!("{path}.{}", field.name);
        collect_foreach_collections_from_type(
            &child_path,
            &field.type_name,
            struct_fields_by_name,
            type_table,
            constant_values,
            out,
            visiting_structs,
        )?;
    }
    visiting_structs.pop();
    Ok(())
}

fn build_collection_info_for_element_type(
    element_type_name: &str,
    struct_fields_by_name: &BTreeMap<String, Vec<ParsedField>>,
    type_table: &mut TypeTable,
    visiting_structs: &mut Vec<String>,
) -> Result<ForeachCollectionInfo, String> {
    if let Some(type_id) = primitive_global_type_id(element_type_name) {
        return Ok(ForeachCollectionInfo {
            len: 0,
            element_type: Some(type_id),
            field_types: BTreeMap::new(),
        });
    }
    if !struct_fields_by_name.contains_key(element_type_name) {
        let element_type = type_table.resolve_or_intern(element_type_name)?;
        return Ok(ForeachCollectionInfo {
            len: 0,
            element_type: Some(element_type),
            field_types: BTreeMap::new(),
        });
    }
    let mut field_types = BTreeMap::new();
    collect_struct_primitive_leaf_fields(
        "",
        element_type_name,
        struct_fields_by_name,
        type_table,
        &mut field_types,
        visiting_structs,
    )?;
    Ok(ForeachCollectionInfo {
        len: 0,
        element_type: None,
        field_types,
    })
}

fn collect_struct_primitive_leaf_fields(
    prefix: &str,
    struct_name: &str,
    struct_fields_by_name: &BTreeMap<String, Vec<ParsedField>>,
    type_table: &mut TypeTable,
    out: &mut BTreeMap<String, TypeId>,
    visiting_structs: &mut Vec<String>,
) -> Result<(), String> {
    let Some(fields) = struct_fields_by_name.get(struct_name) else {
        return Ok(());
    };
    if visiting_structs
        .iter()
        .any(|existing| existing == struct_name)
    {
        return Err(format!(
            "recursive struct field expansion is unsupported for '{}'",
            struct_name
        ));
    }
    visiting_structs.push(struct_name.to_string());
    for field in fields {
        let field_path = if prefix.is_empty() {
            field.name.clone()
        } else {
            format!("{prefix}.{}", field.name)
        };
        if let Some(type_id) = primitive_global_type_id(field.type_name.trim()) {
            out.insert(field_path, type_id);
            continue;
        }
        if let Some((element_type_name, extent_text)) = parse_array_type_parts(&field.type_name) {
            if !extent_text.is_empty() {
                continue;
            }
            if primitive_global_type_id(element_type_name).is_some() {
                continue;
            }
            continue;
        }
        if struct_fields_by_name.contains_key(field.type_name.trim()) {
            collect_struct_primitive_leaf_fields(
                &field_path,
                field.type_name.trim(),
                struct_fields_by_name,
                type_table,
                out,
                visiting_structs,
            )?;
        } else {
            let type_id = type_table.resolve_or_intern(field.type_name.trim())?;
            out.insert(field_path, type_id);
        }
    }
    visiting_structs.pop();
    Ok(())
}

fn parse_array_type_parts(type_name: &str) -> Option<(&str, &str)> {
    let trimmed = type_name.trim();
    if !trimmed.ends_with(']') {
        return None;
    }
    let open = trimmed.rfind('[')?;
    if open == 0 || open >= trimmed.len() - 1 {
        return None;
    }
    let element = trimmed[..open].trim();
    let extent = trimmed[open + 1..trimmed.len() - 1].trim();
    if element.is_empty() {
        return None;
    }
    Some((element, extent))
}

fn resolve_fixed_array_extent(
    extent_text: &str,
    constant_values: &ConstantValueMap,
) -> Option<i32> {
    if extent_text.is_empty() {
        return None;
    }
    if extent_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return extent_text.parse::<i32>().ok();
    }
    match constant_values.get(extent_text) {
        Some(ConstantValue::I32 { value, .. }) => Some(*value),
        _ => None,
    }
}

fn compile_function_to_jit_module(
    meta: &FunctionMeta,
    hir: &FunctionHIR,
    symbol: &str,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
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
        "stasis_jit_global_i32_array_load",
        stasis_dynload::stasis_jit_global_i32_array_load as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_i32_array_store",
        stasis_dynload::stasis_jit_global_i32_array_store as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_f32_array_load",
        stasis_dynload::stasis_jit_global_f32_array_load as *const u8,
    );
    jit_builder.symbol(
        "stasis_jit_global_f32_array_store",
        stasis_dynload::stasis_jit_global_f32_array_store as *const u8,
    );
    for (extern_symbol, address) in extern_symbol_addresses {
        if *address == 0 {
            continue;
        }
        jit_builder.symbol(extern_symbol, *address as *const u8);
    }
    let mut module = JITModule::new(jit_builder);
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
                    "unsupported JIT return type id {} for function {}",
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
        .map_err(|error| format!("failed to declare JIT function {symbol}: {error}"))?;
    let runtime_call_imports = RuntimeCallImportIds {
        call_i32_0: declare_i32_call_import(&mut module, "stasis_jit_call_i32_0", 1)?,
        call_i32_1: declare_i32_call_import(&mut module, "stasis_jit_call_i32_1", 2)?,
        call_i32_2: declare_i32_call_import(&mut module, "stasis_jit_call_i32_2", 3)?,
        call_i32_3: declare_i32_call_import(&mut module, "stasis_jit_call_i32_3", 4)?,
        call_i32_4: declare_i32_call_import(&mut module, "stasis_jit_call_i32_4", 5)?,
        call_i32_5: declare_i32_call_import(&mut module, "stasis_jit_call_i32_5", 6)?,
        call_i32_6: declare_i32_call_import(&mut module, "stasis_jit_call_i32_6", 7)?,
        call_i32_7: declare_i32_call_import(&mut module, "stasis_jit_call_i32_7", 8)?,
        call_i32_8: declare_i32_call_import(&mut module, "stasis_jit_call_i32_8", 9)?,
        call_i32_f32_1: declare_i32_f32_call_import(&mut module, "stasis_jit_call_i32_f32_1", 1)?,
        call_i32_f32_2: declare_i32_f32_call_import(&mut module, "stasis_jit_call_i32_f32_2", 2)?,
        call_i32_f32_3: declare_i32_f32_call_import(&mut module, "stasis_jit_call_i32_f32_3", 3)?,
        call_i32_f32_4: declare_i32_f32_call_import(&mut module, "stasis_jit_call_i32_f32_4", 4)?,
        call_i32_f32_5: declare_i32_f32_call_import(&mut module, "stasis_jit_call_i32_f32_5", 5)?,
        call_i32_f32_6: declare_i32_f32_call_import(&mut module, "stasis_jit_call_i32_f32_6", 6)?,
        call_i32_f32_7: declare_i32_f32_call_import(&mut module, "stasis_jit_call_i32_f32_7", 7)?,
        call_i32_f32_8: declare_i32_f32_call_import(&mut module, "stasis_jit_call_i32_f32_8", 8)?,
        call_f32_0: declare_f32_call_import(&mut module, "stasis_jit_call_f32_0", 1)?,
        call_f32_1: declare_f32_call_import(&mut module, "stasis_jit_call_f32_1", 2)?,
        call_f32_2: declare_f32_call_import(&mut module, "stasis_jit_call_f32_2", 3)?,
        call_f32_3: declare_f32_call_import(&mut module, "stasis_jit_call_f32_3", 4)?,
        call_f32_4: declare_f32_call_import(&mut module, "stasis_jit_call_f32_4", 5)?,
        call_f32_5: declare_f32_call_import(&mut module, "stasis_jit_call_f32_5", 6)?,
        call_f32_6: declare_f32_call_import(&mut module, "stasis_jit_call_f32_6", 7)?,
        call_f32_7: declare_f32_call_import(&mut module, "stasis_jit_call_f32_7", 8)?,
        call_f32_8: declare_f32_call_import(&mut module, "stasis_jit_call_f32_8", 9)?,
        call_f32_i32_1: declare_f32_i32_call_import(&mut module, "stasis_jit_call_f32_i32_1", 2)?,
        print_i32: declare_void_call_import(&mut module, "stasis_jit_print_i32", 1)?,
        print_string: declare_void_call_import(&mut module, "stasis_jit_print_string", 1)?,
        lookup_code_ptr: declare_lookup_code_ptr_import(&mut module, "stasis_jit_lookup_code_ptr")?,
        sin_fast: declare_direct_f32_unary_import(&mut module, "stasis_jit_sin_fast")?,
        cos_fast: declare_direct_f32_unary_import(&mut module, "stasis_jit_cos_fast")?,
        global_i32_load: declare_i32_call_import(&mut module, "stasis_jit_global_i32_load", 1)?,
        global_i32_store: declare_void_call_import(&mut module, "stasis_jit_global_i32_store", 2)?,
        global_f32_load: declare_f32_global_load_import(&mut module, "stasis_jit_global_f32_load")?,
        global_f32_store: declare_f32_global_store_import(
            &mut module,
            "stasis_jit_global_f32_store",
        )?,
        global_i32_array_load: declare_i32_array_load_import(
            &mut module,
            "stasis_jit_global_i32_array_load",
        )?,
        global_i32_array_store: declare_i32_array_store_import(
            &mut module,
            "stasis_jit_global_i32_array_store",
        )?,
        global_f32_array_load: declare_f32_array_load_import(
            &mut module,
            "stasis_jit_global_f32_array_load",
        )?,
        global_f32_array_store: declare_f32_array_store_import(
            &mut module,
            "stasis_jit_global_f32_array_store",
        )?,
        extern_calls: declare_extern_call_imports(&mut module, call_signatures, type_table)?,
    };

    let mut function_builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut function_builder_context);
        let runtime_call_refs = RuntimeCallRefs {
            call_i32_0: module.declare_func_in_func(runtime_call_imports.call_i32_0, builder.func),
            call_i32_1: module.declare_func_in_func(runtime_call_imports.call_i32_1, builder.func),
            call_i32_2: module.declare_func_in_func(runtime_call_imports.call_i32_2, builder.func),
            call_i32_3: module.declare_func_in_func(runtime_call_imports.call_i32_3, builder.func),
            call_i32_4: module.declare_func_in_func(runtime_call_imports.call_i32_4, builder.func),
            call_i32_5: module.declare_func_in_func(runtime_call_imports.call_i32_5, builder.func),
            call_i32_6: module.declare_func_in_func(runtime_call_imports.call_i32_6, builder.func),
            call_i32_7: module.declare_func_in_func(runtime_call_imports.call_i32_7, builder.func),
            call_i32_8: module.declare_func_in_func(runtime_call_imports.call_i32_8, builder.func),
            call_i32_f32_1: module
                .declare_func_in_func(runtime_call_imports.call_i32_f32_1, builder.func),
            call_i32_f32_2: module
                .declare_func_in_func(runtime_call_imports.call_i32_f32_2, builder.func),
            call_i32_f32_3: module
                .declare_func_in_func(runtime_call_imports.call_i32_f32_3, builder.func),
            call_i32_f32_4: module
                .declare_func_in_func(runtime_call_imports.call_i32_f32_4, builder.func),
            call_i32_f32_5: module
                .declare_func_in_func(runtime_call_imports.call_i32_f32_5, builder.func),
            call_i32_f32_6: module
                .declare_func_in_func(runtime_call_imports.call_i32_f32_6, builder.func),
            call_i32_f32_7: module
                .declare_func_in_func(runtime_call_imports.call_i32_f32_7, builder.func),
            call_i32_f32_8: module
                .declare_func_in_func(runtime_call_imports.call_i32_f32_8, builder.func),
            call_f32_0: module.declare_func_in_func(runtime_call_imports.call_f32_0, builder.func),
            call_f32_1: module.declare_func_in_func(runtime_call_imports.call_f32_1, builder.func),
            call_f32_2: module.declare_func_in_func(runtime_call_imports.call_f32_2, builder.func),
            call_f32_3: module.declare_func_in_func(runtime_call_imports.call_f32_3, builder.func),
            call_f32_4: module.declare_func_in_func(runtime_call_imports.call_f32_4, builder.func),
            call_f32_5: module.declare_func_in_func(runtime_call_imports.call_f32_5, builder.func),
            call_f32_6: module.declare_func_in_func(runtime_call_imports.call_f32_6, builder.func),
            call_f32_7: module.declare_func_in_func(runtime_call_imports.call_f32_7, builder.func),
            call_f32_8: module.declare_func_in_func(runtime_call_imports.call_f32_8, builder.func),
            call_f32_i32_1: module
                .declare_func_in_func(runtime_call_imports.call_f32_i32_1, builder.func),
            print_i32: module.declare_func_in_func(runtime_call_imports.print_i32, builder.func),
            print_string: module
                .declare_func_in_func(runtime_call_imports.print_string, builder.func),
            lookup_code_ptr: module
                .declare_func_in_func(runtime_call_imports.lookup_code_ptr, builder.func),
            sin_fast: module.declare_func_in_func(runtime_call_imports.sin_fast, builder.func),
            cos_fast: module.declare_func_in_func(runtime_call_imports.cos_fast, builder.func),
            global_i32_load: module
                .declare_func_in_func(runtime_call_imports.global_i32_load, builder.func),
            global_i32_store: module
                .declare_func_in_func(runtime_call_imports.global_i32_store, builder.func),
            global_f32_load: module
                .declare_func_in_func(runtime_call_imports.global_f32_load, builder.func),
            global_f32_store: module
                .declare_func_in_func(runtime_call_imports.global_f32_store, builder.func),
            global_i32_array_load: module
                .declare_func_in_func(runtime_call_imports.global_i32_array_load, builder.func),
            global_i32_array_store: module
                .declare_func_in_func(runtime_call_imports.global_i32_array_store, builder.func),
            global_f32_array_load: module
                .declare_func_in_func(runtime_call_imports.global_f32_array_load, builder.func),
            global_f32_array_store: module
                .declare_func_in_func(runtime_call_imports.global_f32_array_store, builder.func),
            extern_calls: runtime_call_imports
                .extern_calls
                .iter()
                .map(|(key, id)| (key.clone(), module.declare_func_in_func(*id, builder.func)))
                .collect(),
        };
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

        let statements = parse_simple_statements(hir, type_table)?;
        let empty_foreach_bindings = ForeachBindingMap::new();
        let terminated = emit_simple_statements(
            &mut builder,
            &statements,
            &mut values_by_name,
            &runtime_call_refs,
            call_signatures,
            type_table,
            global_path_types,
            constant_values,
            collection_infos,
            &empty_foreach_bindings,
            meta.return_type,
            &mut next_variable,
        )?;
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
        .map_err(|error| format!("failed to define JIT function {symbol}: {error}"))?;
    module.clear_context(&mut context);
    module
        .finalize_definitions()
        .map_err(|error| format!("failed to finalize JIT definitions: {error}"))?;
    let code_ptr = module.get_finalized_function(function_id) as usize as u64;
    Ok((module, code_ptr))
}

struct RuntimeCallImportIds {
    call_i32_0: FuncId,
    call_i32_1: FuncId,
    call_i32_2: FuncId,
    call_i32_3: FuncId,
    call_i32_4: FuncId,
    call_i32_5: FuncId,
    call_i32_6: FuncId,
    call_i32_7: FuncId,
    call_i32_8: FuncId,
    call_i32_f32_1: FuncId,
    call_i32_f32_2: FuncId,
    call_i32_f32_3: FuncId,
    call_i32_f32_4: FuncId,
    call_i32_f32_5: FuncId,
    call_i32_f32_6: FuncId,
    call_i32_f32_7: FuncId,
    call_i32_f32_8: FuncId,
    call_f32_0: FuncId,
    call_f32_1: FuncId,
    call_f32_2: FuncId,
    call_f32_3: FuncId,
    call_f32_4: FuncId,
    call_f32_5: FuncId,
    call_f32_6: FuncId,
    call_f32_7: FuncId,
    call_f32_8: FuncId,
    call_f32_i32_1: FuncId,
    print_i32: FuncId,
    print_string: FuncId,
    lookup_code_ptr: FuncId,
    sin_fast: FuncId,
    cos_fast: FuncId,
    global_i32_load: FuncId,
    global_i32_store: FuncId,
    global_f32_load: FuncId,
    global_f32_store: FuncId,
    global_i32_array_load: FuncId,
    global_i32_array_store: FuncId,
    global_f32_array_load: FuncId,
    global_f32_array_store: FuncId,
    extern_calls: BTreeMap<ExternImportKey, FuncId>,
}

struct RuntimeCallRefs {
    call_i32_0: FuncRef,
    call_i32_1: FuncRef,
    call_i32_2: FuncRef,
    call_i32_3: FuncRef,
    call_i32_4: FuncRef,
    call_i32_5: FuncRef,
    call_i32_6: FuncRef,
    call_i32_7: FuncRef,
    call_i32_8: FuncRef,
    call_i32_f32_1: FuncRef,
    call_i32_f32_2: FuncRef,
    call_i32_f32_3: FuncRef,
    call_i32_f32_4: FuncRef,
    call_i32_f32_5: FuncRef,
    call_i32_f32_6: FuncRef,
    call_i32_f32_7: FuncRef,
    call_i32_f32_8: FuncRef,
    call_f32_0: FuncRef,
    call_f32_1: FuncRef,
    call_f32_2: FuncRef,
    call_f32_3: FuncRef,
    call_f32_4: FuncRef,
    call_f32_5: FuncRef,
    call_f32_6: FuncRef,
    call_f32_7: FuncRef,
    call_f32_8: FuncRef,
    call_f32_i32_1: FuncRef,
    print_i32: FuncRef,
    print_string: FuncRef,
    lookup_code_ptr: FuncRef,
    sin_fast: FuncRef,
    cos_fast: FuncRef,
    global_i32_load: FuncRef,
    global_i32_store: FuncRef,
    global_f32_load: FuncRef,
    global_f32_store: FuncRef,
    global_i32_array_load: FuncRef,
    global_i32_array_store: FuncRef,
    global_f32_array_load: FuncRef,
    global_f32_array_store: FuncRef,
    extern_calls: BTreeMap<ExternImportKey, FuncRef>,
}

#[derive(Clone, Copy)]
struct ValueBinding {
    value: Value,
    type_id: TypeId,
}

#[derive(Clone, Copy)]
struct LocalBinding {
    var: Variable,
    type_id: TypeId,
}

fn declare_i32_call_import(
    module: &mut JITModule,
    symbol: &str,
    param_count: usize,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    for _ in 0..param_count {
        signature.params.push(AbiParam::new(types::I32));
    }
    signature.returns.push(AbiParam::new(types::I32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_i32_f32_call_import(
    module: &mut JITModule,
    symbol: &str,
    f32_arg_count: usize,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    for _ in 0..f32_arg_count {
        signature.params.push(AbiParam::new(types::F32));
    }
    signature.returns.push(AbiParam::new(types::I32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_lookup_code_ptr_import(module: &mut JITModule, symbol: &str) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_direct_f32_unary_import(module: &mut JITModule, symbol: &str) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::F32));
    signature.returns.push(AbiParam::new(types::F32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_f32_call_import(
    module: &mut JITModule,
    symbol: &str,
    param_count: usize,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    for _ in 0..param_count.saturating_sub(1) {
        signature.params.push(AbiParam::new(types::F32));
    }
    signature.returns.push(AbiParam::new(types::F32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_f32_i32_call_import(
    module: &mut JITModule,
    symbol: &str,
    param_count: usize,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    if param_count == 0 {
        return Err("f32(i32)-call import requires at least fn-id parameter".to_string());
    }
    signature.params.push(AbiParam::new(types::I32));
    for _ in 0..param_count.saturating_sub(1) {
        signature.params.push(AbiParam::new(types::I32));
    }
    signature.returns.push(AbiParam::new(types::F32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_void_call_import(
    module: &mut JITModule,
    symbol: &str,
    param_count: usize,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    for _ in 0..param_count {
        signature.params.push(AbiParam::new(types::I32));
    }
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_f32_global_load_import(module: &mut JITModule, symbol: &str) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_f32_global_store_import(module: &mut JITModule, symbol: &str) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_i32_array_load_import(module: &mut JITModule, symbol: &str) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_i32_array_store_import(module: &mut JITModule, symbol: &str) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_f32_array_load_import(module: &mut JITModule, symbol: &str) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_f32_array_store_import(module: &mut JITModule, symbol: &str) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

fn declare_extern_call_imports(
    module: &mut JITModule,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
) -> Result<BTreeMap<ExternImportKey, FuncId>, String> {
    let mut out = BTreeMap::new();
    for signatures in call_signatures.values() {
        for signature in signatures {
            let Some(symbol) = signature.extern_symbol.as_ref() else {
                continue;
            };
            let key = ExternImportKey {
                symbol: symbol.clone(),
                params: signature.params.clone(),
                return_type: signature.return_type,
            };
            if out.contains_key(&key) {
                continue;
            }
            let mut clif_signature = module.make_signature();
            for param in &signature.params {
                clif_signature
                    .params
                    .push(AbiParam::new(clif_type_for_type_id(*param, type_table)?));
            }
            if signature.return_type != TYPE_ID_VOID {
                clif_signature
                    .returns
                    .push(AbiParam::new(clif_type_for_type_id(
                        signature.return_type,
                        type_table,
                    )?));
            }
            let func_id = module
                .declare_function(symbol, Linkage::Import, &clif_signature)
                .map_err(|error| {
                    format!(
                        "failed to declare extern import '{}' with params {:?} return {}: {}",
                        symbol, signature.params, signature.return_type, error
                    )
                })?;
            out.insert(key, func_id);
        }
    }
    Ok(out)
}

fn declare_new_variable(
    builder: &mut FunctionBuilder<'_>,
    next_variable: &mut u32,
    initial_value: Value,
    type_id: TypeId,
    type_table: &TypeTable,
) -> Result<Variable, String> {
    let next = *next_variable;
    let variable = Variable::from_u32(next);
    *next_variable = next_variable
        .checked_add(1)
        .ok_or_else(|| "too many local variables".to_string())?;
    builder.declare_var(variable, clif_type_for_type_id(type_id, type_table)?);
    builder.def_var(variable, initial_value);
    Ok(variable)
}

fn clif_type_for_type_id(
    type_id: TypeId,
    type_table: &TypeTable,
) -> Result<cranelift_codegen::ir::Type, String> {
    match type_id {
        TYPE_ID_I32 => Ok(types::I32),
        TYPE_ID_F32 => Ok(types::F32),
        TYPE_ID_BOOL => Ok(types::I32),
        TYPE_ID_VOID => Err("void is not a value type".to_string()),
        other => {
            let Some(info) = type_table.type_info(other) else {
                return Err(format!("unsupported type id {other} in current jit path"));
            };
            match info.category {
                TypeCategory::Builtin => {
                    Err(format!("unsupported type id {other} in current jit path"))
                }
                TypeCategory::Named
                | TypeCategory::ArrayFixed
                | TypeCategory::ArrayView
                | TypeCategory::AsciiFixed
                | TypeCategory::AsciiView
                | TypeCategory::Utf8Fixed
                | TypeCategory::Utf8View => Ok(types::I32),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum SimpleStmt {
    Noop,
    Let {
        name: String,
        type_id: Option<TypeId>,
        expression: SimpleExpr,
    },
    Assign {
        target: AssignTarget,
        op: AssignOp,
        expression: SimpleExpr,
    },
    Convert {
        target: AssignTarget,
        kind: ConversionKind,
        source: SimpleExpr,
    },
    If {
        condition: SimpleCondition,
        then_statements: Vec<SimpleStmt>,
        else_statements: Option<Vec<SimpleStmt>>,
    },
    For {
        init: Box<SimpleStmt>,
        condition: SimpleCondition,
        step: Box<SimpleStmt>,
        body_statements: Vec<SimpleStmt>,
    },
    Foreach {
        item_name: String,
        index_name: Option<String>,
        collection_path: String,
        body_statements: Vec<SimpleStmt>,
    },
    Expr(SimpleExpr),
    Return(SimpleExpr),
    ReturnVoid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Debug, Clone, PartialEq)]
enum AssignTarget {
    Local(String),
    GlobalPath(String),
    IndexedPath {
        collection_path: String,
        index: SimpleExpr,
        suffix: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConversionKind {
    FromI32,
    FromF32,
}

#[derive(Debug, Clone, PartialEq)]
enum SimpleCondition {
    Comparison {
        lhs: SimpleExpr,
        op: ComparisonOp,
        rhs: SimpleExpr,
    },
    Expr(SimpleExpr),
    And(Box<SimpleCondition>, Box<SimpleCondition>),
    Or(Box<SimpleCondition>, Box<SimpleCondition>),
    Not(Box<SimpleCondition>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComparisonOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

fn parse_simple_statements(
    hir: &FunctionHIR,
    type_table: &TypeTable,
) -> Result<Vec<SimpleStmt>, String> {
    let body = extract_function_body(hir)?;
    parse_simple_statements_from_block(body, type_table)
}

fn extract_function_body(hir: &FunctionHIR) -> Result<&str, String> {
    let Some(block) = hir.blocks.first() else {
        return Err("function body missing block text".to_string());
    };
    Ok(block.source.as_str())
}

fn parse_simple_statements_from_block(
    block_text: &str,
    type_table: &TypeTable,
) -> Result<Vec<SimpleStmt>, String> {
    let trimmed = block_text.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err("expected function body block enclosed in '{...}'".to_string());
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    let mut statements = Vec::new();
    let mut cursor = 0usize;
    while cursor < inner.len() {
        cursor = skip_ascii_whitespace_and_comments(inner, cursor);
        if cursor >= inner.len() {
            break;
        }
        if starts_with_keyword(inner, cursor, "let") {
            let let_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[let_start..semicolon].trim();
            statements.push(parse_let_statement(statement_text, type_table)?);
            cursor = semicolon + 1;
            continue;
        }
        if starts_with_keyword(inner, cursor, "return") {
            let return_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[return_start..semicolon].trim();
            statements.push(parse_return_statement(statement_text)?);
            cursor = semicolon + 1;
            continue;
        }
        if starts_with_keyword(inner, cursor, "for") {
            let (statement, next_cursor) = parse_for_statement(inner, cursor, type_table)?;
            statements.push(statement);
            cursor = next_cursor;
            continue;
        }
        if starts_with_keyword(inner, cursor, "foreach") {
            let (statement, next_cursor) = parse_foreach_statement(inner, cursor, type_table)?;
            statements.push(statement);
            cursor = next_cursor;
            continue;
        }
        if starts_with_keyword(inner, cursor, "if") {
            let (statement, next_cursor) = parse_if_statement(inner, cursor, type_table)?;
            statements.push(statement);
            cursor = next_cursor;
            continue;
        }
        if starts_with_keyword(inner, cursor, "while") {
            return Err(format!(
                "unsupported statement in function body near '{}'",
                snippet_from(inner, cursor)
            ));
        }
        if looks_like_from_conversion_statement(inner, cursor) {
            let start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[start..semicolon].trim();
            statements.push(parse_from_conversion_statement(statement_text)?);
            cursor = semicolon + 1;
            continue;
        }
        if looks_like_assignment(inner, cursor) {
            let assignment_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[assignment_start..semicolon].trim();
            statements.push(parse_assignment_statement(statement_text)?);
            cursor = semicolon + 1;
            continue;
        }
        if looks_like_call_statement(inner, cursor) {
            let call_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[call_start..semicolon].trim();
            statements.push(parse_call_statement(statement_text)?);
            cursor = semicolon + 1;
            continue;
        }
        return Err(format!(
            "unsupported statement in function body near '{}'",
            snippet_from(inner, cursor)
        ));
    }
    Ok(statements)
}

fn parse_let_statement(statement_text: &str, type_table: &TypeTable) -> Result<SimpleStmt, String> {
    let after_let = statement_text
        .strip_prefix("let")
        .ok_or_else(|| format!("invalid let statement '{statement_text}'"))?;
    let mut cursor = skip_ascii_whitespace(after_let, 0);
    let (name, next) = parse_identifier(after_let, cursor)?;
    cursor = skip_ascii_whitespace(after_let, next);
    let (type_id, expression) = match after_let.as_bytes().get(cursor).copied() {
        Some(b':') => {
            cursor += 1;
            cursor = skip_ascii_whitespace(after_let, cursor);
            let (type_name, next) = parse_identifier(after_let, cursor)?;
            let resolved_type_id = type_table.resolve(type_name).ok_or_else(|| {
                format!(
                    "unsupported let type '{}' in statement '{}'",
                    type_name, statement_text
                )
            })?;
            cursor = skip_ascii_whitespace(after_let, next);
            let expression = if cursor < after_let.len() && after_let.as_bytes()[cursor] == b'=' {
                cursor += 1;
                let expression_text = after_let[cursor..].trim();
                if expression_text.is_empty() {
                    return Err(format!(
                        "missing expression in let statement '{statement_text}'"
                    ));
                }
                parse_value_expression(expression_text)?
            } else if resolved_type_id == TYPE_ID_I32 || resolved_type_id == TYPE_ID_BOOL {
                SimpleExpr::Int(0)
            } else {
                SimpleExpr::Float(0.0)
            };
            (Some(resolved_type_id), expression)
        }
        Some(b'=') => {
            cursor += 1;
            let expression_text = after_let[cursor..].trim();
            if expression_text.is_empty() {
                return Err(format!(
                    "missing expression in let statement '{}'",
                    statement_text
                ));
            }
            (None, parse_value_expression(expression_text)?)
        }
        _ => {
            return Err(format!(
                "invalid let statement '{}': expected ':' type annotation or '=' inferred initializer",
                statement_text
            ));
        }
    };
    Ok(SimpleStmt::Let {
        name: name.to_string(),
        type_id,
        expression,
    })
}

fn parse_assignment_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    let mut cursor = skip_ascii_whitespace(statement_text, 0);
    let (target, next) = parse_assignment_target(statement_text, cursor)?;
    cursor = skip_ascii_whitespace(statement_text, next);
    let (op, op_width) = if statement_text
        .as_bytes()
        .get(cursor..cursor + 2)
        .is_some_and(|bytes| bytes == b"+=")
    {
        (AssignOp::Add, 2)
    } else if statement_text
        .as_bytes()
        .get(cursor..cursor + 2)
        .is_some_and(|bytes| bytes == b"-=")
    {
        (AssignOp::Sub, 2)
    } else if statement_text
        .as_bytes()
        .get(cursor..cursor + 2)
        .is_some_and(|bytes| bytes == b"*=")
    {
        (AssignOp::Mul, 2)
    } else if statement_text
        .as_bytes()
        .get(cursor..cursor + 2)
        .is_some_and(|bytes| bytes == b"/=")
    {
        (AssignOp::Div, 2)
    } else if statement_text
        .as_bytes()
        .get(cursor..cursor + 2)
        .is_some_and(|bytes| bytes == b"%=")
    {
        (AssignOp::Mod, 2)
    } else if statement_text
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| *byte == b'=')
    {
        (AssignOp::Set, 1)
    } else {
        return Err(format!(
            "unsupported assignment operator in statement '{}'",
            statement_text
        ));
    };
    cursor += op_width;
    let expression_text = statement_text[cursor..].trim();
    if expression_text.is_empty() {
        return Err(format!(
            "missing expression in assignment statement '{}'",
            statement_text
        ));
    }
    Ok(SimpleStmt::Assign {
        target,
        op,
        expression: parse_value_expression(expression_text)?,
    })
}

fn parse_assignment_target(source: &str, cursor: usize) -> Result<(AssignTarget, usize), String> {
    let (first, mut next) = parse_identifier(source, cursor)?;
    let mut collection_path = first.to_string();
    let mut index_expr: Option<SimpleExpr> = None;
    let mut suffix = String::new();

    loop {
        next = skip_ascii_whitespace(source, next);
        let Some(byte) = source.as_bytes().get(next).copied() else {
            break;
        };
        if byte == b'.' {
            next += 1;
            next = skip_ascii_whitespace(source, next);
            let (segment, after_segment) = parse_identifier(source, next)?;
            if index_expr.is_none() {
                collection_path.push('.');
                collection_path.push_str(segment);
            } else {
                if !suffix.is_empty() {
                    suffix.push('.');
                }
                suffix.push_str(segment);
            }
            next = after_segment;
            continue;
        }
        if byte == b'[' {
            if index_expr.is_some() {
                return Err(format!(
                    "multiple index segments are unsupported in assignment target near '{}'",
                    snippet_from(source, next)
                ));
            }
            let close = find_matching_delimiter(source, next, b'[', b']').ok_or_else(|| {
                format!(
                    "missing closing ']' in assignment target near '{}'",
                    snippet_from(source, next)
                )
            })?;
            let index_text = source[next + 1..close].trim();
            if index_text.is_empty() {
                return Err(format!(
                    "empty index expression in assignment target near '{}'",
                    snippet_from(source, next)
                ));
            }
            index_expr = Some(parse_simple_expression(index_text)?);
            next = close + 1;
            continue;
        }
        break;
    }

    if let Some(index) = index_expr {
        Ok((
            AssignTarget::IndexedPath {
                collection_path,
                index,
                suffix,
            },
            next,
        ))
    } else {
        Ok((assign_target_from_path(collection_path), next))
    }
}

fn parse_from_conversion_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    let trimmed = statement_text.trim();
    let marker_i32 = ".from_i32(";
    let marker_f32 = ".from_f32(";
    let (marker_pos, marker, kind) = if let Some(pos) = trimmed.find(marker_i32) {
        (pos, marker_i32, ConversionKind::FromI32)
    } else if let Some(pos) = trimmed.find(marker_f32) {
        (pos, marker_f32, ConversionKind::FromF32)
    } else {
        return Err(format!(
            "unsupported conversion statement '{}': expected from_i32 or from_f32",
            statement_text
        ));
    };

    let target_text = trimmed[..marker_pos].trim();
    if target_text.is_empty() {
        return Err(format!(
            "missing conversion target in statement '{}'",
            statement_text
        ));
    }

    let open = marker_pos + marker.len() - 1;
    let close = find_matching_delimiter(trimmed, open, b'(', b')')
        .ok_or_else(|| format!("missing ')' in conversion statement '{statement_text}'"))?;
    let arg_text = trimmed[open + 1..close].trim();
    if arg_text.is_empty() {
        return Err(format!(
            "missing source expression in conversion statement '{}'",
            statement_text
        ));
    }
    let source = parse_simple_expression(arg_text)?;
    let trailing = trimmed[close + 1..].trim();
    if !trailing.is_empty() {
        return Err(format!(
            "unexpected trailing tokens in conversion statement '{}'",
            statement_text
        ));
    }
    let (target, next) = parse_assignment_target(target_text, 0)?;
    if skip_ascii_whitespace(target_text, next) != target_text.len() {
        return Err(format!(
            "unsupported conversion target '{}' in statement '{}'",
            target_text, statement_text
        ));
    }
    Ok(SimpleStmt::Convert {
        target,
        kind,
        source,
    })
}

fn parse_call_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    let expression = parse_value_expression(statement_text)?;
    if matches!(expression, SimpleExpr::Call { .. }) {
        Ok(SimpleStmt::Expr(expression))
    } else {
        Err(format!(
            "unsupported expression statement '{}': expected call expression",
            statement_text
        ))
    }
}

fn parse_return_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    let after_return = statement_text
        .strip_prefix("return")
        .ok_or_else(|| format!("invalid return statement '{statement_text}'"))?;
    let expression_text = after_return.trim();
    if expression_text.is_empty() {
        return Ok(SimpleStmt::ReturnVoid);
    }
    Ok(SimpleStmt::Return(parse_value_expression(expression_text)?))
}

fn parse_for_statement(
    source: &str,
    start: usize,
    type_table: &TypeTable,
) -> Result<(SimpleStmt, usize), String> {
    let mut cursor = start + "for".len();
    cursor = skip_ascii_whitespace_and_comments(source, cursor);
    cursor = expect_byte(source, cursor, b'(', "'(' after for")?;
    let header_open = cursor - 1;
    let header_close = find_matching_delimiter(source, header_open, b'(', b')')
        .ok_or_else(|| "missing ')' for for-header".to_string())?;
    let header = source[header_open + 1..header_close].trim();
    let header_parts = split_for_header(header)?;
    let init_text = header_parts[0].trim();
    let condition_text = header_parts[1].trim();
    let step_text = header_parts[2].trim();
    if init_text.is_empty() || condition_text.is_empty() || step_text.is_empty() {
        return Err(format!(
            "for header must include init, condition, and step: '{}'",
            header
        ));
    }

    let init = parse_for_control_segment(init_text, type_table)?;
    let condition = parse_simple_condition(condition_text)?;
    let step = parse_for_control_segment(step_text, type_table)?;

    cursor = skip_ascii_whitespace_and_comments(source, header_close + 1);
    cursor = expect_byte(source, cursor, b'{', "'{' after for header")?;
    let body_open = cursor - 1;
    let body_close = find_matching_delimiter(source, body_open, b'{', b'}')
        .ok_or_else(|| "missing '}' for for body".to_string())?;
    let body_block = &source[body_open..=body_close];
    let body_statements = parse_simple_statements_from_block(body_block, type_table)?;
    let next_cursor = body_close + 1;

    Ok((
        SimpleStmt::For {
            init: Box::new(init),
            condition,
            step: Box::new(step),
            body_statements,
        },
        next_cursor,
    ))
}

fn parse_for_control_segment(
    segment_text: &str,
    type_table: &TypeTable,
) -> Result<SimpleStmt, String> {
    let trimmed = segment_text.trim();
    if trimmed.is_empty() {
        return Ok(SimpleStmt::Noop);
    }
    if starts_with_keyword(trimmed, 0, "let") {
        return parse_let_statement(trimmed, type_table);
    }
    if trimmed.contains(".from_i32(") || trimmed.contains(".from_f32(") {
        return parse_from_conversion_statement(trimmed);
    }
    if looks_like_assignment(trimmed, 0) {
        return parse_assignment_statement(trimmed);
    }
    if let Ok(call_statement) = parse_call_statement(trimmed) {
        return Ok(call_statement);
    }
    Err(format!(
        "unsupported for-loop control segment '{}'",
        trimmed
    ))
}

fn parse_foreach_statement(
    source: &str,
    start: usize,
    type_table: &TypeTable,
) -> Result<(SimpleStmt, usize), String> {
    let mut cursor = start + "foreach".len();
    cursor = skip_ascii_whitespace_and_comments(source, cursor);
    cursor = expect_byte(source, cursor, b'(', "'(' after foreach")?;
    let header_open = cursor - 1;
    let header_close = find_matching_delimiter(source, header_open, b'(', b')')
        .ok_or_else(|| "missing ')' for foreach-header".to_string())?;
    let header = source[header_open + 1..header_close].trim();
    if !starts_with_keyword(header, 0, "let") {
        return Err(format!(
            "foreach header must start with 'let': '{}'",
            header
        ));
    }
    let header_body = header
        .strip_prefix("let")
        .ok_or_else(|| format!("invalid foreach header '{}'", header))?;
    let mut header_cursor = skip_ascii_whitespace(header_body, 0);
    let (first_identifier, next) = parse_identifier(header_body, header_cursor)?;
    header_cursor = skip_ascii_whitespace(header_body, next);

    let mut item_name = first_identifier.to_string();
    let mut index_name: Option<String> = None;
    if header_body.as_bytes().get(header_cursor).copied() == Some(b',') {
        header_cursor += 1;
        header_cursor = skip_ascii_whitespace(header_body, header_cursor);
        let (second_identifier, next) = parse_identifier(header_body, header_cursor)?;
        item_name = first_identifier.to_string();
        index_name = Some(second_identifier.to_string());
        header_cursor = skip_ascii_whitespace(header_body, next);
    }
    if !starts_with_keyword(header_body, header_cursor, "in") {
        return Err(format!(
            "foreach header must include 'in <collection>' segment: '{}'",
            header
        ));
    }
    header_cursor += "in".len();
    header_cursor = skip_ascii_whitespace(header_body, header_cursor);
    let (collection_path, next) = parse_identifier_path(header_body, header_cursor)?;
    header_cursor = skip_ascii_whitespace(header_body, next);
    if header_cursor != header_body.len() {
        return Err(format!(
            "unexpected trailing tokens in foreach header '{}'",
            header
        ));
    }

    cursor = skip_ascii_whitespace_and_comments(source, header_close + 1);
    cursor = expect_byte(source, cursor, b'{', "'{' after foreach header")?;
    let body_open = cursor - 1;
    let body_close = find_matching_delimiter(source, body_open, b'{', b'}')
        .ok_or_else(|| "missing '}' for foreach body".to_string())?;
    let body_block = &source[body_open..=body_close];
    let body_statements = parse_simple_statements_from_block(body_block, type_table)?;
    let next_cursor = body_close + 1;

    Ok((
        SimpleStmt::Foreach {
            item_name,
            index_name,
            collection_path,
            body_statements,
        },
        next_cursor,
    ))
}

fn parse_if_statement(
    source: &str,
    start: usize,
    type_table: &TypeTable,
) -> Result<(SimpleStmt, usize), String> {
    let mut cursor = start + "if".len();
    cursor = skip_ascii_whitespace_and_comments(source, cursor);
    cursor = expect_byte(source, cursor, b'(', "'(' after if")?;
    let condition_open = cursor - 1;
    let condition_close = find_matching_delimiter(source, condition_open, b'(', b')')
        .ok_or_else(|| "missing ')' for if condition".to_string())?;
    let condition_text = source[condition_open + 1..condition_close].trim();
    if condition_text.is_empty() {
        return Err("if condition expression cannot be empty".to_string());
    }
    let condition = parse_simple_condition(condition_text)?;

    cursor = skip_ascii_whitespace_and_comments(source, condition_close + 1);
    cursor = expect_byte(source, cursor, b'{', "'{' after if condition")?;
    let then_open = cursor - 1;
    let then_close = find_matching_delimiter(source, then_open, b'{', b'}')
        .ok_or_else(|| "missing '}' for if body".to_string())?;
    let then_block = &source[then_open..=then_close];
    let then_statements = parse_simple_statements_from_block(then_block, type_table)?;
    let mut next_cursor = then_close + 1;
    let mut else_statements: Option<Vec<SimpleStmt>> = None;

    let else_cursor = skip_ascii_whitespace_and_comments(source, next_cursor);
    if starts_with_keyword(source, else_cursor, "else") {
        let mut cursor = else_cursor + "else".len();
        cursor = skip_ascii_whitespace_and_comments(source, cursor);
        if starts_with_keyword(source, cursor, "if") {
            let (else_if_statement, after_else_if) =
                parse_if_statement(source, cursor, type_table)?;
            else_statements = Some(vec![else_if_statement]);
            next_cursor = after_else_if;
        } else {
            cursor = expect_byte(source, cursor, b'{', "'{' after else")?;
            let else_open = cursor - 1;
            let else_close = find_matching_delimiter(source, else_open, b'{', b'}')
                .ok_or_else(|| "missing '}' for else body".to_string())?;
            let else_block = &source[else_open..=else_close];
            else_statements = Some(parse_simple_statements_from_block(else_block, type_table)?);
            next_cursor = else_close + 1;
        }
    }

    Ok((
        SimpleStmt::If {
            condition,
            then_statements,
            else_statements,
        },
        next_cursor,
    ))
}

fn parse_simple_condition(condition_text: &str) -> Result<SimpleCondition, String> {
    parse_or_condition(condition_text.trim())
}

fn parse_or_condition(condition_text: &str) -> Result<SimpleCondition, String> {
    let parts = split_top_level_condition(condition_text, b"||");
    if parts.len() == 1 {
        return parse_and_condition(parts[0]);
    }
    let mut cursor = parts.into_iter();
    let first = cursor
        .next()
        .ok_or_else(|| format!("invalid logical-or condition '{}'", condition_text))?;
    let mut out = parse_and_condition(first)?;
    for part in cursor {
        let rhs = parse_and_condition(part)?;
        out = SimpleCondition::Or(Box::new(out), Box::new(rhs));
    }
    Ok(out)
}

fn parse_and_condition(condition_text: &str) -> Result<SimpleCondition, String> {
    let parts = split_top_level_condition(condition_text, b"&&");
    if parts.len() == 1 {
        return parse_not_condition(parts[0]);
    }
    let mut cursor = parts.into_iter();
    let first = cursor
        .next()
        .ok_or_else(|| format!("invalid logical-and condition '{}'", condition_text))?;
    let mut out = parse_not_condition(first)?;
    for part in cursor {
        let rhs = parse_not_condition(part)?;
        out = SimpleCondition::And(Box::new(out), Box::new(rhs));
    }
    Ok(out)
}

fn parse_not_condition(condition_text: &str) -> Result<SimpleCondition, String> {
    let trimmed = condition_text.trim();
    if trimmed.is_empty() {
        return Err("condition expression cannot be empty".to_string());
    }
    if let Some(rest) = trimmed.strip_prefix('!') {
        let inner = parse_not_condition(rest)?;
        return Ok(SimpleCondition::Not(Box::new(inner)));
    }
    parse_condition_atom(trimmed)
}

fn parse_condition_atom(condition_text: &str) -> Result<SimpleCondition, String> {
    let trimmed = condition_text.trim();
    if trimmed.is_empty() {
        return Err("condition expression cannot be empty".to_string());
    }
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        if let Some(close_index) = find_matching_delimiter(trimmed, 0, b'(', b')') {
            if close_index == trimmed.len() - 1 {
                let inner = &trimmed[1..trimmed.len() - 1];
                return parse_or_condition(inner.trim());
            }
        }
    }
    if let Some((op, position, width)) = find_condition_operator(trimmed) {
        let lhs_text = trimmed[..position].trim();
        let rhs_text = trimmed[position + width..].trim();
        if lhs_text.is_empty() || rhs_text.is_empty() {
            return Err(format!(
                "invalid if condition '{}': both sides of comparison are required",
                trimmed
            ));
        }
        return Ok(SimpleCondition::Comparison {
            lhs: parse_simple_expression(lhs_text)?,
            op,
            rhs: parse_simple_expression(rhs_text)?,
        });
    }
    Ok(SimpleCondition::Expr(parse_simple_expression(trimmed)?))
}

fn split_top_level_condition<'a>(condition_text: &'a str, op: &[u8; 2]) -> Vec<&'a str> {
    let bytes = condition_text.as_bytes();
    let mut parts: Vec<&'a str> = Vec::new();
    let mut depth = 0i32;
    let mut segment_start = 0usize;
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        if in_string {
            if escaped {
                escaped = false;
                index += 1;
                continue;
            }
            if bytes[index] == b'\\' {
                escaped = true;
                index += 1;
                continue;
            }
            if bytes[index] == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            index += 2;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        match bytes[index] {
            b'"' => {
                in_string = true;
                index += 1;
                continue;
            }
            b'(' => {
                depth += 1;
                index += 1;
                continue;
            }
            b')' => {
                depth -= 1;
                index += 1;
                continue;
            }
            _ => {}
        }
        if depth == 0
            && index + 1 < bytes.len()
            && bytes[index] == op[0]
            && bytes[index + 1] == op[1]
        {
            parts.push(condition_text[segment_start..index].trim());
            segment_start = index + 2;
            index += 2;
            continue;
        }
        index += 1;
    }
    parts.push(condition_text[segment_start..].trim());
    parts
}

fn find_condition_operator(condition_text: &str) -> Option<(ComparisonOp, usize, usize)> {
    let bytes = condition_text.as_bytes();
    let mut depth = 0i32;
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        if in_string {
            if escaped {
                escaped = false;
                index += 1;
                continue;
            }
            if bytes[index] == b'\\' {
                escaped = true;
                index += 1;
                continue;
            }
            if bytes[index] == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            index += 2;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        match bytes[index] {
            b'"' => in_string = true,
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'=' | b'!' | b'<' | b'>' if depth == 0 => {
                if index + 1 < bytes.len() {
                    match (bytes[index], bytes[index + 1]) {
                        (b'=', b'=') => return Some((ComparisonOp::Eq, index, 2)),
                        (b'!', b'=') => return Some((ComparisonOp::Ne, index, 2)),
                        (b'<', b'=') => return Some((ComparisonOp::Le, index, 2)),
                        (b'>', b'=') => return Some((ComparisonOp::Ge, index, 2)),
                        _ => {}
                    }
                }
                match bytes[index] {
                    b'<' => return Some((ComparisonOp::Lt, index, 1)),
                    b'>' => return Some((ComparisonOp::Gt, index, 1)),
                    _ => {}
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn skip_ascii_whitespace(source: &str, mut cursor: usize) -> usize {
    while cursor < source.len() && source.as_bytes()[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

fn skip_ascii_whitespace_and_comments(source: &str, mut cursor: usize) -> usize {
    loop {
        cursor = skip_ascii_whitespace(source, cursor);
        let bytes = source.as_bytes();
        if cursor + 1 < bytes.len() && bytes[cursor] == b'/' && bytes[cursor + 1] == b'/' {
            cursor += 2;
            while cursor < bytes.len() && bytes[cursor] != b'\n' {
                cursor += 1;
            }
            continue;
        }
        if cursor + 1 < bytes.len() && bytes[cursor] == b'/' && bytes[cursor + 1] == b'*' {
            cursor += 2;
            let mut closed = false;
            while cursor + 1 < bytes.len() {
                if bytes[cursor] == b'*' && bytes[cursor + 1] == b'/' {
                    cursor += 2;
                    closed = true;
                    break;
                }
                cursor += 1;
            }
            if !closed {
                return bytes.len();
            }
            continue;
        }
        return cursor;
    }
}

fn starts_with_keyword(source: &str, cursor: usize, keyword: &str) -> bool {
    let Some(tail) = source.get(cursor..) else {
        return false;
    };
    if !tail.starts_with(keyword) {
        return false;
    }
    let end = cursor + keyword.len();
    if end >= source.len() {
        return true;
    }
    !source.as_bytes()[end].is_ascii_alphanumeric() && source.as_bytes()[end] != b'_'
}

fn looks_like_assignment(source: &str, cursor: usize) -> bool {
    let bytes = source.as_bytes();
    if cursor >= bytes.len() {
        return false;
    }
    if !bytes[cursor].is_ascii_alphabetic() && bytes[cursor] != b'_' {
        return false;
    }
    let mut index = cursor;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => paren_depth += 1,
            b')' => {
                paren_depth -= 1;
                if paren_depth < 0 {
                    return false;
                }
            }
            b'[' => bracket_depth += 1,
            b']' => {
                bracket_depth -= 1;
                if bracket_depth < 0 {
                    return false;
                }
            }
            b';' if paren_depth == 0 && bracket_depth == 0 => return false,
            b'=' if paren_depth == 0 && bracket_depth == 0 => {
                if index + 1 < bytes.len() && bytes[index + 1] == b'=' {
                    return false;
                }
                return true;
            }
            _ => {}
        }
        index += 1;
    }
    false
}

fn looks_like_from_conversion_statement(source: &str, cursor: usize) -> bool {
    let Ok(semicolon) = find_statement_terminator(source, cursor) else {
        return false;
    };
    let tail = source.get(cursor..semicolon).unwrap_or_default().trim();
    let Some(dot_pos) = tail.find(".from_") else {
        return false;
    };
    if dot_pos == 0 {
        return false;
    }
    let prefix = tail[..dot_pos].trim();
    if prefix.is_empty() {
        return false;
    }
    let first = prefix.as_bytes()[0];
    if !first.is_ascii_alphabetic() && first != b'_' {
        return false;
    }
    let method_tail = &tail[dot_pos..];
    method_tail.starts_with(".from_i32(") || method_tail.starts_with(".from_f32(")
}

fn looks_like_call_statement(source: &str, cursor: usize) -> bool {
    let Ok(semicolon) = find_statement_terminator(source, cursor) else {
        return false;
    };
    let statement_text = source.get(cursor..semicolon).unwrap_or_default().trim();
    if statement_text.is_empty() {
        return false;
    }
    let Ok(expression) = parse_simple_expression(statement_text) else {
        return false;
    };
    matches!(expression, SimpleExpr::Call { .. })
}

fn split_for_header(header: &str) -> Result<[String; 3], String> {
    let mut parts: Vec<String> = Vec::new();
    let bytes = header.as_bytes();
    let mut depth = 0i32;
    let mut segment_start = 0usize;
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        if in_string {
            if escaped {
                escaped = false;
                index += 1;
                continue;
            }
            if bytes[index] == b'\\' {
                escaped = true;
                index += 1;
                continue;
            }
            if bytes[index] == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            index += 2;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        match bytes[index] {
            b'"' => in_string = true,
            b'(' => depth += 1,
            b')' => depth -= 1,
            b';' if depth == 0 => {
                parts.push(header[segment_start..index].to_string());
                segment_start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    parts.push(header[segment_start..].to_string());
    if parts.len() != 3 {
        return Err(format!(
            "for header must contain exactly 3 segments separated by ';': '{}'",
            header
        ));
    }
    Ok([parts.remove(0), parts.remove(0), parts.remove(0)])
}

fn find_statement_terminator(source: &str, start: usize) -> Result<usize, String> {
    let bytes = source.as_bytes();
    let mut paren_depth = 0i32;
    let mut brace_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut index = start;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        if in_string {
            let byte = bytes[index];
            if escaped {
                escaped = false;
                index += 1;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                index += 1;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            index += 2;
            let mut closed = false;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    closed = true;
                    break;
                }
                index += 1;
            }
            if !closed {
                return Err(format!(
                    "unterminated block comment near '{}'",
                    snippet_from(source, start)
                ));
            }
            continue;
        }
        match bytes[index] {
            b'"' => in_string = true,
            b'(' => paren_depth += 1,
            b')' => paren_depth -= 1,
            b'{' => brace_depth += 1,
            b'}' => brace_depth -= 1,
            b'[' => bracket_depth += 1,
            b']' => bracket_depth -= 1,
            b';' if paren_depth == 0 && brace_depth == 0 && bracket_depth == 0 => return Ok(index),
            _ => {}
        }
        index += 1;
    }
    Err(format!(
        "missing ';' terminator near '{}'",
        snippet_from(source, start)
    ))
}

fn find_matching_delimiter(source: &str, open_index: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(open_index).copied() != Some(open) {
        return None;
    }
    let mut depth = 0i32;
    let mut index = open_index;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        if in_string {
            let byte = bytes[index];
            if escaped {
                escaped = false;
                index += 1;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                index += 1;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            index += 2;
            let mut closed = false;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    closed = true;
                    break;
                }
                index += 1;
            }
            if !closed {
                return None;
            }
            continue;
        }
        let byte = bytes[index];
        if byte == b'"' {
            in_string = true;
        } else if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

fn expect_byte(source: &str, cursor: usize, expected: u8, context: &str) -> Result<usize, String> {
    if cursor >= source.len() || source.as_bytes()[cursor] != expected {
        return Err(format!(
            "expected {} near '{}'",
            context,
            snippet_from(source, cursor)
        ));
    }
    Ok(cursor + 1)
}

fn parse_identifier(source: &str, cursor: usize) -> Result<(&str, usize), String> {
    let bytes = source.as_bytes();
    if cursor >= bytes.len() {
        return Err("expected identifier but reached end of statement".to_string());
    }
    let start_byte = bytes[cursor];
    if !start_byte.is_ascii_alphabetic() && start_byte != b'_' {
        return Err(format!(
            "expected identifier near '{}'",
            snippet_from(source, cursor)
        ));
    }
    let mut end = cursor + 1;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    Ok((&source[cursor..end], end))
}

fn parse_identifier_path(source: &str, cursor: usize) -> Result<(String, usize), String> {
    let (first, mut next) = parse_identifier(source, cursor)?;
    let mut path = first.to_string();
    loop {
        next = skip_ascii_whitespace(source, next);
        if source.as_bytes().get(next).copied() != Some(b'.') {
            break;
        }
        next += 1;
        next = skip_ascii_whitespace(source, next);
        let (segment, after_segment) = parse_identifier(source, next)?;
        path.push('.');
        path.push_str(segment);
        next = after_segment;
    }
    Ok((path, next))
}

fn assign_target_from_path(path: String) -> AssignTarget {
    if path.contains('.') {
        AssignTarget::GlobalPath(path)
    } else {
        AssignTarget::Local(path)
    }
}

fn snippet_from(source: &str, cursor: usize) -> String {
    source
        .get(cursor..)
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect()
}

fn emit_host_print_call_statement(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    call_signatures: &CallSignatureMap,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<bool, String> {
    if target != "print_i32"
        && target != "print_string"
        && target != "print_int"
        && target != "print_char"
    {
        return Ok(false);
    }
    if args.len() != 1 {
        return Err(format!(
            "host extern '{}' expects exactly one argument, found {}",
            target,
            args.len()
        ));
    }
    let argument = emit_simple_expression(
        builder,
        &args[0],
        values_by_name,
        runtime_call_refs,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        foreach_bindings,
    )?;
    if !is_i32_abi_compatible_type(argument.type_id, type_table) {
        return Err(format!(
            "host extern '{}' requires i32-abi-compatible argument, found type {}",
            target, argument.type_id
        ));
    }
    if target == "print_i32" || target == "print_int" || target == "print_char" {
        builder
            .ins()
            .call(runtime_call_refs.print_i32, &[argument.value]);
    } else {
        builder
            .ins()
            .call(runtime_call_refs.print_string, &[argument.value]);
    }
    Ok(true)
}

fn emit_indirect_call_for_signature(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    signature: &CallSignature,
    arg_values: &[Value],
    type_table: &TypeTable,
) -> Result<Option<Value>, String> {
    let function_id = signature
        .function_id
        .ok_or_else(|| "internal dispatch requested for extern call signature".to_string())?;
    let function_id_i32 = i32::try_from(function_id).map_err(|_| {
        format!(
            "function id {} out of i32 range for indirect call",
            function_id
        )
    })?;
    let fn_id_value = builder.ins().iconst(types::I32, i64::from(function_id_i32));
    let lookup = builder
        .ins()
        .call(runtime_call_refs.lookup_code_ptr, &[fn_id_value]);
    let code_ptr = builder.inst_results(lookup)[0];

    let mut indirect_signature =
        cranelift_codegen::ir::Signature::new(builder.func.signature.call_conv);
    for param_type in &signature.params {
        indirect_signature
            .params
            .push(AbiParam::new(clif_type_for_type_id(
                *param_type,
                type_table,
            )?));
    }
    if signature.return_type != TYPE_ID_VOID {
        indirect_signature
            .returns
            .push(AbiParam::new(clif_type_for_type_id(
                signature.return_type,
                type_table,
            )?));
    }
    let signature_ref = builder.func.import_signature(indirect_signature);
    let call = builder
        .ins()
        .call_indirect(signature_ref, code_ptr, arg_values);
    if signature.return_type == TYPE_ID_VOID {
        Ok(None)
    } else {
        let result =
            builder.inst_results(call).first().copied().ok_or_else(|| {
                "indirect call expected value result but produced none".to_string()
            })?;
        Ok(Some(result))
    }
}

fn emit_extern_call_for_signature(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    signature: &CallSignature,
    arg_values: &[Value],
) -> Result<Option<Value>, String> {
    let Some(symbol) = signature.extern_symbol.as_ref() else {
        return Err("extern dispatch requested for internal call signature".to_string());
    };
    let key = ExternImportKey {
        symbol: symbol.clone(),
        params: signature.params.clone(),
        return_type: signature.return_type,
    };
    let Some(func_ref) = runtime_call_refs.extern_calls.get(&key).copied() else {
        return Err(format!(
            "missing extern import binding for symbol '{}' with params {:?} return {}",
            symbol, signature.params, signature.return_type
        ));
    };
    let call = builder.ins().call(func_ref, arg_values);
    if signature.return_type == TYPE_ID_VOID {
        Ok(None)
    } else {
        let value = builder.inst_results(call).first().copied().ok_or_else(|| {
            format!(
                "extern call to '{}' expected value result but produced none",
                symbol
            )
        })?;
        Ok(Some(value))
    }
}

fn ensure_no_variable_shadowing(
    name: &str,
    values_by_name: &BTreeMap<String, LocalBinding>,
    foreach_bindings: &ForeachBindingMap,
    binding_kind: &str,
) -> Result<(), String> {
    if values_by_name.contains_key(name) || foreach_bindings.contains_key(name) {
        return Err(format!(
            "{} '{}' shadows existing variable",
            binding_kind, name
        ));
    }
    Ok(())
}

fn emit_simple_statements(
    builder: &mut FunctionBuilder<'_>,
    statements: &[SimpleStmt],
    values_by_name: &mut BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    foreach_bindings: &ForeachBindingMap,
    expected_return_type: TypeId,
    next_variable: &mut u32,
) -> Result<bool, String> {
    for statement in statements {
        match statement {
            SimpleStmt::Noop => {}
            SimpleStmt::Let {
                name,
                type_id,
                expression,
            } => {
                ensure_no_variable_shadowing(
                    name,
                    values_by_name,
                    foreach_bindings,
                    "let binding",
                )?;
                let binding = emit_simple_expression(
                    builder,
                    expression,
                    values_by_name,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                )?;
                let local_type_id = if let Some(declared_type_id) = *type_id {
                    if !are_assignment_types_compatible(
                        declared_type_id,
                        binding.type_id,
                        type_table,
                    ) {
                        return Err(format!(
                            "let binding '{}' expected type {} expression but found {}",
                            name, declared_type_id, binding.type_id
                        ));
                    }
                    declared_type_id
                } else {
                    binding.type_id
                };
                let variable = declare_new_variable(
                    builder,
                    next_variable,
                    binding.value,
                    local_type_id,
                    type_table,
                )?;
                values_by_name.insert(
                    name.clone(),
                    LocalBinding {
                        var: variable,
                        type_id: local_type_id,
                    },
                );
            }
            SimpleStmt::Assign {
                target,
                op,
                expression,
            } => {
                let rhs = emit_simple_expression(
                    builder,
                    expression,
                    values_by_name,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                )?;
                match target {
                    AssignTarget::Local(name) => {
                        if let Some(local) = values_by_name.get(name).copied() {
                            if !are_assignment_types_compatible(
                                local.type_id,
                                rhs.type_id,
                                type_table,
                            ) {
                                return Err(format!(
                                    "assignment type mismatch for '{}': target type {}, expression type {}",
                                    name, local.type_id, rhs.type_id
                                ));
                            }
                            let value = if is_i32_scalar_lane_type(local.type_id, type_table) {
                                match op {
                                    AssignOp::Set => rhs.value,
                                    AssignOp::Add => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().iadd(lhs, rhs.value)
                                    }
                                    AssignOp::Sub => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().isub(lhs, rhs.value)
                                    }
                                    AssignOp::Mul => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().imul(lhs, rhs.value)
                                    }
                                    AssignOp::Div => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().sdiv(lhs, rhs.value)
                                    }
                                    AssignOp::Mod => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().srem(lhs, rhs.value)
                                    }
                                }
                            } else if local.type_id == TYPE_ID_F32 {
                                match op {
                                    AssignOp::Set => rhs.value,
                                    AssignOp::Add => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().fadd(lhs, rhs.value)
                                    }
                                    AssignOp::Sub => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().fsub(lhs, rhs.value)
                                    }
                                    AssignOp::Mul => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().fmul(lhs, rhs.value)
                                    }
                                    AssignOp::Div => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().fdiv(lhs, rhs.value)
                                    }
                                    AssignOp::Mod => {
                                        return Err(format!(
                                            "'%=' requires i32 target in current jit path for '{}'",
                                            name
                                        ));
                                    }
                                }
                            } else if local.type_id == TYPE_ID_BOOL {
                                if *op != AssignOp::Set {
                                    return Err(format!(
                                        "bool assignment only supports '=' in current jit path for '{}'",
                                        name
                                    ));
                                }
                                rhs.value
                            } else {
                                return Err(format!(
                                    "unsupported local assignment type {} for '{}'",
                                    local.type_id, name
                                ));
                            };
                            builder.def_var(local.var, value);
                        } else {
                            if let Some((binding, suffix)) =
                                resolve_foreach_binding_for_path(name, foreach_bindings)
                            {
                                emit_foreach_binding_assignment(
                                    builder,
                                    runtime_call_refs,
                                    type_table,
                                    binding,
                                    &suffix,
                                    *op,
                                    rhs,
                                )?;
                                continue;
                            }
                            let Some(path_type) = global_path_types.get(name).copied() else {
                                return Err(format!(
                                    "unknown assignment target '{}' in current jit path",
                                    name
                                ));
                            };
                            emit_global_assignment(
                                builder,
                                runtime_call_refs,
                                type_table,
                                name,
                                path_type,
                                *op,
                                rhs,
                            )?;
                        }
                    }
                    AssignTarget::GlobalPath(path) => {
                        if let Some((binding, suffix)) =
                            resolve_foreach_binding_for_path(path, foreach_bindings)
                        {
                            emit_foreach_binding_assignment(
                                builder,
                                runtime_call_refs,
                                type_table,
                                binding,
                                &suffix,
                                *op,
                                rhs,
                            )?;
                            continue;
                        }
                        let Some(path_type) = global_path_types.get(path).copied() else {
                            return Err(format!(
                                "unknown global path '{}' in current jit path",
                                path
                            ));
                        };
                        emit_global_assignment(
                            builder,
                            runtime_call_refs,
                            type_table,
                            path,
                            path_type,
                            *op,
                            rhs,
                        )?;
                    }
                    AssignTarget::IndexedPath {
                        collection_path,
                        index,
                        suffix,
                    } => {
                        if let Some(local_collection) = values_by_name.get(collection_path).copied()
                        {
                            let index_binding = emit_simple_expression(
                                builder,
                                index,
                                values_by_name,
                                runtime_call_refs,
                                call_signatures,
                                type_table,
                                global_path_types,
                                constant_values,
                                collection_infos,
                                foreach_bindings,
                            )?;
                            emit_local_indexed_collection_assignment(
                                builder,
                                runtime_call_refs,
                                type_table,
                                collection_path,
                                local_collection,
                                suffix,
                                index_binding,
                                *op,
                                rhs,
                            )?;
                            continue;
                        }
                        let Some(collection_info) = collection_infos.get(collection_path) else {
                            return Err(format!(
                                "unknown indexed assignment collection '{}' in current jit path",
                                collection_path
                            ));
                        };
                        let index_binding = emit_simple_expression(
                            builder,
                            index,
                            values_by_name,
                            runtime_call_refs,
                            call_signatures,
                            type_table,
                            global_path_types,
                            constant_values,
                            collection_infos,
                            foreach_bindings,
                        )?;
                        emit_indexed_collection_assignment(
                            builder,
                            runtime_call_refs,
                            type_table,
                            collection_path,
                            collection_info,
                            suffix,
                            index_binding,
                            *op,
                            rhs,
                        )?;
                    }
                }
            }
            SimpleStmt::Convert {
                target,
                kind,
                source,
            } => {
                let source_binding = emit_simple_expression(
                    builder,
                    source,
                    values_by_name,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                )?;
                match target {
                    AssignTarget::Local(name) => {
                        let Some(local) = values_by_name.get(name).copied() else {
                            return Err(format!(
                                "conversion target '{}' is not a local binding",
                                name
                            ));
                        };
                        let converted = emit_conversion_assignment_value(
                            builder,
                            *kind,
                            source_binding,
                            local.type_id,
                            name,
                        )?;
                        builder.def_var(local.var, converted.value);
                    }
                    AssignTarget::GlobalPath(path) => {
                        let Some(path_type) = global_path_types.get(path).copied() else {
                            return Err(format!(
                                "unknown global path '{}' in current jit path",
                                path
                            ));
                        };
                        let converted = emit_conversion_assignment_value(
                            builder,
                            *kind,
                            source_binding,
                            path_type,
                            path,
                        )?;
                        emit_global_assignment(
                            builder,
                            runtime_call_refs,
                            type_table,
                            path,
                            path_type,
                            AssignOp::Set,
                            converted,
                        )?;
                    }
                    AssignTarget::IndexedPath {
                        collection_path,
                        index,
                        suffix,
                    } => {
                        let Some(collection_info) = collection_infos.get(collection_path) else {
                            return Err(format!(
                                "unknown indexed conversion collection '{}' in current jit path",
                                collection_path
                            ));
                        };
                        let target_type = resolve_collection_value_type(collection_info, suffix)?;
                        let target_name = format!("{collection_path}[...].{suffix}");
                        let converted = emit_conversion_assignment_value(
                            builder,
                            *kind,
                            source_binding,
                            target_type,
                            &target_name,
                        )?;
                        let index_binding = emit_simple_expression(
                            builder,
                            index,
                            values_by_name,
                            runtime_call_refs,
                            call_signatures,
                            type_table,
                            global_path_types,
                            constant_values,
                            collection_infos,
                            foreach_bindings,
                        )?;
                        emit_indexed_collection_assignment(
                            builder,
                            runtime_call_refs,
                            type_table,
                            collection_path,
                            collection_info,
                            suffix,
                            index_binding,
                            AssignOp::Set,
                            converted,
                        )?;
                    }
                }
            }
            SimpleStmt::Expr(expression) => {
                if let SimpleExpr::Call { target, args } = expression {
                    let handled = emit_host_print_call_statement(
                        builder,
                        runtime_call_refs,
                        type_table,
                        target,
                        args,
                        values_by_name,
                        call_signatures,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        foreach_bindings,
                    )?;
                    if handled {
                        continue;
                    }
                    let mut arg_values: Vec<Value> = Vec::with_capacity(args.len());
                    let mut arg_types: Vec<TypeId> = Vec::with_capacity(args.len());
                    for arg in args {
                        let binding = emit_simple_expression(
                            builder,
                            arg,
                            values_by_name,
                            runtime_call_refs,
                            call_signatures,
                            type_table,
                            global_path_types,
                            constant_values,
                            collection_infos,
                            foreach_bindings,
                        )?;
                        arg_values.push(binding.value);
                        arg_types.push(binding.type_id);
                    }
                    let signature =
                        resolve_call_signature(target, &arg_types, call_signatures, type_table)?;
                    if signature.return_type == TYPE_ID_VOID {
                        if signature.extern_symbol.is_some() {
                            let _ = emit_extern_call_for_signature(
                                builder,
                                runtime_call_refs,
                                signature,
                                &arg_values,
                            )?;
                        } else {
                            let _ = emit_indirect_call_for_signature(
                                builder,
                                runtime_call_refs,
                                signature,
                                &arg_values,
                                type_table,
                            )?;
                        }
                        continue;
                    }
                }
                let _ = emit_simple_expression(
                    builder,
                    expression,
                    values_by_name,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                )?;
            }
            SimpleStmt::Return(expression) => {
                let binding = emit_simple_expression(
                    builder,
                    expression,
                    values_by_name,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                )?;
                if !are_assignment_types_compatible(
                    expected_return_type,
                    binding.type_id,
                    type_table,
                ) {
                    return Err(format!(
                        "return expression expected type {} but found {}",
                        expected_return_type, binding.type_id
                    ));
                }
                builder.ins().return_(&[binding.value]);
                return Ok(true);
            }
            SimpleStmt::ReturnVoid => {
                if expected_return_type == TYPE_ID_VOID {
                    builder.ins().return_(&[]);
                    return Ok(true);
                }
                return Err("void return statement is not allowed in non-void function".to_string());
            }
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                let condition_value = emit_simple_condition(
                    builder,
                    condition,
                    values_by_name,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                )?;
                let then_block = builder.create_block();
                let else_block = builder.create_block();
                let continue_block = builder.create_block();
                builder
                    .ins()
                    .brif(condition_value, then_block, &[], else_block, &[]);
                builder.seal_block(then_block);
                builder.switch_to_block(then_block);

                let mut then_values = values_by_name.clone();
                let then_terminated = emit_simple_statements(
                    builder,
                    then_statements,
                    &mut then_values,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                    expected_return_type,
                    next_variable,
                )?;
                if !then_terminated {
                    builder.ins().jump(continue_block, &[]);
                }

                builder.seal_block(else_block);
                builder.switch_to_block(else_block);
                let else_terminated = if let Some(else_statements) = else_statements {
                    let mut else_values = values_by_name.clone();
                    emit_simple_statements(
                        builder,
                        else_statements,
                        &mut else_values,
                        runtime_call_refs,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        foreach_bindings,
                        expected_return_type,
                        next_variable,
                    )?
                } else {
                    false
                };
                if !else_terminated {
                    builder.ins().jump(continue_block, &[]);
                }
                if then_terminated && else_terminated {
                    return Ok(true);
                }

                builder.seal_block(continue_block);
                builder.switch_to_block(continue_block);
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                let mut loop_values = values_by_name.clone();
                emit_for_control_statement(
                    builder,
                    init.as_ref(),
                    &mut loop_values,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                    expected_return_type,
                    next_variable,
                )?;

                let condition_block = builder.create_block();
                let body_block = builder.create_block();
                let step_block = builder.create_block();
                let continue_block = builder.create_block();

                builder.ins().jump(condition_block, &[]);
                builder.switch_to_block(condition_block);

                let condition_value = emit_simple_condition(
                    builder,
                    condition,
                    &loop_values,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                )?;
                builder
                    .ins()
                    .brif(condition_value, body_block, &[], continue_block, &[]);

                builder.seal_block(body_block);
                builder.switch_to_block(body_block);
                let body_terminated = emit_simple_statements(
                    builder,
                    body_statements,
                    &mut loop_values,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                    expected_return_type,
                    next_variable,
                )?;
                if !body_terminated {
                    builder.ins().jump(step_block, &[]);
                }

                builder.seal_block(step_block);
                builder.switch_to_block(step_block);
                emit_for_control_statement(
                    builder,
                    step.as_ref(),
                    &mut loop_values,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                    expected_return_type,
                    next_variable,
                )?;
                builder.ins().jump(condition_block, &[]);
                builder.seal_block(condition_block);

                builder.seal_block(continue_block);
                builder.switch_to_block(continue_block);
            }
            SimpleStmt::Foreach {
                item_name,
                index_name,
                collection_path,
                body_statements,
            } => {
                ensure_no_variable_shadowing(
                    item_name,
                    values_by_name,
                    foreach_bindings,
                    "foreach item binding",
                )?;
                if let Some(index_name) = index_name {
                    if index_name == item_name {
                        return Err(format!(
                            "foreach index binding '{}' shadows existing variable",
                            index_name
                        ));
                    }
                    ensure_no_variable_shadowing(
                        index_name,
                        values_by_name,
                        foreach_bindings,
                        "foreach index binding",
                    )?;
                }
                let (collection_info, collection_handle) =
                    if let Some(local_collection) = values_by_name.get(collection_path).copied() {
                        let info = build_local_foreach_collection_info(
                            collection_path,
                            local_collection.type_id,
                            type_table,
                        )?;
                        (
                            info,
                            ForeachCollectionHandle::LocalVar(local_collection.var),
                        )
                    } else {
                        let Some(collection_info) = collection_infos.get(collection_path) else {
                            return Err(format!(
                                "unknown foreach collection '{}' in current jit path",
                                collection_path
                            ));
                        };
                        (
                            collection_info.clone(),
                            ForeachCollectionHandle::PathHash(hash_global_path(collection_path)),
                        )
                    };
                let initial_index_value = builder.ins().iconst(types::I32, 0);
                let index_var = declare_new_variable(
                    builder,
                    next_variable,
                    initial_index_value,
                    TYPE_ID_I32,
                    type_table,
                )?;
                let mut loop_values = values_by_name.clone();
                if let Some(index_name) = index_name {
                    loop_values.insert(
                        index_name.clone(),
                        LocalBinding {
                            var: index_var,
                            type_id: TYPE_ID_I32,
                        },
                    );
                }
                let mut loop_foreach_bindings = foreach_bindings.clone();
                loop_foreach_bindings.insert(
                    item_name.clone(),
                    ForeachBinding {
                        collection_handle,
                        index_var,
                        element_type: collection_info.element_type,
                        field_types: collection_info.field_types.clone(),
                    },
                );

                let condition_block = builder.create_block();
                let body_block = builder.create_block();
                let step_block = builder.create_block();
                let continue_block = builder.create_block();

                builder.ins().jump(condition_block, &[]);
                builder.switch_to_block(condition_block);

                let index_value = builder.use_var(index_var);
                let len_value = builder
                    .ins()
                    .iconst(types::I32, i64::from(collection_info.len));
                let condition_value =
                    builder
                        .ins()
                        .icmp(IntCC::SignedLessThan, index_value, len_value);
                builder
                    .ins()
                    .brif(condition_value, body_block, &[], continue_block, &[]);

                builder.seal_block(body_block);
                builder.switch_to_block(body_block);
                let body_terminated = emit_simple_statements(
                    builder,
                    body_statements,
                    &mut loop_values,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    &loop_foreach_bindings,
                    expected_return_type,
                    next_variable,
                )?;
                if !body_terminated {
                    builder.ins().jump(step_block, &[]);
                }

                builder.seal_block(step_block);
                builder.switch_to_block(step_block);
                let current_index = builder.use_var(index_var);
                let next_index = builder.ins().iadd_imm(current_index, 1);
                builder.def_var(index_var, next_index);
                builder.ins().jump(condition_block, &[]);
                builder.seal_block(condition_block);

                builder.seal_block(continue_block);
                builder.switch_to_block(continue_block);
            }
        }
    }
    Ok(false)
}

fn emit_conversion_assignment_value(
    builder: &mut FunctionBuilder<'_>,
    kind: ConversionKind,
    source: ValueBinding,
    target_type: TypeId,
    target_name: &str,
) -> Result<ValueBinding, String> {
    match kind {
        ConversionKind::FromI32 => {
            if source.type_id != TYPE_ID_I32 {
                return Err("from_i32 source expression must be i32".to_string());
            }
            if target_type != TYPE_ID_F32 {
                return Err(format!("from_i32 target '{}' must be f32", target_name));
            }
            Ok(ValueBinding {
                value: builder.ins().fcvt_from_sint(types::F32, source.value),
                type_id: TYPE_ID_F32,
            })
        }
        ConversionKind::FromF32 => {
            if source.type_id != TYPE_ID_F32 {
                return Err("from_f32 source expression must be f32".to_string());
            }
            if target_type != TYPE_ID_I32 {
                return Err(format!("from_f32 target '{}' must be i32", target_name));
            }
            Ok(ValueBinding {
                value: builder.ins().fcvt_to_sint(types::I32, source.value),
                type_id: TYPE_ID_I32,
            })
        }
    }
}

fn emit_for_control_statement(
    builder: &mut FunctionBuilder<'_>,
    statement: &SimpleStmt,
    values_by_name: &mut BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    foreach_bindings: &ForeachBindingMap,
    expected_return_type: TypeId,
    next_variable: &mut u32,
) -> Result<(), String> {
    match statement {
        SimpleStmt::Noop => Ok(()),
        SimpleStmt::Let { .. }
        | SimpleStmt::Assign { .. }
        | SimpleStmt::Convert { .. }
        | SimpleStmt::Expr(_) => {
            let terminated = emit_simple_statements(
                builder,
                std::slice::from_ref(statement),
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
                expected_return_type,
                next_variable,
            )?;
            if terminated {
                return Err("for-loop control statement cannot terminate function".to_string());
            }
            Ok(())
        }
        other => Err(format!(
            "unsupported for-loop control statement in current jit path: {:?}",
            other
        )),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum SimpleExpr {
    Int(i64),
    Float(f32),
    Bool(bool),
    StringLiteral(String),
    Condition(Box<SimpleCondition>),
    Identifier(String),
    IndexedPath {
        collection_path: String,
        index: Box<SimpleExpr>,
        suffix: String,
    },
    Call {
        target: String,
        args: Vec<SimpleExpr>,
    },
    Binary {
        lhs: Box<SimpleExpr>,
        op: char,
        rhs: Box<SimpleExpr>,
    },
}

fn parse_simple_expression(expression: &str) -> Result<SimpleExpr, String> {
    let tokens = tokenize_simple_expression(expression)?;
    let mut parser = ExprParser {
        tokens: &tokens,
        cursor: 0,
    };
    let parsed = parser.parse_precedence(0)?;
    if parser.cursor != parser.tokens.len() {
        return Err(format!(
            "unexpected trailing tokens in expression '{}'",
            expression
        ));
    }
    Ok(parsed)
}

fn parse_value_expression(expression: &str) -> Result<SimpleExpr, String> {
    match parse_simple_expression(expression) {
        Ok(parsed) => Ok(parsed),
        Err(primary_error) => {
            if !looks_like_condition_expression(expression) {
                return Err(primary_error);
            }
            match parse_simple_condition(expression) {
                Ok(condition) => Ok(SimpleExpr::Condition(Box::new(condition))),
                Err(_) => Err(primary_error),
            }
        }
    }
}

fn looks_like_condition_expression(expression: &str) -> bool {
    let bytes = expression.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'<' || byte == b'>' {
            return true;
        }
        if byte == b'=' && index + 1 < bytes.len() && bytes[index + 1] == b'=' {
            return true;
        }
        if byte == b'!' {
            if index + 1 < bytes.len() && bytes[index + 1] == b'=' {
                return true;
            }
            return true;
        }
        if byte == b'&' && index + 1 < bytes.len() && bytes[index + 1] == b'&' {
            return true;
        }
        if byte == b'|' && index + 1 < bytes.len() && bytes[index + 1] == b'|' {
            return true;
        }
        index += 1;
    }
    false
}

#[derive(Debug, Clone, PartialEq)]
enum ExprToken {
    Int(i64),
    Float(f32),
    StringLiteral(String),
    Identifier(String),
    Op(char),
    Comma,
    Dot,
    LBracket,
    RBracket,
    LParen,
    RParen,
}

fn tokenize_simple_expression(expression: &str) -> Result<Vec<ExprToken>, String> {
    let bytes = expression.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if byte == b'/' && index + 1 < bytes.len() {
            let next = bytes[index + 1];
            if next == b'/' {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
                continue;
            }
            if next == b'*' {
                index += 2;
                let mut closed = false;
                while index + 1 < bytes.len() {
                    if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                        index += 2;
                        closed = true;
                        break;
                    }
                    index += 1;
                }
                if !closed {
                    return Err(format!(
                        "unterminated block comment in expression '{}'",
                        expression
                    ));
                }
                continue;
            }
        }
        if byte == b'"' {
            index += 1;
            let mut literal = String::new();
            let mut closed = false;
            while index < bytes.len() {
                let current = bytes[index];
                if current == b'\\' {
                    index += 1;
                    if index >= bytes.len() {
                        return Err(format!(
                            "unterminated escape sequence in string literal '{}'",
                            expression
                        ));
                    }
                    let escaped = bytes[index];
                    let decoded = match escaped {
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'0' => '\0',
                        b'\\' => '\\',
                        b'"' => '"',
                        _ => {
                            return Err(format!(
                                "unsupported escape sequence '\\{}' in expression '{}'",
                                escaped as char, expression
                            ))
                        }
                    };
                    literal.push(decoded);
                    index += 1;
                    continue;
                }
                if current == b'"' {
                    index += 1;
                    closed = true;
                    break;
                }
                let Some(next_char) = expression[index..].chars().next() else {
                    return Err(format!(
                        "unterminated string literal in expression '{}'",
                        expression
                    ));
                };
                literal.push(next_char);
                index += next_char.len_utf8();
            }
            if !closed {
                return Err(format!(
                    "unterminated string literal in expression '{}'",
                    expression
                ));
            }
            tokens.push(ExprToken::StringLiteral(literal));
            continue;
        }
        if byte.is_ascii_digit() {
            let start = index;
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            if index < bytes.len()
                && bytes[index] == b'.'
                && index + 1 < bytes.len()
                && bytes[index + 1].is_ascii_digit()
            {
                index += 1;
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    index += 1;
                }
                let text = &expression[start..index];
                let value = text
                    .parse::<f32>()
                    .map_err(|error| format!("invalid float literal '{text}': {error}"))?;
                tokens.push(ExprToken::Float(value));
            } else {
                let text = &expression[start..index];
                let value = text
                    .parse::<i64>()
                    .map_err(|error| format!("invalid integer literal '{text}': {error}"))?;
                tokens.push(ExprToken::Int(value));
            }
            continue;
        }
        if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = index;
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            tokens.push(ExprToken::Identifier(expression[start..index].to_string()));
            continue;
        }
        match byte {
            b'+' | b'-' | b'*' | b'/' | b'%' => {
                tokens.push(ExprToken::Op(byte as char));
                index += 1;
            }
            b',' => {
                tokens.push(ExprToken::Comma);
                index += 1;
            }
            b'.' => {
                tokens.push(ExprToken::Dot);
                index += 1;
            }
            b'(' => {
                tokens.push(ExprToken::LParen);
                index += 1;
            }
            b')' => {
                tokens.push(ExprToken::RParen);
                index += 1;
            }
            b'[' => {
                tokens.push(ExprToken::LBracket);
                index += 1;
            }
            b']' => {
                tokens.push(ExprToken::RBracket);
                index += 1;
            }
            _ => {
                return Err(format!(
                    "unsupported token '{}' in return expression '{}'",
                    byte as char, expression
                ));
            }
        }
    }
    Ok(tokens)
}

struct ExprParser<'a> {
    tokens: &'a [ExprToken],
    cursor: usize,
}

impl ExprParser<'_> {
    fn parse_precedence(&mut self, min_precedence: u8) -> Result<SimpleExpr, String> {
        let mut lhs = self.parse_primary()?;
        while let Some((operator, precedence)) = self.peek_binary_operator() {
            if precedence < min_precedence {
                break;
            }
            self.cursor += 1;
            let rhs = self.parse_precedence(precedence + 1)?;
            lhs = SimpleExpr::Binary {
                lhs: Box::new(lhs),
                op: operator,
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_primary(&mut self) -> Result<SimpleExpr, String> {
        let token = self
            .tokens
            .get(self.cursor)
            .ok_or_else(|| "unexpected end of expression".to_string())?
            .clone();
        self.cursor += 1;
        match token {
            ExprToken::Int(value) => Ok(SimpleExpr::Int(value)),
            ExprToken::Float(value) => Ok(SimpleExpr::Float(value)),
            ExprToken::StringLiteral(value) => Ok(SimpleExpr::StringLiteral(value)),
            ExprToken::Identifier(name) => {
                if name == "true" {
                    return Ok(SimpleExpr::Bool(true));
                }
                if name == "false" {
                    return Ok(SimpleExpr::Bool(false));
                }
                if matches!(self.tokens.get(self.cursor), Some(ExprToken::LParen)) {
                    self.cursor += 1;
                    let mut args = Vec::new();
                    if !matches!(self.tokens.get(self.cursor), Some(ExprToken::RParen)) {
                        loop {
                            args.push(self.parse_precedence(0)?);
                            if matches!(self.tokens.get(self.cursor), Some(ExprToken::Comma)) {
                                self.cursor += 1;
                                continue;
                            }
                            break;
                        }
                    }
                    match self.tokens.get(self.cursor) {
                        Some(ExprToken::RParen) => {
                            self.cursor += 1;
                            Ok(SimpleExpr::Call { target: name, args })
                        }
                        _ => Err("expected ')' after call arguments".to_string()),
                    }
                } else if matches!(self.tokens.get(self.cursor), Some(ExprToken::Dot))
                    && matches!(
                        self.tokens.get(self.cursor + 1),
                        Some(ExprToken::Identifier(_))
                    )
                    && matches!(self.tokens.get(self.cursor + 2), Some(ExprToken::LParen))
                {
                    let receiver = name.clone();
                    self.cursor += 1;
                    let Some(ExprToken::Identifier(segment_name)) =
                        self.tokens.get(self.cursor).cloned()
                    else {
                        return Err("expected identifier after '.' in receiver call".to_string());
                    };
                    self.cursor += 1;
                    self.cursor += 1;
                    let mut args = vec![SimpleExpr::Identifier(receiver)];
                    if !matches!(self.tokens.get(self.cursor), Some(ExprToken::RParen)) {
                        loop {
                            args.push(self.parse_precedence(0)?);
                            if matches!(self.tokens.get(self.cursor), Some(ExprToken::Comma)) {
                                self.cursor += 1;
                                continue;
                            }
                            break;
                        }
                    }
                    match self.tokens.get(self.cursor) {
                        Some(ExprToken::RParen) => {
                            self.cursor += 1;
                            Ok(SimpleExpr::Call {
                                target: segment_name,
                                args,
                            })
                        }
                        _ => Err("expected ')' after call arguments".to_string()),
                    }
                } else {
                    self.parse_identifier_access_chain(name)
                }
            }
            ExprToken::Op('-') => {
                let rhs = self.parse_primary()?;
                let lhs = match rhs {
                    SimpleExpr::Float(_) => SimpleExpr::Float(0.0),
                    _ => SimpleExpr::Int(0),
                };
                Ok(SimpleExpr::Binary {
                    lhs: Box::new(lhs),
                    op: '-',
                    rhs: Box::new(rhs),
                })
            }
            ExprToken::Op('+') => self.parse_primary(),
            ExprToken::LParen => {
                let expr = self.parse_precedence(0)?;
                match self.tokens.get(self.cursor) {
                    Some(ExprToken::RParen) => {
                        self.cursor += 1;
                        Ok(expr)
                    }
                    _ => Err("expected ')' in expression".to_string()),
                }
            }
            other => Err(format!("unexpected token {other:?} in expression")),
        }
    }

    fn parse_identifier_access_chain(&mut self, first: String) -> Result<SimpleExpr, String> {
        let mut collection_path = first;
        let mut index_expr: Option<SimpleExpr> = None;
        let mut suffix = String::new();
        loop {
            if matches!(self.tokens.get(self.cursor), Some(ExprToken::Dot)) {
                self.cursor += 1;
                let Some(ExprToken::Identifier(segment)) = self.tokens.get(self.cursor).cloned()
                else {
                    return Err("expected identifier after '.' in expression path".to_string());
                };
                self.cursor += 1;
                if index_expr.is_none() {
                    collection_path.push('.');
                    collection_path.push_str(&segment);
                } else {
                    if !suffix.is_empty() {
                        suffix.push('.');
                    }
                    suffix.push_str(&segment);
                }
                continue;
            }
            if matches!(self.tokens.get(self.cursor), Some(ExprToken::LBracket)) {
                if index_expr.is_some() {
                    return Err(
                        "multiple index segments are unsupported in expression path".to_string()
                    );
                }
                self.cursor += 1;
                let expression = self.parse_precedence(0)?;
                match self.tokens.get(self.cursor) {
                    Some(ExprToken::RBracket) => {
                        self.cursor += 1;
                        index_expr = Some(expression);
                    }
                    _ => return Err("expected ']' in expression path".to_string()),
                }
                continue;
            }
            break;
        }
        if let Some(index) = index_expr {
            Ok(SimpleExpr::IndexedPath {
                collection_path,
                index: Box::new(index),
                suffix,
            })
        } else {
            Ok(SimpleExpr::Identifier(collection_path))
        }
    }

    fn peek_binary_operator(&self) -> Option<(char, u8)> {
        let ExprToken::Op(op) = self.tokens.get(self.cursor)? else {
            return None;
        };
        let precedence = match *op {
            '*' | '/' | '%' => 20,
            '+' | '-' => 10,
            _ => return None,
        };
        Some((*op, precedence))
    }
}

fn resolve_call_signature<'a>(
    target: &str,
    arg_types: &[TypeId],
    call_signatures: &'a CallSignatureMap,
    type_table: &TypeTable,
) -> Result<&'a CallSignature, String> {
    let Some(candidates) = call_signatures.get(target) else {
        return Err(format!("unknown call target '{}'", target));
    };
    let mut matches = candidates.iter().filter(|candidate| {
        candidate.params.len() == arg_types.len()
            && arg_types
                .iter()
                .zip(candidate.params.iter())
                .all(|(arg, param)| type_table.is_argument_compatible_with_param(*arg, *param))
    });
    let Some(first) = matches.next() else {
        return Err(format!(
            "no matching overload for call target '{}' with parameter types {:?}",
            target, arg_types
        ));
    };
    if matches.next().is_some() {
        return Err(format!(
            "ambiguous overload for call target '{}' with parameter types {:?}",
            target, arg_types
        ));
    }
    Ok(first)
}

fn emit_simple_expression(
    builder: &mut FunctionBuilder<'_>,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<ValueBinding, String> {
    match expression {
        SimpleExpr::Int(value) => {
            let value = i32::try_from(*value).map_err(|_| {
                format!("integer literal out of i32 range in return expression: {value}")
            })?;
            Ok(ValueBinding {
                value: builder.ins().iconst(types::I32, i64::from(value)),
                type_id: TYPE_ID_I32,
            })
        }
        SimpleExpr::Float(value) => Ok(ValueBinding {
            value: builder.ins().f32const(Ieee32::with_float(*value)),
            type_id: TYPE_ID_F32,
        }),
        SimpleExpr::Bool(value) => Ok(ValueBinding {
            value: builder
                .ins()
                .iconst(types::I32, if *value { 1_i64 } else { 0_i64 }),
            type_id: TYPE_ID_BOOL,
        }),
        SimpleExpr::StringLiteral(value) => {
            let literal_id = hash_string_literal(value);
            stasis_dynload::upsert_jit_string_literal(literal_id, value);
            let string_type_id = type_table.resolve("string").unwrap_or(TYPE_ID_I32);
            Ok(ValueBinding {
                value: builder.ins().iconst(types::I32, i64::from(literal_id)),
                type_id: string_type_id,
            })
        }
        SimpleExpr::Condition(condition) => {
            let bool_value = emit_simple_condition(
                builder,
                condition,
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
            )?;
            let one = builder.ins().iconst(types::I32, 1_i64);
            let zero = builder.ins().iconst(types::I32, 0_i64);
            let value = builder.ins().select(bool_value, one, zero);
            Ok(ValueBinding {
                value,
                type_id: TYPE_ID_BOOL,
            })
        }
        SimpleExpr::Identifier(name) => {
            if let Some(local) = values_by_name.get(name).copied() {
                Ok(ValueBinding {
                    value: builder.use_var(local.var),
                    type_id: local.type_id,
                })
            } else if let Some((binding, suffix)) =
                resolve_foreach_binding_for_path(name, foreach_bindings)
            {
                emit_foreach_binding_load(builder, runtime_call_refs, type_table, binding, &suffix)
            } else if let Some(constant) = constant_values.get(name) {
                emit_constant_value(builder, constant)
            } else {
                let Some(path_type) = global_path_types.get(name).copied() else {
                    return Err(format!("unknown identifier '{}' in current jit path", name));
                };
                emit_global_load(builder, runtime_call_refs, type_table, name, path_type)
            }
        }
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            if let Some(local_collection) = values_by_name.get(collection_path).copied() {
                let index_binding = emit_simple_expression(
                    builder,
                    index,
                    values_by_name,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                )?;
                return emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    local_collection,
                    suffix,
                    index_binding,
                );
            }
            let Some(collection_info) = collection_infos.get(collection_path) else {
                return Err(format!(
                    "unknown indexed collection '{}' in current jit path",
                    collection_path
                ));
            };
            let index_binding = emit_simple_expression(
                builder,
                index,
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
            )?;
            emit_indexed_collection_load(
                builder,
                runtime_call_refs,
                type_table,
                collection_path,
                collection_info,
                suffix,
                index_binding,
            )
        }
        SimpleExpr::Call { target, args } => {
            let mut arg_values: Vec<Value> = Vec::with_capacity(args.len());
            let mut arg_types: Vec<TypeId> = Vec::with_capacity(args.len());
            for arg in args {
                let binding = emit_simple_expression(
                    builder,
                    arg,
                    values_by_name,
                    runtime_call_refs,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    foreach_bindings,
                )?;
                arg_values.push(binding.value);
                arg_types.push(binding.type_id);
            }
            if (target == "sin_fast" || target == "cos_fast") && arg_values.len() != 1 {
                return Err(format!(
                    "math intrinsic '{}' expects exactly one argument, found {}",
                    target,
                    arg_values.len()
                ));
            }
            if (target == "sin_fast" || target == "cos_fast") && arg_values.len() == 1 {
                if arg_types[0] != TYPE_ID_F32 {
                    return Err(format!(
                        "math intrinsic '{}' requires f32 argument, found type {}",
                        target, arg_types[0]
                    ));
                }
                let call = if target == "sin_fast" {
                    builder
                        .ins()
                        .call(runtime_call_refs.sin_fast, &[arg_values[0]])
                } else {
                    builder
                        .ins()
                        .call(runtime_call_refs.cos_fast, &[arg_values[0]])
                };
                return Ok(ValueBinding {
                    value: builder.inst_results(call)[0],
                    type_id: TYPE_ID_F32,
                });
            }
            let signature =
                resolve_call_signature(target, &arg_types, call_signatures, type_table)?;
            if signature.extern_symbol.is_some() {
                let result = emit_extern_call_for_signature(
                    builder,
                    runtime_call_refs,
                    signature,
                    &arg_values,
                )?;
                let value = result.ok_or_else(|| {
                    format!(
                        "void call target '{}' cannot be used in value expression",
                        target
                    )
                })?;
                return Ok(ValueBinding {
                    value,
                    type_id: signature.return_type,
                });
            }
            let function_id = signature.function_id.ok_or_else(|| {
                format!(
                    "internal call target '{}' is missing function id metadata",
                    target
                )
            })?;
            let function_id_i32 = i32::try_from(function_id).map_err(|_| {
                format!(
                    "function id {} out of i32 range for call target '{}'",
                    function_id, target
                )
            })?;
            let fn_id_value = builder.ins().iconst(types::I32, i64::from(function_id_i32));
            let call = if is_i32_abi_compatible_type(signature.return_type, type_table) {
                let all_i32_abi_args = arg_types
                    .iter()
                    .all(|type_id| is_i32_abi_compatible_type(*type_id, type_table))
                    && signature
                        .params
                        .iter()
                        .all(|type_id| is_i32_abi_compatible_type(*type_id, type_table));
                let all_f32_args = arg_types.iter().all(|type_id| *type_id == TYPE_ID_F32)
                    && signature
                        .params
                        .iter()
                        .all(|type_id| *type_id == TYPE_ID_F32);
                if all_i32_abi_args {
                    match arg_values.len() {
                        0 => builder
                            .ins()
                            .call(runtime_call_refs.call_i32_0, &[fn_id_value]),
                        1 => builder
                            .ins()
                            .call(runtime_call_refs.call_i32_1, &[fn_id_value, arg_values[0]]),
                        2 => builder.ins().call(
                            runtime_call_refs.call_i32_2,
                            &[fn_id_value, arg_values[0], arg_values[1]],
                        ),
                        3 => builder.ins().call(
                            runtime_call_refs.call_i32_3,
                            &[fn_id_value, arg_values[0], arg_values[1], arg_values[2]],
                        ),
                        4 => builder.ins().call(
                            runtime_call_refs.call_i32_4,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                            ],
                        ),
                        5 => builder.ins().call(
                            runtime_call_refs.call_i32_5,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                                arg_values[4],
                            ],
                        ),
                        6 => builder.ins().call(
                            runtime_call_refs.call_i32_6,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                                arg_values[4],
                                arg_values[5],
                            ],
                        ),
                        7 => builder.ins().call(
                            runtime_call_refs.call_i32_7,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                                arg_values[4],
                                arg_values[5],
                                arg_values[6],
                            ],
                        ),
                        8 => builder.ins().call(
                            runtime_call_refs.call_i32_8,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                                arg_values[4],
                                arg_values[5],
                                arg_values[6],
                                arg_values[7],
                            ],
                        ),
                        other => {
                            return Err(format!(
                                "unsupported call arity {} in expression for target '{}'",
                                other, target
                            ))
                        }
                    }
                } else if all_f32_args {
                    match arg_values.len() {
                        0 => builder
                            .ins()
                            .call(runtime_call_refs.call_i32_0, &[fn_id_value]),
                        1 => builder.ins().call(
                            runtime_call_refs.call_i32_f32_1,
                            &[fn_id_value, arg_values[0]],
                        ),
                        2 => builder.ins().call(
                            runtime_call_refs.call_i32_f32_2,
                            &[fn_id_value, arg_values[0], arg_values[1]],
                        ),
                        3 => builder.ins().call(
                            runtime_call_refs.call_i32_f32_3,
                            &[fn_id_value, arg_values[0], arg_values[1], arg_values[2]],
                        ),
                        4 => builder.ins().call(
                            runtime_call_refs.call_i32_f32_4,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                            ],
                        ),
                        5 => builder.ins().call(
                            runtime_call_refs.call_i32_f32_5,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                                arg_values[4],
                            ],
                        ),
                        6 => builder.ins().call(
                            runtime_call_refs.call_i32_f32_6,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                                arg_values[4],
                                arg_values[5],
                            ],
                        ),
                        7 => builder.ins().call(
                            runtime_call_refs.call_i32_f32_7,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                                arg_values[4],
                                arg_values[5],
                                arg_values[6],
                            ],
                        ),
                        8 => builder.ins().call(
                            runtime_call_refs.call_i32_f32_8,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                                arg_values[4],
                                arg_values[5],
                                arg_values[6],
                                arg_values[7],
                            ],
                        ),
                        other => {
                            return Err(format!(
                                "unsupported call arity {} in expression for target '{}'",
                                other, target
                            ))
                        }
                    }
                } else {
                    let result = if signature.extern_symbol.is_some() {
                        emit_extern_call_for_signature(
                            builder,
                            runtime_call_refs,
                            signature,
                            &arg_values,
                        )?
                    } else {
                        emit_indirect_call_for_signature(
                            builder,
                            runtime_call_refs,
                            signature,
                            &arg_values,
                            type_table,
                        )?
                    };
                    let value = result
                        .ok_or_else(|| format!("call target '{}' did not produce value", target))?;
                    return Ok(ValueBinding {
                        value,
                        type_id: signature.return_type,
                    });
                }
            } else if signature.return_type == TYPE_ID_F32 {
                if arg_values.is_empty() {
                    builder
                        .ins()
                        .call(runtime_call_refs.call_f32_0, &[fn_id_value])
                } else if arg_values.len() == 1
                    && is_i32_abi_compatible_type(signature.params[0], type_table)
                    && is_i32_abi_compatible_type(arg_types[0], type_table)
                {
                    builder.ins().call(
                        runtime_call_refs.call_f32_i32_1,
                        &[fn_id_value, arg_values[0]],
                    )
                } else {
                    if !arg_types.iter().all(|type_id| *type_id == TYPE_ID_F32) {
                        let result = if signature.extern_symbol.is_some() {
                            emit_extern_call_for_signature(
                                builder,
                                runtime_call_refs,
                                signature,
                                &arg_values,
                            )?
                        } else {
                            emit_indirect_call_for_signature(
                                builder,
                                runtime_call_refs,
                                signature,
                                &arg_values,
                                type_table,
                            )?
                        };
                        let value = result.ok_or_else(|| {
                            format!("call target '{}' did not produce value", target)
                        })?;
                        return Ok(ValueBinding {
                            value,
                            type_id: signature.return_type,
                        });
                    }
                    match arg_values.len() {
                        1 => builder
                            .ins()
                            .call(runtime_call_refs.call_f32_1, &[fn_id_value, arg_values[0]]),
                        2 => builder.ins().call(
                            runtime_call_refs.call_f32_2,
                            &[fn_id_value, arg_values[0], arg_values[1]],
                        ),
                        3 => builder.ins().call(
                            runtime_call_refs.call_f32_3,
                            &[fn_id_value, arg_values[0], arg_values[1], arg_values[2]],
                        ),
                        4 => builder.ins().call(
                            runtime_call_refs.call_f32_4,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                            ],
                        ),
                        5 => builder.ins().call(
                            runtime_call_refs.call_f32_5,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                                arg_values[4],
                            ],
                        ),
                        6 => builder.ins().call(
                            runtime_call_refs.call_f32_6,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                                arg_values[4],
                                arg_values[5],
                            ],
                        ),
                        7 => builder.ins().call(
                            runtime_call_refs.call_f32_7,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                                arg_values[4],
                                arg_values[5],
                                arg_values[6],
                            ],
                        ),
                        8 => builder.ins().call(
                            runtime_call_refs.call_f32_8,
                            &[
                                fn_id_value,
                                arg_values[0],
                                arg_values[1],
                                arg_values[2],
                                arg_values[3],
                                arg_values[4],
                                arg_values[5],
                                arg_values[6],
                                arg_values[7],
                            ],
                        ),
                        other => {
                            return Err(format!(
                                "unsupported call arity {} in expression for target '{}'",
                                other, target
                            ))
                        }
                    }
                }
            } else {
                return Err(format!(
                    "unsupported return type {} for call target '{}'",
                    signature.return_type, target
                ));
            };
            let results = builder.inst_results(call);
            let value = results
                .first()
                .copied()
                .ok_or_else(|| format!("call to '{}' produced no value", target))?;
            Ok(ValueBinding {
                value,
                type_id: signature.return_type,
            })
        }
        SimpleExpr::Binary { lhs, op, rhs } => {
            let lhs_value = emit_simple_expression(
                builder,
                lhs,
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
            )?;
            let rhs_value = emit_simple_expression(
                builder,
                rhs,
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
            )?;
            if is_i32_numeric_type(lhs_value.type_id, type_table)
                && is_i32_numeric_type(rhs_value.type_id, type_table)
            {
                let value = match op {
                    '+' => builder.ins().iadd(lhs_value.value, rhs_value.value),
                    '-' => builder.ins().isub(lhs_value.value, rhs_value.value),
                    '*' => builder.ins().imul(lhs_value.value, rhs_value.value),
                    '/' => builder.ins().sdiv(lhs_value.value, rhs_value.value),
                    '%' => builder.ins().srem(lhs_value.value, rhs_value.value),
                    other => {
                        return Err(format!(
                            "unsupported binary operator '{other}' in expression"
                        ))
                    }
                };
                return Ok(ValueBinding {
                    value,
                    type_id: TYPE_ID_I32,
                });
            }

            let (lhs_f32, rhs_f32) =
                coerce_numeric_operands_to_f32(builder, lhs_value, rhs_value, *op, type_table)?;
            let value = match op {
                '+' => builder.ins().fadd(lhs_f32, rhs_f32),
                '-' => builder.ins().fsub(lhs_f32, rhs_f32),
                '*' => builder.ins().fmul(lhs_f32, rhs_f32),
                '/' => builder.ins().fdiv(lhs_f32, rhs_f32),
                '%' => {
                    return Err(
                        "unsupported '%' operator for f32 expression in current jit path"
                            .to_string(),
                    )
                }
                other => {
                    return Err(format!(
                        "unsupported binary operator '{other}' in expression"
                    ))
                }
            };
            Ok(ValueBinding {
                value,
                type_id: TYPE_ID_F32,
            })
        }
    }
}

fn coerce_numeric_operands_to_f32(
    builder: &mut FunctionBuilder<'_>,
    lhs: ValueBinding,
    rhs: ValueBinding,
    op: char,
    type_table: &TypeTable,
) -> Result<(Value, Value), String> {
    let lhs_value = if lhs.type_id == TYPE_ID_F32 {
        lhs.value
    } else if is_i32_numeric_type(lhs.type_id, type_table) {
        builder.ins().fcvt_from_sint(types::F32, lhs.value)
    } else {
        return Err(format!(
            "unsupported lhs type {} for '{}' expression",
            lhs.type_id, op
        ));
    };
    let rhs_value = if rhs.type_id == TYPE_ID_F32 {
        rhs.value
    } else if is_i32_numeric_type(rhs.type_id, type_table) {
        builder.ins().fcvt_from_sint(types::F32, rhs.value)
    } else {
        return Err(format!(
            "unsupported rhs type {} for '{}' expression",
            rhs.type_id, op
        ));
    };
    Ok((lhs_value, rhs_value))
}

fn emit_constant_value(
    builder: &mut FunctionBuilder<'_>,
    constant: &ConstantValue,
) -> Result<ValueBinding, String> {
    match constant {
        ConstantValue::I32 { value, type_id } => Ok(ValueBinding {
            value: builder.ins().iconst(types::I32, i64::from(*value)),
            type_id: *type_id,
        }),
        ConstantValue::F32(value) => Ok(ValueBinding {
            value: builder.ins().f32const(Ieee32::with_float(*value)),
            type_id: TYPE_ID_F32,
        }),
        ConstantValue::Bool(value) => Ok(ValueBinding {
            value: builder
                .ins()
                .iconst(types::I32, if *value { 1_i64 } else { 0_i64 }),
            type_id: TYPE_ID_BOOL,
        }),
        ConstantValue::String { value, type_id } => {
            let literal_id = hash_string_literal(value);
            stasis_dynload::upsert_jit_string_literal(literal_id, value);
            Ok(ValueBinding {
                value: builder.ins().iconst(types::I32, i64::from(literal_id)),
                type_id: *type_id,
            })
        }
    }
}

fn resolve_foreach_binding_for_path<'a>(
    path: &str,
    foreach_bindings: &'a ForeachBindingMap,
) -> Option<(&'a ForeachBinding, String)> {
    let mut segments = path.splitn(2, '.');
    let alias = segments.next()?;
    let suffix = segments.next().unwrap_or("").to_string();
    let binding = foreach_bindings.get(alias)?;
    Some((binding, suffix))
}

fn build_local_foreach_collection_info(
    collection_path: &str,
    collection_type: TypeId,
    type_table: &TypeTable,
) -> Result<ForeachCollectionInfo, String> {
    let len = type_table
        .fixed_collection_len(collection_type)
        .ok_or_else(|| {
            format!(
                "local foreach collection '{}' requires fixed-length array type",
                collection_path
            )
        })?;
    let element_type = type_table
        .indexed_element_type_id(collection_type)
        .ok_or_else(|| {
            format!(
                "local foreach collection '{}' has unsupported type {}",
                collection_path, collection_type
            )
        })?;
    Ok(ForeachCollectionInfo {
        len,
        element_type: Some(element_type),
        field_types: BTreeMap::new(),
    })
}

fn emit_foreach_binding_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    binding: &ForeachBinding,
    suffix: &str,
) -> Result<ValueBinding, String> {
    let resolved = resolve_foreach_binding_value_type(binding, suffix)?;
    let field_hash = hash_foreach_field_suffix(suffix);
    let collection_hash = emit_foreach_collection_handle_value(builder, binding.collection_handle);
    let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
    let index_value = builder.use_var(binding.index_var);
    if is_i32_abi_compatible_type(resolved, type_table) {
        let call = builder.ins().call(
            runtime_call_refs.global_i32_array_load,
            &[collection_hash, field_hash_value, index_value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: resolved,
        });
    }
    if resolved == TYPE_ID_F32 {
        let call = builder.ins().call(
            runtime_call_refs.global_f32_array_load,
            &[collection_hash, field_hash_value, index_value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F32,
        });
    }
    Err(format!(
        "unsupported foreach binding load type {} for suffix '{}'",
        resolved, suffix
    ))
}

fn emit_foreach_binding_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    binding: &ForeachBinding,
    suffix: &str,
    op: AssignOp,
    rhs: ValueBinding,
) -> Result<(), String> {
    let path_type = resolve_foreach_binding_value_type(binding, suffix)?;
    if !are_assignment_types_compatible(path_type, rhs.type_id, type_table) {
        return Err(format!(
            "assignment type mismatch for foreach binding '{}': target type {}, expression type {}",
            suffix, path_type, rhs.type_id
        ));
    }
    let field_hash = hash_foreach_field_suffix(suffix);
    let collection_hash = emit_foreach_collection_handle_value(builder, binding.collection_handle);
    let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
    let index_value = builder.use_var(binding.index_var);

    if is_i32_scalar_lane_type(path_type, type_table) {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().iadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().isub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().imul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().sdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().srem(lhs, rhs.value)
            }
        };
        builder.ins().call(
            runtime_call_refs.global_i32_array_store,
            &[collection_hash, field_hash_value, index_value, value],
        );
        return Ok(());
    }
    if path_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool foreach binding '{}' only supports '=' assignment",
                suffix
            ));
        }
        builder.ins().call(
            runtime_call_refs.global_i32_array_store,
            &[collection_hash, field_hash_value, index_value, rhs.value],
        );
        return Ok(());
    }
    if path_type == TYPE_ID_F32 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().fadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().fsub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().fmul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().fdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f32 foreach binding '{}'",
                    suffix
                ))
            }
        };
        builder.ins().call(
            runtime_call_refs.global_f32_array_store,
            &[collection_hash, field_hash_value, index_value, value],
        );
        return Ok(());
    }
    Err(format!(
        "unsupported foreach binding assignment type {} for suffix '{}'",
        path_type, suffix
    ))
}

fn resolve_foreach_binding_value_type(
    binding: &ForeachBinding,
    suffix: &str,
) -> Result<TypeId, String> {
    if suffix.is_empty() {
        if let Some(type_id) = binding.element_type {
            return Ok(type_id);
        }
        return Err("foreach binding requires field access for struct element".to_string());
    }
    binding
        .field_types
        .get(suffix)
        .copied()
        .ok_or_else(|| format!("unknown foreach field path '{}'", suffix))
}

fn hash_foreach_field_suffix(suffix: &str) -> i32 {
    if suffix.is_empty() {
        0
    } else {
        hash_global_path(suffix)
    }
}

fn emit_foreach_collection_handle_value(
    builder: &mut FunctionBuilder<'_>,
    handle: ForeachCollectionHandle,
) -> Value {
    match handle {
        ForeachCollectionHandle::PathHash(hash) => {
            builder.ins().iconst(types::I32, i64::from(hash))
        }
        ForeachCollectionHandle::LocalVar(var) => builder.use_var(var),
    }
}

fn resolve_collection_value_type(
    collection_info: &ForeachCollectionInfo,
    suffix: &str,
) -> Result<TypeId, String> {
    if suffix.is_empty() {
        if let Some(type_id) = collection_info.element_type {
            return Ok(type_id);
        }
        return Err(
            "indexed collection access requires field path for struct elements".to_string(),
        );
    }
    collection_info
        .field_types
        .get(suffix)
        .copied()
        .ok_or_else(|| format!("unknown indexed collection field path '{}'", suffix))
}

fn normalize_index_binding(
    index: ValueBinding,
    type_table: &TypeTable,
) -> Result<ValueBinding, String> {
    if is_i32_abi_compatible_type(index.type_id, type_table) {
        Ok(index)
    } else {
        Err(format!(
            "indexed collection access requires i32 index, found type {}",
            index.type_id
        ))
    }
}

fn resolve_local_collection_value_type(
    collection_type: TypeId,
    suffix: &str,
    type_table: &TypeTable,
) -> Result<TypeId, String> {
    if !suffix.is_empty() {
        return Err(format!(
            "local indexed collection access does not support field path '{}'",
            suffix
        ));
    }
    type_table
        .indexed_element_type_id(collection_type)
        .ok_or_else(|| {
            format!(
                "local indexed collection access is unsupported for type {}",
                collection_type
            )
        })
}

fn emit_local_indexed_collection_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    collection_name: &str,
    collection_binding: LocalBinding,
    suffix: &str,
    index_binding: ValueBinding,
) -> Result<ValueBinding, String> {
    let resolved =
        resolve_local_collection_value_type(collection_binding.type_id, suffix, type_table)?;
    let index_binding = normalize_index_binding(index_binding, type_table)?;
    let collection_handle = builder.use_var(collection_binding.var);
    let field_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)));
    if is_i32_abi_compatible_type(resolved, type_table) {
        let call = builder.ins().call(
            runtime_call_refs.global_i32_array_load,
            &[collection_handle, field_hash, index_binding.value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: resolved,
        });
    }
    if resolved == TYPE_ID_F32 {
        let call = builder.ins().call(
            runtime_call_refs.global_f32_array_load,
            &[collection_handle, field_hash, index_binding.value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F32,
        });
    }
    Err(format!(
        "unsupported local indexed collection load type {} for '{}[...].{}'",
        resolved, collection_name, suffix
    ))
}

fn emit_local_indexed_collection_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    collection_name: &str,
    collection_binding: LocalBinding,
    suffix: &str,
    index_binding: ValueBinding,
    op: AssignOp,
    rhs: ValueBinding,
) -> Result<(), String> {
    let path_type =
        resolve_local_collection_value_type(collection_binding.type_id, suffix, type_table)?;
    let index_binding = normalize_index_binding(index_binding, type_table)?;
    if !are_assignment_types_compatible(path_type, rhs.type_id, type_table) {
        return Err(format!(
            "local indexed assignment type mismatch for '{}[...].{}': target type {}, expression type {}",
            collection_name, suffix, path_type, rhs.type_id
        ));
    }
    let collection_handle = builder.use_var(collection_binding.var);
    let field_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)));

    if is_i32_scalar_lane_type(path_type, type_table) {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_name,
                    collection_binding,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().iadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs = emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_name,
                    collection_binding,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().isub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs = emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_name,
                    collection_binding,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().imul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs = emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_name,
                    collection_binding,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().sdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                let lhs = emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_name,
                    collection_binding,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().srem(lhs, rhs.value)
            }
        };
        builder.ins().call(
            runtime_call_refs.global_i32_array_store,
            &[collection_handle, field_hash, index_binding.value, value],
        );
        return Ok(());
    }
    if path_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool local indexed assignment only supports '=' for '{}[...].{}'",
                collection_name, suffix
            ));
        }
        builder.ins().call(
            runtime_call_refs.global_i32_array_store,
            &[
                collection_handle,
                field_hash,
                index_binding.value,
                rhs.value,
            ],
        );
        return Ok(());
    }
    if path_type == TYPE_ID_F32 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_name,
                    collection_binding,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().fadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs = emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_name,
                    collection_binding,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().fsub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs = emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_name,
                    collection_binding,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().fmul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs = emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_name,
                    collection_binding,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().fdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f32 local indexed assignment '{}[...].{}'",
                    collection_name, suffix
                ));
            }
        };
        builder.ins().call(
            runtime_call_refs.global_f32_array_store,
            &[collection_handle, field_hash, index_binding.value, value],
        );
        return Ok(());
    }
    Err(format!(
        "unsupported local indexed collection assignment type {} for '{}[...].{}'",
        path_type, collection_name, suffix
    ))
}

fn emit_indexed_collection_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    collection_path: &str,
    collection_info: &ForeachCollectionInfo,
    suffix: &str,
    index_binding: ValueBinding,
) -> Result<ValueBinding, String> {
    let resolved = resolve_collection_value_type(collection_info, suffix)?;
    let index_binding = normalize_index_binding(index_binding, type_table)?;
    let collection_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_global_path(collection_path)));
    let field_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)));
    if is_i32_abi_compatible_type(resolved, type_table) {
        let call = builder.ins().call(
            runtime_call_refs.global_i32_array_load,
            &[collection_hash, field_hash, index_binding.value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: resolved,
        });
    }
    if resolved == TYPE_ID_F32 {
        let call = builder.ins().call(
            runtime_call_refs.global_f32_array_load,
            &[collection_hash, field_hash, index_binding.value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F32,
        });
    }
    Err(format!(
        "unsupported indexed collection load type {} for '{}[...].{}'",
        resolved, collection_path, suffix
    ))
}

fn emit_indexed_collection_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    collection_path: &str,
    collection_info: &ForeachCollectionInfo,
    suffix: &str,
    index_binding: ValueBinding,
    op: AssignOp,
    rhs: ValueBinding,
) -> Result<(), String> {
    let path_type = resolve_collection_value_type(collection_info, suffix)?;
    let index_binding = normalize_index_binding(index_binding, type_table)?;
    if !are_assignment_types_compatible(path_type, rhs.type_id, type_table) {
        return Err(format!(
            "indexed assignment type mismatch for '{}[...].{}': target type {}, expression type {}",
            collection_path, suffix, path_type, rhs.type_id
        ));
    }
    let collection_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_global_path(collection_path)));
    let field_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)));

    if is_i32_scalar_lane_type(path_type, type_table) {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().iadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().isub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().imul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().sdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().srem(lhs, rhs.value)
            }
        };
        builder.ins().call(
            runtime_call_refs.global_i32_array_store,
            &[collection_hash, field_hash, index_binding.value, value],
        );
        return Ok(());
    }
    if path_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool indexed assignment only supports '=' for '{}[...].{}'",
                collection_path, suffix
            ));
        }
        builder.ins().call(
            runtime_call_refs.global_i32_array_store,
            &[collection_hash, field_hash, index_binding.value, rhs.value],
        );
        return Ok(());
    }
    if path_type == TYPE_ID_F32 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().fadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().fsub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().fmul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                )?
                .value;
                builder.ins().fdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f32 indexed assignment '{}[...].{}'",
                    collection_path, suffix
                ))
            }
        };
        builder.ins().call(
            runtime_call_refs.global_f32_array_store,
            &[collection_hash, field_hash, index_binding.value, value],
        );
        return Ok(());
    }
    Err(format!(
        "unsupported indexed collection assignment type {} for '{}[...].{}'",
        path_type, collection_path, suffix
    ))
}

fn emit_global_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    path: &str,
    path_type: TypeId,
) -> Result<ValueBinding, String> {
    let path_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_global_path(path)));
    if is_collection_handle_type(path_type, type_table) {
        return Ok(ValueBinding {
            value: path_hash,
            type_id: path_type,
        });
    }
    if is_i32_abi_compatible_type(path_type, type_table) {
        let call = builder
            .ins()
            .call(runtime_call_refs.global_i32_load, &[path_hash]);
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: path_type,
        });
    }
    if path_type == TYPE_ID_F32 {
        let call = builder
            .ins()
            .call(runtime_call_refs.global_f32_load, &[path_hash]);
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F32,
        });
    }
    Err(format!(
        "unsupported global path type {} for '{}'",
        path_type, path
    ))
}

fn emit_global_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    path: &str,
    path_type: TypeId,
    op: AssignOp,
    rhs: ValueBinding,
) -> Result<(), String> {
    if !are_assignment_types_compatible(path_type, rhs.type_id, type_table) {
        return Err(format!(
            "assignment type mismatch for global path '{}': target type {}, expression type {}",
            path, path_type, rhs.type_id
        ));
    }
    if is_collection_handle_type(path_type, type_table) {
        return Err(format!(
            "direct assignment to collection path '{}' is unsupported",
            path
        ));
    }
    if is_i32_scalar_lane_type(path_type, type_table) {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().iadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().isub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().imul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().sdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().srem(lhs, rhs.value)
            }
        };
        let path_hash = builder
            .ins()
            .iconst(types::I32, i64::from(hash_global_path(path)));
        builder
            .ins()
            .call(runtime_call_refs.global_i32_store, &[path_hash, value]);
        return Ok(());
    }
    if path_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool global path '{}' only supports '=' assignment",
                path
            ));
        }
        let path_hash = builder
            .ins()
            .iconst(types::I32, i64::from(hash_global_path(path)));
        builder
            .ins()
            .call(runtime_call_refs.global_i32_store, &[path_hash, rhs.value]);
        return Ok(());
    }
    if path_type == TYPE_ID_F32 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().fadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().fsub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().fmul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().fdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f32 global path '{}'",
                    path
                ))
            }
        };
        let path_hash = builder
            .ins()
            .iconst(types::I32, i64::from(hash_global_path(path)));
        builder
            .ins()
            .call(runtime_call_refs.global_f32_store, &[path_hash, value]);
        return Ok(());
    }
    Err(format!(
        "unsupported global path type {} for '{}'",
        path_type, path
    ))
}

fn hash_global_path(path: &str) -> i32 {
    let mut hash: u32 = 2166136261;
    for byte in path.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16777619);
    }
    hash as i32
}

fn hash_string_literal(value: &str) -> i32 {
    hash_global_path(value)
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
                seed_collection_max_length(path, -8, max_length);
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
                seed_collection_max_length(path, -12, max_length);
            }
            TypeCategory::ArrayFixed => {
                let Some(max_length) = fixed_array_extent_from_type_name(&type_info.name) else {
                    continue;
                };
                seed_collection_max_length(path, -4, max_length);
            }
            _ => {}
        }
    }
    Ok(())
}

fn fixed_array_extent_from_type_name(type_name: &str) -> Option<i32> {
    let (_, extent_text) = parse_array_type_parts(type_name)?;
    if extent_text.is_empty() {
        return None;
    }
    extent_text.parse::<i32>().ok()
}

fn seed_collection_max_length(path: &str, header_start_index: i32, max_length: i32) {
    let collection_hash = hash_global_path(path);
    seed_i32_header_word(collection_hash, 0, header_start_index, max_length);
    let max_length_path = format!("{path}.max_length");
    stasis_dynload::stasis_jit_global_i32_store(hash_global_path(&max_length_path), max_length);
}

fn seed_i32_header_word(collection_hash: i32, field_hash: i32, start_index: i32, value: i32) {
    let bytes = value.to_le_bytes();
    for (offset, byte) in bytes.iter().enumerate() {
        stasis_dynload::stasis_jit_global_i32_array_store(
            collection_hash,
            field_hash,
            start_index + offset as i32,
            i32::from(*byte),
        );
    }
}

fn emit_simple_condition(
    builder: &mut FunctionBuilder<'_>,
    condition: &SimpleCondition,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Value, String> {
    match condition {
        SimpleCondition::Comparison { lhs, op, rhs } => {
            let lhs = emit_simple_expression(
                builder,
                lhs,
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
            )?;
            let rhs = emit_simple_expression(
                builder,
                rhs,
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
            )?;
            if is_i32_abi_compatible_type(lhs.type_id, type_table)
                && is_i32_abi_compatible_type(rhs.type_id, type_table)
            {
                let intcc = match op {
                    ComparisonOp::Eq => IntCC::Equal,
                    ComparisonOp::Ne => IntCC::NotEqual,
                    ComparisonOp::Lt => IntCC::SignedLessThan,
                    ComparisonOp::Le => IntCC::SignedLessThanOrEqual,
                    ComparisonOp::Gt => IntCC::SignedGreaterThan,
                    ComparisonOp::Ge => IntCC::SignedGreaterThanOrEqual,
                };
                return Ok(builder.ins().icmp(intcc, lhs.value, rhs.value));
            }

            let (lhs_f32, rhs_f32) =
                coerce_numeric_operands_to_f32(builder, lhs, rhs, '?', type_table)?;
            let floatcc = match op {
                ComparisonOp::Eq => FloatCC::Equal,
                ComparisonOp::Ne => FloatCC::NotEqual,
                ComparisonOp::Lt => FloatCC::LessThan,
                ComparisonOp::Le => FloatCC::LessThanOrEqual,
                ComparisonOp::Gt => FloatCC::GreaterThan,
                ComparisonOp::Ge => FloatCC::GreaterThanOrEqual,
            };
            Ok(builder.ins().fcmp(floatcc, lhs_f32, rhs_f32))
        }
        SimpleCondition::Expr(expression) => {
            let binding = emit_simple_expression(
                builder,
                expression,
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
            )?;
            if binding.type_id == TYPE_ID_BOOL {
                return Ok(builder.ins().icmp_imm(IntCC::NotEqual, binding.value, 0));
            }
            Err(format!(
                "condition expression must be bool in current jit path; found type {} for expression {:?}",
                binding.type_id, expression
            ))
        }
        SimpleCondition::And(lhs, rhs) => {
            let lhs_value = emit_simple_condition(
                builder,
                lhs,
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
            )?;
            let rhs_block = builder.create_block();
            let false_block = builder.create_block();
            let merge_block = builder.create_block();
            let bool_type = builder.func.dfg.value_type(lhs_value);
            builder.append_block_param(merge_block, bool_type);
            builder
                .ins()
                .brif(lhs_value, rhs_block, &[], false_block, &[]);

            builder.seal_block(rhs_block);
            builder.switch_to_block(rhs_block);
            let rhs_value = emit_simple_condition(
                builder,
                rhs,
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
            )?;
            builder.ins().jump(merge_block, &[rhs_value]);

            builder.seal_block(false_block);
            builder.switch_to_block(false_block);
            builder.ins().jump(merge_block, &[lhs_value]);

            builder.seal_block(merge_block);
            builder.switch_to_block(merge_block);
            Ok(builder.block_params(merge_block)[0])
        }
        SimpleCondition::Or(lhs, rhs) => {
            let lhs_value = emit_simple_condition(
                builder,
                lhs,
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
            )?;
            let true_block = builder.create_block();
            let rhs_block = builder.create_block();
            let merge_block = builder.create_block();
            let bool_type = builder.func.dfg.value_type(lhs_value);
            builder.append_block_param(merge_block, bool_type);
            builder
                .ins()
                .brif(lhs_value, true_block, &[], rhs_block, &[]);

            builder.seal_block(true_block);
            builder.switch_to_block(true_block);
            builder.ins().jump(merge_block, &[lhs_value]);

            builder.seal_block(rhs_block);
            builder.switch_to_block(rhs_block);
            let rhs_value = emit_simple_condition(
                builder,
                rhs,
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
            )?;
            builder.ins().jump(merge_block, &[rhs_value]);

            builder.seal_block(merge_block);
            builder.switch_to_block(merge_block);
            Ok(builder.block_params(merge_block)[0])
        }
        SimpleCondition::Not(inner) => {
            let inner_value = emit_simple_condition(
                builder,
                inner,
                values_by_name,
                runtime_call_refs,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                foreach_bindings,
            )?;
            let true_block = builder.create_block();
            let false_block = builder.create_block();
            let merge_block = builder.create_block();
            let bool_type = builder.func.dfg.value_type(inner_value);
            builder.append_block_param(merge_block, bool_type);
            builder
                .ins()
                .brif(inner_value, true_block, &[], false_block, &[]);

            builder.seal_block(true_block);
            builder.switch_to_block(true_block);
            let false_value = emit_bool_constant(builder, false);
            builder.ins().jump(merge_block, &[false_value]);

            builder.seal_block(false_block);
            builder.switch_to_block(false_block);
            let true_value = emit_bool_constant(builder, true);
            builder.ins().jump(merge_block, &[true_value]);

            builder.seal_block(merge_block);
            builder.switch_to_block(merge_block);
            Ok(builder.block_params(merge_block)[0])
        }
    }
}

fn emit_bool_constant(builder: &mut FunctionBuilder<'_>, value: bool) -> Value {
    let literal = if value { 1 } else { 0 };
    let i32_value = builder.ins().iconst(types::I32, literal);
    builder.ins().icmp_imm(IntCC::NotEqual, i32_value, 0)
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
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
        stasis_dynload::clear_jit_string_literal_table();
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
        stasis_dynload::clear_jit_string_literal_table();
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
    fn jit_process_accepts_utf8_literal_for_ascii_parameter_call() {
        stasis_dynload::clear_jit_string_literal_table();
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
        stasis_dynload::clear_jit_string_literal_table();
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function take_text(value: utf8[]): i32 { return 9; }\nfunction main(): i32 { return take_text(\"café ☕\"); }\n",
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
        stasis_dynload::clear_jit_string_literal_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
    fn jit_process_supports_global_path_from_i32_conversion_target() {
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
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
    fn jit_process_supports_indexed_path_from_i32_conversion_target() {
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global a: ascii[32];\nglobal u: utf8[64];\nfunction main(): i32 {\n    let a_max: i32 = a[-8] + a[-7] * 256 + a[-6] * 65536 + a[-5] * 16777216;\n    let u_max: i32 = u[-12] + u[-11] * 256 + u[-10] * 65536 + u[-9] * 16777216;\n    return a_max + u_max;\n}\n",
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global values: i32[12];\nfunction main(): i32 {\n    let header_max: i32 = values[-4] + values[-3] * 256 + values[-2] * 65536 + values[-1] * 16777216;\n    return header_max + values.max_length;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 24);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_ascii_copy_truncates_to_destination_capacity() {
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();

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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
    fn jit_process_executes_indexed_struct_field_access() {
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
    fn jit_process_executes_indexed_named_field_assignment_from_enum_variant() {
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
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
        stasis_dynload::clear_jit_i32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
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
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_for_loop_with_empty_step_segment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let i: i32 = 0; let sum: i32 = 0; for (; i < 4; ) { sum += i; i += 1; } return sum; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
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
            "function main(): i32 { let i = 2; for (; i; i -= 1) { return 1; } return 0; }\n",
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
        stasis_dynload::clear_jit_f32_array_global_table();
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
