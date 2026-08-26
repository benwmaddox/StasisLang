use crate::backend::runtime_exports::is_aot_runtime_export_symbol;
use crate::compiler::{FunctionId, FunctionMeta, SourceFile};
use crate::data_flow::{FunctionDataFlowSummary, ParameterStorageKind};
use crate::frontend::body_parser::*;
use crate::frontend::parser::{
    parse_top_level_extern_functions, parse_top_level_type_layout, ParsedExternFunctionDeclaration,
    ParsedField,
};
use crate::frontend::types::{
    TypeCategory, TypeId, TypeTable, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_F64, TYPE_ID_I32,
    TYPE_ID_U16, TYPE_ID_U32, TYPE_ID_U8, TYPE_ID_VOID,
};
use crate::ir::hir::{
    eval_const_i64, AssignOp, AssignTarget, ComparisonOp, ConversionKind, DebugStatement,
    FunctionHIR, SimpleCondition, SimpleExpr, SimpleStmt,
};
use cranelift_codegen::ir::{
    condcodes::{FloatCC, IntCC},
    immediates::{Ieee32, Ieee64},
    types, AbiParam, Block, FuncRef, InstBuilder, MemFlags, TrapCode, Value,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{FuncId, Linkage, Module};
use std::collections::{BTreeMap, BTreeSet, HashMap};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
static RUNTIME_HELPER_TRAMPOLINES_DEFINED: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn reset_runtime_helper_trampoline_count_for_test() {
    RUNTIME_HELPER_TRAMPOLINES_DEFINED.store(0, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn runtime_helper_trampoline_count_for_test() -> usize {
    RUNTIME_HELPER_TRAMPOLINES_DEFINED.load(Ordering::SeqCst)
}

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
pub(crate) type ForeachBindingMap = BTreeMap<String, ForeachBinding>;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeachBinding {
    pub(crate) collection_handle: ForeachCollectionHandle,
    pub(crate) index_var: Variable,
    pub(crate) len: i32,
    pub(crate) element_type: Option<TypeId>,
    pub(crate) struct_type_id: Option<TypeId>,
    pub(crate) field_types: BTreeMap<String, TypeId>,
    pub(crate) u8_array_base_ptrs: BTreeMap<String, Value>,
    pub(crate) u16_array_base_ptrs: BTreeMap<String, Value>,
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

fn normalize_unsigned_value(
    builder: &mut FunctionBuilder<'_>,
    value: Value,
    type_id: TypeId,
    type_table: &TypeTable,
) -> Value {
    match type_table.unsigned_integer_bits(type_id) {
        Some(8) => builder.ins().band_imm(value, 0xff),
        Some(16) => builder.ins().band_imm(value, 0xffff),
        _ => value,
    }
}

fn integer_binary_result_type(
    expected_type: Option<TypeId>,
    lhs: TypeId,
    rhs: TypeId,
    type_table: &TypeTable,
) -> TypeId {
    if let Some(expected) = expected_type.filter(|id| type_table.is_integer(*id)) {
        return expected;
    }
    if lhs == rhs && type_table.is_integer(lhs) {
        lhs
    } else {
        TYPE_ID_I32
    }
}

fn unambiguous_call_params(
    target: &str,
    arg_count: usize,
    call_signatures: &CallSignatureMap,
) -> Option<Vec<TypeId>> {
    let mut candidates = call_signatures
        .get(target)?
        .iter()
        .filter(|signature| signature.params.len() == arg_count);
    let first = candidates.next()?.params.clone();
    candidates
        .all(|candidate| candidate.params == first)
        .then_some(first)
}

fn emit_integer_assignment_value(
    builder: &mut FunctionBuilder<'_>,
    lhs: Option<Value>,
    rhs: Value,
    op: AssignOp,
    type_table: &TypeTable,
    type_id: TypeId,
) -> Value {
    let unsigned = type_table.unsigned_integer_bits(type_id).is_some();
    let value = match op {
        AssignOp::Set => rhs,
        AssignOp::Add => builder
            .ins()
            .iadd(lhs.expect("compound assignment lhs"), rhs),
        AssignOp::Sub => builder
            .ins()
            .isub(lhs.expect("compound assignment lhs"), rhs),
        AssignOp::Mul => builder
            .ins()
            .imul(lhs.expect("compound assignment lhs"), rhs),
        AssignOp::Div if unsigned => builder
            .ins()
            .udiv(lhs.expect("compound assignment lhs"), rhs),
        AssignOp::Mod if unsigned => builder
            .ins()
            .urem(lhs.expect("compound assignment lhs"), rhs),
        AssignOp::Div => builder
            .ins()
            .sdiv(lhs.expect("compound assignment lhs"), rhs),
        AssignOp::Mod => builder
            .ins()
            .srem(lhs.expect("compound assignment lhs"), rhs),
    };
    normalize_unsigned_value(builder, value, type_id, type_table)
}

pub(crate) fn are_assignment_types_compatible(
    target_type: TypeId,
    expression_type: TypeId,
    type_table: &TypeTable,
) -> bool {
    type_table.assignment_types_are_compatible(target_type, expression_type)
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
    pub(crate) print_i32: FuncId,
    pub(crate) print_string: FuncId,
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
    pub(crate) debug_frame_enter: Option<FuncId>,
    pub(crate) debug_frame_leave: Option<FuncId>,
    pub(crate) debug_statement: Option<FuncId>,
    pub(crate) debug_values_begin: Option<FuncId>,
    pub(crate) debug_value_i64: Option<FuncId>,
    pub(crate) debug_value_f64: Option<FuncId>,
    pub(crate) profile_frame_enter: Option<FuncId>,
    pub(crate) profile_frame_leave: Option<FuncId>,
    pub(crate) extern_calls: BTreeMap<ExternImportKey, FuncId>,
}

pub(crate) struct RuntimeCallRefs {
    pub(crate) print_i32: FuncRef,
    pub(crate) print_string: FuncRef,
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
    pub(crate) debug: Option<DebugRuntimeRefs>,
    pub(crate) profile: Option<ProfileRuntimeRefs>,
    pub(crate) extern_calls: BTreeMap<ExternImportKey, FuncRef>,
    pub(crate) direct_storage: Option<DirectStorageRefs>,
}

#[derive(Debug, Clone)]
pub(crate) enum DirectStorageBinding {
    Absolute(usize),
    Symbol(String),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DirectStorageBindings {
    pub(crate) scalars: BTreeMap<String, DirectStorageBinding>,
    pub(crate) arrays: BTreeMap<(String, String), DirectArrayStorageBinding>,
}

#[derive(Debug, Clone)]
pub(crate) struct DirectArrayStorageBinding {
    pub(crate) slot: DirectStorageBinding,
    pub(crate) storage_bytes: u8,
    pub(crate) static_len: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum DirectStorageRef {
    Absolute(usize),
    Symbol(cranelift_codegen::ir::GlobalValue),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DirectStorageRefs {
    pub(crate) scalars: BTreeMap<String, DirectStorageRef>,
    pub(crate) arrays: BTreeMap<(String, String), DirectArrayStorageRef>,
    pub(crate) arrays_by_hash: BTreeMap<(i32, i32), DirectArrayStorageRef>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DirectArrayStorageRef {
    pub(crate) slot: DirectStorageRef,
    pub(crate) storage_bytes: u8,
    pub(crate) static_len: Option<usize>,
}

pub(crate) struct DirectCallMode<'a> {
    pub(crate) module: &'a mut dyn Module,
    pub(crate) self_function_id: FunctionId,
    pub(crate) self_clif_func_id: FuncId,
    pub(crate) imported_function_ids: HashMap<FunctionId, FuncId>,
    pub(crate) symbol_prefix: &'static str,
    pub(crate) force_far_nonself_calls: bool,
}

pub(crate) enum InternalCallMode<'a> {
    Direct(DirectCallMode<'a>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SharedCompileBackendMode {
    JitDirect,
    AotDirect,
}

#[derive(Clone, Copy)]
pub(crate) enum RuntimeHelperLinkage<'a> {
    Imported,
    LocalTrampolines(&'a BTreeMap<String, usize>),
}

fn emit_direct_call_for_signature(
    builder: &mut FunctionBuilder<'_>,
    mode: &mut DirectCallMode<'_>,
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
        let symbol = format!("{}{function_id}", mode.symbol_prefix);
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
    if mode.force_far_nonself_calls && function_id != mode.self_function_id {
        builder.func.dfg.ext_funcs[func_ref].colocated = false;
    }
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
    pub(crate) storage_kind: StructViewStorageKind,
    pub(crate) known_collection_hash: Option<i32>,
    pub(crate) bounds_proven: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StructViewStorageKind {
    Dynamic,
    Aos,
    Soa,
}

#[derive(Clone, Copy)]
pub(crate) struct LocalBinding {
    pub(crate) var: Variable,
    pub(crate) type_id: TypeId,
    pub(crate) struct_view: Option<StructViewBinding>,
    pub(crate) proven_index_upper: Option<usize>,
}

#[derive(Clone, Copy)]
pub(crate) struct StructViewBinding {
    pub(crate) index_var: Variable,
    pub(crate) len_var: Variable,
    pub(crate) storage_kind: StructViewStorageKind,
    pub(crate) known_collection_hash: Option<i32>,
    pub(crate) bounds_proven: bool,
}

pub(crate) fn compile_function_with_module<M, T, BeforeStatement, OnFunctionBuilt, Finalize>(
    mut module: M,
    meta: &FunctionMeta,
    hir: &FunctionHIR,
    symbol: &str,
    runtime_helper_linkage: RuntimeHelperLinkage<'_>,
    backend_mode: SharedCompileBackendMode,
    call_signatures: &CallSignatureMap,
    type_table: &mut TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    data_flow_summary: Option<&FunctionDataFlowSummary>,
    direct_storage: Option<&DirectStorageBindings>,
    defined_runtime_helper_trampolines: Option<&mut BTreeSet<String>>,
    debug_instrumentation: bool,
    profile_instrumentation: bool,
    mut before_statement: BeforeStatement,
    on_function_built: OnFunctionBuilt,
    finalize: Finalize,
) -> Result<T, String>
where
    M: Module,
    BeforeStatement: FnMut(&SimpleStmt) -> Result<(), String>,
    OnFunctionBuilt: FnOnce(&FunctionMeta, &cranelift_codegen::ir::Function),
    Finalize: FnOnce(M, FuncId, cranelift_codegen::Context) -> Result<T, String>,
{
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
                    "unsupported return type id {} for function {}",
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
        .map_err(|error| format!("failed to declare function {symbol}: {error}"))?;
    let referenced_call_targets = collect_call_targets_from_hir(hir);
    let has_struct_view_param = meta
        .params
        .iter()
        .any(|type_id| is_struct_view_type(*type_id, named_struct_field_types));
    let uses_runtime_storage = backend_mode == SharedCompileBackendMode::JitDirect
        || !global_path_types.is_empty()
        || has_struct_view_param;
    let uses_collection_runtime = backend_mode == SharedCompileBackendMode::JitDirect
        || !collection_infos.is_empty()
        || has_struct_view_param;
    let runtime_call_imports = match backend_mode {
        SharedCompileBackendMode::JitDirect | SharedCompileBackendMode::AotDirect => {
            build_direct_runtime_call_import_ids(
                &mut module,
                function_id,
                runtime_helper_linkage,
                uses_runtime_storage,
                uses_collection_runtime,
                &referenced_call_targets,
                call_signatures,
                type_table,
                named_struct_field_types,
                debug_instrumentation,
                profile_instrumentation,
            )?
        }
    };

    let mut function_builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut function_builder_context);
        let runtime_call_refs = build_runtime_call_refs(
            &mut module,
            &runtime_call_imports,
            builder.func,
            direct_storage,
        )?;
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
                    (
                        base_var,
                        Some(StructViewBinding {
                            index_var,
                            len_var,
                            storage_kind: data_flow_summary
                                .and_then(|summary| summary.parameter_storage_kinds.get(index))
                                .map_or(StructViewStorageKind::Dynamic, |kind| match kind {
                                    ParameterStorageKind::Dynamic => StructViewStorageKind::Dynamic,
                                    ParameterStorageKind::Aos => StructViewStorageKind::Aos,
                                    ParameterStorageKind::Soa => StructViewStorageKind::Soa,
                                }),
                            known_collection_hash: None,
                            bounds_proven: false,
                        }),
                    )
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
                    proven_index_upper: None,
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
        let symbol_prefix = match backend_mode {
            SharedCompileBackendMode::JitDirect => "jit_fn_",
            SharedCompileBackendMode::AotDirect => "aot_fn_",
        };
        // Cranelift's JIT allocator maps functions independently. AArch64's BL range
        // therefore cannot be assumed between functions, even in one JITModule.
        let force_far_nonself_calls = backend_mode == SharedCompileBackendMode::JitDirect
            && matches!(
                module.isa().triple().architecture,
                target_lexicon::Architecture::Aarch64(_)
            );
        let mut internal_calls = InternalCallMode::Direct(DirectCallMode {
            module: &mut module,
            self_function_id: meta.id,
            self_clif_func_id: function_id,
            imported_function_ids: HashMap::new(),
            symbol_prefix,
            force_far_nonself_calls,
        });
        if let Some(debug) = runtime_call_refs.debug.as_ref() {
            if hir.debug_statements.len() != hir.statements.len() {
                return Err(format!(
                    "debug statement metadata mismatch for function '{}'",
                    meta.name
                ));
            }
            emit_debug_frame_boundary(&mut builder, debug.frame_enter, meta.id);
        }
        if let Some(profile) = runtime_call_refs.profile.as_ref() {
            emit_function_frame_boundary(&mut builder, profile.frame_enter, meta.id);
        }
        let mut terminated = false;
        for (index, statement) in hir.statements.iter().enumerate() {
            if terminated {
                break;
            }
            before_statement(statement)?;
            terminated = emit_simple_statements(
                &mut builder,
                std::slice::from_ref(statement),
                runtime_call_refs
                    .debug
                    .as_ref()
                    .map(|_| std::slice::from_ref(&hir.debug_statements[index])),
                runtime_call_refs.debug.as_ref(),
                meta.id,
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
        }
        if !terminated {
            if meta.return_type == TYPE_ID_VOID {
                if let Some(debug) = runtime_call_refs.debug.as_ref() {
                    emit_debug_frame_boundary(&mut builder, debug.frame_leave, meta.id);
                }
                if let Some(profile) = runtime_call_refs.profile.as_ref() {
                    emit_function_frame_boundary(&mut builder, profile.frame_leave, meta.id);
                }
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

    define_referenced_runtime_helper_trampolines(
        &mut module,
        runtime_helper_linkage,
        &context.func,
        defined_runtime_helper_trampolines,
    )?;

    on_function_built(meta, &context.func);
    finalize(module, function_id, context)
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

pub(crate) fn declare_runtime_helper(
    module: &mut impl Module,
    symbol: &str,
    signature: cranelift_codegen::ir::Signature,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    match linkage {
        RuntimeHelperLinkage::Imported => module
            .declare_function(symbol, Linkage::Import, &signature)
            .map_err(|error| format!("failed to declare JIT import {symbol}: {error}")),
        RuntimeHelperLinkage::LocalTrampolines(addresses) => {
            if !addresses.contains_key(symbol) {
                return Err(format!("missing runtime helper address for {symbol}"));
            }
            let local_symbol = format!("__stasis_runtime_helper_{symbol}");
            // Preemptible deliberately marks the helper as non-colocated. On AArch64,
            // Cranelift then emits an address load plus an indirect call instead of a
            // range-limited BL relocation. The helper is still defined in this private
            // JIT module; the linkage only controls the generated call sequence.
            module
                .declare_function(&local_symbol, Linkage::Preemptible, &signature)
                .map_err(|error| {
                    format!("failed to declare runtime helper trampoline {symbol}: {error}")
                })
        }
    }
}

fn define_referenced_runtime_helper_trampolines(
    module: &mut impl Module,
    linkage: RuntimeHelperLinkage<'_>,
    function: &cranelift_codegen::ir::Function,
    mut defined: Option<&mut BTreeSet<String>>,
) -> Result<(), String> {
    let RuntimeHelperLinkage::LocalTrampolines(addresses) = linkage else {
        return Ok(());
    };
    let mut referenced = BTreeSet::new();
    for block in function.layout.blocks() {
        for instruction in function.layout.block_insts(block) {
            let cranelift_codegen::ir::InstructionData::Call { func_ref, .. } =
                &function.dfg.insts[instruction]
            else {
                continue;
            };
            let cranelift_codegen::ir::ExternalName::User(name_ref) =
                &function.dfg.ext_funcs[*func_ref].name
            else {
                continue;
            };
            let name = &function.params.user_named_funcs()[*name_ref];
            if name.namespace == 0 {
                referenced.insert(FuncId::from_u32(name.index));
            }
        }
    }

    let mut trampolines = Vec::new();
    for func_id in referenced {
        let declaration = module.declarations().get_function_decl(func_id);
        let Some(symbol) = declaration
            .name
            .as_deref()
            .and_then(|name| name.strip_prefix("__stasis_runtime_helper_"))
        else {
            continue;
        };
        let address = addresses
            .get(symbol)
            .copied()
            .ok_or_else(|| format!("missing runtime helper address for {symbol}"))?;
        trampolines.push((
            func_id,
            symbol.to_string(),
            declaration.signature.clone(),
            address,
        ));
    }
    for (func_id, symbol, signature, address) in trampolines {
        if defined
            .as_deref_mut()
            .is_some_and(|defined| !defined.insert(symbol.clone()))
        {
            continue;
        }
        define_runtime_helper_trampoline(module, func_id, &symbol, signature, address)?;
    }
    Ok(())
}

fn define_runtime_helper_trampoline(
    module: &mut impl Module,
    func_id: FuncId,
    symbol: &str,
    signature: cranelift_codegen::ir::Signature,
    address: usize,
) -> Result<(), String> {
    let pointer_type = module.target_config().pointer_type();
    let pointer_bits = pointer_type.bits();
    if pointer_bits < usize::BITS && address > u32::MAX as usize {
        return Err(format!(
            "runtime helper address for {symbol} does not fit target pointer type"
        ));
    }
    let mut context = module.make_context();
    context.func.signature = signature.clone();
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        for param in &signature.params {
            builder.append_block_param(entry, param.value_type);
        }
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let args = builder.block_params(entry).to_vec();
        let address_value = builder.ins().iconst(pointer_type, address as i64);
        let signature_ref = builder.func.import_signature(signature);
        let call = builder
            .ins()
            .call_indirect(signature_ref, address_value, &args);
        let results = builder.inst_results(call).to_vec();
        builder.ins().return_(&results);
        builder.finalize();
    }
    module
        .define_function(func_id, &mut context)
        .map_err(|error| format!("failed to define runtime helper trampoline {symbol}: {error}"))?;
    module.clear_context(&mut context);
    #[cfg(test)]
    RUNTIME_HELPER_TRAMPOLINES_DEFINED.fetch_add(1, Ordering::SeqCst);
    Ok(())
}
pub(crate) fn declare_i32_call_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
    param_count: usize,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    for _ in 0..param_count {
        signature.params.push(AbiParam::new(types::I32));
    }
    signature.returns.push(AbiParam::new(types::I32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_direct_f32_unary_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::F32));
    signature.returns.push(AbiParam::new(types::F32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_void_call_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
    param_count: usize,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    for _ in 0..param_count {
        signature.params.push(AbiParam::new(types::I32));
    }
    declare_runtime_helper(module, symbol, signature, linkage)
}

fn declare_debug_value_i64_import(
    module: &mut impl Module,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I64));
    declare_runtime_helper(module, "stasis_jit_debug_value_i64", signature, linkage)
}

fn declare_debug_value_f64_import(
    module: &mut impl Module,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F64));
    declare_runtime_helper(module, "stasis_jit_debug_value_f64", signature, linkage)
}

pub(crate) fn declare_f32_global_load_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f32_global_store_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f64_global_load_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f64_global_store_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_i32_array_load_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_i32_array_store_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_i32_array_ptr_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f32_array_load_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f32_array_store_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f32_array_ptr_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f64_array_load_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f64_array_store_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f64_array_ptr_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_extern_call_imports(
    module: &mut impl Module,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
    linkage: RuntimeHelperLinkage<'_>,
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
            let func_id = declare_runtime_helper(module, symbol, clif_signature, linkage).map_err(
                |error| {
                    format!(
                        "failed to declare extern import '{}' with params {:?} return {}: {}",
                        symbol, signature.params, signature.return_type, error
                    )
                },
            )?;
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
    let initial_value = normalize_unsigned_value(builder, initial_value, type_id, type_table);
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
        TYPE_ID_BOOL | TYPE_ID_U8 | TYPE_ID_U16 | TYPE_ID_U32 => Ok(types::I32),
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct LoopControlContext {
    pub(crate) continue_block: Block,
}

#[derive(Clone, Copy)]
pub(crate) struct DebugRuntimeRefs {
    pub(crate) frame_enter: FuncRef,
    pub(crate) frame_leave: FuncRef,
    pub(crate) statement: FuncRef,
    pub(crate) values_begin: FuncRef,
    pub(crate) value_i64: FuncRef,
    pub(crate) value_f64: FuncRef,
}

#[derive(Clone, Copy)]
pub(crate) struct ProfileRuntimeRefs {
    pub(crate) frame_enter: FuncRef,
    pub(crate) frame_leave: FuncRef,
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

pub(crate) fn emit_internal_call_for_signature(
    builder: &mut FunctionBuilder<'_>,
    _runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    signature: &CallSignature,
    arg_values: &[Value],
    _arg_types: &[TypeId],
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
    _target: &str,
) -> Result<Option<Value>, String> {
    if signature.extern_symbol.is_some() {
        return Err("internal direct call requested for extern signature".to_string());
    }
    let InternalCallMode::Direct(mode) = internal_calls;
    emit_direct_call_for_signature(
        builder,
        mode,
        signature,
        arg_values,
        type_table,
        named_struct_field_types,
    )
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
    let source_bounds_proven =
        static_index_bounds_proven(source_index, source_info.len as usize, values_by_name);
    let target_bounds_proven =
        static_index_bounds_proven(target_index, target_info.len as usize, values_by_name);

    for field_name in target_info.field_types.keys() {
        let source_value = emit_indexed_collection_load(
            builder,
            runtime_call_refs,
            type_table,
            source_collection,
            source_info,
            field_name,
            source_index_binding,
            source_bounds_proven,
        )?;
        emit_indexed_collection_assignment(
            builder,
            runtime_call_refs,
            type_table,
            target_collection,
            target_info,
            field_name,
            target_index_binding,
            target_bounds_proven,
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
    let source_bounds_proven =
        static_index_bounds_proven(source_index, source_info.len as usize, values_by_name);
    for (field_name, field_type) in &source_info.field_types {
        let source_value = emit_indexed_collection_load(
            builder,
            runtime_call_refs,
            type_table,
            source_collection,
            source_info,
            field_name,
            source_index_binding,
            source_bounds_proven,
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
    let target_bounds_proven =
        static_index_bounds_proven(target_index, target_info.len as usize, values_by_name);
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
            target_bounds_proven,
            AssignOp::Set,
            source_value,
        )?;
    }
    Ok(true)
}

pub(crate) fn debug_variable_slot(name: &str) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn emit_function_frame_boundary(
    builder: &mut FunctionBuilder<'_>,
    function: FuncRef,
    function_id: FunctionId,
) {
    let function_id = builder.ins().iconst(types::I32, i64::from(function_id));
    builder.ins().call(function, &[function_id]);
}

fn emit_debug_frame_boundary(
    builder: &mut FunctionBuilder<'_>,
    function: FuncRef,
    function_id: FunctionId,
) {
    emit_function_frame_boundary(builder, function, function_id);
}

fn emit_debug_statement(
    builder: &mut FunctionBuilder<'_>,
    debug: &DebugRuntimeRefs,
    function_id: FunctionId,
    site_id: u32,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    foreach_bindings: &ForeachBindingMap,
) -> Result<(), String> {
    builder.ins().call(debug.values_begin, &[]);
    let mut slots = HashMap::<u32, &str>::new();
    for (name, binding) in values_by_name {
        let slot = debug_variable_slot(name);
        if let Some(existing) = slots.insert(slot, name) {
            return Err(format!(
                "debug variable slot collision between '{existing}' and '{name}'"
            ));
        }
        let value = builder.use_var(binding.var);
        emit_debug_value(
            builder,
            debug,
            name,
            slot,
            binding.type_id,
            value,
            type_table,
        )?;
    }
    for (name, binding) in foreach_bindings {
        if binding.element_type.is_none() {
            continue;
        }
        let slot = debug_variable_slot(name);
        if let Some(existing) = slots.insert(slot, name) {
            return Err(format!(
                "debug variable slot collision between '{existing}' and '{name}'"
            ));
        }
        let value = emit_foreach_binding_load(builder, runtime_call_refs, type_table, binding, "")?;
        emit_debug_value(
            builder,
            debug,
            name,
            slot,
            value.type_id,
            value.value,
            type_table,
        )?;
    }
    let function_id = builder.ins().iconst(types::I32, i64::from(function_id));
    let site_id = builder.ins().iconst(types::I32, i64::from(site_id as i32));
    builder.ins().call(debug.statement, &[function_id, site_id]);
    Ok(())
}

fn emit_debug_value(
    builder: &mut FunctionBuilder<'_>,
    debug: &DebugRuntimeRefs,
    name: &str,
    slot: u32,
    type_id: TypeId,
    value: Value,
    type_table: &TypeTable,
) -> Result<(), String> {
    let slot_value = builder.ins().iconst(types::I32, i64::from(slot as i32));
    let type_value = builder.ins().iconst(types::I32, i64::from(type_id));
    match type_id {
        TYPE_ID_F32 => {
            let value = builder.ins().fpromote(types::F64, value);
            builder
                .ins()
                .call(debug.value_f64, &[slot_value, type_value, value]);
        }
        TYPE_ID_F64 => {
            builder
                .ins()
                .call(debug.value_f64, &[slot_value, type_value, value]);
        }
        _ => {
            let clif_type = clif_type_for_type_id(type_id, type_table)?;
            if clif_type != types::I32 {
                return Err(format!(
                    "unsupported debug value type id {type_id} for '{name}'"
                ));
            }
            let value = if type_id == TYPE_ID_I32 {
                builder.ins().sextend(types::I64, value)
            } else {
                builder.ins().uextend(types::I64, value)
            };
            builder
                .ins()
                .call(debug.value_i64, &[slot_value, type_value, value]);
        }
    }
    Ok(())
}

pub(crate) fn emit_simple_statements(
    builder: &mut FunctionBuilder<'_>,
    statements: &[SimpleStmt],
    debug_statements: Option<&[DebugStatement]>,
    debug_refs: Option<&DebugRuntimeRefs>,
    function_id: FunctionId,
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
    if debug_refs.is_some() && debug_statements.is_none_or(|debug| debug.len() != statements.len())
    {
        return Err("debug statement metadata does not match lowered statements".to_string());
    }
    for (index, statement) in statements.iter().enumerate() {
        if let Some(debug_refs) = debug_refs {
            let debug = &debug_statements.expect("debug metadata was validated")[index];
            emit_debug_statement(
                builder,
                debug_refs,
                function_id,
                debug.source_offset,
                values_by_name,
                runtime_call_refs,
                type_table,
                foreach_bindings,
            )?;
        }
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
                    let mut view_bounds_proven = struct_view.bounds_proven;
                    // A fixed global struct-array view with an arbitrary index must still fail
                    // fatally, but the check belongs to creation of the alias rather than every
                    // field access through that alias. Once validated, all loads and stores from
                    // the same {base,index,len} view may reuse the fact.
                    if !view_bounds_proven
                        && struct_view.storage_kind == StructViewStorageKind::Soa
                        && struct_view.known_collection_hash.is_some()
                    {
                        emit_array_bounds_trap(builder, struct_view.index, struct_view.len);
                        view_bounds_proven = true;
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
                            struct_view: Some(StructViewBinding {
                                index_var,
                                len_var,
                                storage_kind: struct_view.storage_kind,
                                known_collection_hash: struct_view.known_collection_hash,
                                bounds_proven: view_bounds_proven,
                            }),
                            proven_index_upper: None,
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
                        let expected = type_table
                            .type_info(declared_type_id)
                            .map_or_else(|| declared_type_id.to_string(), |info| info.name.clone());
                        let found = type_table
                            .type_info(binding.type_id)
                            .map_or_else(|| binding.type_id.to_string(), |info| info.name.clone());
                        return Err(format!(
                            "let binding '{name}' expected {expected} expression but found {found}"
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
                        proven_index_upper: None,
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
                                let unsigned =
                                    type_table.unsigned_integer_bits(local.type_id).is_some();
                                let value = match op {
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
                                    AssignOp::Div if unsigned => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().udiv(lhs, rhs.value)
                                    }
                                    AssignOp::Mod if unsigned => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().urem(lhs, rhs.value)
                                    }
                                    AssignOp::Div => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().sdiv(lhs, rhs.value)
                                    }
                                    AssignOp::Mod => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().srem(lhs, rhs.value)
                                    }
                                };
                                normalize_unsigned_value(builder, value, local.type_id, type_table)
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
                                        let lhs = if *op == AssignOp::Set {
                                            None
                                        } else {
                                            let call = builder.ins().call(
                                                runtime_call_refs.global_i32_load,
                                                &[path_hash],
                                            );
                                            Some(builder.inst_results(call)[0])
                                        };
                                        let value = emit_integer_assignment_value(
                                            builder, lhs, rhs.value, *op, type_table, field_type,
                                        );
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
                        let bounds_proven = static_index_bounds_proven(
                            index,
                            collection_info.len as usize,
                            values_by_name,
                        );
                        emit_indexed_collection_assignment(
                            builder,
                            runtime_call_refs,
                            type_table,
                            collection_path,
                            collection_info,
                            suffix,
                            index_binding,
                            bounds_proven,
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
                        let bounds_proven = static_index_bounds_proven(
                            index,
                            collection_info.len as usize,
                            values_by_name,
                        );
                        emit_indexed_collection_assignment(
                            builder,
                            runtime_call_refs,
                            type_table,
                            collection_path,
                            collection_info,
                            suffix,
                            index_binding,
                            bounds_proven,
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
                    let expected_params =
                        unambiguous_call_params(target, args.len(), call_signatures);
                    for (arg_index, arg) in args.iter().enumerate() {
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
                            expected_params
                                .as_ref()
                                .and_then(|params| params.get(arg_index).copied()),
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
                            let _ = emit_internal_call_for_signature(
                                builder,
                                runtime_call_refs,
                                internal_calls,
                                signature,
                                &arg_values,
                                &arg_types,
                                type_table,
                                named_struct_field_types,
                                target,
                            )?;
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
                    let expected = type_table.type_info(expected_return_type).map_or_else(
                        || expected_return_type.to_string(),
                        |info| info.name.clone(),
                    );
                    let found = type_table
                        .type_info(binding.type_id)
                        .map_or_else(|| binding.type_id.to_string(), |info| info.name.clone());
                    return Err(format!(
                        "return expression expected {expected} but found {found}"
                    ));
                }
                let value = normalize_unsigned_value(
                    builder,
                    binding.value,
                    expected_return_type,
                    type_table,
                );
                if let Some(debug) = debug_refs {
                    emit_debug_frame_boundary(builder, debug.frame_leave, function_id);
                }
                if let Some(profile) = runtime_call_refs.profile.as_ref() {
                    emit_function_frame_boundary(builder, profile.frame_leave, function_id);
                }
                builder.ins().return_(&[value]);
                return Ok(true);
            }
            SimpleStmt::ReturnVoid => {
                if expected_return_type == TYPE_ID_VOID {
                    if let Some(debug) = debug_refs {
                        emit_debug_frame_boundary(builder, debug.frame_leave, function_id);
                    }
                    if let Some(profile) = runtime_call_refs.profile.as_ref() {
                        emit_function_frame_boundary(builder, profile.frame_leave, function_id);
                    }
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

                let expected_children = then_statements.len()
                    + else_statements
                        .as_ref()
                        .map_or(0, |statements| statements.len());
                let nested_debug = debug_statements.map(|debug| debug[index].children.as_slice());
                if nested_debug.is_some_and(|debug| debug.len() != expected_children) {
                    return Err("if debug metadata does not match branch statements".to_string());
                }
                let then_debug = nested_debug.map(|debug| &debug[..then_statements.len()]);
                let else_debug = nested_debug.map(|debug| &debug[then_statements.len()..]);

                let mut then_values = values_by_name.clone();
                let then_terminated = emit_simple_statements(
                    builder,
                    then_statements,
                    then_debug,
                    debug_refs,
                    function_id,
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
                        else_debug,
                        debug_refs,
                        function_id,
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
                    function_id,
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
                if let Some((index_name, upper)) = canonical_fixed_array_loop_bound(
                    init,
                    condition,
                    step,
                    body_statements,
                    collection_infos,
                ) {
                    if let Some(binding) = loop_values.get_mut(&index_name) {
                        binding.proven_index_upper = Some(upper);
                    }
                }

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
                    debug_statements.map(|debug| debug[index].children.as_slice()),
                    debug_refs,
                    function_id,
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
                    function_id,
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
                let mut loop_len_value = len_value;
                let mut i32_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                let mut u8_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                let mut u16_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                let mut f32_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                let mut f64_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                if collection_info.element_type.is_some_and(|type_id| {
                    is_i32_abi_compatible_type(type_id, type_table)
                        && !is_u8_lane(type_table, type_id)
                        && type_id != TYPE_ID_U16
                }) {
                    let direct = matches!(collection_handle, ForeachCollectionHandle::PathHash(_))
                        .then(|| runtime_call_refs.direct_storage.as_ref())
                        .flatten()
                        .and_then(|bindings| {
                            bindings
                                .arrays
                                .get(&(collection_path.clone(), String::new()))
                        })
                        .copied();
                    let base = if let Some(direct) = direct {
                        loop_len_value =
                            emit_bounded_direct_array_len(builder, direct, loop_len_value);
                        emit_direct_slot_data_ptr(builder, direct.slot)
                    } else {
                        let field_hash_value = builder.ins().iconst(types::I32, 0);
                        let call = builder.ins().call(
                            runtime_call_refs.global_i32_array_ptr,
                            &[collection_hash_value, field_hash_value, len_value],
                        );
                        builder.inst_results(call)[0]
                    };
                    i32_array_base_ptrs.insert(String::new(), base);
                }
                if collection_info
                    .element_type
                    .is_some_and(|type_id| is_u8_lane(type_table, type_id))
                {
                    if let Some(direct) =
                        matches!(collection_handle, ForeachCollectionHandle::PathHash(_))
                            .then(|| runtime_call_refs.direct_storage.as_ref())
                            .flatten()
                            .and_then(|bindings| {
                                bindings
                                    .arrays
                                    .get(&(collection_path.clone(), String::new()))
                            })
                            .copied()
                    {
                        loop_len_value =
                            emit_bounded_direct_array_len(builder, direct, loop_len_value);
                        u8_array_base_ptrs.insert(
                            String::new(),
                            emit_direct_slot_data_ptr(builder, direct.slot),
                        );
                    }
                }
                if collection_info.element_type == Some(TYPE_ID_U16) {
                    if let Some(direct) =
                        matches!(collection_handle, ForeachCollectionHandle::PathHash(_))
                            .then(|| runtime_call_refs.direct_storage.as_ref())
                            .flatten()
                            .and_then(|bindings| {
                                bindings
                                    .arrays
                                    .get(&(collection_path.clone(), String::new()))
                            })
                            .copied()
                    {
                        loop_len_value =
                            emit_bounded_direct_array_len(builder, direct, loop_len_value);
                        u16_array_base_ptrs.insert(
                            String::new(),
                            emit_direct_slot_data_ptr(builder, direct.slot),
                        );
                    }
                }
                if collection_info.element_type == Some(TYPE_ID_F32) {
                    let direct = matches!(collection_handle, ForeachCollectionHandle::PathHash(_))
                        .then(|| runtime_call_refs.direct_storage.as_ref())
                        .flatten()
                        .and_then(|bindings| {
                            bindings
                                .arrays
                                .get(&(collection_path.clone(), String::new()))
                        })
                        .copied();
                    let base = if let Some(direct) = direct {
                        loop_len_value =
                            emit_bounded_direct_array_len(builder, direct, loop_len_value);
                        emit_direct_slot_data_ptr(builder, direct.slot)
                    } else {
                        let field_hash_value = builder.ins().iconst(types::I32, 0);
                        let call = builder.ins().call(
                            runtime_call_refs.global_f32_array_ptr,
                            &[collection_hash_value, field_hash_value, len_value],
                        );
                        builder.inst_results(call)[0]
                    };
                    f32_array_base_ptrs.insert(String::new(), base);
                }
                if collection_info.element_type == Some(TYPE_ID_F64) {
                    let direct = matches!(collection_handle, ForeachCollectionHandle::PathHash(_))
                        .then(|| runtime_call_refs.direct_storage.as_ref())
                        .flatten()
                        .and_then(|bindings| {
                            bindings
                                .arrays
                                .get(&(collection_path.clone(), String::new()))
                        })
                        .copied();
                    let base = if let Some(direct) = direct {
                        loop_len_value =
                            emit_bounded_direct_array_len(builder, direct, loop_len_value);
                        emit_direct_slot_data_ptr(builder, direct.slot)
                    } else {
                        let field_hash_value = builder.ins().iconst(types::I32, 0);
                        let call = builder.ins().call(
                            runtime_call_refs.global_f64_array_ptr,
                            &[collection_hash_value, field_hash_value, len_value],
                        );
                        builder.inst_results(call)[0]
                    };
                    f64_array_base_ptrs.insert(String::new(), base);
                }
                for (suffix, type_id) in &collection_info.field_types {
                    let field_hash = hash_foreach_field_suffix(suffix);
                    let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
                    let direct = matches!(collection_handle, ForeachCollectionHandle::PathHash(_))
                        .then(|| runtime_call_refs.direct_storage.as_ref())
                        .flatten()
                        .and_then(|bindings| {
                            bindings
                                .arrays
                                .get(&(collection_path.clone(), suffix.clone()))
                        })
                        .copied();
                    if is_i32_abi_compatible_type(*type_id, type_table)
                        && !is_u8_lane(type_table, *type_id)
                        && *type_id != TYPE_ID_U16
                    {
                        let base = if let Some(direct) = direct {
                            loop_len_value =
                                emit_bounded_direct_array_len(builder, direct, loop_len_value);
                            emit_direct_slot_data_ptr(builder, direct.slot)
                        } else {
                            let call = builder.ins().call(
                                runtime_call_refs.global_i32_array_ptr,
                                &[collection_hash_value, field_hash_value, len_value],
                            );
                            builder.inst_results(call)[0]
                        };
                        i32_array_base_ptrs.insert(suffix.clone(), base);
                    } else if is_u8_lane(type_table, *type_id) {
                        if let Some(direct) = direct {
                            loop_len_value =
                                emit_bounded_direct_array_len(builder, direct, loop_len_value);
                            u8_array_base_ptrs.insert(
                                suffix.clone(),
                                emit_direct_slot_data_ptr(builder, direct.slot),
                            );
                        }
                    } else if *type_id == TYPE_ID_U16 {
                        if let Some(direct) = direct {
                            loop_len_value =
                                emit_bounded_direct_array_len(builder, direct, loop_len_value);
                            u16_array_base_ptrs.insert(
                                suffix.clone(),
                                emit_direct_slot_data_ptr(builder, direct.slot),
                            );
                        }
                    } else if *type_id == TYPE_ID_F32 {
                        let base = if let Some(direct) = direct {
                            loop_len_value =
                                emit_bounded_direct_array_len(builder, direct, loop_len_value);
                            emit_direct_slot_data_ptr(builder, direct.slot)
                        } else {
                            let call = builder.ins().call(
                                runtime_call_refs.global_f32_array_ptr,
                                &[collection_hash_value, field_hash_value, len_value],
                            );
                            builder.inst_results(call)[0]
                        };
                        f32_array_base_ptrs.insert(suffix.clone(), base);
                    } else if *type_id == TYPE_ID_F64 {
                        let base = if let Some(direct) = direct {
                            loop_len_value =
                                emit_bounded_direct_array_len(builder, direct, loop_len_value);
                            emit_direct_slot_data_ptr(builder, direct.slot)
                        } else {
                            let call = builder.ins().call(
                                runtime_call_refs.global_f64_array_ptr,
                                &[collection_hash_value, field_hash_value, len_value],
                            );
                            builder.inst_results(call)[0]
                        };
                        f64_array_base_ptrs.insert(suffix.clone(), base);
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
                            proven_index_upper: Some(collection_info.len as usize),
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
                        u8_array_base_ptrs,
                        u16_array_base_ptrs,
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
                let condition_value =
                    builder
                        .ins()
                        .icmp(IntCC::SignedLessThan, index_value, loop_len_value);
                builder
                    .ins()
                    .brif(condition_value, body_block, &[], exit_block, &[]);

                builder.seal_block(body_block);
                builder.switch_to_block(body_block);
                let body_terminated = emit_simple_statements(
                    builder,
                    body_statements,
                    debug_statements.map(|debug| debug[index].children.as_slice()),
                    debug_refs,
                    function_id,
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
    function_id: FunctionId,
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
                None,
                None,
                function_id,
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

fn collect_call_targets_from_hir(hir: &FunctionHIR) -> BTreeSet<String> {
    fn expression(value: &SimpleExpr, out: &mut BTreeSet<String>) {
        match value {
            SimpleExpr::Condition(condition) => condition_targets(condition, out),
            SimpleExpr::IndexedPath { index, .. } => expression(index, out),
            SimpleExpr::Call { target, args } => {
                out.insert(target.clone());
                for argument in args {
                    expression(argument, out);
                }
            }
            SimpleExpr::Binary { lhs, rhs, .. } => {
                expression(lhs, out);
                expression(rhs, out);
            }
            SimpleExpr::DefaultValue(_)
            | SimpleExpr::Int(_)
            | SimpleExpr::Float(_)
            | SimpleExpr::Bool(_)
            | SimpleExpr::StringLiteral(_)
            | SimpleExpr::Identifier(_) => {}
        }
    }

    fn condition_targets(condition: &SimpleCondition, out: &mut BTreeSet<String>) {
        match condition {
            SimpleCondition::Comparison { lhs, rhs, .. } => {
                expression(lhs, out);
                expression(rhs, out);
            }
            SimpleCondition::Expr(expression_value) => expression(expression_value, out),
            SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
                condition_targets(lhs, out);
                condition_targets(rhs, out);
            }
            SimpleCondition::Not(inner) => condition_targets(inner, out),
        }
    }

    fn statement(value: &SimpleStmt, out: &mut BTreeSet<String>) {
        match value {
            SimpleStmt::Let {
                expression: value, ..
            }
            | SimpleStmt::Assign {
                expression: value, ..
            }
            | SimpleStmt::Expr(value)
            | SimpleStmt::Return(value) => expression(value, out),
            SimpleStmt::Convert { source, .. } => expression(source, out),
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                condition_targets(condition, out);
                for nested in then_statements {
                    statement(nested, out);
                }
                if let Some(nested_statements) = else_statements {
                    for nested in nested_statements {
                        statement(nested, out);
                    }
                }
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                statement(init, out);
                condition_targets(condition, out);
                statement(step, out);
                for nested in body_statements {
                    statement(nested, out);
                }
            }
            SimpleStmt::Foreach {
                body_statements, ..
            } => {
                for nested in body_statements {
                    statement(nested, out);
                }
            }
            SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
        }
    }

    let mut targets = BTreeSet::new();
    for statement_value in &hir.statements {
        statement(statement_value, &mut targets);
    }
    targets
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
                        storage_kind: struct_view.storage_kind,
                        known_collection_hash: struct_view.known_collection_hash,
                        bounds_proven: struct_view.bounds_proven,
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
                        storage_kind: StructViewStorageKind::Soa,
                        known_collection_hash: match binding.collection_handle {
                            ForeachCollectionHandle::PathHash(hash) => Some(hash),
                            ForeachCollectionHandle::LocalVar(_) => None,
                        },
                        bounds_proven: true,
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
                        storage_kind: StructViewStorageKind::Aos,
                        known_collection_hash: None,
                        bounds_proven: true,
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
                storage_kind: StructViewStorageKind::Soa,
                known_collection_hash: (!values_by_name.contains_key(collection_path))
                    .then(|| hash_global_path(collection_path)),
                bounds_proven: known_len.is_some_and(|len| {
                    static_index_bounds_proven(index, len as usize, values_by_name)
                }),
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
        SimpleExpr::DefaultValue(type_id) => {
            let value = match *type_id {
                TYPE_ID_F32 => builder.ins().f32const(Ieee32::with_float(0.0)),
                TYPE_ID_F64 => builder.ins().f64const(Ieee64::with_float(0.0)),
                _ => builder.ins().iconst(types::I32, 0),
            };
            Ok(ValueBinding {
                value,
                type_id: *type_id,
            })
        }
        SimpleExpr::Int(value) => {
            let literal_type = expected_type
                .filter(|type_id| type_table.is_integer(*type_id))
                .unwrap_or(TYPE_ID_I32);
            let bits = type_table.unsigned_integer_bits(literal_type);
            let value = match bits {
                Some(bits) => {
                    let maximum = if bits == 32 {
                        i64::from(u32::MAX)
                    } else {
                        (1i64 << bits) - 1
                    };
                    if *value < 0 || *value > maximum {
                        return Err(format!(
                            "integer literal {value} is outside u{bits} range 0..={maximum}"
                        ));
                    }
                    *value as u32 as i32
                }
                None => i32::try_from(*value).map_err(|_| {
                    format!("integer literal out of i32 range in expression: {value}")
                })?,
            };
            Ok(ValueBinding {
                value: builder.ins().iconst(types::I32, i64::from(value)),
                type_id: literal_type,
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
                if let Some(collection_path) = name.strip_suffix(".max_length") {
                    if let Some(max_length) = global_path_types
                        .get(collection_path)
                        .and_then(|type_id| type_table.fixed_collection_len(*type_id))
                    {
                        return Ok(ValueBinding {
                            value: builder.ins().iconst(types::I32, i64::from(max_length)),
                            type_id: TYPE_ID_I32,
                        });
                    }
                }
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
            let bounds_proven =
                static_index_bounds_proven(index, collection_info.len as usize, values_by_name);
            emit_indexed_collection_load(
                builder,
                runtime_call_refs,
                type_table,
                collection_path,
                collection_info,
                suffix,
                index_binding,
                bounds_proven,
            )
        }
        SimpleExpr::Call { target, args } => {
            let mut arg_values: Vec<Value> = Vec::with_capacity(args.len());
            let mut arg_types: Vec<TypeId> = Vec::with_capacity(args.len());
            let expected_params = unambiguous_call_params(target, args.len(), call_signatures);
            for (arg_index, arg) in args.iter().enumerate() {
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
                    expected_params
                        .as_ref()
                        .and_then(|params| params.get(arg_index).copied()),
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
            if matches!(
                target.as_str(),
                "fixed32_from_i32"
                    | "fixed32_to_i32"
                    | "fixed32_mul"
                    | "fixed32_div"
                    | "fixed32_from_ratio"
            ) {
                let expected_arity = if target == "fixed32_from_i32" || target == "fixed32_to_i32" {
                    1
                } else {
                    2
                };
                if arg_values.len() != expected_arity {
                    return Err(format!(
                        "deterministic numeric intrinsic '{target}' expects {expected_arity} argument(s), found {}",
                        arg_values.len()
                    ));
                }
                if let Some(type_id) = arg_types
                    .iter()
                    .copied()
                    .find(|type_id| *type_id != TYPE_ID_I32)
                {
                    return Err(format!(
                        "deterministic numeric intrinsic '{target}' requires exact i32 arguments, found type {type_id}"
                    ));
                }
                let value = match target.as_str() {
                    "fixed32_from_i32" => builder.ins().ishl_imm(arg_values[0], 16),
                    "fixed32_to_i32" => {
                        let scale = builder.ins().iconst(types::I32, 65_536);
                        builder.ins().sdiv(arg_values[0], scale)
                    }
                    "fixed32_mul" => {
                        let lhs = builder.ins().sextend(types::I64, arg_values[0]);
                        let rhs = builder.ins().sextend(types::I64, arg_values[1]);
                        let product = builder.ins().imul(lhs, rhs);
                        let scale = builder.ins().iconst(types::I64, 65_536);
                        let scaled = builder.ins().sdiv(product, scale);
                        builder.ins().ireduce(types::I32, scaled)
                    }
                    "fixed32_div" | "fixed32_from_ratio" => {
                        let lhs = builder.ins().sextend(types::I64, arg_values[0]);
                        let numerator = builder.ins().ishl_imm(lhs, 16);
                        let denominator = builder.ins().sextend(types::I64, arg_values[1]);
                        let quotient = builder.ins().sdiv(numerator, denominator);
                        builder.ins().ireduce(types::I32, quotient)
                    }
                    _ => unreachable!(),
                };
                return Ok(ValueBinding {
                    value,
                    type_id: TYPE_ID_I32,
                });
            }
            if target == "i32_to_f32" {
                if arg_values.len() != 1 {
                    return Err(format!(
                        "math intrinsic 'i32_to_f32' expects exactly one argument, found {}",
                        arg_values.len()
                    ));
                }
                if arg_types[0] != TYPE_ID_I32 {
                    return Err(format!(
                        "math intrinsic 'i32_to_f32' requires exact i32 argument, found type {}",
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
            let value = emit_internal_call_for_signature(
                builder,
                runtime_call_refs,
                internal_calls,
                signature,
                &arg_values,
                &arg_types,
                type_table,
                named_struct_field_types,
                target,
            )?
            .ok_or_else(|| {
                format!(
                    "void call target '{}' cannot be used in value expression",
                    target
                )
            })?;
            Ok(ValueBinding {
                value,
                type_id: signature.return_type,
            })
        }
        SimpleExpr::Binary { lhs, op, rhs } => {
            let child_expected = match expected_type {
                Some(TYPE_ID_F32) => Some(TYPE_ID_F32),
                Some(TYPE_ID_F64) => Some(TYPE_ID_F64),
                Some(type_id) if type_table.is_integer(type_id) => Some(type_id),
                _ => None,
            };
            let (lhs_value, rhs_value) =
                if child_expected.is_none() && matches!(lhs.as_ref(), SimpleExpr::Int(_)) {
                    let rhs_value = emit_simple_expression(
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
                    let lhs_expected = type_table
                        .unsigned_integer_bits(rhs_value.type_id)
                        .is_some()
                        .then_some(rhs_value.type_id);
                    let lhs_value = emit_simple_expression(
                        builder,
                        lhs,
                        lhs_expected,
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
                    (lhs_value, rhs_value)
                } else {
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
                    let rhs_expected = child_expected.or_else(|| {
                        type_table
                            .unsigned_integer_bits(lhs_value.type_id)
                            .is_some()
                            .then_some(lhs_value.type_id)
                    });
                    let rhs_value = emit_simple_expression(
                        builder,
                        rhs,
                        rhs_expected,
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
                    (lhs_value, rhs_value)
                };
            if is_i32_numeric_type(lhs_value.type_id, type_table)
                && is_i32_numeric_type(rhs_value.type_id, type_table)
            {
                let result_type = integer_binary_result_type(
                    expected_type,
                    lhs_value.type_id,
                    rhs_value.type_id,
                    type_table,
                );
                let unsigned = type_table.unsigned_integer_bits(result_type).is_some();
                let value = match op {
                    '+' => builder.ins().iadd(lhs_value.value, rhs_value.value),
                    '-' => builder.ins().isub(lhs_value.value, rhs_value.value),
                    '*' => builder.ins().imul(lhs_value.value, rhs_value.value),
                    '/' if unsigned => builder.ins().udiv(lhs_value.value, rhs_value.value),
                    '%' if unsigned => builder.ins().urem(lhs_value.value, rhs_value.value),
                    '/' => builder.ins().sdiv(lhs_value.value, rhs_value.value),
                    '%' => builder.ins().srem(lhs_value.value, rhs_value.value),
                    other => {
                        return Err(format!(
                            "unsupported binary operator '{other}' in expression"
                        ))
                    }
                };
                return Ok(ValueBinding {
                    value: normalize_unsigned_value(builder, value, result_type, type_table),
                    type_id: result_type,
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
        if type_table.unsigned_integer_bits(lhs.type_id).is_some() {
            builder.ins().fcvt_from_uint(types::F32, lhs.value)
        } else {
            builder.ins().fcvt_from_sint(types::F32, lhs.value)
        }
    } else {
        return Err(format!(
            "unsupported lhs type {} for '{}' expression",
            lhs.type_id, op
        ));
    };
    let rhs_value = if rhs.type_id == TYPE_ID_F32 {
        rhs.value
    } else if is_i32_numeric_type(rhs.type_id, type_table) {
        if type_table.unsigned_integer_bits(rhs.type_id).is_some() {
            builder.ins().fcvt_from_uint(types::F32, rhs.value)
        } else {
            builder.ins().fcvt_from_sint(types::F32, rhs.value)
        }
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
        if type_table.unsigned_integer_bits(lhs.type_id).is_some() {
            builder.ins().fcvt_from_uint(types::F64, lhs.value)
        } else {
            builder.ins().fcvt_from_sint(types::F64, lhs.value)
        }
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
        if type_table.unsigned_integer_bits(rhs.type_id).is_some() {
            builder.ins().fcvt_from_uint(types::F64, rhs.value)
        } else {
            builder.ins().fcvt_from_sint(types::F64, rhs.value)
        }
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
            element_shape: type_table
                .type_info(element_type)
                .map_or_else(|| format!("type#{element_type}"), |info| info.name.clone()),
            fully_migratable: true,
        });
    }
    Ok(ForeachCollectionInfo {
        len,
        element_type: Some(element_type),
        field_types: BTreeMap::new(),
        element_shape: type_table
            .type_info(element_type)
            .map_or_else(|| format!("type#{element_type}"), |info| info.name.clone()),
        fully_migratable: true,
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
    if is_u8_lane(type_table, resolved) {
        if let Some(base_ptr) = binding.u8_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let address = builder.ins().iadd(base_ptr, index_i64);
            let byte = builder.ins().load(types::I8, MemFlags::new(), address, 0);
            return Ok(ValueBinding {
                value: builder.ins().uextend(types::I32, byte),
                type_id: resolved,
            });
        }
    }
    if resolved == TYPE_ID_U16 {
        if let Some(base_ptr) = binding.u16_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 1);
            let address = builder.ins().iadd(base_ptr, byte_offset);
            let word = builder.ins().load(types::I16, MemFlags::new(), address, 0);
            return Ok(ValueBinding {
                value: builder.ins().uextend(types::I32, word),
                type_id: resolved,
            });
        }
    }
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
        let lhs = if op == AssignOp::Set {
            None
        } else {
            Some(
                emit_foreach_binding_load(builder, runtime_call_refs, type_table, binding, suffix)?
                    .value,
            )
        };
        let value =
            emit_integer_assignment_value(builder, lhs, rhs.value, op, type_table, path_type);
        if let Some(base_ptr) = binding.u8_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let addr = builder.ins().iadd(base_ptr, index_i64);
            let byte = builder.ins().ireduce(types::I8, value);
            builder.ins().store(MemFlags::new(), byte, addr, 0);
        } else if let Some(base_ptr) = binding.u16_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 1);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            let word = builder.ins().ireduce(types::I16, value);
            builder.ins().store(MemFlags::new(), word, addr, 0);
        } else if let Some(base_ptr) = binding.i32_array_base_ptrs.get(suffix).copied() {
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
    let direct_array = binding.known_collection_hash.and_then(|collection_hash| {
        runtime_call_refs
            .direct_storage
            .as_ref()
            .and_then(|storage| {
                storage
                    .arrays_by_hash
                    .get(&(collection_hash, hash_foreach_field_suffix(suffix)))
                    .copied()
            })
    });
    if binding.storage_kind == StructViewStorageKind::Aos {
        return emit_struct_view_field_load_for_storage(
            builder,
            runtime_call_refs,
            type_table,
            true,
            base_hash,
            index_value,
            suffix,
            field_type,
            None,
            true,
        );
    }
    if binding.storage_kind == StructViewStorageKind::Soa {
        return emit_struct_view_field_load_for_storage(
            builder,
            runtime_call_refs,
            type_table,
            false,
            base_hash,
            index_value,
            suffix,
            field_type,
            direct_array,
            binding.bounds_proven,
        );
    }
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
    let aos_value = emit_struct_view_field_load_for_storage(
        builder,
        runtime_call_refs,
        type_table,
        true,
        base_hash,
        index_value,
        suffix,
        field_type,
        None,
        true,
    )?
    .value;
    builder.ins().jump(merge_block, &[aos_value]);
    builder.seal_block(aos_block);

    builder.switch_to_block(soa_block);
    let soa_value = emit_struct_view_field_load_for_storage(
        builder,
        runtime_call_refs,
        type_table,
        false,
        base_hash,
        index_value,
        suffix,
        field_type,
        direct_array,
        binding.bounds_proven,
    )?
    .value;
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

fn emit_struct_view_field_load_for_storage(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    aos: bool,
    base_hash: Value,
    index_value: Value,
    suffix: &str,
    field_type: TypeId,
    direct_array: Option<DirectArrayStorageRef>,
    bounds_proven: bool,
) -> Result<ValueBinding, String> {
    let value = if aos {
        let path_hash = emit_local_struct_field_path_hash(base_hash, suffix, builder);
        if is_i32_abi_compatible_type(field_type, type_table) {
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
        }
    } else if let Some(direct) = direct_array {
        emit_direct_array_load(
            builder,
            direct.slot,
            index_value,
            field_type,
            type_table,
            direct.storage_bytes,
            direct.static_len,
            bounds_proven,
        )?
    } else {
        let field_hash = builder
            .ins()
            .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)));
        if is_i32_abi_compatible_type(field_type, type_table) {
            let call = builder.ins().call(
                runtime_call_refs.global_i32_array_load,
                &[base_hash, field_hash, index_value],
            );
            builder.inst_results(call)[0]
        } else if field_type == TYPE_ID_F32 {
            let call = builder.ins().call(
                runtime_call_refs.global_f32_array_load,
                &[base_hash, field_hash, index_value],
            );
            builder.inst_results(call)[0]
        } else if field_type == TYPE_ID_F64 {
            let call = builder.ins().call(
                runtime_call_refs.global_f64_array_load,
                &[base_hash, field_hash, index_value],
            );
            builder.inst_results(call)[0]
        } else {
            return Err(format!(
                "unsupported struct view field type {} for suffix '{}'",
                field_type, suffix
            ));
        }
    };
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
    let direct_array = binding.known_collection_hash.and_then(|collection_hash| {
        runtime_call_refs
            .direct_storage
            .as_ref()
            .and_then(|storage| {
                storage
                    .arrays_by_hash
                    .get(&(collection_hash, hash_foreach_field_suffix(suffix)))
                    .copied()
            })
    });
    match binding.storage_kind {
        StructViewStorageKind::Aos => {
            return emit_struct_view_field_assignment_for_storage(
                builder,
                runtime_call_refs,
                type_table,
                true,
                base_hash,
                index_value,
                suffix,
                field_type,
                op,
                rhs,
                None,
                true,
            );
        }
        StructViewStorageKind::Soa => {
            return emit_struct_view_field_assignment_for_storage(
                builder,
                runtime_call_refs,
                type_table,
                false,
                base_hash,
                index_value,
                suffix,
                field_type,
                op,
                rhs,
                direct_array,
                binding.bounds_proven,
            );
        }
        StructViewStorageKind::Dynamic => {}
    }
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
    emit_struct_view_field_assignment_for_storage(
        builder,
        runtime_call_refs,
        type_table,
        true,
        base_hash,
        index_value,
        suffix,
        field_type,
        op,
        rhs,
        None,
        true,
    )?;
    builder.ins().jump(merge_block, &[]);
    builder.seal_block(aos_block);

    builder.switch_to_block(soa_block);
    emit_struct_view_field_assignment_for_storage(
        builder,
        runtime_call_refs,
        type_table,
        false,
        base_hash,
        index_value,
        suffix,
        field_type,
        op,
        rhs,
        direct_array,
        binding.bounds_proven,
    )?;
    builder.ins().jump(merge_block, &[]);
    builder.seal_block(soa_block);

    builder.seal_block(merge_block);
    builder.switch_to_block(merge_block);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_struct_view_field_assignment_for_storage(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    aos: bool,
    base_hash: Value,
    index_value: Value,
    suffix: &str,
    field_type: TypeId,
    op: AssignOp,
    rhs: ValueBinding,
    direct_array: Option<DirectArrayStorageRef>,
    bounds_proven: bool,
) -> Result<(), String> {
    let field_key = if aos {
        emit_local_struct_field_path_hash(base_hash, suffix, builder)
    } else {
        builder
            .ins()
            .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)))
    };

    if let Some(direct) = direct_array {
        let lhs = if op == AssignOp::Set {
            None
        } else {
            Some(emit_direct_array_load(
                builder,
                direct.slot,
                index_value,
                field_type,
                type_table,
                direct.storage_bytes,
                direct.static_len,
                bounds_proven,
            )?)
        };
        let value = if is_i32_abi_compatible_type(field_type, type_table) {
            emit_integer_assignment_value(builder, lhs, rhs.value, op, type_table, field_type)
        } else {
            match op {
                AssignOp::Set => rhs.value,
                AssignOp::Add => builder
                    .ins()
                    .fadd(lhs.expect("compound assignment lhs"), rhs.value),
                AssignOp::Sub => builder
                    .ins()
                    .fsub(lhs.expect("compound assignment lhs"), rhs.value),
                AssignOp::Mul => builder
                    .ins()
                    .fmul(lhs.expect("compound assignment lhs"), rhs.value),
                AssignOp::Div => builder
                    .ins()
                    .fdiv(lhs.expect("compound assignment lhs"), rhs.value),
                AssignOp::Mod => {
                    return Err(format!(
                        "'%=' is unsupported for floating-point struct view field '{}'",
                        suffix
                    ))
                }
            }
        };
        return emit_direct_array_store(
            builder,
            direct.slot,
            index_value,
            value,
            field_type,
            direct.storage_bytes,
            direct.static_len,
            bounds_proven,
        );
    }

    if is_i32_scalar_lane_type(field_type, type_table) {
        let lhs = if op == AssignOp::Set {
            None
        } else if aos {
            let call = builder
                .ins()
                .call(runtime_call_refs.global_i32_load, &[field_key]);
            Some(builder.inst_results(call)[0])
        } else {
            let call = builder.ins().call(
                runtime_call_refs.global_i32_array_load,
                &[base_hash, field_key, index_value],
            );
            Some(builder.inst_results(call)[0])
        };
        let value =
            emit_integer_assignment_value(builder, lhs, rhs.value, op, type_table, field_type);
        if aos {
            builder
                .ins()
                .call(runtime_call_refs.global_i32_store, &[field_key, value]);
        } else {
            builder.ins().call(
                runtime_call_refs.global_i32_array_store,
                &[base_hash, field_key, index_value, value],
            );
        }
        return Ok(());
    }

    if field_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool assignment only supports '=' in current jit path for struct view field '{}'",
                suffix
            ));
        }
        if aos {
            builder
                .ins()
                .call(runtime_call_refs.global_i32_store, &[field_key, rhs.value]);
        } else {
            builder.ins().call(
                runtime_call_refs.global_i32_array_store,
                &[base_hash, field_key, index_value, rhs.value],
            );
        }
        return Ok(());
    }

    let lhs = match (field_type, op) {
        (_, AssignOp::Set) => None,
        (TYPE_ID_F32, AssignOp::Mod) => {
            return Err(format!(
                "'%=' is unsupported for f32 struct view field '{}'",
                suffix
            ));
        }
        (TYPE_ID_F64, AssignOp::Mod) => {
            return Err(format!(
                "'%=' is unsupported for f64 struct view field '{}'",
                suffix
            ));
        }
        (TYPE_ID_F32, _) => {
            let call = if aos {
                builder
                    .ins()
                    .call(runtime_call_refs.global_f32_load, &[field_key])
            } else {
                builder.ins().call(
                    runtime_call_refs.global_f32_array_load,
                    &[base_hash, field_key, index_value],
                )
            };
            Some(builder.inst_results(call)[0])
        }
        (TYPE_ID_F64, _) => {
            let call = if aos {
                builder
                    .ins()
                    .call(runtime_call_refs.global_f64_load, &[field_key])
            } else {
                builder.ins().call(
                    runtime_call_refs.global_f64_array_load,
                    &[base_hash, field_key, index_value],
                )
            };
            Some(builder.inst_results(call)[0])
        }
        _ => {
            return Err(format!(
                "unsupported struct view field type {} for suffix '{}'",
                field_type, suffix
            ));
        }
    };
    let value = match op {
        AssignOp::Set => rhs.value,
        AssignOp::Add => builder
            .ins()
            .fadd(lhs.expect("compound assignment lhs"), rhs.value),
        AssignOp::Sub => builder
            .ins()
            .fsub(lhs.expect("compound assignment lhs"), rhs.value),
        AssignOp::Mul => builder
            .ins()
            .fmul(lhs.expect("compound assignment lhs"), rhs.value),
        AssignOp::Div => builder
            .ins()
            .fdiv(lhs.expect("compound assignment lhs"), rhs.value),
        AssignOp::Mod => unreachable!(),
    };
    if field_type == TYPE_ID_F32 {
        if aos {
            builder
                .ins()
                .call(runtime_call_refs.global_f32_store, &[field_key, value]);
        } else {
            builder.ins().call(
                runtime_call_refs.global_f32_array_store,
                &[base_hash, field_key, index_value, value],
            );
        }
    } else if aos {
        builder
            .ins()
            .call(runtime_call_refs.global_f64_store, &[field_key, value]);
    } else {
        builder.ins().call(
            runtime_call_refs.global_f64_array_store,
            &[base_hash, field_key, index_value, value],
        );
    }
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
        let lhs = if op == AssignOp::Set {
            None
        } else {
            Some(
                emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
                    collection_name,
                    collection_binding,
                    suffix,
                    index_binding,
                )?
                .value,
            )
        };
        let value =
            emit_integer_assignment_value(builder, lhs, rhs.value, op, type_table, path_type);
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

fn is_u8_lane(type_table: &TypeTable, type_id: TypeId) -> bool {
    type_table
        .type_info(type_id)
        .is_some_and(|info| info.name == "u8")
}

fn emit_direct_array_load(
    builder: &mut FunctionBuilder<'_>,
    slot_ref: DirectStorageRef,
    index: Value,
    type_id: TypeId,
    type_table: &TypeTable,
    storage_bytes: u8,
    static_len: Option<usize>,
    bounds_proven: bool,
) -> Result<Value, String> {
    let data = emit_direct_slot_data_ptr(builder, slot_ref);
    let len = if let Some(len) = static_len {
        builder.ins().iconst(types::I64, len as i64)
    } else {
        let slot = emit_direct_slot_address(builder, slot_ref);
        builder.ins().load(
            types::I64,
            MemFlags::new(),
            slot,
            stasis_dynload::JitStorageSlot::LEN_OFFSET,
        )
    };
    let index_i64 = builder.ins().sextend(types::I64, index);
    let result_type = if is_i32_abi_compatible_type(type_id, type_table) {
        types::I32
    } else if type_id == TYPE_ID_F32 {
        types::F32
    } else if type_id == TYPE_ID_F64 {
        types::F64
    } else {
        return Err(format!("unsupported direct array load type {type_id}"));
    };
    if !bounds_proven {
        let non_negative = builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, index, 0);
        let below_len = builder.ins().icmp(IntCC::UnsignedLessThan, index_i64, len);
        let valid = builder.ins().band(non_negative, below_len);
        builder.ins().trapz(valid, TrapCode::HEAP_OUT_OF_BOUNDS);
    }
    let shift = match storage_bytes {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        other => return Err(format!("unsupported direct array element width {other}")),
    };
    let byte_offset = if shift == 0 {
        index_i64
    } else {
        builder.ins().ishl_imm(index_i64, shift)
    };
    let address = builder.ins().iadd(data, byte_offset);
    let value = if storage_bytes == 1 {
        let byte = builder.ins().load(types::I8, MemFlags::new(), address, 0);
        builder.ins().uextend(types::I32, byte)
    } else if storage_bytes == 2 {
        let word = builder.ins().load(types::I16, MemFlags::new(), address, 0);
        builder.ins().uextend(types::I32, word)
    } else {
        builder.ins().load(result_type, MemFlags::new(), address, 0)
    };
    Ok(value)
}

fn emit_direct_array_store(
    builder: &mut FunctionBuilder<'_>,
    slot_ref: DirectStorageRef,
    index: Value,
    value: Value,
    _type_id: TypeId,
    storage_bytes: u8,
    static_len: Option<usize>,
    bounds_proven: bool,
) -> Result<(), String> {
    let data = emit_direct_slot_data_ptr(builder, slot_ref);
    let len = if let Some(len) = static_len {
        builder.ins().iconst(types::I64, len as i64)
    } else {
        let slot = emit_direct_slot_address(builder, slot_ref);
        builder.ins().load(
            types::I64,
            MemFlags::new(),
            slot,
            stasis_dynload::JitStorageSlot::LEN_OFFSET,
        )
    };
    let index_i64 = builder.ins().sextend(types::I64, index);
    if !bounds_proven {
        let non_negative = builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, index, 0);
        let below_len = builder.ins().icmp(IntCC::UnsignedLessThan, index_i64, len);
        let valid = builder.ins().band(non_negative, below_len);
        builder.ins().trapz(valid, TrapCode::HEAP_OUT_OF_BOUNDS);
    }
    let shift = match storage_bytes {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        other => return Err(format!("unsupported direct array element width {other}")),
    };
    let byte_offset = if shift == 0 {
        index_i64
    } else {
        builder.ins().ishl_imm(index_i64, shift)
    };
    let address = builder.ins().iadd(data, byte_offset);
    let stored = if storage_bytes == 1 {
        builder.ins().ireduce(types::I8, value)
    } else if storage_bytes == 2 {
        builder.ins().ireduce(types::I16, value)
    } else {
        value
    };
    builder.ins().store(MemFlags::new(), stored, address, 0);
    Ok(())
}

fn emit_array_bounds_trap(builder: &mut FunctionBuilder<'_>, index: Value, len: Value) {
    let non_negative = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, index, 0);
    let below_len = builder.ins().icmp(IntCC::UnsignedLessThan, index, len);
    let valid = builder.ins().band(non_negative, below_len);
    builder.ins().trapz(valid, TrapCode::HEAP_OUT_OF_BOUNDS);
}

fn static_index_bounds_proven(
    index: &SimpleExpr,
    collection_len: usize,
    values_by_name: &BTreeMap<String, LocalBinding>,
) -> bool {
    if let Some(value) = eval_const_i64(index) {
        return usize::try_from(value).is_ok_and(|value| value < collection_len);
    }
    match index {
        SimpleExpr::Identifier(name) => {
            values_by_name
                .get(name)
                .and_then(|binding| binding.proven_index_upper)
                == Some(collection_len)
        }
        _ => false,
    }
}

fn statement_assigns_local(statement: &SimpleStmt, name: &str) -> bool {
    match statement {
        SimpleStmt::Assign {
            target: AssignTarget::Local(target),
            ..
        }
        | SimpleStmt::Convert {
            target: AssignTarget::Local(target),
            ..
        } => target == name,
        SimpleStmt::If {
            then_statements,
            else_statements,
            ..
        } => {
            then_statements
                .iter()
                .any(|statement| statement_assigns_local(statement, name))
                || else_statements.as_ref().is_some_and(|statements| {
                    statements
                        .iter()
                        .any(|statement| statement_assigns_local(statement, name))
                })
        }
        SimpleStmt::For {
            init,
            step,
            body_statements,
            ..
        } => {
            statement_assigns_local(init, name)
                || statement_assigns_local(step, name)
                || body_statements
                    .iter()
                    .any(|statement| statement_assigns_local(statement, name))
        }
        SimpleStmt::Foreach {
            body_statements, ..
        } => body_statements
            .iter()
            .any(|statement| statement_assigns_local(statement, name)),
        _ => false,
    }
}

fn canonical_fixed_array_loop_bound(
    init: &SimpleStmt,
    condition: &SimpleCondition,
    step: &SimpleStmt,
    body_statements: &[SimpleStmt],
    collection_infos: &CollectionInfoMap,
) -> Option<(String, usize)> {
    let SimpleStmt::Assign {
        target: AssignTarget::Local(index_name),
        op: AssignOp::Set,
        expression: SimpleExpr::Int(0),
    } = init
    else {
        return None;
    };
    let SimpleCondition::Comparison {
        lhs: SimpleExpr::Identifier(condition_index),
        op: ComparisonOp::Lt,
        rhs: SimpleExpr::Identifier(max_length_path),
    } = condition
    else {
        return None;
    };
    let collection_path = max_length_path.strip_suffix(".max_length")?;
    let SimpleStmt::Assign {
        target: AssignTarget::Local(step_index),
        op: AssignOp::Set,
        expression: SimpleExpr::Binary { lhs, op: '+', rhs },
    } = step
    else {
        return None;
    };
    if condition_index != index_name
        || step_index != index_name
        || lhs.as_ref() != &SimpleExpr::Identifier(index_name.clone())
        || rhs.as_ref() != &SimpleExpr::Int(1)
        || body_statements
            .iter()
            .any(|statement| statement_assigns_local(statement, index_name))
    {
        return None;
    }
    collection_infos
        .get(collection_path)
        .map(|info| (index_name.clone(), info.len as usize))
}

pub(crate) fn emit_indexed_collection_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    collection_path: &str,
    collection_info: &ForeachCollectionInfo,
    suffix: &str,
    index_binding: ValueBinding,
    bounds_proven: bool,
) -> Result<ValueBinding, String> {
    let resolved = resolve_collection_value_type(collection_info, suffix)?;
    let index_binding = normalize_index_binding(index_binding, type_table)?;
    if let Some(direct) = runtime_call_refs
        .direct_storage
        .as_ref()
        .and_then(|bindings| {
            bindings
                .arrays
                .get(&(collection_path.to_string(), suffix.to_string()))
        })
        .copied()
    {
        return Ok(ValueBinding {
            value: emit_direct_array_load(
                builder,
                direct.slot,
                index_binding.value,
                resolved,
                type_table,
                direct.storage_bytes,
                direct.static_len,
                bounds_proven && direct.static_len == Some(collection_info.len as usize),
            )?,
            type_id: resolved,
        });
    }
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
    bounds_proven: bool,
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
    let direct_slot = runtime_call_refs
        .direct_storage
        .as_ref()
        .and_then(|bindings| {
            bindings
                .arrays
                .get(&(collection_path.to_string(), suffix.to_string()))
        })
        .copied();
    let collection_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_global_path(collection_path)));
    let field_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)));

    if is_i32_scalar_lane_type(path_type, type_table) {
        let lhs = if op == AssignOp::Set {
            None
        } else {
            Some(
                emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                    false,
                )?
                .value,
            )
        };
        let value =
            emit_integer_assignment_value(builder, lhs, rhs.value, op, type_table, path_type);
        if let Some(direct) = direct_slot {
            emit_direct_array_store(
                builder,
                direct.slot,
                index_binding.value,
                value,
                path_type,
                direct.storage_bytes,
                direct.static_len,
                bounds_proven,
            )?;
        } else {
            builder.ins().call(
                runtime_call_refs.global_i32_array_store,
                &[collection_hash, field_hash, index_binding.value, value],
            );
        }
        return Ok(());
    }
    if path_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool indexed assignment only supports '=' for '{}[...].{}'",
                collection_path, suffix
            ));
        }
        if let Some(direct) = direct_slot {
            emit_direct_array_store(
                builder,
                direct.slot,
                index_binding.value,
                rhs.value,
                path_type,
                direct.storage_bytes,
                direct.static_len,
                bounds_proven,
            )?;
        } else {
            builder.ins().call(
                runtime_call_refs.global_i32_array_store,
                &[collection_hash, field_hash, index_binding.value, rhs.value],
            );
        }
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
        if let Some(direct) = direct_slot {
            emit_direct_array_store(
                builder,
                direct.slot,
                index_binding.value,
                value,
                path_type,
                direct.storage_bytes,
                direct.static_len,
                bounds_proven,
            )?;
        } else {
            builder.ins().call(
                runtime_call_refs.global_f32_array_store,
                &[collection_hash, field_hash, index_binding.value, value],
            );
        }
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
        if let Some(direct) = direct_slot {
            emit_direct_array_store(
                builder,
                direct.slot,
                index_binding.value,
                value,
                path_type,
                direct.storage_bytes,
                direct.static_len,
                bounds_proven,
            )?;
        } else {
            builder.ins().call(
                runtime_call_refs.global_f64_array_store,
                &[collection_hash, field_hash, index_binding.value, value],
            );
        }
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
    if let Some(slot_address) = runtime_call_refs
        .direct_storage
        .as_ref()
        .and_then(|bindings| bindings.scalars.get(path))
        .copied()
    {
        return Ok(ValueBinding {
            value: emit_direct_scalar_load(builder, slot_address, path_type, type_table)?,
            type_id: path_type,
        });
    }
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
        let unsigned = type_table.unsigned_integer_bits(path_type).is_some();
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
            AssignOp::Div if unsigned => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().udiv(lhs, rhs.value)
            }
            AssignOp::Mod if unsigned => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().urem(lhs, rhs.value)
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
        emit_global_scalar_store(
            builder,
            runtime_call_refs,
            type_table,
            path,
            path_type,
            value,
        )?;
        return Ok(());
    }
    if path_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool global path '{}' only supports '=' assignment",
                path
            ));
        }
        emit_global_scalar_store(
            builder,
            runtime_call_refs,
            type_table,
            path,
            path_type,
            rhs.value,
        )?;
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
        emit_global_scalar_store(
            builder,
            runtime_call_refs,
            type_table,
            path,
            path_type,
            value,
        )?;
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
        emit_global_scalar_store(
            builder,
            runtime_call_refs,
            type_table,
            path,
            path_type,
            value,
        )?;
        return Ok(());
    }
    Err(format!(
        "unsupported global path type {} for '{}'",
        path_type, path
    ))
}

fn emit_direct_slot_address(
    builder: &mut FunctionBuilder<'_>,
    slot_ref: DirectStorageRef,
) -> Value {
    match slot_ref {
        DirectStorageRef::Absolute(address) => builder.ins().iconst(types::I64, address as i64),
        DirectStorageRef::Symbol(symbol) => builder.ins().global_value(types::I64, symbol),
    }
}

fn emit_direct_slot_data_ptr(
    builder: &mut FunctionBuilder<'_>,
    slot_ref: DirectStorageRef,
) -> Value {
    match slot_ref {
        DirectStorageRef::Absolute(_) => {
            let slot = emit_direct_slot_address(builder, slot_ref);
            builder.ins().load(
                types::I64,
                MemFlags::new(),
                slot,
                stasis_dynload::JitStorageSlot::DATA_OFFSET,
            )
        }
        DirectStorageRef::Symbol(symbol) => builder.ins().global_value(types::I64, symbol),
    }
}

fn emit_bounded_direct_array_len(
    builder: &mut FunctionBuilder<'_>,
    direct: DirectArrayStorageRef,
    current_len: Value,
) -> Value {
    if direct.static_len.is_some() {
        return current_len;
    }
    let slot = emit_direct_slot_address(builder, direct.slot);
    let direct_len = builder.ins().load(
        types::I64,
        MemFlags::new(),
        slot,
        stasis_dynload::JitStorageSlot::LEN_OFFSET,
    );
    let current_len_i64 = builder.ins().uextend(types::I64, current_len);
    let direct_is_shorter =
        builder
            .ins()
            .icmp(IntCC::UnsignedLessThan, direct_len, current_len_i64);
    let bounded_len = builder
        .ins()
        .select(direct_is_shorter, direct_len, current_len_i64);
    builder.ins().ireduce(types::I32, bounded_len)
}

fn emit_direct_scalar_load(
    builder: &mut FunctionBuilder<'_>,
    slot_ref: DirectStorageRef,
    type_id: TypeId,
    type_table: &TypeTable,
) -> Result<Value, String> {
    let data = emit_direct_slot_data_ptr(builder, slot_ref);
    if type_table.unsigned_integer_bits(type_id) == Some(8) {
        let value = builder.ins().load(types::I8, MemFlags::new(), data, 0);
        return Ok(builder.ins().uextend(types::I32, value));
    }
    if type_table.unsigned_integer_bits(type_id) == Some(16) {
        let value = builder.ins().load(types::I16, MemFlags::new(), data, 0);
        return Ok(builder.ins().uextend(types::I32, value));
    }
    let clif_type = if is_i32_abi_compatible_type(type_id, type_table) {
        types::I32
    } else if type_id == TYPE_ID_F32 {
        types::F32
    } else if type_id == TYPE_ID_F64 {
        types::F64
    } else {
        return Err(format!("unsupported direct scalar load type {type_id}"));
    };
    Ok(builder.ins().load(clif_type, MemFlags::new(), data, 0))
}

fn emit_global_scalar_store(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    path: &str,
    type_id: TypeId,
    value: Value,
) -> Result<(), String> {
    if let Some(slot_ref) = runtime_call_refs
        .direct_storage
        .as_ref()
        .and_then(|bindings| bindings.scalars.get(path))
        .copied()
    {
        let data = emit_direct_slot_data_ptr(builder, slot_ref);
        let value = match type_table.unsigned_integer_bits(type_id) {
            Some(8) => builder.ins().ireduce(types::I8, value),
            Some(16) => builder.ins().ireduce(types::I16, value),
            _ => value,
        };
        builder.ins().store(MemFlags::new(), value, data, 0);
        return Ok(());
    }
    let path_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_global_path(path)));
    let helper = if is_i32_abi_compatible_type(type_id, type_table) {
        runtime_call_refs.global_i32_store
    } else if type_id == TYPE_ID_F32 {
        runtime_call_refs.global_f32_store
    } else if type_id == TYPE_ID_F64 {
        runtime_call_refs.global_f64_store
    } else {
        return Err(format!("unsupported global scalar store type {type_id}"));
    };
    builder.ins().call(helper, &[path_hash, value]);
    Ok(())
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
            let (lhs, rhs) = if matches!(lhs, SimpleExpr::Int(_)) {
                let rhs_value = emit_simple_expression(
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
                let lhs_expected = type_table
                    .unsigned_integer_bits(rhs_value.type_id)
                    .is_some()
                    .then_some(rhs_value.type_id);
                let lhs_value = emit_simple_expression(
                    builder,
                    lhs,
                    lhs_expected,
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
                (lhs_value, rhs_value)
            } else {
                let lhs_value = emit_simple_expression(
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
                let rhs_expected = type_table
                    .unsigned_integer_bits(lhs_value.type_id)
                    .is_some()
                    .then_some(lhs_value.type_id);
                let rhs_value = emit_simple_expression(
                    builder,
                    rhs,
                    rhs_expected,
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
                (lhs_value, rhs_value)
            };
            if is_i32_abi_compatible_type(lhs.type_id, type_table)
                && is_i32_abi_compatible_type(rhs.type_id, type_table)
            {
                let unsigned = type_table.unsigned_integer_bits(lhs.type_id).is_some()
                    || type_table.unsigned_integer_bits(rhs.type_id).is_some();
                let intcc = match op {
                    ComparisonOp::Eq => IntCC::Equal,
                    ComparisonOp::Ne => IntCC::NotEqual,
                    ComparisonOp::Lt if unsigned => IntCC::UnsignedLessThan,
                    ComparisonOp::Le if unsigned => IntCC::UnsignedLessThanOrEqual,
                    ComparisonOp::Gt if unsigned => IntCC::UnsignedGreaterThan,
                    ComparisonOp::Ge if unsigned => IntCC::UnsignedGreaterThanOrEqual,
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

fn build_direct_runtime_call_import_ids(
    module: &mut impl Module,
    fallback: FuncId,
    linkage: RuntimeHelperLinkage<'_>,
    uses_runtime_storage: bool,
    uses_collection_runtime: bool,
    referenced_call_targets: &BTreeSet<String>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
    debug_instrumentation: bool,
    profile_instrumentation: bool,
) -> Result<RuntimeCallImportIds, String> {
    let print_i32 = if referenced_call_targets
        .iter()
        .any(|target| matches!(target.as_str(), "print_i32" | "print_int" | "print_char"))
    {
        declare_void_call_import(module, "stasis_jit_print_i32", linkage, 1)?
    } else {
        fallback
    };
    let print_string = if referenced_call_targets.contains("print_string") {
        declare_void_call_import(module, "stasis_jit_print_string", linkage, 1)?
    } else {
        fallback
    };
    let sin_fast = if referenced_call_targets.contains("sin_fast") {
        declare_direct_f32_unary_import(module, "stasis_jit_sin_fast", linkage)?
    } else {
        fallback
    };
    let cos_fast = if referenced_call_targets.contains("cos_fast") {
        declare_direct_f32_unary_import(module, "stasis_jit_cos_fast", linkage)?
    } else {
        fallback
    };
    let referenced_extern_signatures: CallSignatureMap = call_signatures
        .iter()
        .filter(|(target, _)| referenced_call_targets.contains(*target))
        .map(|(target, signatures)| (target.clone(), signatures.clone()))
        .collect();
    macro_rules! storage_import {
        ($used:expr, $declaration:expr) => {
            if $used {
                $declaration?
            } else {
                fallback
            }
        };
    }
    Ok(RuntimeCallImportIds {
        print_i32,
        print_string,
        sin_fast,
        cos_fast,
        // Direct storage handles known standalone globals, but dynamic paths and
        // bounds fallbacks still use the runtime registry. Never alias a runtime
        // helper to the current function: their ABIs are unrelated.
        global_i32_load: storage_import!(
            uses_runtime_storage,
            declare_i32_call_import(module, "stasis_jit_global_i32_load", linkage, 1)
        ),
        global_i32_store: storage_import!(
            uses_runtime_storage,
            declare_void_call_import(module, "stasis_jit_global_i32_store", linkage, 2,)
        ),
        global_f32_load: storage_import!(
            uses_runtime_storage,
            declare_f32_global_load_import(module, "stasis_jit_global_f32_load", linkage,)
        ),
        global_f32_store: storage_import!(
            uses_runtime_storage,
            declare_f32_global_store_import(module, "stasis_jit_global_f32_store", linkage,)
        ),
        global_f64_load: storage_import!(
            uses_runtime_storage,
            declare_f64_global_load_import(module, "stasis_jit_global_f64_load", linkage,)
        ),
        global_f64_store: storage_import!(
            uses_runtime_storage,
            declare_f64_global_store_import(module, "stasis_jit_global_f64_store", linkage,)
        ),
        global_i32_array_load: storage_import!(
            uses_collection_runtime,
            declare_i32_array_load_import(module, "stasis_jit_global_i32_array_load", linkage,)
        ),
        global_i32_array_store: storage_import!(
            uses_collection_runtime,
            declare_i32_array_store_import(module, "stasis_jit_global_i32_array_store", linkage,)
        ),
        global_i32_array_ptr: storage_import!(
            uses_collection_runtime,
            declare_i32_array_ptr_import(module, "stasis_jit_global_i32_array_ptr", linkage,)
        ),
        global_f32_array_load: storage_import!(
            uses_collection_runtime,
            declare_f32_array_load_import(module, "stasis_jit_global_f32_array_load", linkage,)
        ),
        global_f32_array_store: storage_import!(
            uses_collection_runtime,
            declare_f32_array_store_import(module, "stasis_jit_global_f32_array_store", linkage,)
        ),
        global_f32_array_ptr: storage_import!(
            uses_collection_runtime,
            declare_f32_array_ptr_import(module, "stasis_jit_global_f32_array_ptr", linkage,)
        ),
        global_f64_array_load: storage_import!(
            uses_collection_runtime,
            declare_f64_array_load_import(module, "stasis_jit_global_f64_array_load", linkage,)
        ),
        global_f64_array_store: storage_import!(
            uses_collection_runtime,
            declare_f64_array_store_import(module, "stasis_jit_global_f64_array_store", linkage,)
        ),
        global_f64_array_ptr: storage_import!(
            uses_collection_runtime,
            declare_f64_array_ptr_import(module, "stasis_jit_global_f64_array_ptr", linkage,)
        ),
        collection_i32_load: storage_import!(
            uses_collection_runtime,
            declare_i32_call_import(module, "stasis_jit_collection_i32_load", linkage, 2,)
        ),
        collection_i32_store: storage_import!(
            uses_collection_runtime,
            declare_void_call_import(module, "stasis_jit_collection_i32_store", linkage, 3,)
        ),
        debug_frame_enter: debug_instrumentation
            .then(|| declare_void_call_import(module, "stasis_jit_debug_frame_enter", linkage, 1))
            .transpose()?,
        debug_frame_leave: debug_instrumentation
            .then(|| declare_void_call_import(module, "stasis_jit_debug_frame_leave", linkage, 1))
            .transpose()?,
        debug_statement: debug_instrumentation
            .then(|| declare_void_call_import(module, "stasis_jit_debug_statement", linkage, 2))
            .transpose()?,
        debug_values_begin: debug_instrumentation
            .then(|| declare_void_call_import(module, "stasis_jit_debug_values_begin", linkage, 0))
            .transpose()?,
        debug_value_i64: debug_instrumentation
            .then(|| declare_debug_value_i64_import(module, linkage))
            .transpose()?,
        debug_value_f64: debug_instrumentation
            .then(|| declare_debug_value_f64_import(module, linkage))
            .transpose()?,
        profile_frame_enter: profile_instrumentation
            .then(|| declare_void_call_import(module, "stasis_jit_profile_frame_enter", linkage, 1))
            .transpose()?,
        profile_frame_leave: profile_instrumentation
            .then(|| declare_void_call_import(module, "stasis_jit_profile_frame_leave", linkage, 1))
            .transpose()?,
        extern_calls: declare_extern_call_imports(
            module,
            &referenced_extern_signatures,
            type_table,
            named_struct_field_types,
            linkage,
        )?,
    })
}

pub(crate) fn build_runtime_call_refs(
    module: &mut impl Module,
    imports: &RuntimeCallImportIds,
    func: &mut cranelift_codegen::ir::Function,
    direct_storage: Option<&DirectStorageBindings>,
) -> Result<RuntimeCallRefs, String> {
    let direct_storage = direct_storage
        .map(|bindings| resolve_direct_storage_refs(module, func, bindings))
        .transpose()?;
    let debug = imports
        .debug_frame_enter
        .zip(imports.debug_frame_leave)
        .zip(imports.debug_statement)
        .zip(imports.debug_values_begin)
        .zip(imports.debug_value_i64)
        .zip(imports.debug_value_f64)
        .map(
            |(((((frame_enter, frame_leave), statement), values_begin), value_i64), value_f64)| {
                DebugRuntimeRefs {
                    frame_enter: module.declare_func_in_func(frame_enter, func),
                    frame_leave: module.declare_func_in_func(frame_leave, func),
                    statement: module.declare_func_in_func(statement, func),
                    values_begin: module.declare_func_in_func(values_begin, func),
                    value_i64: module.declare_func_in_func(value_i64, func),
                    value_f64: module.declare_func_in_func(value_f64, func),
                }
            },
        );
    let profile = imports
        .profile_frame_enter
        .zip(imports.profile_frame_leave)
        .map(|(frame_enter, frame_leave)| ProfileRuntimeRefs {
            frame_enter: module.declare_func_in_func(frame_enter, func),
            frame_leave: module.declare_func_in_func(frame_leave, func),
        });
    Ok(RuntimeCallRefs {
        print_i32: module.declare_func_in_func(imports.print_i32, func),
        print_string: module.declare_func_in_func(imports.print_string, func),
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
        debug,
        profile,
        extern_calls: imports
            .extern_calls
            .iter()
            .map(|(key, id)| (key.clone(), module.declare_func_in_func(*id, func)))
            .collect(),
        direct_storage,
    })
}

fn resolve_direct_storage_refs(
    module: &mut impl Module,
    func: &mut cranelift_codegen::ir::Function,
    bindings: &DirectStorageBindings,
) -> Result<DirectStorageRefs, String> {
    fn resolve(
        module: &mut impl Module,
        func: &mut cranelift_codegen::ir::Function,
        binding: &DirectStorageBinding,
    ) -> Result<DirectStorageRef, String> {
        match binding {
            DirectStorageBinding::Absolute(address) => Ok(DirectStorageRef::Absolute(*address)),
            DirectStorageBinding::Symbol(symbol) => {
                let data_id = module
                    .declare_data(symbol, Linkage::Import, true, false)
                    .map_err(|error| {
                        format!("failed to declare direct storage symbol '{symbol}': {error}")
                    })?;
                Ok(DirectStorageRef::Symbol(
                    module.declare_data_in_func(data_id, func),
                ))
            }
        }
    }

    Ok(DirectStorageRefs {
        scalars: bindings
            .scalars
            .iter()
            .map(|(path, binding)| Ok((path.clone(), resolve(module, func, binding)?)))
            .collect::<Result<_, String>>()?,
        arrays: bindings
            .arrays
            .iter()
            .map(|(key, binding)| {
                Ok((
                    key.clone(),
                    DirectArrayStorageRef {
                        slot: resolve(module, func, &binding.slot)?,
                        storage_bytes: binding.storage_bytes,
                        static_len: binding.static_len,
                    },
                ))
            })
            .collect::<Result<_, String>>()?,
        arrays_by_hash: bindings
            .arrays
            .iter()
            .map(|((path, field), binding)| {
                Ok((
                    (hash_global_path(path), hash_foreach_field_suffix(field)),
                    DirectArrayStorageRef {
                        slot: resolve(module, func, &binding.slot)?,
                        storage_bytes: binding.storage_bytes,
                        static_len: binding.static_len,
                    },
                ))
            })
            .collect::<Result<_, String>>()?,
    })
}
