use crate::compiler::{FunctionId, FunctionMeta, SourceFile};
use crate::frontend::parser::{
    parse_top_level_extern_functions, parse_top_level_type_layout, ParsedExternFunctionDeclaration,
    ParsedField,
};
use crate::frontend::types::{
    TypeCategory, TypeId, TypeTable, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_F64, TYPE_ID_I32,
    TYPE_ID_VOID,
};
use crate::ir::hir::FunctionHIR;
use cranelift_codegen::ir::{
    condcodes::{FloatCC, IntCC},
    immediates::{Ieee32, Ieee64},
    types, AbiParam, Block, FuncRef, InstBuilder, MemFlags, Value,
};
use cranelift_frontend::{FunctionBuilder, Variable};
use cranelift_module::{FuncId, Linkage, Module};
use cranelift_object::ObjectModule;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(crate) struct CallSignature {
    pub(crate) function_id: Option<FunctionId>,
    pub(crate) extern_symbol: Option<String>,
    pub(crate) params: Vec<TypeId>,
    pub(crate) return_type: TypeId,
}

#[derive(Debug, Clone)]
pub(crate) struct ExternCallSignature {
    pub(crate) name: String,
    pub(crate) symbol_candidates: Vec<String>,
    pub(crate) params: Vec<TypeId>,
    pub(crate) return_type: TypeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedExternCallSignature {
    pub(crate) name: String,
    pub(crate) symbol: String,
    pub(crate) params: Vec<TypeId>,
    pub(crate) return_type: TypeId,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ExternImportKey {
    pub(crate) symbol: String,
    pub(crate) params: Vec<TypeId>,
    pub(crate) return_type: TypeId,
}

pub(crate) type CallSignatureMap = HashMap<String, Vec<CallSignature>>;
pub(crate) type GlobalPathTypeMap = BTreeMap<String, TypeId>;
pub(crate) type ConstantValueMap = BTreeMap<String, ConstantValue>;
pub(crate) type CollectionInfoMap = BTreeMap<String, ForeachCollectionInfo>;
pub(crate) type NamedStructFieldTypeMap = BTreeMap<TypeId, BTreeMap<String, TypeId>>;
pub(crate) type ForeachBindingMap = BTreeMap<String, ForeachBinding>;
pub(crate) type ExternSymbolAddressMap = BTreeMap<String, usize>;

#[derive(Debug, Clone)]
pub(crate) struct CompileAnalysisCache {
    pub(crate) files_fingerprint: u64,
    pub(crate) call_signatures: CallSignatureMap,
    pub(crate) resolved_extern_signatures: Vec<ResolvedExternCallSignature>,
    pub(crate) global_path_types: GlobalPathTypeMap,
    pub(crate) constant_values: ConstantValueMap,
    pub(crate) collection_infos: CollectionInfoMap,
    pub(crate) named_struct_field_types: NamedStructFieldTypeMap,
    pub(crate) extern_symbol_addresses: ExternSymbolAddressMap,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConstantValue {
    I32 { value: i32, type_id: TypeId },
    F32(f32),
    F64(f64),
    Bool(bool),
    String { value: String, type_id: TypeId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeachCollectionInfo {
    pub(crate) len: i32,
    pub(crate) element_type: Option<TypeId>,
    pub(crate) field_types: BTreeMap<String, TypeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeachBinding {
    pub(crate) collection_handle: ForeachCollectionHandle,
    pub(crate) index_var: Variable,
    pub(crate) len: i32,
    pub(crate) element_type: Option<TypeId>,
    pub(crate) struct_type_id: Option<TypeId>,
    pub(crate) field_types: BTreeMap<String, TypeId>,
    pub(crate) i32_array_base_ptrs: BTreeMap<String, Value>,
    pub(crate) f32_array_base_ptrs: BTreeMap<String, Value>,
    pub(crate) f64_array_base_ptrs: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForeachCollectionHandle {
    PathHash(i32),
    LocalVar(Variable),
}

pub(crate) fn collect_supported_call_signatures(
    functions: &[FunctionMeta],
    extern_signatures: &[ResolvedExternCallSignature],
    type_table: &TypeTable,
) -> CallSignatureMap {
    let mut map: CallSignatureMap = HashMap::new();
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

pub(crate) fn is_supported_call_lane_type(
    type_id: TypeId,
    type_table: &TypeTable,
    allow_void: bool,
) -> bool {
    if allow_void && type_id == TYPE_ID_VOID {
        return true;
    }
    type_id == TYPE_ID_F32
        || type_id == TYPE_ID_F64
        || is_i32_abi_compatible_type(type_id, type_table)
}

pub(crate) fn collect_supported_extern_call_signatures(
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

pub(crate) fn resolve_extern_call_signatures_with(
    extern_signatures: &[ExternCallSignature],
    mut resolve_candidate: impl FnMut(&ExternCallSignature, &str) -> Option<usize>,
) -> Result<(Vec<ResolvedExternCallSignature>, ExternSymbolAddressMap), String> {
    let mut resolved = Vec::with_capacity(extern_signatures.len());
    let mut symbol_addresses: ExternSymbolAddressMap = BTreeMap::new();
    for signature in extern_signatures {
        let mut selected: Option<(String, usize)> = None;
        for candidate in &signature.symbol_candidates {
            if let Some(address) = resolve_candidate(signature, candidate) {
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
            symbol,
            params: signature.params.clone(),
            return_type: signature.return_type,
        });
    }
    Ok((resolved, symbol_addresses))
}

pub(crate) fn is_known_aot_runtime_extern_symbol(symbol: &str) -> bool {
    symbol == "stasis_get_time_ms"
        || symbol == "stasis_get_time_us"
        || matches!(
            symbol,
            "stasis_jit_print_i32"
                | "stasis_jit_print_string"
                | "stasis_jit_load_font"
                | "stasis_jit_measure_text"
                | "stasis_jit_sleep_ms"
                | "stasis_jit_sin_fast"
                | "stasis_jit_cos_fast"
                | "stasis_jit_global_i32_load"
                | "stasis_jit_global_i32_store"
                | "stasis_jit_global_f32_load"
                | "stasis_jit_global_f32_store"
                | "stasis_jit_global_f64_load"
                | "stasis_jit_global_f64_store"
                | "stasis_jit_collection_i32_load"
                | "stasis_jit_collection_i32_store"
                | "stasis_jit_sys_memcpy_u8"
                | "stasis_jit_sys_memcpy_i32"
                | "stasis_jit_sys_memcpy_f32"
                | "stasis_jit_sys_memmove_u8"
                | "stasis_jit_sys_memmove_i32"
                | "stasis_jit_sys_memmove_f32"
        )
        || symbol.starts_with("stasis_jit_gfx_")
        || symbol.starts_with("stasis_jit_audio_")
}

pub(crate) fn resolve_preferred_extern_call_signatures(
    extern_signatures: &[ExternCallSignature],
) -> Result<(Vec<ResolvedExternCallSignature>, ExternSymbolAddressMap), String> {
    resolve_extern_call_signatures_with(extern_signatures, |signature, candidate| {
        if is_known_aot_runtime_extern_symbol(candidate) || signature.symbol_candidates.len() == 1
        {
            Some(0)
        } else {
            None
        }
    })
}

pub(crate) fn build_compile_analysis_cache(
    files: &[SourceFile],
    functions: &[FunctionMeta],
    type_table: &mut TypeTable,
    files_fingerprint: u64,
    extern_resolver: impl FnOnce(
        &[ExternCallSignature],
    ) -> Result<(Vec<ResolvedExternCallSignature>, ExternSymbolAddressMap), String>,
) -> Result<CompileAnalysisCache, String> {
    let extern_signatures = collect_supported_extern_call_signatures(files, type_table)?;
    let (resolved_extern_signatures, extern_symbol_addresses) =
        extern_resolver(&extern_signatures)?;
    build_compile_analysis_cache_from_resolved_externs(
        files,
        functions,
        type_table,
        files_fingerprint,
        resolved_extern_signatures,
        extern_symbol_addresses,
    )
}

pub(crate) fn build_compile_analysis_cache_from_resolved_externs(
    files: &[SourceFile],
    functions: &[FunctionMeta],
    type_table: &mut TypeTable,
    files_fingerprint: u64,
    resolved_extern_signatures: Vec<ResolvedExternCallSignature>,
    extern_symbol_addresses: ExternSymbolAddressMap,
) -> Result<CompileAnalysisCache, String> {
    let call_signatures =
        collect_supported_call_signatures(functions, &resolved_extern_signatures, type_table);
    let constant_values = collect_top_level_constant_values(files, type_table)?;
    let global_path_types = collect_global_path_types(files, type_table, &constant_values)?;
    let collection_infos = collect_foreach_collection_infos(files, type_table, &constant_values)?;
    let named_struct_field_types = collect_named_struct_field_types(files, type_table)?;
    Ok(CompileAnalysisCache {
        files_fingerprint,
        call_signatures,
        resolved_extern_signatures,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        extern_symbol_addresses,
    })
}

pub(crate) fn build_extern_call_signature(
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

pub(crate) fn build_extern_symbol_candidates(
    symbol_name: &str,
    explicit_symbol: bool,
) -> Vec<String> {
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

pub(crate) fn is_i32_abi_compatible_type(type_id: TypeId, type_table: &TypeTable) -> bool {
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

pub(crate) fn is_collection_handle_type(type_id: TypeId, type_table: &TypeTable) -> bool {
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

pub(crate) fn is_i32_scalar_lane_type(type_id: TypeId, type_table: &TypeTable) -> bool {
    type_id != TYPE_ID_BOOL && is_i32_abi_compatible_type(type_id, type_table)
}

pub(crate) fn is_i32_numeric_type(type_id: TypeId, type_table: &TypeTable) -> bool {
    if type_id == TYPE_ID_I32 {
        return true;
    }
    let Some(type_info) = type_table.type_info(type_id) else {
        return false;
    };
    matches!(type_info.category, TypeCategory::Named)
}

pub(crate) fn are_assignment_types_compatible(
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

pub(crate) fn parse_import_paths(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim().trim_start_matches('\u{feff}');
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

pub(crate) fn resolve_import_path(base_file: &str, import_path: &str) -> PathBuf {
    let import = Path::new(import_path);
    if import.is_absolute() {
        return import.to_path_buf();
    }
    let base = Path::new(base_file);
    let parent = base.parent().unwrap_or_else(|| Path::new("."));
    parent.join(import)
}

pub(crate) fn normalize_path_for_compiler_key(path: &Path) -> String {
    match std::fs::canonicalize(path) {
        Ok(canonical) => canonical.to_string_lossy().to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

pub(crate) fn compile_analysis_requires_reemit(
    previous: &CompileAnalysisCache,
    next: &CompileAnalysisCache,
) -> bool {
    previous.resolved_extern_signatures != next.resolved_extern_signatures
        || previous.extern_symbol_addresses != next.extern_symbol_addresses
        || previous.constant_values != next.constant_values
        || previous.global_path_types != next.global_path_types
        || previous.collection_infos != next.collection_infos
        || previous.named_struct_field_types != next.named_struct_field_types
}

pub(crate) fn select_emit_function_ids(
    functions: &[FunctionMeta],
    required_emit_roots: &[String],
    compiled_body_hashes: &HashMap<FunctionId, u64>,
    force_reemit_reachable: bool,
) -> Vec<FunctionId> {
    let reachable = crate::backend::reachability::compute_reachable_function_ids(
        functions,
        required_emit_roots,
    );
    functions
        .iter()
        .filter(|function| reachable.contains(&function.id))
        .filter(|function| {
            if force_reemit_reachable {
                return true;
            }
            let compiled_body_hash = compiled_body_hashes.get(&function.id).copied();
            let artifact_matches_body_hash = compiled_body_hash == Some(function.body_hash);
            function.dirty || !artifact_matches_body_hash
        })
        .map(|function| function.id)
        .collect()
}

pub(crate) fn compute_files_fingerprint(files: &[SourceFile]) -> u64 {
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

pub(crate) fn collect_global_path_types(
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

pub(crate) fn expand_global_type_paths(
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
    let struct_type_id = type_table.resolve_or_intern(trimmed)?;
    out.insert(path.to_string(), struct_type_id);
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

pub(crate) fn resolve_global_path_type_id(
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

pub(crate) fn is_primitive_scalar_type_id(type_id: TypeId) -> bool {
    matches!(
        type_id,
        TYPE_ID_I32 | TYPE_ID_F32 | TYPE_ID_F64 | TYPE_ID_BOOL
    )
}

pub(crate) fn resolve_primitive_scalar_type_id(
    type_name: &str,
    type_table: &TypeTable,
) -> Option<TypeId> {
    let type_id = type_table.resolve(type_name.trim())?;
    if is_primitive_scalar_type_id(type_id) {
        Some(type_id)
    } else {
        None
    }
}

pub(crate) fn collect_top_level_constant_values(
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

pub(crate) fn parse_top_level_constant_literal(
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
    let type_id = type_table.resolve_or_intern(type_name).map_err(|error| {
        format!(
            "invalid type '{}' for constant '{}': {error}",
            type_name, name
        )
    })?;
    if type_id == TYPE_ID_I32 {
        let value = initializer
            .parse::<i32>()
            .map_err(|error| format!("invalid i32 initializer for constant '{}': {error}", name))?;
        return Ok(Some(ConstantValue::I32 { value, type_id }));
    }
    if type_id == TYPE_ID_F32 {
        let value = initializer
            .parse::<f32>()
            .map_err(|error| format!("invalid f32 initializer for constant '{}': {error}", name))?;
        return Ok(Some(ConstantValue::F32(value)));
    }
    if type_id == TYPE_ID_F64 {
        let value = initializer
            .parse::<f64>()
            .map_err(|error| format!("invalid f64 initializer for constant '{}': {error}", name))?;
        return Ok(Some(ConstantValue::F64(value)));
    }
    if type_id == TYPE_ID_BOOL {
        return match initializer {
            "true" => Ok(Some(ConstantValue::Bool(true))),
            "false" => Ok(Some(ConstantValue::Bool(false))),
            other => Err(format!(
                "invalid bool initializer '{}' for constant '{}'",
                other, name
            )),
        };
    }
    let Some(type_info) = type_table.type_info(type_id) else {
        return Ok(None);
    };
    if matches!(
        type_info.category,
        TypeCategory::AsciiView | TypeCategory::Utf8View
    ) {
        let value = parse_constant_string_initializer(name, initializer)?;
        let literal_id = hash_string_literal(&value);
        stasis_dynload::upsert_jit_string_literal(literal_id, &value);
        return Ok(Some(ConstantValue::String { value, type_id }));
    }
    Ok(None)
}

pub(crate) fn parse_constant_string_initializer(
    name: &str,
    initializer: &str,
) -> Result<String, String> {
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

pub(crate) fn collect_foreach_collection_infos(
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

pub(crate) fn collect_named_struct_field_types(
    files: &[SourceFile],
    type_table: &mut TypeTable,
) -> Result<NamedStructFieldTypeMap, String> {
    let mut struct_fields_by_name: BTreeMap<String, Vec<ParsedField>> = BTreeMap::new();
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
    }

    let mut out = NamedStructFieldTypeMap::new();
    for struct_name in struct_fields_by_name.keys() {
        let struct_type_id = type_table.resolve_or_intern(struct_name)?;
        let mut field_types = BTreeMap::new();
        collect_struct_primitive_leaf_fields(
            "",
            struct_name,
            &struct_fields_by_name,
            type_table,
            &mut field_types,
            &mut Vec::new(),
        )?;
        out.insert(struct_type_id, field_types);
    }
    Ok(out)
}

pub(crate) fn collect_foreach_collections_from_type(
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
    if resolve_primitive_scalar_type_id(trimmed, type_table).is_some() {
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

pub(crate) fn build_collection_info_for_element_type(
    element_type_name: &str,
    struct_fields_by_name: &BTreeMap<String, Vec<ParsedField>>,
    type_table: &mut TypeTable,
    visiting_structs: &mut Vec<String>,
) -> Result<ForeachCollectionInfo, String> {
    if let Some(type_id) = resolve_primitive_scalar_type_id(element_type_name, type_table) {
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

pub(crate) fn collect_struct_primitive_leaf_fields(
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
        if let Some(type_id) = resolve_primitive_scalar_type_id(field.type_name.trim(), type_table)
        {
            out.insert(field_path, type_id);
            continue;
        }
        if let Some((element_type_name, extent_text)) = parse_array_type_parts(&field.type_name) {
            if !extent_text.is_empty() {
                continue;
            }
            if resolve_primitive_scalar_type_id(element_type_name, type_table).is_some() {
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

pub(crate) fn parse_array_type_parts(type_name: &str) -> Option<(&str, &str)> {
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

pub(crate) fn resolve_fixed_array_extent(
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

pub(crate) struct RuntimeCallImportIds {
    pub(crate) call_i32_0: FuncId,
    pub(crate) call_i32_1: FuncId,
    pub(crate) call_i32_2: FuncId,
    pub(crate) call_i32_3: FuncId,
    pub(crate) call_i32_4: FuncId,
    pub(crate) call_i32_5: FuncId,
    pub(crate) call_i32_6: FuncId,
    pub(crate) call_i32_7: FuncId,
    pub(crate) call_i32_8: FuncId,
    pub(crate) call_i32_f32_1: FuncId,
    pub(crate) call_i32_f32_2: FuncId,
    pub(crate) call_i32_f32_3: FuncId,
    pub(crate) call_i32_f32_4: FuncId,
    pub(crate) call_i32_f32_5: FuncId,
    pub(crate) call_i32_f32_6: FuncId,
    pub(crate) call_i32_f32_7: FuncId,
    pub(crate) call_i32_f32_8: FuncId,
    pub(crate) call_f32_0: FuncId,
    pub(crate) call_f32_1: FuncId,
    pub(crate) call_f32_2: FuncId,
    pub(crate) call_f32_3: FuncId,
    pub(crate) call_f32_4: FuncId,
    pub(crate) call_f32_5: FuncId,
    pub(crate) call_f32_6: FuncId,
    pub(crate) call_f32_7: FuncId,
    pub(crate) call_f32_8: FuncId,
    pub(crate) call_f32_i32_1: FuncId,
    pub(crate) print_i32: FuncId,
    pub(crate) print_string: FuncId,
    pub(crate) lookup_code_ptr: FuncId,
    pub(crate) sin_fast: FuncId,
    pub(crate) cos_fast: FuncId,
    pub(crate) global_i32_load: FuncId,
    pub(crate) global_i32_store: FuncId,
    pub(crate) global_f32_load: FuncId,
    pub(crate) global_f32_store: FuncId,
    pub(crate) global_f64_load: FuncId,
    pub(crate) global_f64_store: FuncId,
    pub(crate) global_i32_array_load: FuncId,
    pub(crate) global_i32_array_store: FuncId,
    pub(crate) global_i32_array_ptr: FuncId,
    pub(crate) global_f32_array_load: FuncId,
    pub(crate) global_f32_array_store: FuncId,
    pub(crate) global_f32_array_ptr: FuncId,
    pub(crate) global_f64_array_load: FuncId,
    pub(crate) global_f64_array_store: FuncId,
    pub(crate) global_f64_array_ptr: FuncId,
    pub(crate) collection_i32_load: FuncId,
    pub(crate) collection_i32_store: FuncId,
    pub(crate) extern_calls: BTreeMap<ExternImportKey, FuncId>,
}

pub(crate) struct RuntimeCallRefs {
    pub(crate) call_i32_0: FuncRef,
    pub(crate) call_i32_1: FuncRef,
    pub(crate) call_i32_2: FuncRef,
    pub(crate) call_i32_3: FuncRef,
    pub(crate) call_i32_4: FuncRef,
    pub(crate) call_i32_5: FuncRef,
    pub(crate) call_i32_6: FuncRef,
    pub(crate) call_i32_7: FuncRef,
    pub(crate) call_i32_8: FuncRef,
    pub(crate) call_i32_f32_1: FuncRef,
    pub(crate) call_i32_f32_2: FuncRef,
    pub(crate) call_i32_f32_3: FuncRef,
    pub(crate) call_i32_f32_4: FuncRef,
    pub(crate) call_i32_f32_5: FuncRef,
    pub(crate) call_i32_f32_6: FuncRef,
    pub(crate) call_i32_f32_7: FuncRef,
    pub(crate) call_i32_f32_8: FuncRef,
    pub(crate) call_f32_0: FuncRef,
    pub(crate) call_f32_1: FuncRef,
    pub(crate) call_f32_2: FuncRef,
    pub(crate) call_f32_3: FuncRef,
    pub(crate) call_f32_4: FuncRef,
    pub(crate) call_f32_5: FuncRef,
    pub(crate) call_f32_6: FuncRef,
    pub(crate) call_f32_7: FuncRef,
    pub(crate) call_f32_8: FuncRef,
    pub(crate) call_f32_i32_1: FuncRef,
    pub(crate) print_i32: FuncRef,
    pub(crate) print_string: FuncRef,
    pub(crate) lookup_code_ptr: FuncRef,
    pub(crate) sin_fast: FuncRef,
    pub(crate) cos_fast: FuncRef,
    pub(crate) global_i32_load: FuncRef,
    pub(crate) global_i32_store: FuncRef,
    pub(crate) global_f32_load: FuncRef,
    pub(crate) global_f32_store: FuncRef,
    pub(crate) global_f64_load: FuncRef,
    pub(crate) global_f64_store: FuncRef,
    pub(crate) global_i32_array_load: FuncRef,
    pub(crate) global_i32_array_store: FuncRef,
    pub(crate) global_i32_array_ptr: FuncRef,
    pub(crate) global_f32_array_load: FuncRef,
    pub(crate) global_f32_array_store: FuncRef,
    pub(crate) global_f32_array_ptr: FuncRef,
    pub(crate) global_f64_array_load: FuncRef,
    pub(crate) global_f64_array_store: FuncRef,
    pub(crate) global_f64_array_ptr: FuncRef,
    pub(crate) collection_i32_load: FuncRef,
    pub(crate) collection_i32_store: FuncRef,
    pub(crate) extern_calls: BTreeMap<ExternImportKey, FuncRef>,
}

pub(crate) struct AotDirectCallMode<'a> {
    pub(crate) module: &'a mut ObjectModule,
    pub(crate) self_function_id: FunctionId,
    pub(crate) self_clif_func_id: FuncId,
    pub(crate) imported_function_ids: HashMap<FunctionId, FuncId>,
}

pub(crate) enum InternalCallMode<'a> {
    Jit,
    AotDirect(AotDirectCallMode<'a>),
}

fn aot_symbol_name(function_id: FunctionId) -> String {
    format!("aot_fn_{function_id}")
}

fn emit_aot_direct_call_for_signature(
    builder: &mut FunctionBuilder<'_>,
    mode: &mut AotDirectCallMode<'_>,
    signature: &CallSignature,
    arg_values: &[Value],
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> Result<Option<Value>, String> {
    if signature.extern_symbol.is_some() {
        return Err("direct call emission requested for extern signature".to_string());
    }
    let function_id = signature
        .function_id
        .ok_or_else(|| "direct call emission requested for missing function id".to_string())?;

    let callee_func_id = if function_id == mode.self_function_id {
        mode.self_clif_func_id
    } else if let Some(existing) = mode.imported_function_ids.get(&function_id).copied() {
        existing
    } else {
        let symbol = aot_symbol_name(function_id);
        let mut import_signature = mode.module.make_signature();
        for param_type in &signature.params {
            append_abi_params_for_type_id(
                &mut import_signature.params,
                *param_type,
                type_table,
                named_struct_field_types,
            )?;
        }
        if signature.return_type != TYPE_ID_VOID {
            import_signature
                .returns
                .push(AbiParam::new(clif_type_for_type_id(
                    signature.return_type,
                    type_table,
                )?));
        }
        let func_id = mode
            .module
            .declare_function(&symbol, Linkage::Import, &import_signature)
            .map_err(|error| format!("failed to declare AOT import {symbol}: {error}"))?;
        mode.imported_function_ids.insert(function_id, func_id);
        func_id
    };

    let func_ref = mode
        .module
        .declare_func_in_func(callee_func_id, builder.func);
    let call = builder.ins().call(func_ref, arg_values);
    if signature.return_type == TYPE_ID_VOID {
        Ok(None)
    } else {
        let value = builder
            .inst_results(call)
            .first()
            .copied()
            .ok_or_else(|| "direct call expected value result but produced none".to_string())?;
        Ok(Some(value))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ValueBinding {
    pub(crate) value: Value,
    pub(crate) type_id: TypeId,
}

#[derive(Clone, Copy)]
pub(crate) struct StructViewValue {
    pub(crate) type_id: TypeId,
    pub(crate) base: Value,
    pub(crate) index: Value,
    pub(crate) len: Value,
}

#[derive(Clone, Copy)]
pub(crate) struct LocalBinding {
    pub(crate) var: Variable,
    pub(crate) type_id: TypeId,
    pub(crate) struct_view: Option<StructViewBinding>,
}

#[derive(Clone, Copy)]
pub(crate) struct StructViewBinding {
    pub(crate) index_var: Variable,
    pub(crate) len_var: Variable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CollectionMetaKind {
    Length = 1,
    MaxLength = 2,
    CharLength = 3,
}

pub(crate) fn collection_meta_kind_from_suffix(suffix: &str) -> Option<CollectionMetaKind> {
    match suffix {
        "length" => Some(CollectionMetaKind::Length),
        "max_length" => Some(CollectionMetaKind::MaxLength),
        "char_length" => Some(CollectionMetaKind::CharLength),
        // Alias: treat byte_length as length (read-only in source-level semantics for now).
        "byte_length" => Some(CollectionMetaKind::Length),
        _ => None,
    }
}

pub(crate) fn declare_i32_call_import(
    module: &mut impl Module,
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

pub(crate) fn declare_i32_f32_call_import(
    module: &mut impl Module,
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

pub(crate) fn declare_lookup_code_ptr_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_direct_f32_unary_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::F32));
    signature.returns.push(AbiParam::new(types::F32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_f32_call_import(
    module: &mut impl Module,
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

pub(crate) fn declare_f32_i32_call_import(
    module: &mut impl Module,
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

pub(crate) fn declare_void_call_import(
    module: &mut impl Module,
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

pub(crate) fn declare_f32_global_load_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_f32_global_store_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_f64_global_load_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F64));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_f64_global_store_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F64));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_i32_array_load_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_i32_array_store_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_i32_array_ptr_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_f32_array_load_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_f32_array_store_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F32));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_f32_array_ptr_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_f64_array_load_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F64));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_f64_array_store_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F64));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_f64_array_ptr_import(
    module: &mut impl Module,
    symbol: &str,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(symbol, Linkage::Import, &signature)
        .map_err(|error| format!("failed to declare JIT import {symbol}: {error}"))
}

pub(crate) fn declare_extern_call_imports(
    module: &mut impl Module,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
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
                append_abi_params_for_type_id(
                    &mut clif_signature.params,
                    *param,
                    type_table,
                    named_struct_field_types,
                )?;
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

pub(crate) fn declare_new_variable(
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

pub(crate) const STRUCT_VIEW_ABI_WORDS: usize = 3;
pub(crate) const STRUCT_VIEW_AOS_INDEX_SENTINEL: i32 = -1;
pub(crate) const STRUCT_VIEW_AOS_LEN_SENTINEL: i32 = 0;

pub(crate) fn is_struct_view_type(
    type_id: TypeId,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> bool {
    named_struct_field_types.contains_key(&type_id)
}

pub(crate) fn abi_word_count_for_param_type(
    type_id: TypeId,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> usize {
    if is_struct_view_type(type_id, named_struct_field_types) {
        STRUCT_VIEW_ABI_WORDS
    } else {
        1
    }
}

pub(crate) fn abi_word_count_for_params(
    params: &[TypeId],
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> usize {
    params
        .iter()
        .copied()
        .map(|type_id| abi_word_count_for_param_type(type_id, named_struct_field_types))
        .sum()
}

pub(crate) fn append_abi_params_for_type_id(
    params: &mut Vec<AbiParam>,
    type_id: TypeId,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> Result<(), String> {
    if is_struct_view_type(type_id, named_struct_field_types) {
        for _ in 0..STRUCT_VIEW_ABI_WORDS {
            params.push(AbiParam::new(types::I32));
        }
        Ok(())
    } else {
        params.push(AbiParam::new(clif_type_for_type_id(type_id, type_table)?));
        Ok(())
    }
}

pub(crate) fn clif_type_for_type_id(
    type_id: TypeId,
    type_table: &TypeTable,
) -> Result<cranelift_codegen::ir::Type, String> {
    match type_id {
        TYPE_ID_I32 => Ok(types::I32),
        TYPE_ID_F32 => Ok(types::F32),
        TYPE_ID_F64 => Ok(types::F64),
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
pub(crate) enum SimpleStmt {
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
    Continue,
    Return(SimpleExpr),
    ReturnVoid,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LoopControlContext {
    pub(crate) continue_block: Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AssignTarget {
    Local(String),
    GlobalPath(String),
    IndexedPath {
        collection_path: String,
        index: SimpleExpr,
        suffix: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConversionKind {
    FromI32,
    FromF32,
    FromF64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SimpleCondition {
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
pub(crate) enum ComparisonOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

pub(crate) fn extract_function_body(hir: &FunctionHIR) -> Result<&str, String> {
    let Some(block) = hir.blocks.first() else {
        return Err("function body missing block text".to_string());
    };
    Ok(block.source.as_str())
}

pub(crate) fn parse_simple_statements_from_block_with<F>(
    block_text: &str,
    type_table: &mut TypeTable,
    mut visitor: F,
) -> Result<(), String>
where
    F: FnMut(&TypeTable, SimpleStmt) -> Result<(), String>,
{
    let trimmed = block_text.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err("expected function body block enclosed in '{...}'".to_string());
    }
    let inner = &trimmed[1..trimmed.len() - 1];
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
            let statement = parse_let_statement(statement_text, type_table)?;
            visitor(type_table, statement)?;
            cursor = semicolon + 1;
            continue;
        }
        if starts_with_keyword(inner, cursor, "return") {
            let return_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[return_start..semicolon].trim();
            let statement = parse_return_statement(statement_text)?;
            visitor(type_table, statement)?;
            cursor = semicolon + 1;
            continue;
        }
        if starts_with_keyword(inner, cursor, "continue") {
            let continue_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[continue_start..semicolon].trim();
            let statement = parse_continue_statement(statement_text)?;
            visitor(type_table, statement)?;
            cursor = semicolon + 1;
            continue;
        }
        if starts_with_keyword(inner, cursor, "for") {
            let (statement, next_cursor) = parse_for_statement(inner, cursor, type_table)?;
            visitor(type_table, statement)?;
            cursor = next_cursor;
            continue;
        }
        if starts_with_keyword(inner, cursor, "foreach") {
            let (statement, next_cursor) = parse_foreach_statement(inner, cursor, type_table)?;
            visitor(type_table, statement)?;
            cursor = next_cursor;
            continue;
        }
        if starts_with_keyword(inner, cursor, "if") {
            let (statement, next_cursor) = parse_if_statement(inner, cursor, type_table)?;
            visitor(type_table, statement)?;
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
            let statement = parse_from_conversion_statement(statement_text)?;
            visitor(type_table, statement)?;
            cursor = semicolon + 1;
            continue;
        }
        if looks_like_assignment(inner, cursor) {
            let assignment_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[assignment_start..semicolon].trim();
            let statement = parse_assignment_statement(statement_text)?;
            visitor(type_table, statement)?;
            cursor = semicolon + 1;
            continue;
        }
        if looks_like_call_statement(inner, cursor) {
            let call_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[call_start..semicolon].trim();
            let statement = parse_call_statement(statement_text)?;
            visitor(type_table, statement)?;
            cursor = semicolon + 1;
            continue;
        }
        return Err(format!(
            "unsupported statement in function body near '{}'",
            snippet_from(inner, cursor)
        ));
    }
    Ok(())
}

pub(crate) fn parse_simple_statements_from_block(
    block_text: &str,
    type_table: &mut TypeTable,
) -> Result<Vec<SimpleStmt>, String> {
    let mut statements = Vec::new();
    parse_simple_statements_from_block_with(block_text, type_table, |_type_table, statement| {
        statements.push(statement);
        Ok(())
    })?;
    Ok(statements)
}

pub(crate) fn parse_let_statement(
    statement_text: &str,
    type_table: &mut TypeTable,
) -> Result<SimpleStmt, String> {
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
            let (type_name, initializer) =
                split_type_annotation_and_initializer(after_let, cursor)?;
            let resolved_type_id = type_table.resolve_or_intern(type_name).map_err(|_| {
                format!(
                    "unsupported let type '{}' in statement '{}'",
                    type_name, statement_text
                )
            })?;
            let expression = if let Some(expression_text) = initializer {
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

pub(crate) fn split_type_annotation_and_initializer<'a>(
    source: &'a str,
    type_start: usize,
) -> Result<(&'a str, Option<&'a str>), String> {
    if type_start >= source.len() {
        return Err("missing type annotation in let statement".to_string());
    }
    let bytes = source.as_bytes();
    let mut cursor = type_start;
    while cursor < bytes.len() {
        if bytes[cursor] == b'=' {
            let type_name = source[type_start..cursor].trim();
            if type_name.is_empty() {
                return Err("missing type annotation in let statement".to_string());
            }
            let initializer = source[cursor + 1..].trim();
            if initializer.is_empty() {
                return Err("missing expression in let statement".to_string());
            }
            return Ok((type_name, Some(initializer)));
        }
        cursor += 1;
    }
    let type_name = source[type_start..].trim();
    if type_name.is_empty() {
        return Err("missing type annotation in let statement".to_string());
    }
    Ok((type_name, None))
}

pub(crate) fn parse_assignment_statement(statement_text: &str) -> Result<SimpleStmt, String> {
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

pub(crate) fn parse_assignment_target(
    source: &str,
    cursor: usize,
) -> Result<(AssignTarget, usize), String> {
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
            if let Some(const_i64) = eval_const_i64(index_expr.as_ref().expect("index expr set")) {
                if const_i64 < 0 {
                    return Err(
                        "negative collection indices are unsupported (use .length/.max_length)"
                            .to_string(),
                    );
                }
            }
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

pub(crate) fn parse_from_conversion_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    let trimmed = statement_text.trim();
    let marker_i32 = ".from_i32(";
    let marker_f32 = ".from_f32(";
    let marker_f64 = ".from_f64(";
    let (marker_pos, marker, kind) = if let Some(pos) = trimmed.find(marker_i32) {
        (pos, marker_i32, ConversionKind::FromI32)
    } else if let Some(pos) = trimmed.find(marker_f32) {
        (pos, marker_f32, ConversionKind::FromF32)
    } else if let Some(pos) = trimmed.find(marker_f64) {
        (pos, marker_f64, ConversionKind::FromF64)
    } else {
        return Err(format!(
            "unsupported conversion statement '{}': expected from_i32, from_f32, or from_f64",
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

pub(crate) fn parse_call_statement(statement_text: &str) -> Result<SimpleStmt, String> {
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

pub(crate) fn parse_return_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    let after_return = statement_text
        .strip_prefix("return")
        .ok_or_else(|| format!("invalid return statement '{statement_text}'"))?;
    let expression_text = after_return.trim();
    if expression_text.is_empty() {
        return Ok(SimpleStmt::ReturnVoid);
    }
    Ok(SimpleStmt::Return(parse_value_expression(expression_text)?))
}

pub(crate) fn parse_continue_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    if statement_text.trim() != "continue" {
        return Err(format!(
            "invalid continue statement '{}': expected bare continue",
            statement_text
        ));
    }
    Ok(SimpleStmt::Continue)
}

pub(crate) fn parse_for_statement(
    source: &str,
    start: usize,
    type_table: &mut TypeTable,
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

pub(crate) fn parse_for_control_segment(
    segment_text: &str,
    type_table: &mut TypeTable,
) -> Result<SimpleStmt, String> {
    let trimmed = segment_text.trim();
    if trimmed.is_empty() {
        return Ok(SimpleStmt::Noop);
    }
    if starts_with_keyword(trimmed, 0, "let") {
        return parse_let_statement(trimmed, type_table);
    }
    if trimmed.contains(".from_i32(")
        || trimmed.contains(".from_f32(")
        || trimmed.contains(".from_f64(")
    {
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

pub(crate) fn parse_foreach_statement(
    source: &str,
    start: usize,
    type_table: &mut TypeTable,
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

pub(crate) fn parse_if_statement(
    source: &str,
    start: usize,
    type_table: &mut TypeTable,
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

pub(crate) fn parse_simple_condition(condition_text: &str) -> Result<SimpleCondition, String> {
    parse_or_condition(condition_text.trim())
}

pub(crate) fn parse_or_condition(condition_text: &str) -> Result<SimpleCondition, String> {
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

pub(crate) fn parse_and_condition(condition_text: &str) -> Result<SimpleCondition, String> {
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

pub(crate) fn parse_not_condition(condition_text: &str) -> Result<SimpleCondition, String> {
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

pub(crate) fn parse_condition_atom(condition_text: &str) -> Result<SimpleCondition, String> {
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

pub(crate) fn split_top_level_condition<'a>(condition_text: &'a str, op: &[u8; 2]) -> Vec<&'a str> {
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

pub(crate) fn find_condition_operator(
    condition_text: &str,
) -> Option<(ComparisonOp, usize, usize)> {
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

pub(crate) fn skip_ascii_whitespace(source: &str, mut cursor: usize) -> usize {
    while cursor < source.len() && source.as_bytes()[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

pub(crate) fn skip_ascii_whitespace_and_comments(source: &str, mut cursor: usize) -> usize {
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

pub(crate) fn starts_with_keyword(source: &str, cursor: usize, keyword: &str) -> bool {
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

pub(crate) fn looks_like_assignment(source: &str, cursor: usize) -> bool {
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

pub(crate) fn looks_like_from_conversion_statement(source: &str, cursor: usize) -> bool {
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
    method_tail.starts_with(".from_i32(")
        || method_tail.starts_with(".from_f32(")
        || method_tail.starts_with(".from_f64(")
}

pub(crate) fn looks_like_call_statement(source: &str, cursor: usize) -> bool {
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

pub(crate) fn split_for_header(header: &str) -> Result<[String; 3], String> {
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

pub(crate) fn find_statement_terminator(source: &str, start: usize) -> Result<usize, String> {
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

pub(crate) fn find_matching_delimiter(
    source: &str,
    open_index: usize,
    open: u8,
    close: u8,
) -> Option<usize> {
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

pub(crate) fn expect_byte(
    source: &str,
    cursor: usize,
    expected: u8,
    context: &str,
) -> Result<usize, String> {
    if cursor >= source.len() || source.as_bytes()[cursor] != expected {
        return Err(format!(
            "expected {} near '{}'",
            context,
            snippet_from(source, cursor)
        ));
    }
    Ok(cursor + 1)
}

pub(crate) fn parse_identifier(source: &str, cursor: usize) -> Result<(&str, usize), String> {
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

pub(crate) fn parse_identifier_path(
    source: &str,
    cursor: usize,
) -> Result<(String, usize), String> {
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

pub(crate) fn assign_target_from_path(path: String) -> AssignTarget {
    if path.contains('.') {
        AssignTarget::GlobalPath(path)
    } else {
        AssignTarget::Local(path)
    }
}

pub(crate) fn snippet_from(source: &str, cursor: usize) -> String {
    source
        .get(cursor..)
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect()
}

pub(crate) fn emit_host_print_call_statement(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    type_table: &TypeTable,
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    call_signatures: &CallSignatureMap,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
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
        None,
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
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

pub(crate) fn emit_indirect_call_for_signature(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    signature: &CallSignature,
    arg_values: &[Value],
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
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
        append_abi_params_for_type_id(
            &mut indirect_signature.params,
            *param_type,
            type_table,
            named_struct_field_types,
        )?;
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

pub(crate) fn emit_extern_call_for_signature(
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

pub(crate) fn ensure_no_variable_shadowing(
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

pub(crate) fn try_emit_indexed_struct_copy_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    type_table: &TypeTable,
    target: &AssignTarget,
    op: AssignOp,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, LocalBinding>,
    call_signatures: &CallSignatureMap,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<bool, String> {
    let AssignTarget::IndexedPath {
        collection_path: target_collection,
        index: target_index,
        suffix: target_suffix,
    } = target
    else {
        return Ok(false);
    };
    if !target_suffix.is_empty() {
        return Ok(false);
    }
    let SimpleExpr::IndexedPath {
        collection_path: source_collection,
        index: source_index,
        suffix: source_suffix,
    } = expression
    else {
        return Ok(false);
    };
    if !source_suffix.is_empty() {
        return Ok(false);
    }

    if op != AssignOp::Set {
        return Err(format!(
            "struct indexed copy assignment only supports '=' for '{}[...]'",
            target_collection
        ));
    }

    let target_index_binding = emit_simple_expression(
        builder,
        target_index,
        None,
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    let source_index_binding = emit_simple_expression(
        builder,
        source_index,
        None,
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;

    let local_target = values_by_name.get(target_collection).copied();
    let local_source = values_by_name.get(source_collection).copied();
    if let (Some(target_local), Some(source_local)) = (local_target, local_source) {
        let Some(target_element_type) = type_table.indexed_element_type_id(target_local.type_id)
        else {
            return Ok(false);
        };
        let Some(source_element_type) = type_table.indexed_element_type_id(source_local.type_id)
        else {
            return Ok(false);
        };
        let Some(target_fields) = named_struct_field_types.get(&target_element_type) else {
            return Ok(false);
        };
        let Some(source_fields) = named_struct_field_types.get(&source_element_type) else {
            return Ok(false);
        };
        if target_fields != source_fields {
            return Err(format!(
                "struct indexed copy assignment requires matching field layout for '{}[...]' and '{}[...]'",
                target_collection, source_collection
            ));
        }
        for field_name in target_fields.keys() {
            let source_value = emit_local_indexed_collection_load(
                builder,
                runtime_call_refs,
                type_table,
                named_struct_field_types,
                source_collection,
                source_local,
                field_name,
                source_index_binding,
            )?;
            emit_local_indexed_collection_assignment(
                builder,
                runtime_call_refs,
                type_table,
                named_struct_field_types,
                target_collection,
                target_local,
                field_name,
                target_index_binding,
                AssignOp::Set,
                source_value,
            )?;
        }
        return Ok(true);
    }

    if local_target.is_some() || local_source.is_some() {
        return Ok(false);
    }

    let Some(target_info) = collection_infos.get(target_collection) else {
        return Ok(false);
    };
    let Some(source_info) = collection_infos.get(source_collection) else {
        return Ok(false);
    };
    if target_info.field_types.is_empty() || source_info.field_types.is_empty() {
        return Ok(false);
    }
    if target_info.field_types != source_info.field_types {
        return Err(format!(
            "struct indexed copy assignment requires matching field layout for '{}[...]' and '{}[...]'",
            target_collection, source_collection
        ));
    }

    for field_name in target_info.field_types.keys() {
        let source_value = emit_indexed_collection_load(
            builder,
            runtime_call_refs,
            type_table,
            source_collection,
            source_info,
            field_name,
            source_index_binding,
        )?;
        emit_indexed_collection_assignment(
            builder,
            runtime_call_refs,
            type_table,
            target_collection,
            target_info,
            field_name,
            target_index_binding,
            AssignOp::Set,
            source_value,
        )?;
    }
    Ok(true)
}

pub(crate) fn try_emit_global_struct_copy_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    target: &AssignTarget,
    op: AssignOp,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, LocalBinding>,
    global_path_types: &GlobalPathTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<bool, String> {
    let target_path = match target {
        AssignTarget::GlobalPath(path) => path.as_str(),
        AssignTarget::Local(path) => {
            if values_by_name.contains_key(path) || foreach_bindings.contains_key(path) {
                return Ok(false);
            }
            path.as_str()
        }
        AssignTarget::IndexedPath { .. } => return Ok(false),
    };
    let SimpleExpr::Identifier(source_path) = expression else {
        return Ok(false);
    };
    if values_by_name.contains_key(source_path) || foreach_bindings.contains_key(source_path) {
        return Ok(false);
    }
    if let Some(type_id) = global_path_types.get(target_path).copied() {
        let is_named_struct = type_table
            .type_info(type_id)
            .is_some_and(|info| info.category == TypeCategory::Named);
        if !is_named_struct {
            return Ok(false);
        }
    }
    if let Some(type_id) = global_path_types.get(source_path).copied() {
        let is_named_struct = type_table
            .type_info(type_id)
            .is_some_and(|info| info.category == TypeCategory::Named);
        if !is_named_struct {
            return Ok(false);
        }
    }

    let target_prefix = format!("{target_path}.");
    let source_prefix = format!("{source_path}.");
    let mut fields: Vec<(String, TypeId)> = global_path_types
        .iter()
        .filter_map(|(path, type_id)| {
            path.strip_prefix(&target_prefix)
                .map(|suffix| (suffix.to_string(), *type_id))
        })
        .collect();
    if fields.is_empty() {
        return Ok(false);
    }
    if op != AssignOp::Set {
        return Err(format!(
            "struct path copy assignment only supports '=' for '{}'",
            target_path
        ));
    }
    fields.sort_by(|left, right| left.0.cmp(&right.0));

    for (suffix, target_type) in &fields {
        if is_collection_handle_type(*target_type, type_table) {
            return Err(format!(
                "struct path copy assignment currently supports scalar fields only for '{}'",
                target_path
            ));
        }
        let source_field = format!("{source_prefix}{suffix}");
        let Some(source_type) = global_path_types.get(&source_field).copied() else {
            return Err(format!(
                "struct path copy assignment requires matching field path '{}'",
                source_field
            ));
        };
        if source_type != *target_type {
            return Err(format!(
                "struct path copy assignment type mismatch at field '{}': target {} source {}",
                suffix, target_type, source_type
            ));
        }
    }

    for (suffix, field_type) in fields {
        let source_field = format!("{source_prefix}{suffix}");
        let target_field = format!("{target_prefix}{suffix}");
        let source_value = emit_global_load(
            builder,
            runtime_call_refs,
            type_table,
            &source_field,
            field_type,
        )?;
        emit_global_assignment(
            builder,
            runtime_call_refs,
            type_table,
            &target_field,
            field_type,
            AssignOp::Set,
            source_value,
        )?;
    }
    Ok(true)
}

pub(crate) fn try_emit_struct_copy_from_indexed_to_global(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    type_table: &TypeTable,
    target: &AssignTarget,
    op: AssignOp,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, LocalBinding>,
    call_signatures: &CallSignatureMap,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<bool, String> {
    let target_path = match target {
        AssignTarget::GlobalPath(path) => path.as_str(),
        AssignTarget::Local(path) => {
            if values_by_name.contains_key(path) || foreach_bindings.contains_key(path) {
                return Ok(false);
            }
            path.as_str()
        }
        AssignTarget::IndexedPath { .. } => return Ok(false),
    };
    let SimpleExpr::IndexedPath {
        collection_path: source_collection,
        index: source_index,
        suffix: source_suffix,
    } = expression
    else {
        return Ok(false);
    };
    if !source_suffix.is_empty() {
        return Ok(false);
    }
    if values_by_name.contains_key(source_collection)
        || foreach_bindings.contains_key(source_collection)
    {
        return Ok(false);
    }
    let Some(source_info) = collection_infos.get(source_collection) else {
        return Ok(false);
    };
    if source_info.field_types.is_empty() {
        return Ok(false);
    }
    if op != AssignOp::Set {
        return Err(format!(
            "struct copy assignment from indexed source only supports '=' for '{}'",
            target_path
        ));
    }
    let target_prefix = format!("{target_path}.");
    let target_fields: BTreeMap<String, TypeId> = global_path_types
        .iter()
        .filter_map(|(path, type_id)| {
            path.strip_prefix(&target_prefix)
                .map(|suffix| (suffix.to_string(), *type_id))
        })
        .collect();
    if target_fields.is_empty() {
        return Ok(false);
    }
    if target_fields != source_info.field_types {
        return Err(format!(
            "struct copy assignment from indexed source requires matching field layout for '{}' and '{}[...]'",
            target_path, source_collection
        ));
    }
    for type_id in target_fields.values() {
        if is_collection_handle_type(*type_id, type_table) {
            return Err(format!(
                "struct copy assignment from indexed source currently supports scalar fields only for '{}'",
                target_path
            ));
        }
    }

    let source_index_binding = emit_simple_expression(
        builder,
        source_index,
        None,
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    for (field_name, field_type) in &source_info.field_types {
        let source_value = emit_indexed_collection_load(
            builder,
            runtime_call_refs,
            type_table,
            source_collection,
            source_info,
            field_name,
            source_index_binding,
        )?;
        let target_field = format!("{target_prefix}{field_name}");
        emit_global_assignment(
            builder,
            runtime_call_refs,
            type_table,
            &target_field,
            *field_type,
            AssignOp::Set,
            source_value,
        )?;
    }
    Ok(true)
}

pub(crate) fn try_emit_struct_copy_from_global_to_indexed(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    type_table: &TypeTable,
    target: &AssignTarget,
    op: AssignOp,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, LocalBinding>,
    call_signatures: &CallSignatureMap,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<bool, String> {
    let AssignTarget::IndexedPath {
        collection_path: target_collection,
        index: target_index,
        suffix: target_suffix,
    } = target
    else {
        return Ok(false);
    };
    if !target_suffix.is_empty() {
        return Ok(false);
    }
    if values_by_name.contains_key(target_collection)
        || foreach_bindings.contains_key(target_collection)
    {
        return Ok(false);
    }
    let SimpleExpr::Identifier(source_path) = expression else {
        return Ok(false);
    };
    if values_by_name.contains_key(source_path) || foreach_bindings.contains_key(source_path) {
        return Ok(false);
    }
    let Some(target_info) = collection_infos.get(target_collection) else {
        return Ok(false);
    };
    if target_info.field_types.is_empty() {
        return Ok(false);
    }
    if op != AssignOp::Set {
        return Err(format!(
            "struct copy assignment to indexed target only supports '=' for '{}[...]'",
            target_collection
        ));
    }
    let source_prefix = format!("{source_path}.");
    let source_fields: BTreeMap<String, TypeId> = global_path_types
        .iter()
        .filter_map(|(path, type_id)| {
            path.strip_prefix(&source_prefix)
                .map(|suffix| (suffix.to_string(), *type_id))
        })
        .collect();
    if source_fields.is_empty() {
        return Ok(false);
    }
    if source_fields != target_info.field_types {
        return Err(format!(
            "struct copy assignment to indexed target requires matching field layout for '{}[...]' and '{}'",
            target_collection, source_path
        ));
    }
    for type_id in source_fields.values() {
        if is_collection_handle_type(*type_id, type_table) {
            return Err(format!(
                "struct copy assignment to indexed target currently supports scalar fields only for '{}[...]'",
                target_collection
            ));
        }
    }

    let target_index_binding = emit_simple_expression(
        builder,
        target_index,
        None,
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    for (field_name, field_type) in &target_info.field_types {
        let source_field = format!("{source_prefix}{field_name}");
        let source_value = emit_global_load(
            builder,
            runtime_call_refs,
            type_table,
            &source_field,
            *field_type,
        )?;
        emit_indexed_collection_assignment(
            builder,
            runtime_call_refs,
            type_table,
            target_collection,
            target_info,
            field_name,
            target_index_binding,
            AssignOp::Set,
            source_value,
        )?;
    }
    Ok(true)
}

pub(crate) fn emit_simple_statements(
    builder: &mut FunctionBuilder<'_>,
    statements: &[SimpleStmt],
    values_by_name: &mut BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
    loop_control: Option<&LoopControlContext>,
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

                if let Some(struct_view) = try_emit_struct_view_value(
                    builder,
                    expression,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )? {
                    let local_type_id = type_id.unwrap_or(struct_view.type_id);
                    if local_type_id != struct_view.type_id {
                        return Err(format!(
                            "let binding '{}' expected type {} view expression but found {}",
                            name, local_type_id, struct_view.type_id
                        ));
                    }
                    let variable = declare_new_variable(
                        builder,
                        next_variable,
                        struct_view.base,
                        local_type_id,
                        type_table,
                    )?;
                    let index_var = declare_new_variable(
                        builder,
                        next_variable,
                        struct_view.index,
                        TYPE_ID_I32,
                        type_table,
                    )?;
                    let len_var = declare_new_variable(
                        builder,
                        next_variable,
                        struct_view.len,
                        TYPE_ID_I32,
                        type_table,
                    )?;
                    values_by_name.insert(
                        name.clone(),
                        LocalBinding {
                            var: variable,
                            type_id: local_type_id,
                            struct_view: Some(StructViewBinding { index_var, len_var }),
                        },
                    );
                    continue;
                } else if type_id
                    .is_some_and(|type_id| is_struct_view_type(type_id, named_struct_field_types))
                {
                    return Err(format!(
                        "let binding '{}' requires view initializer for struct type {}",
                        name,
                        type_id.unwrap_or(TYPE_ID_VOID)
                    ));
                }

                let binding = emit_simple_expression(
                    builder,
                    expression,
                    *type_id,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
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
                        struct_view: None,
                    },
                );
            }
            SimpleStmt::Assign {
                target,
                op,
                expression,
            } => {
                if try_emit_indexed_struct_copy_assignment(
                    builder,
                    runtime_call_refs,
                    internal_calls,
                    type_table,
                    target,
                    *op,
                    expression,
                    values_by_name,
                    call_signatures,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )? {
                    continue;
                }
                if try_emit_global_struct_copy_assignment(
                    builder,
                    runtime_call_refs,
                    type_table,
                    target,
                    *op,
                    expression,
                    values_by_name,
                    global_path_types,
                    foreach_bindings,
                )? {
                    continue;
                }
                if try_emit_struct_copy_from_indexed_to_global(
                    builder,
                    runtime_call_refs,
                    internal_calls,
                    type_table,
                    target,
                    *op,
                    expression,
                    values_by_name,
                    call_signatures,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )? {
                    continue;
                }
                if try_emit_struct_copy_from_global_to_indexed(
                    builder,
                    runtime_call_refs,
                    internal_calls,
                    type_table,
                    target,
                    *op,
                    expression,
                    values_by_name,
                    call_signatures,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )? {
                    continue;
                }
                let expected_rhs_type = match target {
                    AssignTarget::Local(name) => values_by_name
                        .get(name)
                        .map(|binding| binding.type_id)
                        .or_else(|| global_path_types.get(name).copied()),
                    AssignTarget::GlobalPath(path) => {
                        global_path_types.get(path).copied().or_else(|| {
                            let (base, suffix) = path.split_once('.')?;
                            let local = values_by_name.get(base)?;
                            let field_types = named_struct_field_types.get(&local.type_id)?;
                            field_types.get(suffix).copied()
                        })
                    }
                    AssignTarget::IndexedPath {
                        collection_path,
                        suffix,
                        ..
                    } => {
                        if let Some(local_collection) = values_by_name.get(collection_path).copied()
                        {
                            Some(resolve_local_collection_value_type(
                                local_collection.type_id,
                                suffix,
                                type_table,
                                named_struct_field_types,
                            )?)
                        } else if let Some(collection_info) = collection_infos.get(collection_path)
                        {
                            Some(resolve_collection_value_type(collection_info, suffix)?)
                        } else {
                            None
                        }
                    }
                };
                let rhs = emit_simple_expression(
                    builder,
                    expression,
                    expected_rhs_type,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                match target {
                    AssignTarget::Local(name) => {
                        if let Some(local) = values_by_name.get(name).copied() {
                            if is_struct_view_type(local.type_id, named_struct_field_types) {
                                return Err(format!(
                                    "assignment target '{}' is a view binding and is not rebindable in current jit path",
                                    name
                                ));
                            }
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
                            let value = if is_collection_handle_type(local.type_id, type_table) {
                                if *op != AssignOp::Set {
                                    return Err(format!(
                                        "collection handle assignment only supports '=' in current jit path for '{}'",
                                        name
                                    ));
                                }
                                rhs.value
                            } else if is_i32_scalar_lane_type(local.type_id, type_table) {
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
                            } else if local.type_id == TYPE_ID_F64 {
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
                        if let Some((base, suffix)) = path.split_once('.') {
                            if let Some(local) = values_by_name.get(base).copied() {
                                if let Some(kind) = collection_meta_kind_from_suffix(suffix) {
                                    if suffix == "max_length" {
                                        return Err(format!(
                                            "assignment target '{}.{}' is read-only in current jit path",
                                            base, suffix
                                        ));
                                    }
                                    if suffix == "byte_length" {
                                        return Err(format!(
                                            "assignment target '{}.{}' is read-only (assign to '{}.length') in current jit path",
                                            base, suffix, base
                                        ));
                                    }
                                    if !is_collection_handle_type(local.type_id, type_table) {
                                        return Err(format!(
                                            "assignment target '{}.{}' requires collection handle base in current jit path",
                                            base, suffix
                                        ));
                                    }
                                    if rhs.type_id != TYPE_ID_I32 {
                                        return Err(format!(
                                            "assignment type mismatch for '{}.{}': expected i32 expression but found {}",
                                            base, suffix, rhs.type_id
                                        ));
                                    }

                                    let base_value = builder.use_var(local.var);
                                    let kind_value =
                                        builder.ins().iconst(types::I32, i64::from(kind as i32));

                                    let value = match op {
                                        AssignOp::Set => rhs.value,
                                        AssignOp::Add
                                        | AssignOp::Sub
                                        | AssignOp::Mul
                                        | AssignOp::Div
                                        | AssignOp::Mod => {
                                            let current = builder.ins().call(
                                                runtime_call_refs.collection_i32_load,
                                                &[base_value, kind_value],
                                            );
                                            let current_value = builder.inst_results(current)[0];
                                            match op {
                                                AssignOp::Add => {
                                                    builder.ins().iadd(current_value, rhs.value)
                                                }
                                                AssignOp::Sub => {
                                                    builder.ins().isub(current_value, rhs.value)
                                                }
                                                AssignOp::Mul => {
                                                    builder.ins().imul(current_value, rhs.value)
                                                }
                                                AssignOp::Div => {
                                                    builder.ins().sdiv(current_value, rhs.value)
                                                }
                                                AssignOp::Mod => {
                                                    builder.ins().srem(current_value, rhs.value)
                                                }
                                                AssignOp::Set => unreachable!(),
                                            }
                                        }
                                    };

                                    builder.ins().call(
                                        runtime_call_refs.collection_i32_store,
                                        &[base_value, kind_value, value],
                                    );
                                    continue;
                                }
                                if let Some(field_types) =
                                    named_struct_field_types.get(&local.type_id)
                                {
                                    let Some(field_type) = field_types.get(suffix).copied() else {
                                        return Err(format!(
                                            "unknown local struct field path '{}.{}' in current jit path",
                                            base, suffix
                                        ));
                                    };
                                    if is_collection_handle_type(field_type, type_table) {
                                        return Err(format!(
                                            "local struct field assignment to collection handle '{}.{}' is unsupported in current jit path",
                                            base, suffix
                                        ));
                                    }
                                    if !are_assignment_types_compatible(
                                        field_type,
                                        rhs.type_id,
                                        type_table,
                                    ) {
                                        return Err(format!(
                                            "assignment type mismatch for local struct field '{}.{}': target type {}, expression type {}",
                                            base, suffix, field_type, rhs.type_id
                                        ));
                                    }

                                    let base_hash = builder.use_var(local.var);
                                    if let Some(struct_view) = local.struct_view {
                                        emit_struct_view_field_assignment(
                                            builder,
                                            runtime_call_refs,
                                            type_table,
                                            struct_view,
                                            base_hash,
                                            suffix,
                                            field_type,
                                            *op,
                                            rhs,
                                        )?;
                                        continue;
                                    }

                                    let path_hash = emit_local_struct_field_path_hash(
                                        base_hash, suffix, builder,
                                    );
                                    if is_i32_scalar_lane_type(field_type, type_table) {
                                        let value = match op {
                                            AssignOp::Set => rhs.value,
                                            AssignOp::Add => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_i32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().iadd(lhs, rhs.value)
                                            }
                                            AssignOp::Sub => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_i32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().isub(lhs, rhs.value)
                                            }
                                            AssignOp::Mul => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_i32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().imul(lhs, rhs.value)
                                            }
                                            AssignOp::Div => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_i32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().sdiv(lhs, rhs.value)
                                            }
                                            AssignOp::Mod => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_i32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().srem(lhs, rhs.value)
                                            }
                                        };
                                        builder.ins().call(
                                            runtime_call_refs.global_i32_store,
                                            &[path_hash, value],
                                        );
                                        continue;
                                    }
                                    if field_type == TYPE_ID_BOOL {
                                        if *op != AssignOp::Set {
                                            return Err(format!(
                                                "bool local struct field assignment only supports '=' for '{}.{}'",
                                                base, suffix
                                            ));
                                        }
                                        builder.ins().call(
                                            runtime_call_refs.global_i32_store,
                                            &[path_hash, rhs.value],
                                        );
                                        continue;
                                    }
                                    if field_type == TYPE_ID_F32 {
                                        let value = match op {
                                            AssignOp::Set => rhs.value,
                                            AssignOp::Add => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fadd(lhs, rhs.value)
                                            }
                                            AssignOp::Sub => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fsub(lhs, rhs.value)
                                            }
                                            AssignOp::Mul => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fmul(lhs, rhs.value)
                                            }
                                            AssignOp::Div => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fdiv(lhs, rhs.value)
                                            }
                                            AssignOp::Mod => {
                                                return Err(format!(
                                                    "'%=' is unsupported for f32 local struct field '{}.{}'",
                                                    base, suffix
                                                ));
                                            }
                                        };
                                        builder.ins().call(
                                            runtime_call_refs.global_f32_store,
                                            &[path_hash, value],
                                        );
                                        continue;
                                    }
                                    if field_type == TYPE_ID_F64 {
                                        let value = match op {
                                            AssignOp::Set => rhs.value,
                                            AssignOp::Add => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f64_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fadd(lhs, rhs.value)
                                            }
                                            AssignOp::Sub => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f64_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fsub(lhs, rhs.value)
                                            }
                                            AssignOp::Mul => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f64_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fmul(lhs, rhs.value)
                                            }
                                            AssignOp::Div => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f64_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fdiv(lhs, rhs.value)
                                            }
                                            AssignOp::Mod => {
                                                return Err(format!(
                                                    "'%=' is unsupported for f64 local struct field '{}.{}'",
                                                    base,
                                                    suffix
                                                ));
                                            }
                                        };
                                        builder.ins().call(
                                            runtime_call_refs.global_f64_store,
                                            &[path_hash, value],
                                        );
                                        continue;
                                    }
                                    return Err(format!(
                                        "unsupported local struct field type {} for '{}.{}'",
                                        field_type, base, suffix
                                    ));
                                }
                            }
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
                                Some(TYPE_ID_I32),
                                values_by_name,
                                runtime_call_refs,
                                internal_calls,
                                call_signatures,
                                type_table,
                                global_path_types,
                                constant_values,
                                collection_infos,
                                named_struct_field_types,
                                foreach_bindings,
                            )?;
                            emit_local_indexed_collection_assignment(
                                builder,
                                runtime_call_refs,
                                type_table,
                                named_struct_field_types,
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
                            Some(TYPE_ID_I32),
                            values_by_name,
                            runtime_call_refs,
                            internal_calls,
                            call_signatures,
                            type_table,
                            global_path_types,
                            constant_values,
                            collection_infos,
                            named_struct_field_types,
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
                let expected_source_type = match kind {
                    ConversionKind::FromI32 => Some(TYPE_ID_I32),
                    ConversionKind::FromF32 => Some(TYPE_ID_F32),
                    ConversionKind::FromF64 => Some(TYPE_ID_F64),
                };
                let source_binding = emit_simple_expression(
                    builder,
                    source,
                    expected_source_type,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
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
                            Some(TYPE_ID_I32),
                            values_by_name,
                            runtime_call_refs,
                            internal_calls,
                            call_signatures,
                            type_table,
                            global_path_types,
                            constant_values,
                            collection_infos,
                            named_struct_field_types,
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
                        internal_calls,
                        type_table,
                        target,
                        args,
                        values_by_name,
                        call_signatures,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?;
                    if handled {
                        continue;
                    }
                    let mut arg_values: Vec<Value> = Vec::with_capacity(args.len());
                    let mut arg_types: Vec<TypeId> = Vec::with_capacity(args.len());
                    for arg in args {
                        if let Some(struct_view) = try_emit_struct_view_value(
                            builder,
                            arg,
                            values_by_name,
                            runtime_call_refs,
                            internal_calls,
                            call_signatures,
                            type_table,
                            global_path_types,
                            constant_values,
                            collection_infos,
                            named_struct_field_types,
                            foreach_bindings,
                        )? {
                            arg_values.push(struct_view.base);
                            arg_values.push(struct_view.index);
                            arg_values.push(struct_view.len);
                            arg_types.push(struct_view.type_id);
                            continue;
                        }
                        let binding = emit_simple_expression(
                            builder,
                            arg,
                            None,
                            values_by_name,
                            runtime_call_refs,
                            internal_calls,
                            call_signatures,
                            type_table,
                            global_path_types,
                            constant_values,
                            collection_infos,
                            named_struct_field_types,
                            foreach_bindings,
                        )?;
                        arg_values.push(binding.value);
                        arg_types.push(binding.type_id);
                    }
                    let signature = resolve_call_signature(
                        target,
                        &arg_types,
                        call_signatures,
                        type_table,
                        named_struct_field_types,
                    )?;
                    if signature.return_type == TYPE_ID_VOID {
                        if signature.extern_symbol.is_some() {
                            let _ = emit_extern_call_for_signature(
                                builder,
                                runtime_call_refs,
                                signature,
                                &arg_values,
                            )?;
                        } else {
                            match internal_calls {
                                InternalCallMode::Jit => {
                                    let _ = emit_indirect_call_for_signature(
                                        builder,
                                        runtime_call_refs,
                                        signature,
                                        &arg_values,
                                        type_table,
                                        named_struct_field_types,
                                    )?;
                                }
                                InternalCallMode::AotDirect(mode) => {
                                    let _ = emit_aot_direct_call_for_signature(
                                        builder,
                                        mode,
                                        signature,
                                        &arg_values,
                                        type_table,
                                        named_struct_field_types,
                                    )?;
                                }
                            }
                        }
                        continue;
                    }
                }
                let _ = emit_simple_expression(
                    builder,
                    expression,
                    None,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
            }
            SimpleStmt::Continue => {
                let Some(loop_control) = loop_control else {
                    return Err("continue statement is only valid inside loops".to_string());
                };
                builder.ins().jump(loop_control.continue_block, &[]);
                return Ok(true);
            }
            SimpleStmt::Return(expression) => {
                let binding = emit_simple_expression(
                    builder,
                    expression,
                    Some(expected_return_type),
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
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
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
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
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                    loop_control,
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
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                        loop_control,
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
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                    expected_return_type,
                    next_variable,
                )?;

                let condition_block = builder.create_block();
                let body_block = builder.create_block();
                let step_block = builder.create_block();
                let exit_block = builder.create_block();
                let loop_control = LoopControlContext {
                    continue_block: step_block,
                };

                builder.ins().jump(condition_block, &[]);
                builder.switch_to_block(condition_block);

                let condition_value = emit_simple_condition(
                    builder,
                    condition,
                    &loop_values,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                builder
                    .ins()
                    .brif(condition_value, body_block, &[], exit_block, &[]);

                builder.seal_block(body_block);
                builder.switch_to_block(body_block);
                let body_terminated = emit_simple_statements(
                    builder,
                    body_statements,
                    &mut loop_values,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                    Some(&loop_control),
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
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                    expected_return_type,
                    next_variable,
                )?;
                builder.ins().jump(condition_block, &[]);
                builder.seal_block(condition_block);

                builder.seal_block(exit_block);
                builder.switch_to_block(exit_block);
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
                let (collection_info, collection_handle, collection_struct_type_id) =
                    if let Some(local_collection) = values_by_name.get(collection_path).copied() {
                        let info = build_local_foreach_collection_info(
                            collection_path,
                            local_collection.type_id,
                            type_table,
                            named_struct_field_types,
                        )?;
                        let element_type =
                            type_table.indexed_element_type_id(local_collection.type_id);
                        let struct_type_id = element_type
                            .filter(|type_id| named_struct_field_types.contains_key(type_id));
                        (
                            info,
                            ForeachCollectionHandle::LocalVar(local_collection.var),
                            struct_type_id,
                        )
                    } else {
                        let Some(collection_info) = collection_infos.get(collection_path) else {
                            return Err(format!(
                                "unknown foreach collection '{}' in current jit path",
                                collection_path
                            ));
                        };
                        let collection_type = global_path_types
                            .get(collection_path)
                            .copied()
                            .ok_or_else(|| {
                                format!(
                                    "unknown foreach collection '{}' in current jit path",
                                    collection_path
                                )
                            })?;
                        let element_type = type_table.indexed_element_type_id(collection_type);
                        let struct_type_id = element_type
                            .filter(|type_id| named_struct_field_types.contains_key(type_id));
                        (
                            collection_info.clone(),
                            ForeachCollectionHandle::PathHash(hash_global_path(collection_path)),
                            struct_type_id,
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

                // Cache collection field pointers once per foreach loop, so hot inner loops can
                // use direct loads/stores instead of calling runtime helpers on every iteration.
                let collection_hash_value =
                    emit_foreach_collection_handle_value(builder, collection_handle);
                let len_value = builder
                    .ins()
                    .iconst(types::I32, i64::from(collection_info.len));
                let mut i32_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                let mut f32_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                let mut f64_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                if collection_info
                    .element_type
                    .is_some_and(|type_id| is_i32_abi_compatible_type(type_id, type_table))
                {
                    let field_hash_value = builder.ins().iconst(types::I32, 0);
                    let call = builder.ins().call(
                        runtime_call_refs.global_i32_array_ptr,
                        &[collection_hash_value, field_hash_value, len_value],
                    );
                    i32_array_base_ptrs.insert(String::new(), builder.inst_results(call)[0]);
                }
                if collection_info.element_type == Some(TYPE_ID_F32) {
                    let field_hash_value = builder.ins().iconst(types::I32, 0);
                    let call = builder.ins().call(
                        runtime_call_refs.global_f32_array_ptr,
                        &[collection_hash_value, field_hash_value, len_value],
                    );
                    f32_array_base_ptrs.insert(String::new(), builder.inst_results(call)[0]);
                }
                if collection_info.element_type == Some(TYPE_ID_F64) {
                    let field_hash_value = builder.ins().iconst(types::I32, 0);
                    let call = builder.ins().call(
                        runtime_call_refs.global_f64_array_ptr,
                        &[collection_hash_value, field_hash_value, len_value],
                    );
                    f64_array_base_ptrs.insert(String::new(), builder.inst_results(call)[0]);
                }
                for (suffix, type_id) in &collection_info.field_types {
                    let field_hash = hash_foreach_field_suffix(suffix);
                    let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
                    if is_i32_abi_compatible_type(*type_id, type_table) {
                        let call = builder.ins().call(
                            runtime_call_refs.global_i32_array_ptr,
                            &[collection_hash_value, field_hash_value, len_value],
                        );
                        i32_array_base_ptrs.insert(suffix.clone(), builder.inst_results(call)[0]);
                    }
                    if *type_id == TYPE_ID_F32 {
                        let call = builder.ins().call(
                            runtime_call_refs.global_f32_array_ptr,
                            &[collection_hash_value, field_hash_value, len_value],
                        );
                        f32_array_base_ptrs.insert(suffix.clone(), builder.inst_results(call)[0]);
                    }
                    if *type_id == TYPE_ID_F64 {
                        let call = builder.ins().call(
                            runtime_call_refs.global_f64_array_ptr,
                            &[collection_hash_value, field_hash_value, len_value],
                        );
                        f64_array_base_ptrs.insert(suffix.clone(), builder.inst_results(call)[0]);
                    }
                }

                let mut loop_values = values_by_name.clone();
                if let Some(index_name) = index_name {
                    loop_values.insert(
                        index_name.clone(),
                        LocalBinding {
                            var: index_var,
                            type_id: TYPE_ID_I32,
                            struct_view: None,
                        },
                    );
                }
                let mut loop_foreach_bindings = foreach_bindings.clone();
                loop_foreach_bindings.insert(
                    item_name.clone(),
                    ForeachBinding {
                        collection_handle,
                        index_var,
                        len: collection_info.len,
                        element_type: collection_info.element_type,
                        struct_type_id: collection_struct_type_id,
                        field_types: collection_info.field_types.clone(),
                        i32_array_base_ptrs,
                        f32_array_base_ptrs,
                        f64_array_base_ptrs,
                    },
                );

                let condition_block = builder.create_block();
                let body_block = builder.create_block();
                let step_block = builder.create_block();
                let exit_block = builder.create_block();
                let loop_control = LoopControlContext {
                    continue_block: step_block,
                };

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
                    .brif(condition_value, body_block, &[], exit_block, &[]);

                builder.seal_block(body_block);
                builder.switch_to_block(body_block);
                let body_terminated = emit_simple_statements(
                    builder,
                    body_statements,
                    &mut loop_values,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    &loop_foreach_bindings,
                    Some(&loop_control),
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

                builder.seal_block(exit_block);
                builder.switch_to_block(exit_block);
            }
        }
    }
    Ok(false)
}

pub(crate) fn emit_conversion_assignment_value(
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
            if target_type == TYPE_ID_F32 {
                return Ok(ValueBinding {
                    value: builder.ins().fcvt_from_sint(types::F32, source.value),
                    type_id: TYPE_ID_F32,
                });
            }
            if target_type == TYPE_ID_F64 {
                return Ok(ValueBinding {
                    value: builder.ins().fcvt_from_sint(types::F64, source.value),
                    type_id: TYPE_ID_F64,
                });
            }
            Err(format!(
                "from_i32 target '{}' must be f32 or f64",
                target_name
            ))
        }
        ConversionKind::FromF32 => {
            if source.type_id != TYPE_ID_F32 {
                return Err("from_f32 source expression must be f32".to_string());
            }
            if target_type == TYPE_ID_I32 {
                return Ok(ValueBinding {
                    value: builder.ins().fcvt_to_sint(types::I32, source.value),
                    type_id: TYPE_ID_I32,
                });
            }
            if target_type == TYPE_ID_F64 {
                return Ok(ValueBinding {
                    value: builder.ins().fpromote(types::F64, source.value),
                    type_id: TYPE_ID_F64,
                });
            }
            Err(format!(
                "from_f32 target '{}' must be i32 or f64",
                target_name
            ))
        }
        ConversionKind::FromF64 => {
            if source.type_id != TYPE_ID_F64 {
                return Err("from_f64 source expression must be f64".to_string());
            }
            if target_type == TYPE_ID_I32 {
                return Ok(ValueBinding {
                    value: builder.ins().fcvt_to_sint(types::I32, source.value),
                    type_id: TYPE_ID_I32,
                });
            }
            if target_type == TYPE_ID_F32 {
                return Ok(ValueBinding {
                    value: builder.ins().fdemote(types::F32, source.value),
                    type_id: TYPE_ID_F32,
                });
            }
            Err(format!(
                "from_f64 target '{}' must be i32 or f32",
                target_name
            ))
        }
    }
}

pub(crate) fn emit_for_control_statement(
    builder: &mut FunctionBuilder<'_>,
    statement: &SimpleStmt,
    values_by_name: &mut BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
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
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
                None,
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
pub(crate) enum SimpleExpr {
    Int(i64),
    Float(f64),
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

pub(crate) fn eval_const_i64(expression: &SimpleExpr) -> Option<i64> {
    match expression {
        SimpleExpr::Int(value) => Some(*value),
        SimpleExpr::Binary { lhs, op, rhs } => {
            let lhs = eval_const_i64(lhs)?;
            let rhs = eval_const_i64(rhs)?;
            match *op {
                '+' => lhs.checked_add(rhs),
                '-' => lhs.checked_sub(rhs),
                '*' => lhs.checked_mul(rhs),
                '/' => {
                    if rhs == 0 {
                        None
                    } else {
                        lhs.checked_div(rhs)
                    }
                }
                '%' => {
                    if rhs == 0 {
                        None
                    } else {
                        lhs.checked_rem(rhs)
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

pub(crate) fn parse_simple_expression(expression: &str) -> Result<SimpleExpr, String> {
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

pub(crate) fn parse_value_expression(expression: &str) -> Result<SimpleExpr, String> {
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

pub(crate) fn looks_like_condition_expression(expression: &str) -> bool {
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
pub(crate) enum ExprToken {
    Int(i64),
    Float(f64),
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

pub(crate) fn tokenize_simple_expression(expression: &str) -> Result<Vec<ExprToken>, String> {
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
                    .parse::<f64>()
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

pub(crate) struct ExprParser<'a> {
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
                if let Some(const_i64) = eval_const_i64(&expression) {
                    if const_i64 < 0 {
                        return Err(
                            "negative collection indices are unsupported (use .length/.max_length)"
                                .to_string(),
                        );
                    }
                }
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

pub(crate) fn resolve_call_signature<'a>(
    target: &str,
    arg_types: &[TypeId],
    call_signatures: &'a CallSignatureMap,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> Result<&'a CallSignature, String> {
    let Some(candidates) = call_signatures.get(target) else {
        return Err(format!("unknown call target '{}'", target));
    };
    let mut matches = candidates.iter().filter(|candidate| {
        candidate.params.len() == arg_types.len()
            && arg_types
                .iter()
                .zip(candidate.params.iter())
                .all(|(arg, param)| {
                    are_call_argument_and_param_compatible(
                        *arg,
                        *param,
                        type_table,
                        named_struct_field_types,
                    )
                })
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

pub(crate) fn are_call_argument_and_param_compatible(
    argument: TypeId,
    parameter: TypeId,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> bool {
    if is_struct_view_type(argument, named_struct_field_types)
        || is_struct_view_type(parameter, named_struct_field_types)
    {
        return argument == parameter;
    }
    type_table.is_argument_compatible_with_param(argument, parameter)
}

pub(crate) fn try_emit_struct_view_value(
    builder: &mut FunctionBuilder<'_>,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Option<StructViewValue>, String> {
    match expression {
        SimpleExpr::Identifier(name) => {
            if let Some(local) = values_by_name.get(name).copied() {
                if is_struct_view_type(local.type_id, named_struct_field_types) {
                    let Some(struct_view) = local.struct_view else {
                        return Err(format!(
                            "struct local '{}' is missing struct view metadata in current jit path",
                            name
                        ));
                    };
                    return Ok(Some(StructViewValue {
                        type_id: local.type_id,
                        base: builder.use_var(local.var),
                        index: builder.use_var(struct_view.index_var),
                        len: builder.use_var(struct_view.len_var),
                    }));
                }
            }
            if let Some(binding) = foreach_bindings.get(name) {
                if let Some(struct_type_id) = binding.struct_type_id {
                    let base =
                        emit_foreach_collection_handle_value(builder, binding.collection_handle);
                    let index = builder.use_var(binding.index_var);
                    let len = builder.ins().iconst(types::I32, i64::from(binding.len));
                    return Ok(Some(StructViewValue {
                        type_id: struct_type_id,
                        base,
                        index,
                        len,
                    }));
                }
            }
            if let Some(path_type) = global_path_types.get(name).copied() {
                if is_struct_view_type(path_type, named_struct_field_types) {
                    let base = builder
                        .ins()
                        .iconst(types::I32, i64::from(hash_global_path(name)));
                    let index = builder
                        .ins()
                        .iconst(types::I32, i64::from(STRUCT_VIEW_AOS_INDEX_SENTINEL));
                    let len = builder
                        .ins()
                        .iconst(types::I32, i64::from(STRUCT_VIEW_AOS_LEN_SENTINEL));
                    return Ok(Some(StructViewValue {
                        type_id: path_type,
                        base,
                        index,
                        len,
                    }));
                }
            }
            Ok(None)
        }
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            if !suffix.is_empty() {
                return Ok(None);
            }

            let (collection_handle, collection_type_id, known_len) =
                if let Some(local_collection) = values_by_name.get(collection_path).copied() {
                    let len = type_table.fixed_collection_len(local_collection.type_id);
                    (
                        builder.use_var(local_collection.var),
                        local_collection.type_id,
                        len,
                    )
                } else {
                    let Some(collection_type_id) = global_path_types.get(collection_path).copied()
                    else {
                        return Ok(None);
                    };
                    let len = collection_infos
                        .get(collection_path)
                        .map(|info| info.len)
                        .or_else(|| type_table.fixed_collection_len(collection_type_id));
                    (
                        builder
                            .ins()
                            .iconst(types::I32, i64::from(hash_global_path(collection_path))),
                        collection_type_id,
                        len,
                    )
                };

            let Some(element_type_id) = type_table.indexed_element_type_id(collection_type_id)
            else {
                return Ok(None);
            };
            if !is_struct_view_type(element_type_id, named_struct_field_types) {
                return Ok(None);
            }

            let index_binding = emit_simple_expression(
                builder,
                index,
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            let index_binding = normalize_index_binding(index_binding, type_table)?;

            let len_value = if let Some(known_len) = known_len {
                builder.ins().iconst(types::I32, i64::from(known_len))
            } else {
                let kind_value = builder
                    .ins()
                    .iconst(types::I32, i64::from(CollectionMetaKind::Length as i32));
                let call = builder.ins().call(
                    runtime_call_refs.collection_i32_load,
                    &[collection_handle, kind_value],
                );
                builder.inst_results(call)[0]
            };

            Ok(Some(StructViewValue {
                type_id: element_type_id,
                base: collection_handle,
                index: index_binding.value,
                len: len_value,
            }))
        }
        _ => Ok(None),
    }
}

pub(crate) fn emit_simple_expression(
    builder: &mut FunctionBuilder<'_>,
    expression: &SimpleExpr,
    expected_type: Option<TypeId>,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
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
        SimpleExpr::Float(value) => {
            if expected_type == Some(TYPE_ID_F64) {
                Ok(ValueBinding {
                    value: builder.ins().f64const(Ieee64::with_float(*value)),
                    type_id: TYPE_ID_F64,
                })
            } else {
                Ok(ValueBinding {
                    value: builder.ins().f32const(Ieee32::with_float(*value as f32)),
                    type_id: TYPE_ID_F32,
                })
            }
        }
        SimpleExpr::Bool(value) => Ok(ValueBinding {
            value: builder
                .ins()
                .iconst(types::I32, if *value { 1_i64 } else { 0_i64 }),
            type_id: TYPE_ID_BOOL,
        }),
        SimpleExpr::StringLiteral(value) => {
            let literal_id = hash_string_literal(value);
            stasis_dynload::upsert_jit_string_literal(literal_id, value);
            let string_type_id = type_table.string_literal_type_id().unwrap_or(TYPE_ID_I32);
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
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
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
            if let Some((base, suffix)) = name.split_once('.') {
                if let Some(local) = values_by_name.get(base).copied() {
                    if let Some(kind) = collection_meta_kind_from_suffix(suffix) {
                        if is_collection_handle_type(local.type_id, type_table) {
                            let base_value = builder.use_var(local.var);
                            let kind_value =
                                builder.ins().iconst(types::I32, i64::from(kind as i32));
                            let call = builder.ins().call(
                                runtime_call_refs.collection_i32_load,
                                &[base_value, kind_value],
                            );
                            return Ok(ValueBinding {
                                value: builder.inst_results(call)[0],
                                type_id: TYPE_ID_I32,
                            });
                        }
                    }
                    if let Some(field_types) = named_struct_field_types.get(&local.type_id) {
                        let Some(field_type) = field_types.get(suffix).copied() else {
                            return Err(format!(
                                "unknown local struct field path '{}.{}' in current jit path",
                                base, suffix
                            ));
                        };
                        let base_hash = builder.use_var(local.var);
                        if let Some(struct_view) = local.struct_view {
                            return emit_struct_view_field_load(
                                builder,
                                runtime_call_refs,
                                type_table,
                                struct_view,
                                base_hash,
                                suffix,
                                field_type,
                            );
                        }

                        let path_hash =
                            emit_local_struct_field_path_hash(base_hash, suffix, builder);
                        if is_collection_handle_type(field_type, type_table) {
                            return Ok(ValueBinding {
                                value: path_hash,
                                type_id: field_type,
                            });
                        }
                        if is_i32_abi_compatible_type(field_type, type_table) {
                            let call = builder
                                .ins()
                                .call(runtime_call_refs.global_i32_load, &[path_hash]);
                            return Ok(ValueBinding {
                                value: builder.inst_results(call)[0],
                                type_id: field_type,
                            });
                        }
                        if field_type == TYPE_ID_F32 {
                            let call = builder
                                .ins()
                                .call(runtime_call_refs.global_f32_load, &[path_hash]);
                            return Ok(ValueBinding {
                                value: builder.inst_results(call)[0],
                                type_id: TYPE_ID_F32,
                            });
                        }
                        if field_type == TYPE_ID_F64 {
                            let call = builder
                                .ins()
                                .call(runtime_call_refs.global_f64_load, &[path_hash]);
                            return Ok(ValueBinding {
                                value: builder.inst_results(call)[0],
                                type_id: TYPE_ID_F64,
                            });
                        }
                        return Err(format!(
                            "unsupported local struct field type {} for '{}.{}'",
                            field_type, base, suffix
                        ));
                    }
                }
            }
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
                if named_struct_field_types.contains_key(&path_type) {
                    return Ok(ValueBinding {
                        value: builder
                            .ins()
                            .iconst(types::I32, i64::from(hash_global_path(name))),
                        type_id: path_type,
                    });
                }
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
                    Some(TYPE_ID_I32),
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                return emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
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
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
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
                if let Some(struct_view) = try_emit_struct_view_value(
                    builder,
                    arg,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )? {
                    arg_values.push(struct_view.base);
                    arg_values.push(struct_view.index);
                    arg_values.push(struct_view.len);
                    arg_types.push(struct_view.type_id);
                    continue;
                }

                let binding = emit_simple_expression(
                    builder,
                    arg,
                    None,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                arg_values.push(binding.value);
                arg_types.push(binding.type_id);
            }
            if target == "i32_to_f32" {
                if arg_values.len() != 1 {
                    return Err(format!(
                        "math intrinsic 'i32_to_f32' expects exactly one argument, found {}",
                        arg_values.len()
                    ));
                }
                if !is_i32_abi_compatible_type(arg_types[0], type_table) {
                    return Err(format!(
                        "math intrinsic 'i32_to_f32' requires i32-compatible argument, found type {}",
                        arg_types[0]
                    ));
                }
                return Ok(ValueBinding {
                    value: builder.ins().fcvt_from_sint(types::F32, arg_values[0]),
                    type_id: TYPE_ID_F32,
                });
            }
            if target == "f32_to_i32" {
                if arg_values.len() != 1 {
                    return Err(format!(
                        "math intrinsic 'f32_to_i32' expects exactly one argument, found {}",
                        arg_values.len()
                    ));
                }
                if arg_types[0] != TYPE_ID_F32 {
                    return Err(format!(
                        "math intrinsic 'f32_to_i32' requires f32 argument, found type {}",
                        arg_types[0]
                    ));
                }
                return Ok(ValueBinding {
                    value: builder.ins().fcvt_to_sint(types::I32, arg_values[0]),
                    type_id: TYPE_ID_I32,
                });
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
            let signature = resolve_call_signature(
                target,
                &arg_types,
                call_signatures,
                type_table,
                named_struct_field_types,
            )?;
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
            if let InternalCallMode::AotDirect(mode) = internal_calls {
                let result = emit_aot_direct_call_for_signature(
                    builder,
                    mode,
                    signature,
                    &arg_values,
                    type_table,
                    named_struct_field_types,
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
                        _ => {
                            let value = emit_indirect_call_for_signature(
                                builder,
                                runtime_call_refs,
                                signature,
                                &arg_values,
                                type_table,
                                named_struct_field_types,
                            )?
                            .ok_or_else(|| {
                                format!("call target '{}' did not produce value", target)
                            })?;
                            return Ok(ValueBinding {
                                value,
                                type_id: signature.return_type,
                            });
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
                        _ => {
                            let value = emit_indirect_call_for_signature(
                                builder,
                                runtime_call_refs,
                                signature,
                                &arg_values,
                                type_table,
                                named_struct_field_types,
                            )?
                            .ok_or_else(|| {
                                format!("call target '{}' did not produce value", target)
                            })?;
                            return Ok(ValueBinding {
                                value,
                                type_id: signature.return_type,
                            });
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
                            named_struct_field_types,
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
                                named_struct_field_types,
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
                        _ => {
                            let value = emit_indirect_call_for_signature(
                                builder,
                                runtime_call_refs,
                                signature,
                                &arg_values,
                                type_table,
                                named_struct_field_types,
                            )?
                            .ok_or_else(|| {
                                format!("call target '{}' did not produce value", target)
                            })?;
                            return Ok(ValueBinding {
                                value,
                                type_id: signature.return_type,
                            });
                        }
                    }
                }
            } else if signature.return_type == TYPE_ID_F64 {
                let value = emit_indirect_call_for_signature(
                    builder,
                    runtime_call_refs,
                    signature,
                    &arg_values,
                    type_table,
                    named_struct_field_types,
                )?
                .ok_or_else(|| format!("call target '{}' did not produce value", target))?;
                return Ok(ValueBinding {
                    value,
                    type_id: TYPE_ID_F64,
                });
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
            let child_expected = match expected_type {
                Some(TYPE_ID_F32) => Some(TYPE_ID_F32),
                Some(TYPE_ID_F64) => Some(TYPE_ID_F64),
                _ => None,
            };
            let lhs_value = emit_simple_expression(
                builder,
                lhs,
                child_expected,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            let rhs_value = emit_simple_expression(
                builder,
                rhs,
                child_expected,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
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

            if lhs_value.type_id == TYPE_ID_F64 || rhs_value.type_id == TYPE_ID_F64 {
                let (lhs_f64, rhs_f64) =
                    coerce_numeric_operands_to_f64(builder, lhs_value, rhs_value, *op, type_table)?;
                let value = match op {
                    '+' => builder.ins().fadd(lhs_f64, rhs_f64),
                    '-' => builder.ins().fsub(lhs_f64, rhs_f64),
                    '*' => builder.ins().fmul(lhs_f64, rhs_f64),
                    '/' => builder.ins().fdiv(lhs_f64, rhs_f64),
                    '%' => {
                        return Err(
                            "unsupported '%' operator for f64 expression in current jit path"
                                .to_string(),
                        )
                    }
                    other => {
                        return Err(format!(
                            "unsupported binary operator '{other}' in expression"
                        ))
                    }
                };
                return Ok(ValueBinding {
                    value,
                    type_id: TYPE_ID_F64,
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

pub(crate) fn coerce_numeric_operands_to_f32(
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

pub(crate) fn coerce_numeric_operands_to_f64(
    builder: &mut FunctionBuilder<'_>,
    lhs: ValueBinding,
    rhs: ValueBinding,
    op: char,
    type_table: &TypeTable,
) -> Result<(Value, Value), String> {
    let lhs_value = if lhs.type_id == TYPE_ID_F64 {
        lhs.value
    } else if lhs.type_id == TYPE_ID_F32 {
        builder.ins().fpromote(types::F64, lhs.value)
    } else if is_i32_numeric_type(lhs.type_id, type_table) {
        builder.ins().fcvt_from_sint(types::F64, lhs.value)
    } else {
        return Err(format!(
            "unsupported lhs type {} for '{}' expression",
            lhs.type_id, op
        ));
    };
    let rhs_value = if rhs.type_id == TYPE_ID_F64 {
        rhs.value
    } else if rhs.type_id == TYPE_ID_F32 {
        builder.ins().fpromote(types::F64, rhs.value)
    } else if is_i32_numeric_type(rhs.type_id, type_table) {
        builder.ins().fcvt_from_sint(types::F64, rhs.value)
    } else {
        return Err(format!(
            "unsupported rhs type {} for '{}' expression",
            rhs.type_id, op
        ));
    };
    Ok((lhs_value, rhs_value))
}

pub(crate) fn emit_constant_value(
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
        ConstantValue::F64(value) => Ok(ValueBinding {
            value: builder.ins().f64const(Ieee64::with_float(*value)),
            type_id: TYPE_ID_F64,
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

pub(crate) fn resolve_foreach_binding_for_path<'a>(
    path: &str,
    foreach_bindings: &'a ForeachBindingMap,
) -> Option<(&'a ForeachBinding, String)> {
    let mut segments = path.splitn(2, '.');
    let alias = segments.next()?;
    let suffix = segments.next().unwrap_or("").to_string();
    let binding = foreach_bindings.get(alias)?;
    Some((binding, suffix))
}

pub(crate) fn build_local_foreach_collection_info(
    collection_path: &str,
    collection_type: TypeId,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
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
    if let Some(field_types) = named_struct_field_types.get(&element_type) {
        return Ok(ForeachCollectionInfo {
            len,
            element_type: None,
            field_types: field_types.clone(),
        });
    }
    Ok(ForeachCollectionInfo {
        len,
        element_type: Some(element_type),
        field_types: BTreeMap::new(),
    })
}

pub(crate) fn emit_foreach_binding_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    binding: &ForeachBinding,
    suffix: &str,
) -> Result<ValueBinding, String> {
    let resolved = resolve_foreach_binding_value_type(binding, suffix)?;
    let index_value = builder.use_var(binding.index_var);
    if is_i32_abi_compatible_type(resolved, type_table) {
        if let Some(base_ptr) = binding.i32_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 2);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            let value = builder.ins().load(types::I32, MemFlags::new(), addr, 0);
            return Ok(ValueBinding {
                value,
                type_id: resolved,
            });
        }

        let field_hash = hash_foreach_field_suffix(suffix);
        let collection_hash =
            emit_foreach_collection_handle_value(builder, binding.collection_handle);
        let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
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
        if let Some(base_ptr) = binding.f32_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 2);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            let value = builder.ins().load(types::F32, MemFlags::new(), addr, 0);
            return Ok(ValueBinding {
                value,
                type_id: TYPE_ID_F32,
            });
        }

        let field_hash = hash_foreach_field_suffix(suffix);
        let collection_hash =
            emit_foreach_collection_handle_value(builder, binding.collection_handle);
        let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
        let call = builder.ins().call(
            runtime_call_refs.global_f32_array_load,
            &[collection_hash, field_hash_value, index_value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F32,
        });
    }
    if resolved == TYPE_ID_F64 {
        if let Some(base_ptr) = binding.f64_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 3);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            let value = builder.ins().load(types::F64, MemFlags::new(), addr, 0);
            return Ok(ValueBinding {
                value,
                type_id: TYPE_ID_F64,
            });
        }

        let field_hash = hash_foreach_field_suffix(suffix);
        let collection_hash =
            emit_foreach_collection_handle_value(builder, binding.collection_handle);
        let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
        let call = builder.ins().call(
            runtime_call_refs.global_f64_array_load,
            &[collection_hash, field_hash_value, index_value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F64,
        });
    }
    Err(format!(
        "unsupported foreach binding load type {} for suffix '{}'",
        resolved, suffix
    ))
}

pub(crate) fn emit_foreach_binding_assignment(
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
        if let Some(base_ptr) = binding.i32_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 2);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            builder.ins().store(MemFlags::new(), value, addr, 0);
        } else {
            builder.ins().call(
                runtime_call_refs.global_i32_array_store,
                &[collection_hash, field_hash_value, index_value, value],
            );
        }
        return Ok(());
    }
    if path_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool foreach binding '{}' only supports '=' assignment",
                suffix
            ));
        }
        if let Some(base_ptr) = binding.i32_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 2);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            builder.ins().store(MemFlags::new(), rhs.value, addr, 0);
        } else {
            builder.ins().call(
                runtime_call_refs.global_i32_array_store,
                &[collection_hash, field_hash_value, index_value, rhs.value],
            );
        }
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
        if let Some(base_ptr) = binding.f32_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 2);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            builder.ins().store(MemFlags::new(), value, addr, 0);
        } else {
            builder.ins().call(
                runtime_call_refs.global_f32_array_store,
                &[collection_hash, field_hash_value, index_value, value],
            );
        }
        return Ok(());
    }
    if path_type == TYPE_ID_F64 {
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
                    "'%=' is unsupported for f64 foreach binding '{}'",
                    suffix
                ))
            }
        };
        if let Some(base_ptr) = binding.f64_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 3);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            builder.ins().store(MemFlags::new(), value, addr, 0);
        } else {
            builder.ins().call(
                runtime_call_refs.global_f64_array_store,
                &[collection_hash, field_hash_value, index_value, value],
            );
        }
        return Ok(());
    }
    Err(format!(
        "unsupported foreach binding assignment type {} for suffix '{}'",
        path_type, suffix
    ))
}

pub(crate) fn resolve_foreach_binding_value_type(
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

pub(crate) fn hash_foreach_field_suffix(suffix: &str) -> i32 {
    if suffix.is_empty() {
        0
    } else {
        hash_global_path(suffix)
    }
}

pub(crate) fn emit_local_struct_field_path_hash(
    base_hash: Value,
    suffix: &str,
    builder: &mut FunctionBuilder<'_>,
) -> Value {
    let mut hash_value = base_hash;
    let dot = builder.ins().iconst(types::I32, i64::from(b'.'));
    hash_value = builder.ins().bxor(hash_value, dot);
    hash_value = builder.ins().imul_imm(hash_value, 16_777_619);
    for byte in suffix.bytes() {
        let byte_value = builder.ins().iconst(types::I32, i64::from(byte));
        hash_value = builder.ins().bxor(hash_value, byte_value);
        hash_value = builder.ins().imul_imm(hash_value, 16_777_619);
    }
    hash_value
}

pub(crate) fn emit_struct_view_field_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    binding: StructViewBinding,
    base_hash: Value,
    suffix: &str,
    field_type: TypeId,
) -> Result<ValueBinding, String> {
    if is_collection_handle_type(field_type, type_table) {
        return Err(format!(
            "struct view field '{}' resolves to collection handle type {} which is unsupported in current jit path",
            suffix, field_type
        ));
    }
    let index_value = builder.use_var(binding.index_var);
    let aos_condition = builder
        .ins()
        .icmp_imm(IntCC::SignedLessThan, index_value, 0);

    let aos_block = builder.create_block();
    let soa_block = builder.create_block();
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, clif_type_for_type_id(field_type, type_table)?);

    builder
        .ins()
        .brif(aos_condition, aos_block, &[], soa_block, &[]);

    builder.switch_to_block(aos_block);
    let path_hash = emit_local_struct_field_path_hash(base_hash, suffix, builder);
    let aos_value = if is_i32_abi_compatible_type(field_type, type_table) {
        let call = builder
            .ins()
            .call(runtime_call_refs.global_i32_load, &[path_hash]);
        builder.inst_results(call)[0]
    } else if field_type == TYPE_ID_F32 {
        let call = builder
            .ins()
            .call(runtime_call_refs.global_f32_load, &[path_hash]);
        builder.inst_results(call)[0]
    } else if field_type == TYPE_ID_F64 {
        let call = builder
            .ins()
            .call(runtime_call_refs.global_f64_load, &[path_hash]);
        builder.inst_results(call)[0]
    } else {
        return Err(format!(
            "unsupported struct view field type {} for suffix '{}'",
            field_type, suffix
        ));
    };
    builder.ins().jump(merge_block, &[aos_value]);
    builder.seal_block(aos_block);

    builder.switch_to_block(soa_block);
    let field_hash = hash_foreach_field_suffix(suffix);
    let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
    let soa_value = if is_i32_abi_compatible_type(field_type, type_table) {
        let call = builder.ins().call(
            runtime_call_refs.global_i32_array_load,
            &[base_hash, field_hash_value, index_value],
        );
        builder.inst_results(call)[0]
    } else if field_type == TYPE_ID_F32 {
        let call = builder.ins().call(
            runtime_call_refs.global_f32_array_load,
            &[base_hash, field_hash_value, index_value],
        );
        builder.inst_results(call)[0]
    } else if field_type == TYPE_ID_F64 {
        let call = builder.ins().call(
            runtime_call_refs.global_f64_array_load,
            &[base_hash, field_hash_value, index_value],
        );
        builder.inst_results(call)[0]
    } else {
        return Err(format!(
            "unsupported struct view field type {} for suffix '{}'",
            field_type, suffix
        ));
    };
    builder.ins().jump(merge_block, &[soa_value]);
    builder.seal_block(soa_block);

    builder.seal_block(merge_block);
    builder.switch_to_block(merge_block);
    let value = builder
        .block_params(merge_block)
        .first()
        .copied()
        .ok_or_else(|| "struct view merge block missing value param".to_string())?;
    Ok(ValueBinding {
        value,
        type_id: field_type,
    })
}

pub(crate) fn emit_struct_view_field_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    binding: StructViewBinding,
    base_hash: Value,
    suffix: &str,
    field_type: TypeId,
    op: AssignOp,
    rhs: ValueBinding,
) -> Result<(), String> {
    if is_collection_handle_type(field_type, type_table) {
        return Err(format!(
            "struct view field '{}' resolves to collection handle type {} which is unsupported in current jit path",
            suffix, field_type
        ));
    }
    if !are_assignment_types_compatible(field_type, rhs.type_id, type_table) {
        return Err(format!(
            "assignment type mismatch for struct view field '{}': target type {}, expression type {}",
            suffix, field_type, rhs.type_id
        ));
    }

    let index_value = builder.use_var(binding.index_var);
    let aos_condition = builder
        .ins()
        .icmp_imm(IntCC::SignedLessThan, index_value, 0);
    let aos_block = builder.create_block();
    let soa_block = builder.create_block();
    let merge_block = builder.create_block();
    builder
        .ins()
        .brif(aos_condition, aos_block, &[], soa_block, &[]);

    builder.switch_to_block(aos_block);
    let path_hash = emit_local_struct_field_path_hash(base_hash, suffix, builder);
    if is_i32_scalar_lane_type(field_type, type_table) {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add | AssignOp::Sub | AssignOp::Mul | AssignOp::Div | AssignOp::Mod => {
                let call = builder
                    .ins()
                    .call(runtime_call_refs.global_i32_load, &[path_hash]);
                let lhs = builder.inst_results(call)[0];
                match op {
                    AssignOp::Add => builder.ins().iadd(lhs, rhs.value),
                    AssignOp::Sub => builder.ins().isub(lhs, rhs.value),
                    AssignOp::Mul => builder.ins().imul(lhs, rhs.value),
                    AssignOp::Div => builder.ins().sdiv(lhs, rhs.value),
                    AssignOp::Mod => builder.ins().srem(lhs, rhs.value),
                    AssignOp::Set => unreachable!(),
                }
            }
        };
        builder
            .ins()
            .call(runtime_call_refs.global_i32_store, &[path_hash, value]);
    } else if field_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool assignment only supports '=' in current jit path for struct view field '{}'",
                suffix
            ));
        }
        builder
            .ins()
            .call(runtime_call_refs.global_i32_store, &[path_hash, rhs.value]);
    } else if field_type == TYPE_ID_F32 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add | AssignOp::Sub | AssignOp::Mul | AssignOp::Div => {
                let call = builder
                    .ins()
                    .call(runtime_call_refs.global_f32_load, &[path_hash]);
                let lhs = builder.inst_results(call)[0];
                match op {
                    AssignOp::Add => builder.ins().fadd(lhs, rhs.value),
                    AssignOp::Sub => builder.ins().fsub(lhs, rhs.value),
                    AssignOp::Mul => builder.ins().fmul(lhs, rhs.value),
                    AssignOp::Div => builder.ins().fdiv(lhs, rhs.value),
                    AssignOp::Mod => unreachable!(),
                    AssignOp::Set => unreachable!(),
                }
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f32 struct view field '{}'",
                    suffix
                ));
            }
        };
        builder
            .ins()
            .call(runtime_call_refs.global_f32_store, &[path_hash, value]);
    } else if field_type == TYPE_ID_F64 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add | AssignOp::Sub | AssignOp::Mul | AssignOp::Div => {
                let call = builder
                    .ins()
                    .call(runtime_call_refs.global_f64_load, &[path_hash]);
                let lhs = builder.inst_results(call)[0];
                match op {
                    AssignOp::Add => builder.ins().fadd(lhs, rhs.value),
                    AssignOp::Sub => builder.ins().fsub(lhs, rhs.value),
                    AssignOp::Mul => builder.ins().fmul(lhs, rhs.value),
                    AssignOp::Div => builder.ins().fdiv(lhs, rhs.value),
                    AssignOp::Mod => unreachable!(),
                    AssignOp::Set => unreachable!(),
                }
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f64 struct view field '{}'",
                    suffix
                ));
            }
        };
        builder
            .ins()
            .call(runtime_call_refs.global_f64_store, &[path_hash, value]);
    } else {
        return Err(format!(
            "unsupported struct view field type {} for suffix '{}'",
            field_type, suffix
        ));
    }
    builder.ins().jump(merge_block, &[]);
    builder.seal_block(aos_block);

    builder.switch_to_block(soa_block);
    let field_hash = hash_foreach_field_suffix(suffix);
    let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
    if is_i32_scalar_lane_type(field_type, type_table) {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add | AssignOp::Sub | AssignOp::Mul | AssignOp::Div | AssignOp::Mod => {
                let call = builder.ins().call(
                    runtime_call_refs.global_i32_array_load,
                    &[base_hash, field_hash_value, index_value],
                );
                let lhs = builder.inst_results(call)[0];
                match op {
                    AssignOp::Add => builder.ins().iadd(lhs, rhs.value),
                    AssignOp::Sub => builder.ins().isub(lhs, rhs.value),
                    AssignOp::Mul => builder.ins().imul(lhs, rhs.value),
                    AssignOp::Div => builder.ins().sdiv(lhs, rhs.value),
                    AssignOp::Mod => builder.ins().srem(lhs, rhs.value),
                    AssignOp::Set => unreachable!(),
                }
            }
        };
        builder.ins().call(
            runtime_call_refs.global_i32_array_store,
            &[base_hash, field_hash_value, index_value, value],
        );
    } else if field_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool assignment only supports '=' in current jit path for struct view field '{}'",
                suffix
            ));
        }
        builder.ins().call(
            runtime_call_refs.global_i32_array_store,
            &[base_hash, field_hash_value, index_value, rhs.value],
        );
    } else if field_type == TYPE_ID_F32 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add | AssignOp::Sub | AssignOp::Mul | AssignOp::Div => {
                let call = builder.ins().call(
                    runtime_call_refs.global_f32_array_load,
                    &[base_hash, field_hash_value, index_value],
                );
                let lhs = builder.inst_results(call)[0];
                match op {
                    AssignOp::Add => builder.ins().fadd(lhs, rhs.value),
                    AssignOp::Sub => builder.ins().fsub(lhs, rhs.value),
                    AssignOp::Mul => builder.ins().fmul(lhs, rhs.value),
                    AssignOp::Div => builder.ins().fdiv(lhs, rhs.value),
                    AssignOp::Mod => unreachable!(),
                    AssignOp::Set => unreachable!(),
                }
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f32 struct view field '{}'",
                    suffix
                ));
            }
        };
        builder.ins().call(
            runtime_call_refs.global_f32_array_store,
            &[base_hash, field_hash_value, index_value, value],
        );
    } else if field_type == TYPE_ID_F64 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add | AssignOp::Sub | AssignOp::Mul | AssignOp::Div => {
                let call = builder.ins().call(
                    runtime_call_refs.global_f64_array_load,
                    &[base_hash, field_hash_value, index_value],
                );
                let lhs = builder.inst_results(call)[0];
                match op {
                    AssignOp::Add => builder.ins().fadd(lhs, rhs.value),
                    AssignOp::Sub => builder.ins().fsub(lhs, rhs.value),
                    AssignOp::Mul => builder.ins().fmul(lhs, rhs.value),
                    AssignOp::Div => builder.ins().fdiv(lhs, rhs.value),
                    AssignOp::Mod => unreachable!(),
                    AssignOp::Set => unreachable!(),
                }
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f64 struct view field '{}'",
                    suffix
                ));
            }
        };
        builder.ins().call(
            runtime_call_refs.global_f64_array_store,
            &[base_hash, field_hash_value, index_value, value],
        );
    } else {
        return Err(format!(
            "unsupported struct view field type {} for suffix '{}'",
            field_type, suffix
        ));
    }
    builder.ins().jump(merge_block, &[]);
    builder.seal_block(soa_block);

    builder.seal_block(merge_block);
    builder.switch_to_block(merge_block);
    Ok(())
}

pub(crate) fn emit_foreach_collection_handle_value(
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

pub(crate) fn resolve_collection_value_type(
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

pub(crate) fn normalize_index_binding(
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

pub(crate) fn resolve_local_collection_value_type(
    collection_type: TypeId,
    suffix: &str,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> Result<TypeId, String> {
    let element_type = type_table
        .indexed_element_type_id(collection_type)
        .ok_or_else(|| {
            format!(
                "local indexed collection access is unsupported for type {}",
                collection_type
            )
        })?;
    if suffix.is_empty() {
        if named_struct_field_types.contains_key(&element_type) {
            return Err(
                "local indexed collection access requires field path for struct elements"
                    .to_string(),
            );
        }
        return Ok(element_type);
    }
    let Some(field_types) = named_struct_field_types.get(&element_type) else {
        return Err(format!(
            "local indexed collection access does not support field path '{}'",
            suffix
        ));
    };
    field_types
        .get(suffix)
        .copied()
        .ok_or_else(|| format!("unknown local indexed collection field path '{}'", suffix))
}

pub(crate) fn emit_local_indexed_collection_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
    collection_name: &str,
    collection_binding: LocalBinding,
    suffix: &str,
    index_binding: ValueBinding,
) -> Result<ValueBinding, String> {
    let resolved = resolve_local_collection_value_type(
        collection_binding.type_id,
        suffix,
        type_table,
        named_struct_field_types,
    )?;
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
    if resolved == TYPE_ID_F64 {
        let call = builder.ins().call(
            runtime_call_refs.global_f64_array_load,
            &[collection_handle, field_hash, index_binding.value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F64,
        });
    }
    Err(format!(
        "unsupported local indexed collection load type {} for '{}[...].{}'",
        resolved, collection_name, suffix
    ))
}

pub(crate) fn emit_local_indexed_collection_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
    collection_name: &str,
    collection_binding: LocalBinding,
    suffix: &str,
    index_binding: ValueBinding,
    op: AssignOp,
    rhs: ValueBinding,
) -> Result<(), String> {
    let path_type = resolve_local_collection_value_type(
        collection_binding.type_id,
        suffix,
        type_table,
        named_struct_field_types,
    )?;
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
                    named_struct_field_types,
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
                    named_struct_field_types,
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
                    named_struct_field_types,
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
                    named_struct_field_types,
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
                    named_struct_field_types,
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
                    named_struct_field_types,
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
                    named_struct_field_types,
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
                    named_struct_field_types,
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
                    named_struct_field_types,
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
    if path_type == TYPE_ID_F64 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
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
                    named_struct_field_types,
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
                    named_struct_field_types,
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
                    named_struct_field_types,
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
                    "'%=' is unsupported for f64 local indexed assignment '{}[...].{}'",
                    collection_name, suffix
                ));
            }
        };
        builder.ins().call(
            runtime_call_refs.global_f64_array_store,
            &[collection_handle, field_hash, index_binding.value, value],
        );
        return Ok(());
    }
    Err(format!(
        "unsupported local indexed collection assignment type {} for '{}[...].{}'",
        path_type, collection_name, suffix
    ))
}

pub(crate) fn emit_indexed_collection_load(
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
    if resolved == TYPE_ID_F64 {
        let call = builder.ins().call(
            runtime_call_refs.global_f64_array_load,
            &[collection_hash, field_hash, index_binding.value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F64,
        });
    }
    Err(format!(
        "unsupported indexed collection load type {} for '{}[...].{}'",
        resolved, collection_path, suffix
    ))
}

pub(crate) fn emit_indexed_collection_assignment(
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
    if path_type == TYPE_ID_F64 {
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
                    "'%=' is unsupported for f64 indexed assignment '{}[...].{}'",
                    collection_path, suffix
                ))
            }
        };
        builder.ins().call(
            runtime_call_refs.global_f64_array_store,
            &[collection_hash, field_hash, index_binding.value, value],
        );
        return Ok(());
    }
    Err(format!(
        "unsupported indexed collection assignment type {} for '{}[...].{}'",
        path_type, collection_path, suffix
    ))
}

pub(crate) fn emit_global_load(
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
    if path_type == TYPE_ID_F64 {
        let call = builder
            .ins()
            .call(runtime_call_refs.global_f64_load, &[path_hash]);
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F64,
        });
    }
    Err(format!(
        "unsupported global path type {} for '{}'",
        path_type, path
    ))
}

pub(crate) fn emit_global_assignment(
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
    if path_type == TYPE_ID_F64 {
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
                    "'%=' is unsupported for f64 global path '{}'",
                    path
                ))
            }
        };
        let path_hash = builder
            .ins()
            .iconst(types::I32, i64::from(hash_global_path(path)));
        builder
            .ins()
            .call(runtime_call_refs.global_f64_store, &[path_hash, value]);
        return Ok(());
    }
    Err(format!(
        "unsupported global path type {} for '{}'",
        path_type, path
    ))
}

pub(crate) fn hash_global_path(path: &str) -> i32 {
    let mut hash: u32 = 2166136261;
    for byte in path.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16777619);
    }
    hash as i32
}

pub(crate) fn hash_string_literal(value: &str) -> i32 {
    hash_global_path(value)
}

pub(crate) fn emit_simple_condition(
    builder: &mut FunctionBuilder<'_>,
    condition: &SimpleCondition,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Value, String> {
    match condition {
        SimpleCondition::Comparison { lhs, op, rhs } => {
            let lhs = emit_simple_expression(
                builder,
                lhs,
                None,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            let rhs = emit_simple_expression(
                builder,
                rhs,
                None,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
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

            let floatcc = match op {
                ComparisonOp::Eq => FloatCC::Equal,
                ComparisonOp::Ne => FloatCC::NotEqual,
                ComparisonOp::Lt => FloatCC::LessThan,
                ComparisonOp::Le => FloatCC::LessThanOrEqual,
                ComparisonOp::Gt => FloatCC::GreaterThan,
                ComparisonOp::Ge => FloatCC::GreaterThanOrEqual,
            };

            if lhs.type_id == TYPE_ID_F64 || rhs.type_id == TYPE_ID_F64 {
                let (lhs_f64, rhs_f64) =
                    coerce_numeric_operands_to_f64(builder, lhs, rhs, '?', type_table)?;
                return Ok(builder.ins().fcmp(floatcc, lhs_f64, rhs_f64));
            }

            let (lhs_f32, rhs_f32) =
                coerce_numeric_operands_to_f32(builder, lhs, rhs, '?', type_table)?;
            Ok(builder.ins().fcmp(floatcc, lhs_f32, rhs_f32))
        }
        SimpleCondition::Expr(expression) => {
            let binding = emit_simple_expression(
                builder,
                expression,
                Some(TYPE_ID_BOOL),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
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
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
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
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
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
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
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
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
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
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
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

pub(crate) fn emit_bool_constant(builder: &mut FunctionBuilder<'_>, value: bool) -> Value {
    let literal = if value { 1 } else { 0 };
    let i32_value = builder.ins().iconst(types::I32, literal);
    builder.ins().icmp_imm(IntCC::NotEqual, i32_value, 0)
}

pub(crate) fn build_runtime_call_import_ids(
    module: &mut impl Module,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> Result<RuntimeCallImportIds, String> {
    Ok(RuntimeCallImportIds {
        call_i32_0: declare_i32_call_import(module, "stasis_jit_call_i32_0", 1)?,
        call_i32_1: declare_i32_call_import(module, "stasis_jit_call_i32_1", 2)?,
        call_i32_2: declare_i32_call_import(module, "stasis_jit_call_i32_2", 3)?,
        call_i32_3: declare_i32_call_import(module, "stasis_jit_call_i32_3", 4)?,
        call_i32_4: declare_i32_call_import(module, "stasis_jit_call_i32_4", 5)?,
        call_i32_5: declare_i32_call_import(module, "stasis_jit_call_i32_5", 6)?,
        call_i32_6: declare_i32_call_import(module, "stasis_jit_call_i32_6", 7)?,
        call_i32_7: declare_i32_call_import(module, "stasis_jit_call_i32_7", 8)?,
        call_i32_8: declare_i32_call_import(module, "stasis_jit_call_i32_8", 9)?,
        call_i32_f32_1: declare_i32_f32_call_import(module, "stasis_jit_call_i32_f32_1", 1)?,
        call_i32_f32_2: declare_i32_f32_call_import(module, "stasis_jit_call_i32_f32_2", 2)?,
        call_i32_f32_3: declare_i32_f32_call_import(module, "stasis_jit_call_i32_f32_3", 3)?,
        call_i32_f32_4: declare_i32_f32_call_import(module, "stasis_jit_call_i32_f32_4", 4)?,
        call_i32_f32_5: declare_i32_f32_call_import(module, "stasis_jit_call_i32_f32_5", 5)?,
        call_i32_f32_6: declare_i32_f32_call_import(module, "stasis_jit_call_i32_f32_6", 6)?,
        call_i32_f32_7: declare_i32_f32_call_import(module, "stasis_jit_call_i32_f32_7", 7)?,
        call_i32_f32_8: declare_i32_f32_call_import(module, "stasis_jit_call_i32_f32_8", 8)?,
        call_f32_0: declare_f32_call_import(module, "stasis_jit_call_f32_0", 1)?,
        call_f32_1: declare_f32_call_import(module, "stasis_jit_call_f32_1", 2)?,
        call_f32_2: declare_f32_call_import(module, "stasis_jit_call_f32_2", 3)?,
        call_f32_3: declare_f32_call_import(module, "stasis_jit_call_f32_3", 4)?,
        call_f32_4: declare_f32_call_import(module, "stasis_jit_call_f32_4", 5)?,
        call_f32_5: declare_f32_call_import(module, "stasis_jit_call_f32_5", 6)?,
        call_f32_6: declare_f32_call_import(module, "stasis_jit_call_f32_6", 7)?,
        call_f32_7: declare_f32_call_import(module, "stasis_jit_call_f32_7", 8)?,
        call_f32_8: declare_f32_call_import(module, "stasis_jit_call_f32_8", 9)?,
        call_f32_i32_1: declare_f32_i32_call_import(module, "stasis_jit_call_f32_i32_1", 2)?,
        print_i32: declare_void_call_import(module, "stasis_jit_print_i32", 1)?,
        print_string: declare_void_call_import(module, "stasis_jit_print_string", 1)?,
        lookup_code_ptr: declare_lookup_code_ptr_import(module, "stasis_jit_lookup_code_ptr")?,
        sin_fast: declare_direct_f32_unary_import(module, "stasis_jit_sin_fast")?,
        cos_fast: declare_direct_f32_unary_import(module, "stasis_jit_cos_fast")?,
        global_i32_load: declare_i32_call_import(module, "stasis_jit_global_i32_load", 1)?,
        global_i32_store: declare_void_call_import(module, "stasis_jit_global_i32_store", 2)?,
        global_f32_load: declare_f32_global_load_import(module, "stasis_jit_global_f32_load")?,
        global_f32_store: declare_f32_global_store_import(module, "stasis_jit_global_f32_store")?,
        global_f64_load: declare_f64_global_load_import(module, "stasis_jit_global_f64_load")?,
        global_f64_store: declare_f64_global_store_import(module, "stasis_jit_global_f64_store")?,
        global_i32_array_load: declare_i32_array_load_import(
            module,
            "stasis_jit_global_i32_array_load",
        )?,
        global_i32_array_store: declare_i32_array_store_import(
            module,
            "stasis_jit_global_i32_array_store",
        )?,
        global_i32_array_ptr: declare_i32_array_ptr_import(
            module,
            "stasis_jit_global_i32_array_ptr",
        )?,
        global_f32_array_load: declare_f32_array_load_import(
            module,
            "stasis_jit_global_f32_array_load",
        )?,
        global_f32_array_store: declare_f32_array_store_import(
            module,
            "stasis_jit_global_f32_array_store",
        )?,
        global_f32_array_ptr: declare_f32_array_ptr_import(
            module,
            "stasis_jit_global_f32_array_ptr",
        )?,
        global_f64_array_load: declare_f64_array_load_import(
            module,
            "stasis_jit_global_f64_array_load",
        )?,
        global_f64_array_store: declare_f64_array_store_import(
            module,
            "stasis_jit_global_f64_array_store",
        )?,
        global_f64_array_ptr: declare_f64_array_ptr_import(
            module,
            "stasis_jit_global_f64_array_ptr",
        )?,
        collection_i32_load: declare_i32_call_import(module, "stasis_jit_collection_i32_load", 2)?,
        collection_i32_store: declare_void_call_import(
            module,
            "stasis_jit_collection_i32_store",
            3,
        )?,
        extern_calls: declare_extern_call_imports(
            module,
            call_signatures,
            type_table,
            named_struct_field_types,
        )?,
    })
}

pub(crate) fn build_runtime_call_refs(
    module: &mut impl Module,
    imports: &RuntimeCallImportIds,
    func: &mut cranelift_codegen::ir::Function,
) -> RuntimeCallRefs {
    RuntimeCallRefs {
        call_i32_0: module.declare_func_in_func(imports.call_i32_0, func),
        call_i32_1: module.declare_func_in_func(imports.call_i32_1, func),
        call_i32_2: module.declare_func_in_func(imports.call_i32_2, func),
        call_i32_3: module.declare_func_in_func(imports.call_i32_3, func),
        call_i32_4: module.declare_func_in_func(imports.call_i32_4, func),
        call_i32_5: module.declare_func_in_func(imports.call_i32_5, func),
        call_i32_6: module.declare_func_in_func(imports.call_i32_6, func),
        call_i32_7: module.declare_func_in_func(imports.call_i32_7, func),
        call_i32_8: module.declare_func_in_func(imports.call_i32_8, func),
        call_i32_f32_1: module.declare_func_in_func(imports.call_i32_f32_1, func),
        call_i32_f32_2: module.declare_func_in_func(imports.call_i32_f32_2, func),
        call_i32_f32_3: module.declare_func_in_func(imports.call_i32_f32_3, func),
        call_i32_f32_4: module.declare_func_in_func(imports.call_i32_f32_4, func),
        call_i32_f32_5: module.declare_func_in_func(imports.call_i32_f32_5, func),
        call_i32_f32_6: module.declare_func_in_func(imports.call_i32_f32_6, func),
        call_i32_f32_7: module.declare_func_in_func(imports.call_i32_f32_7, func),
        call_i32_f32_8: module.declare_func_in_func(imports.call_i32_f32_8, func),
        call_f32_0: module.declare_func_in_func(imports.call_f32_0, func),
        call_f32_1: module.declare_func_in_func(imports.call_f32_1, func),
        call_f32_2: module.declare_func_in_func(imports.call_f32_2, func),
        call_f32_3: module.declare_func_in_func(imports.call_f32_3, func),
        call_f32_4: module.declare_func_in_func(imports.call_f32_4, func),
        call_f32_5: module.declare_func_in_func(imports.call_f32_5, func),
        call_f32_6: module.declare_func_in_func(imports.call_f32_6, func),
        call_f32_7: module.declare_func_in_func(imports.call_f32_7, func),
        call_f32_8: module.declare_func_in_func(imports.call_f32_8, func),
        call_f32_i32_1: module.declare_func_in_func(imports.call_f32_i32_1, func),
        print_i32: module.declare_func_in_func(imports.print_i32, func),
        print_string: module.declare_func_in_func(imports.print_string, func),
        lookup_code_ptr: module.declare_func_in_func(imports.lookup_code_ptr, func),
        sin_fast: module.declare_func_in_func(imports.sin_fast, func),
        cos_fast: module.declare_func_in_func(imports.cos_fast, func),
        global_i32_load: module.declare_func_in_func(imports.global_i32_load, func),
        global_i32_store: module.declare_func_in_func(imports.global_i32_store, func),
        global_f32_load: module.declare_func_in_func(imports.global_f32_load, func),
        global_f32_store: module.declare_func_in_func(imports.global_f32_store, func),
        global_f64_load: module.declare_func_in_func(imports.global_f64_load, func),
        global_f64_store: module.declare_func_in_func(imports.global_f64_store, func),
        global_i32_array_load: module.declare_func_in_func(imports.global_i32_array_load, func),
        global_i32_array_store: module.declare_func_in_func(imports.global_i32_array_store, func),
        global_i32_array_ptr: module.declare_func_in_func(imports.global_i32_array_ptr, func),
        global_f32_array_load: module.declare_func_in_func(imports.global_f32_array_load, func),
        global_f32_array_store: module.declare_func_in_func(imports.global_f32_array_store, func),
        global_f32_array_ptr: module.declare_func_in_func(imports.global_f32_array_ptr, func),
        global_f64_array_load: module.declare_func_in_func(imports.global_f64_array_load, func),
        global_f64_array_store: module.declare_func_in_func(imports.global_f64_array_store, func),
        global_f64_array_ptr: module.declare_func_in_func(imports.global_f64_array_ptr, func),
        collection_i32_load: module.declare_func_in_func(imports.collection_i32_load, func),
        collection_i32_store: module.declare_func_in_func(imports.collection_i32_store, func),
        extern_calls: imports
            .extern_calls
            .iter()
            .map(|(key, id)| (key.clone(), module.declare_func_in_func(*id, func)))
            .collect(),
    }
}
