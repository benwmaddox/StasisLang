//! Source-derived compiler analysis shared by all target backends.
#![allow(clippy::type_complexity)]

use crate::backend::runtime_exports::is_aot_runtime_export_symbol;
use crate::compiler::{FunctionId, FunctionMeta, SourceFile};
use crate::frontend::body_parser::*;
use crate::frontend::parser::{
    parse_top_level_extern_functions, parse_top_level_type_layout, ParsedExternFunctionDeclaration,
    ParsedField,
};
use crate::frontend::types::{
    TypeCategory, TypeId, TypeTable, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_F64, TYPE_ID_I32,
    TYPE_ID_U16, TYPE_ID_U32, TYPE_ID_U8, TYPE_ID_VOID,
};
use std::collections::{BTreeMap, HashMap};

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
    pub(crate) source_path: String,
    pub(crate) source_start: usize,
    pub(crate) source_end: usize,
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
pub(crate) type ExternSymbolAddressMap = BTreeMap<String, usize>;

#[derive(Debug, Clone)]
pub(crate) struct CompileAnalysisCache {
    #[allow(dead_code)]
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
    pub(crate) element_shape: String,
    pub(crate) fully_migratable: bool,
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
        map.entry(format!("{}.{}", function.module_alias, function.name))
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
            let mut signature = build_extern_call_signature(type_table, declaration.clone())?;
            signature.source_path = file.path.clone();
            signature.source_start = declaration.name_range.start;
            signature.source_end = declaration.name_range.end;
            out.push(signature);
        }
    }
    Ok(out)
}

pub(crate) fn resolve_extern_call_signatures_with(
    extern_signatures: &[ExternCallSignature],
    resolve_candidate: impl FnMut(&ExternCallSignature, &str) -> Option<usize>,
) -> Result<(Vec<ResolvedExternCallSignature>, ExternSymbolAddressMap), String> {
    resolve_extern_call_signatures_with_index(extern_signatures, resolve_candidate)
        .map_err(|(_, error)| error)
}

pub(crate) fn resolve_extern_call_signatures_with_index(
    extern_signatures: &[ExternCallSignature],
    mut resolve_candidate: impl FnMut(&ExternCallSignature, &str) -> Option<usize>,
) -> Result<(Vec<ResolvedExternCallSignature>, ExternSymbolAddressMap), (usize, String)> {
    let mut resolved = Vec::with_capacity(extern_signatures.len());
    let mut symbol_addresses: ExternSymbolAddressMap = BTreeMap::new();
    for (index, signature) in extern_signatures.iter().enumerate() {
        let mut selected: Option<(String, usize)> = None;
        for candidate in &signature.symbol_candidates {
            if let Some(address) = resolve_candidate(signature, candidate) {
                selected = Some((candidate.clone(), address));
                break;
            }
        }
        let Some((symbol, address)) = selected else {
            return Err((
                index,
                format!(
                    "unresolved extern call target '{}' with candidates {:?}",
                    signature.name, signature.symbol_candidates
                ),
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

pub(crate) fn resolve_preferred_extern_call_signatures(
    extern_signatures: &[ExternCallSignature],
) -> Result<(Vec<ResolvedExternCallSignature>, ExternSymbolAddressMap), String> {
    resolve_extern_call_signatures_with(extern_signatures, |signature, candidate| {
        if is_aot_runtime_export_symbol(candidate) || signature.symbol_candidates.len() == 1 {
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
    ) -> Result<
        (Vec<ResolvedExternCallSignature>, ExternSymbolAddressMap),
        String,
    >,
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
    let symbol_name = match declaration.symbol_name.as_str() {
        // Stasis strings are stable integer IDs. Keep legacy stdlib declarations on the
        // adapter that resolves the ID before entering the native renderer.
        "stasis_gfx_cache_text" => "stasis_jit_gfx_cache_text",
        symbol_name => symbol_name,
    };
    Ok(ExternCallSignature {
        name: declaration.name,
        symbol_candidates: build_extern_symbol_candidates(symbol_name, declaration.explicit_symbol),
        params,
        return_type,
        source_path: String::new(),
        source_start: 0,
        source_end: 0,
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
    type_table.is_i32_abi_compatible(type_id)
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
    if type_table.is_integer(type_id) {
        return true;
    }
    let Some(type_info) = type_table.type_info(type_id) else {
        return false;
    };
    matches!(type_info.category, TypeCategory::Named)
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
        TYPE_ID_I32
            | TYPE_ID_F32
            | TYPE_ID_F64
            | TYPE_ID_BOOL
            | TYPE_ID_U8
            | TYPE_ID_U16
            | TYPE_ID_U32
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
            let enum_type_id = type_table.resolve_or_intern(&parsed_enum.name)?;
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
    if type_table.is_integer(type_id) {
        let value = parse_integer_initializer(name, initializer, type_id, type_table)?;
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

fn parse_integer_initializer(
    name: &str,
    initializer: &str,
    type_id: TypeId,
    type_table: &TypeTable,
) -> Result<i32, String> {
    let Some(bits) = type_table.unsigned_integer_bits(type_id) else {
        return initializer
            .parse::<i32>()
            .map_err(|error| format!("invalid i32 initializer for constant '{}': {error}", name));
    };
    let value = initializer.parse::<u64>().map_err(|error| {
        format!(
            "invalid u{bits} initializer for constant '{}': {error}",
            name
        )
    })?;
    let maximum = if bits == 32 {
        u64::from(u32::MAX)
    } else {
        (1u64 << bits) - 1
    };
    if value > maximum {
        return Err(format!(
            "u{bits} initializer for constant '{}' is outside 0..={maximum}: {value}",
            name
        ));
    }
    Ok(value as u32 as i32)
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
            constant_values,
            visiting_structs,
        )?;
        let info = ForeachCollectionInfo {
            len,
            element_type: collection.element_type,
            field_types: collection.field_types,
            element_shape: collection.element_shape,
            fully_migratable: collection.fully_migratable,
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
    constant_values: &ConstantValueMap,
    visiting_structs: &mut Vec<String>,
) -> Result<ForeachCollectionInfo, String> {
    let (element_shape, fully_migratable) = collection_element_shape(
        element_type_name,
        struct_fields_by_name,
        constant_values,
        visiting_structs,
    )?;
    if let Some(type_id) = resolve_primitive_scalar_type_id(element_type_name, type_table) {
        return Ok(ForeachCollectionInfo {
            len: 0,
            element_type: Some(type_id),
            field_types: BTreeMap::new(),
            element_shape,
            fully_migratable,
        });
    }
    if !struct_fields_by_name.contains_key(element_type_name) {
        let element_type = type_table.resolve_or_intern(element_type_name)?;
        return Ok(ForeachCollectionInfo {
            len: 0,
            element_type: Some(element_type),
            field_types: BTreeMap::new(),
            element_shape,
            fully_migratable,
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
        element_shape,
        fully_migratable,
    })
}

fn collection_element_shape(
    type_name: &str,
    struct_fields_by_name: &BTreeMap<String, Vec<ParsedField>>,
    constant_values: &ConstantValueMap,
    visiting_structs: &mut Vec<String>,
) -> Result<(String, bool), String> {
    let type_name = type_name.trim();
    if matches!(
        type_name,
        "i32" | "f32" | "f64" | "bool" | "u8" | "u16" | "u32" | "ascii" | "utf8"
    ) {
        return Ok((type_name.to_string(), true));
    }
    let Some(fields) = struct_fields_by_name.get(type_name) else {
        return Ok((type_name.to_string(), false));
    };
    if visiting_structs
        .iter()
        .any(|existing| existing == type_name)
    {
        return Err(format!(
            "recursive collection element shape is unsupported for '{type_name}'"
        ));
    }
    visiting_structs.push(type_name.to_string());
    let mut shape = String::from("{");
    let mut fully_migratable = true;
    for field in fields {
        if shape.len() > 1 {
            shape.push(',');
        }
        shape.push_str(&field.name);
        shape.push(':');
        let field_type = field.type_name.trim();
        if let Some((element, extent)) = parse_array_type_parts(field_type) {
            let (element_shape, _) = collection_element_shape(
                element,
                struct_fields_by_name,
                constant_values,
                visiting_structs,
            )?;
            let extent = resolve_fixed_array_extent(extent, constant_values).ok_or_else(|| {
                format!(
                    "collection element field '{}.{}' has unresolved fixed extent '{}'",
                    type_name, field.name, extent
                )
            })?;
            shape.push_str(&element_shape);
            shape.push('[');
            shape.push_str(&extent.to_string());
            shape.push(']');
            fully_migratable = false;
        } else {
            let (field_shape, field_migratable) = collection_element_shape(
                field_type,
                struct_fields_by_name,
                constant_values,
                visiting_structs,
            )?;
            shape.push_str(&field_shape);
            fully_migratable &= field_migratable;
        }
    }
    shape.push('}');
    visiting_structs.pop();
    Ok((shape, fully_migratable))
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
                if extent_text.bytes().all(|byte| byte.is_ascii_digit()) {
                    let type_id = type_table.resolve_or_intern(field.type_name.trim())?;
                    out.insert(field_path, type_id);
                }
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
pub(crate) fn is_struct_view_type(
    type_id: TypeId,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> bool {
    named_struct_field_types.contains_key(&type_id)
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
// String literals use the same stable FNV-1a path hash as runtime global paths. Keep this
// helper private here so source analysis remains independent of lowering/runtime emission.
fn hash_string_literal(value: &str) -> i32 {
    let mut hash: u32 = 2166136261;
    for byte in value.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16777619);
    }
    hash as i32
}
