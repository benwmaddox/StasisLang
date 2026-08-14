//! Browser WebAssembly emission for the scalar Stasis lane.
//!
//! This intentionally consumes the same parsed HIR as JIT/AOT. Unsupported
//! storage or expression shapes fail at package time instead of receiving a
//! target-specific substitute implementation.

use crate::backend::emit::{
    build_compile_analysis_cache, compute_files_fingerprint, hash_global_path,
    resolve_extern_call_signatures_with, AssignOp, AssignTarget, ComparisonOp, ConstantValue,
    SimpleCondition, SimpleExpr, SimpleStmt,
};
use crate::backend::program_snapshot::ProgramSnapshot;
use crate::compiler::{CompileError, CompileReport, CompileResult, Compiler, FunctionMeta};
use crate::frontend::types::{
    TypeCategory, TypeId, TypeTable, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_F64, TYPE_ID_I32,
    TYPE_ID_U16, TYPE_ID_U32, TYPE_ID_U8, TYPE_ID_VOID,
};
use crate::ir::hir::FunctionHIR;
use std::collections::{BTreeMap, BTreeSet};

const I32: u8 = 0x7f;
const F32: u8 = 0x7d;
const F64: u8 = 0x7c;

pub fn wasm_global_hash(path: &str) -> i32 {
    hash_global_path(path)
}

#[derive(Debug, Clone, Default)]
pub struct WasmProcess {
    compiler: Compiler,
    required_roots: Vec<String>,
    module: Vec<u8>,
    string_literals: BTreeMap<i32, String>,
    memory_layout: BTreeMap<String, WasmMemoryLayout>,
    struct_views: BTreeMap<i32, BTreeMap<String, String>>,
    debug_symbols: bool,
    global_types: BTreeMap<String, TypeId>,
    program_snapshot: Option<ProgramSnapshot>,
}

impl WasmProcess {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_project_root(&mut self, root: impl Into<String>) -> Result<(), String> {
        self.compiler.set_project_root(root)
    }

    pub fn set_required_emit_roots(&mut self, roots: &[String]) {
        self.required_roots = roots.to_vec();
        self.compiler.set_analysis_required_roots(roots);
    }

    pub fn set_debug_symbols(&mut self, enabled: bool) {
        self.debug_symbols = enabled;
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.compiler.upsert_file(path, content);
    }

    pub fn module_bytes(&self) -> &[u8] {
        &self.module
    }

    pub fn string_literals(&self) -> &BTreeMap<i32, String> {
        &self.string_literals
    }

    pub fn memory_layout(&self) -> &BTreeMap<String, WasmMemoryLayout> {
        &self.memory_layout
    }

    pub fn struct_views(&self) -> &BTreeMap<i32, BTreeMap<String, String>> {
        &self.struct_views
    }

    pub fn global_types(&self) -> &BTreeMap<String, TypeId> {
        &self.global_types
    }

    pub fn program_snapshot(&self) -> Option<&ProgramSnapshot> {
        self.program_snapshot.as_ref()
    }

    pub fn last_source_diagnostic(&self) -> Option<&crate::SourceDiagnostic> {
        self.compiler.last_source_diagnostic()
    }

    pub fn compile(&mut self) -> CompileResult<CompileReport> {
        let index = self.compiler.index_pass()?;
        let mut types = self.compiler.types().clone();
        let source_revision =
            crate::backend::program_snapshot::semantic_revision_with_required_roots(
                compute_files_fingerprint(self.compiler.files()),
                &self.required_roots,
            );
        let analysis = build_compile_analysis_cache(
            self.compiler.files(),
            self.compiler.functions(),
            &mut types,
            source_revision,
            |signatures| {
                resolve_extern_call_signatures_with(signatures, |_signature, _candidate| Some(0))
            },
        )
        .map_err(CompileError::Backend)?;
        *self.compiler.types_mut() = types.clone();

        let reachable = crate::backend::reachability::compute_reachable_function_ids(
            self.compiler.functions(),
            &self.required_roots,
        );
        let function_ids = self
            .compiler
            .functions()
            .iter()
            .filter(|function| reachable.contains(&function.id))
            .map(|function| function.id)
            .collect::<Vec<_>>();
        let mut lowered = Vec::new();
        let emit = self
            .compiler
            .emit_pass_for_ids_with(&function_ids, &mut |meta, hir, _| {
                lowered.push((meta.clone(), hir.clone()));
                Ok(())
            })?;

        for root in &self.required_roots {
            if root == "on_code_swap" {
                continue;
            }
            if !lowered.iter().any(|(function, _)| &function.name == root) {
                return Err(CompileError::Backend(format!(
                    "web package requires entry function '{root}'"
                )));
            }
        }

        self.string_literals = collect_string_literals(&lowered);
        let (memory_bindings, _) =
            build_memory_bindings(&analysis, &types).map_err(CompileError::Backend)?;
        self.memory_layout = memory_bindings
            .into_iter()
            .map(|(path, binding)| {
                (
                    path,
                    WasmMemoryLayout {
                        offset: binding.offset,
                        type_id: binding.type_id,
                        length: binding.len,
                        stride: binding.stride,
                    },
                )
            })
            .collect();
        self.struct_views.clear();
        self.global_types = analysis.global_path_types.clone();
        for (path, collection) in &analysis.collection_infos {
            if !collection.field_types.is_empty() {
                self.struct_views.insert(
                    hash_global_path(path),
                    collection
                        .field_types
                        .keys()
                        .map(|suffix| (suffix.clone(), format!("{path}.{suffix}")))
                        .collect(),
                );
            }
        }
        for (path, type_id) in &analysis.global_path_types {
            if let Some(fields) = analysis.named_struct_field_types.get(type_id) {
                self.struct_views.insert(
                    hash_global_path(path),
                    fields
                        .keys()
                        .map(|suffix| (suffix.clone(), format!("{path}.{suffix}")))
                        .collect(),
                );
            }
        }
        self.module = encode_module(&lowered, &analysis, &types, self.debug_symbols)
            .map_err(CompileError::Backend)?;
        self.program_snapshot = Some(
            ProgramSnapshot::build(
                source_revision,
                self.compiler.files(),
                self.compiler.module_graph(),
                self.compiler.functions(),
                &types,
                self.compiler.data_flow_summaries_shared(),
                &self.required_roots,
                analysis,
            )
            .map_err(CompileError::Backend)?,
        );
        Ok(CompileReport { index, emit })
    }
}

#[derive(Clone)]
struct Signature {
    params: Vec<TypeId>,
    result: TypeId,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct WasmSignature {
    params: Vec<u8>,
    result: Option<u8>,
}

fn lower_wasm_signature(
    signature: &Signature,
    named_structs: &crate::backend::emit::NamedStructFieldTypeMap,
) -> Result<WasmSignature, String> {
    let mut params = Vec::with_capacity(physical_param_count(&signature.params, named_structs));
    for type_id in &signature.params {
        if is_struct_view_type(*type_id, named_structs) {
            params.extend([I32, I32, I32]);
        } else {
            params.push(wasm_value_type(*type_id)?);
        }
    }
    let result = (signature.result != TYPE_ID_VOID)
        .then(|| wasm_value_type(signature.result))
        .transpose()?;
    Ok(WasmSignature { params, result })
}

fn intern_wasm_signature(
    signature: WasmSignature,
    indices: &mut BTreeMap<WasmSignature, u32>,
    signatures: &mut Vec<WasmSignature>,
) -> u32 {
    if let Some(index) = indices.get(&signature) {
        return *index;
    }
    let index = signatures.len() as u32;
    indices.insert(signature.clone(), index);
    signatures.push(signature);
    index
}

fn is_i32_lane(type_id: TypeId) -> bool {
    matches!(
        type_id,
        TYPE_ID_I32 | TYPE_ID_BOOL | TYPE_ID_U8 | TYPE_ID_U16 | TYPE_ID_U32
    )
}

fn is_web_index_type(type_id: TypeId, context: &EncodeContext<'_>) -> bool {
    wasm_value_type(type_id).is_ok_and(|value_type| value_type == I32)
        && !is_struct_view_type(type_id, context.named_structs)
        && context.types.indexed_element_type_id(type_id).is_none()
}

fn wasm_value_type(type_id: TypeId) -> Result<u8, String> {
    if is_i32_lane(type_id) {
        Ok(I32)
    } else if type_id == TYPE_ID_F32 {
        Ok(F32)
    } else if type_id == TYPE_ID_F64 {
        Ok(F64)
    } else if type_id != TYPE_ID_VOID {
        // String/view handles and opaque host handles cross the web ABI as i32.
        Ok(I32)
    } else {
        Err("void is not a WebAssembly value".to_string())
    }
}

fn validate_signature(name: &str, signature: &Signature) -> Result<(), String> {
    for type_id in &signature.params {
        wasm_value_type(*type_id)
            .map_err(|_| format!("web backend does not support parameter type for '{name}'"))?;
    }
    if signature.result != TYPE_ID_VOID {
        wasm_value_type(signature.result)
            .map_err(|_| format!("web backend does not support return type for '{name}'"))?;
    }
    Ok(())
}

fn is_struct_view_type(
    type_id: TypeId,
    named_structs: &crate::backend::emit::NamedStructFieldTypeMap,
) -> bool {
    named_structs.contains_key(&type_id)
}

fn physical_param_count(
    params: &[TypeId],
    named_structs: &crate::backend::emit::NamedStructFieldTypeMap,
) -> usize {
    params
        .iter()
        .map(|type_id| {
            usize::from(!is_struct_view_type(*type_id, named_structs))
                + 3 * usize::from(is_struct_view_type(*type_id, named_structs))
        })
        .sum()
}

#[derive(Debug, Clone)]
struct MemoryBinding {
    offset: u32,
    type_id: TypeId,
    len: i32,
    width: u32,
    stride: u32,
}

#[derive(Debug, Clone)]
struct StructCollectionBinding {
    base: i32,
    type_id: TypeId,
    len: i32,
    fields: BTreeMap<String, MemoryBinding>,
}

#[derive(Debug, Clone)]
struct StructScalarBinding {
    base: i32,
    type_id: TypeId,
    fields: BTreeMap<String, (u32, TypeId)>,
}

fn build_struct_scalars(
    analysis: &crate::backend::emit::CompileAnalysisCache,
    globals: &BTreeMap<String, u32>,
) -> BTreeMap<String, StructScalarBinding> {
    let mut out = BTreeMap::new();
    for (path, type_id) in &analysis.global_path_types {
        let Some(field_types) = analysis.named_struct_field_types.get(type_id) else {
            continue;
        };
        let fields = field_types
            .iter()
            .filter_map(|(suffix, field_type)| {
                globals
                    .get(&format!("{path}.{suffix}"))
                    .copied()
                    .map(|index| (suffix.clone(), (index, *field_type)))
            })
            .collect::<BTreeMap<_, _>>();
        if !fields.is_empty() {
            out.insert(
                path.clone(),
                StructScalarBinding {
                    base: hash_global_path(path),
                    type_id: *type_id,
                    fields,
                },
            );
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmMemoryLayout {
    pub offset: u32,
    pub type_id: TypeId,
    pub length: i32,
    pub stride: u32,
}

fn storage_width(
    type_id: TypeId,
    types: &TypeTable,
    named_structs: &crate::backend::emit::NamedStructFieldTypeMap,
) -> Result<u32, String> {
    match type_id {
        TYPE_ID_BOOL | TYPE_ID_U8 => Ok(1),
        TYPE_ID_U16 => Ok(2),
        TYPE_ID_I32 | TYPE_ID_U32 | TYPE_ID_F32 => Ok(4),
        TYPE_ID_F64 => Ok(8),
        _ if named_structs.contains_key(&type_id) => Err(format!(
            "web memory requires flattened fields for struct type id {type_id}"
        )),
        _ => match types.type_info(type_id).map(|info| info.category) {
            Some(TypeCategory::Named)
            | Some(TypeCategory::ArrayView)
            | Some(TypeCategory::AsciiView)
            | Some(TypeCategory::Utf8View) => Ok(4),
            Some(category) => Err(format!(
                "web memory does not support {category:?} element type id {type_id}"
            )),
            None => Err(format!(
                "web memory found unknown element type id {type_id}"
            )),
        },
    }
}

fn align_up(value: u32, alignment: u32) -> Result<u32, String> {
    value
        .checked_add(alignment - 1)
        .map(|value| value / alignment * alignment)
        .ok_or_else(|| "web memory layout overflow".to_string())
}

fn build_memory_bindings(
    analysis: &crate::backend::emit::CompileAnalysisCache,
    types: &TypeTable,
) -> Result<(BTreeMap<String, MemoryBinding>, u32), String> {
    let mut offset = 0u32;
    let mut bindings = BTreeMap::new();
    for (path, collection) in &analysis.collection_infos {
        if let Some(type_id) = collection.element_type {
            let width = storage_width(type_id, types, &analysis.named_struct_field_types).map_err(
                |error| format!("web collection '{path}' has unsupported element storage: {error}"),
            )?;
            offset = align_up(offset, width)?;
            bindings.insert(
                path.clone(),
                MemoryBinding {
                    offset,
                    type_id,
                    len: collection.len,
                    width,
                    stride: width,
                },
            );
            offset = offset
                .checked_add(
                    u32::try_from(collection.len)
                        .map_err(|_| format!("negative web collection length for '{path}'"))?
                        .checked_mul(width)
                        .ok_or_else(|| "web memory layout overflow".to_string())?,
                )
                .ok_or_else(|| "web memory layout overflow".to_string())?;
        }
        for (field, type_id) in &collection.field_types {
            let width = storage_width(*type_id, types, &analysis.named_struct_field_types)
                .map_err(|error| {
                    format!(
                        "web collection '{path}.{field}' has unsupported field storage: {error}"
                    )
                })?;
            offset = align_up(offset, width)?;
            let field_path = format!("{path}.{field}");
            bindings.insert(
                field_path,
                MemoryBinding {
                    offset,
                    type_id: *type_id,
                    len: collection.len,
                    width,
                    stride: width,
                },
            );
            offset = offset
                .checked_add(
                    u32::try_from(collection.len)
                        .map_err(|_| format!("negative web collection length for '{path}'"))?
                        .checked_mul(width)
                        .ok_or_else(|| "web memory layout overflow".to_string())?,
                )
                .ok_or_else(|| "web memory layout overflow".to_string())?;
        }
    }
    Ok((bindings, offset))
}

fn build_struct_collections(
    analysis: &crate::backend::emit::CompileAnalysisCache,
    types: &TypeTable,
    memory: &BTreeMap<String, MemoryBinding>,
) -> Result<BTreeMap<String, StructCollectionBinding>, String> {
    let mut out = BTreeMap::new();
    for (path, collection) in &analysis.collection_infos {
        if collection.field_types.is_empty() {
            continue;
        }
        let collection_type = analysis
            .global_path_types
            .get(path)
            .copied()
            .ok_or_else(|| format!("web struct collection '{path}' has no declared type"))?;
        let type_id = types
            .indexed_element_type_id(collection_type)
            .ok_or_else(|| format!("web struct collection '{path}' has no indexed element type"))?;
        if !analysis.named_struct_field_types.contains_key(&type_id) {
            return Err(format!(
                "web struct collection '{path}' element type {type_id} has no field layout"
            ));
        }
        let mut fields = BTreeMap::new();
        for suffix in collection.field_types.keys() {
            let field_path = format!("{path}.{suffix}");
            fields.insert(
                suffix.clone(),
                memory
                    .get(&field_path)
                    .cloned()
                    .ok_or_else(|| format!("missing web SoA field plane '{field_path}'"))?,
            );
        }
        out.insert(
            path.clone(),
            StructCollectionBinding {
                base: hash_global_path(path),
                type_id,
                len: collection.len,
                fields,
            },
        );
    }
    Ok(out)
}

fn encode_module(
    functions: &[(FunctionMeta, FunctionHIR)],
    analysis: &crate::backend::emit::CompileAnalysisCache,
    types: &TypeTable,
    debug_symbols: bool,
) -> Result<Vec<u8>, String> {
    let mut internal_by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, (function, _)) in functions.iter().enumerate() {
        internal_by_name
            .entry(function.name.clone())
            .or_default()
            .push(index);
        internal_by_name
            .entry(format!("{}.{}", function.module_alias, function.name))
            .or_default()
            .push(index);
    }

    let mut called = BTreeSet::new();
    for (_, hir) in functions {
        collect_calls(&hir.statements, &mut called);
    }
    let mut imports = Vec::new();
    for signature in &analysis.resolved_extern_signatures {
        if !called.contains(&signature.name) {
            continue;
        }
        let value = Signature {
            params: signature.params.clone(),
            result: signature.return_type,
        };
        validate_signature(&signature.name, &value)?;
        imports.push((signature.name.clone(), signature.symbol.clone(), value));
    }
    for name in ["sin_fast", "cos_fast"] {
        if called.contains(name) {
            imports.push((
                name.to_string(),
                name.to_string(),
                Signature {
                    params: vec![TYPE_ID_F32],
                    result: TYPE_ID_F32,
                },
            ));
        }
    }
    for name in ["print_i32", "print_int", "print_char", "print_string"] {
        if called.contains(name) {
            imports.push((
                name.to_string(),
                name.to_string(),
                Signature {
                    params: vec![TYPE_ID_I32],
                    result: TYPE_ID_VOID,
                },
            ));
        }
    }
    imports.sort_by(|left, right| left.0.cmp(&right.0));

    let imported_names = imports
        .iter()
        .map(|(name, _, _)| name.clone())
        .collect::<BTreeSet<_>>();
    for target in &called {
        if !internal_by_name.contains_key(target)
            && !imported_names.contains(target)
            && !is_inline_intrinsic(target)
        {
            return Err(format!("unresolved web call target '{target}'"));
        }
        if internal_by_name
            .get(target)
            .is_some_and(|candidates| candidates.len() > 1)
        {
            return Err(format!(
                "web backend cannot yet resolve called overload family '{target}'"
            ));
        }
    }

    let mut signatures = imports
        .iter()
        .map(|(_, _, signature)| signature.clone())
        .collect::<Vec<_>>();
    for (function, _) in functions {
        let signature = Signature {
            params: function.params.clone(),
            result: function.return_type,
        };
        validate_signature(&function.name, &signature)?;
        signatures.push(signature);
    }

    let (memory_bindings, memory_bytes) = build_memory_bindings(analysis, types)?;
    let struct_collections = build_struct_collections(analysis, types, &memory_bindings)?;
    let mut globals = Vec::new();
    for (name, type_id) in &analysis.global_path_types {
        if memory_bindings.contains_key(name) {
            continue;
        }
        let Some(info) = types.type_info(*type_id) else {
            return Err(format!(
                "web backend found unknown global type id {type_id}"
            ));
        };
        if (info.category == TypeCategory::Named
            && analysis.named_struct_field_types.contains_key(type_id))
            || matches!(
                info.category,
                TypeCategory::ArrayFixed
                    | TypeCategory::ArrayView
                    | TypeCategory::AsciiFixed
                    | TypeCategory::AsciiView
                    | TypeCategory::Utf8Fixed
                    | TypeCategory::Utf8View
            )
        {
            continue;
        }
        if wasm_value_type(*type_id).is_err() {
            return Err(format!(
                "web backend does not support global '{name}' with type {}",
                info.name
            ));
        }
        let initial_i32 = [".length", ".max_length"].iter().find_map(|suffix| {
            name.strip_suffix(suffix)
                .and_then(|path| analysis.collection_infos.get(path))
                .map(|collection| collection.len)
        });
        globals.push((name.clone(), *type_id, initial_i32));
    }
    let global_indices = globals
        .iter()
        .enumerate()
        .map(|(index, (name, _, _))| (name.clone(), index as u32))
        .collect::<BTreeMap<_, _>>();
    let struct_scalars = build_struct_scalars(analysis, &global_indices);

    let accessor_signatures = [
        WasmSignature {
            params: vec![I32],
            result: Some(I32),
        },
        WasmSignature {
            params: vec![I32, I32],
            result: Some(I32),
        },
        WasmSignature {
            params: vec![I32],
            result: Some(F32),
        },
        WasmSignature {
            params: vec![I32, F32],
            result: Some(I32),
        },
    ];
    let mut wasm_signatures = Vec::new();
    let mut wasm_signature_indices = BTreeMap::new();
    let signature_type_indices = signatures
        .iter()
        .map(|signature| {
            lower_wasm_signature(signature, &analysis.named_struct_field_types).map(|signature| {
                intern_wasm_signature(signature, &mut wasm_signature_indices, &mut wasm_signatures)
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let accessor_type_indices = accessor_signatures.map(|signature| {
        intern_wasm_signature(signature, &mut wasm_signature_indices, &mut wasm_signatures)
    });

    let import_indices = imports
        .iter()
        .enumerate()
        .map(|(index, (name, _, _))| (name.clone(), index as u32))
        .collect::<BTreeMap<_, _>>();
    let mut internal_indices = BTreeMap::new();
    for (index, (function, _)) in functions.iter().enumerate() {
        let function_index = (imports.len() + index) as u32;
        for name in [
            function.name.clone(),
            format!("{}.{}", function.module_alias, function.name),
        ] {
            if internal_by_name
                .get(&name)
                .is_some_and(|candidates| candidates.len() == 1)
            {
                internal_indices.insert(name, function_index);
            }
        }
    }

    let mut module = b"\0asm\x01\0\0\0".to_vec();

    let mut type_section = Vec::new();
    uleb(wasm_signatures.len() as u32, &mut type_section);
    for signature in &wasm_signatures {
        type_section.push(0x60);
        uleb(signature.params.len() as u32, &mut type_section);
        type_section.extend(&signature.params);
        match signature.result {
            Some(result) => type_section.extend([1, result]),
            None => type_section.push(0),
        }
    }
    section(1, type_section, &mut module);

    if !imports.is_empty() {
        let mut import_section = Vec::new();
        uleb(imports.len() as u32, &mut import_section);
        for (index, (_, symbol, _)) in imports.iter().enumerate() {
            string("env", &mut import_section);
            string(symbol, &mut import_section);
            import_section.push(0);
            uleb(signature_type_indices[index], &mut import_section);
        }
        section(2, import_section, &mut module);
    }

    let mut function_section = Vec::new();
    uleb(functions.len() as u32 + 4, &mut function_section);
    for index in 0..functions.len() {
        uleb(
            signature_type_indices[imports.len() + index],
            &mut function_section,
        );
    }
    for index in accessor_type_indices {
        uleb(index, &mut function_section);
    }
    section(3, function_section, &mut module);

    if memory_bytes > 0 {
        let mut memory_section = vec![1, 0];
        uleb(memory_bytes.div_ceil(65_536).max(1), &mut memory_section);
        section(5, memory_section, &mut module);
    }

    if !globals.is_empty() {
        let mut global_section = Vec::new();
        uleb(globals.len() as u32, &mut global_section);
        for (_, type_id, initial_i32) in &globals {
            global_section.extend([wasm_value_type(*type_id)?, 1]);
            if let Some(value) = initial_i32 {
                global_section.push(0x41);
                sleb(*value, &mut global_section);
            } else {
                encode_zero(*type_id, &mut global_section)?;
            }
            global_section.push(0x0b);
        }
        section(6, global_section, &mut module);
    }

    let mut export_section = Vec::new();
    uleb(
        functions
            .iter()
            .filter(|(function, _)| {
                matches!(
                    function.name.as_str(),
                    "main" | "tick" | "render" | "on_code_swap"
                )
            })
            .count() as u32
            + if debug_symbols {
                globals.len() as u32
            } else {
                0
            }
            + u32::from(memory_bytes > 0)
            + 4,
        &mut export_section,
    );
    for (index, (function, _)) in functions.iter().enumerate() {
        if !matches!(
            function.name.as_str(),
            "main" | "tick" | "render" | "on_code_swap"
        ) {
            continue;
        }
        string(&function.name, &mut export_section);
        export_section.push(0);
        uleb((imports.len() + index) as u32, &mut export_section);
    }
    if debug_symbols {
        for (index, (name, _, _)) in globals.iter().enumerate() {
            string(name, &mut export_section);
            export_section.push(3);
            uleb(index as u32, &mut export_section);
        }
    }
    if memory_bytes > 0 {
        string("memory", &mut export_section);
        export_section.push(2);
        uleb(0, &mut export_section);
    }
    let accessor_base = (imports.len() + functions.len()) as u32;
    for (offset, name) in [
        "__stasis_global_get_i32",
        "__stasis_global_set_i32",
        "__stasis_global_get_f32",
        "__stasis_global_set_f32",
    ]
    .iter()
    .enumerate()
    {
        string(name, &mut export_section);
        export_section.push(0);
        uleb(accessor_base + offset as u32, &mut export_section);
    }
    section(7, export_section, &mut module);

    let mut code_section = Vec::new();
    uleb(functions.len() as u32 + 4, &mut code_section);
    for (function, hir) in functions {
        let body = encode_function(
            function,
            hir,
            &analysis.constant_values,
            &global_indices,
            &analysis.global_path_types,
            &memory_bindings,
            &struct_collections,
            &struct_scalars,
            &analysis.named_struct_field_types,
            types,
            &import_indices,
            &internal_indices,
            &signatures,
        )?;
        uleb(body.len() as u32, &mut code_section);
        code_section.extend(body);
    }
    for (lane, setter) in [(I32, false), (I32, true), (F32, false), (F32, true)] {
        let body = encode_global_accessor(&globals, lane, setter)?;
        uleb(body.len() as u32, &mut code_section);
        code_section.extend(body);
    }
    section(10, code_section, &mut module);
    let function_names = if debug_symbols {
        let mut names = imports
            .iter()
            .enumerate()
            .map(|(index, (_, symbol, _))| (index as u32, symbol.clone()))
            .collect::<Vec<_>>();
        names.extend(functions.iter().enumerate().map(|(index, (function, _))| {
            (
                (imports.len() + index) as u32,
                format!("{}.{}", function.module_alias, function.name),
            )
        }));
        names
    } else {
        functions
            .iter()
            .enumerate()
            .filter(|(_, (function, _))| {
                matches!(
                    function.name.as_str(),
                    "main" | "tick" | "render" | "on_code_swap"
                )
            })
            .map(|(index, (function, _))| ((imports.len() + index) as u32, function.name.clone()))
            .collect()
    };
    append_name_section(&function_names, &mut module);
    Ok(module)
}

fn encode_global_accessor(
    globals: &[(String, TypeId, Option<i32>)],
    lane: u8,
    setter: bool,
) -> Result<Vec<u8>, String> {
    fn branch(
        globals: &[(usize, &(String, TypeId, Option<i32>))],
        lane: u8,
        setter: bool,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        let Some(((index, (name, type_id, _)), rest)) = globals.split_first() else {
            if setter || lane == I32 {
                out.extend([0x41, 0]);
            } else {
                out.push(0x43);
                out.extend(0.0f32.to_le_bytes());
            }
            return Ok(());
        };
        out.extend([0x20, 0, 0x41]);
        sleb(hash_global_path(name), out);
        out.extend([0x46, 0x04, if setter { I32 } else { lane }]);
        if setter {
            out.extend([0x20, 1, 0x24]);
            uleb(*index as u32, out);
            out.extend([0x41, 1]);
        } else {
            out.push(0x23);
            uleb(*index as u32, out);
        }
        out.push(0x05);
        branch(rest, lane, setter, out)?;
        out.push(0x0b);
        let _ = type_id;
        Ok(())
    }

    let matching = globals
        .iter()
        .enumerate()
        .filter(|(_, (_, type_id, _))| wasm_value_type(*type_id).is_ok_and(|value| value == lane))
        .collect::<Vec<_>>();
    let mut body = vec![0];
    branch(&matching, lane, setter, &mut body)?;
    body.push(0x0b);
    Ok(body)
}

fn append_name_section(function_names: &[(u32, String)], module: &mut Vec<u8>) {
    let mut function_subsection = Vec::new();
    uleb(function_names.len() as u32, &mut function_subsection);
    for (index, name) in function_names {
        uleb(*index, &mut function_subsection);
        string(name, &mut function_subsection);
    }
    let mut payload = Vec::new();
    string("name", &mut payload);
    payload.push(1);
    uleb(function_subsection.len() as u32, &mut payload);
    payload.extend(function_subsection);
    section(0, payload, module);
}

fn collect_calls(statements: &[SimpleStmt], out: &mut BTreeSet<String>) {
    fn expression(value: &SimpleExpr, out: &mut BTreeSet<String>) {
        match value {
            SimpleExpr::Call { target, args } => {
                out.insert(target.clone());
                for arg in args {
                    expression(arg, out);
                }
            }
            SimpleExpr::Binary { lhs, rhs, .. } => {
                expression(lhs, out);
                expression(rhs, out);
            }
            SimpleExpr::Condition(condition) => condition_calls(condition, out),
            SimpleExpr::IndexedPath { index, .. } => expression(index, out),
            _ => {}
        }
    }
    fn condition_calls(value: &SimpleCondition, out: &mut BTreeSet<String>) {
        match value {
            SimpleCondition::Comparison { lhs, rhs, .. } => {
                expression(lhs, out);
                expression(rhs, out);
            }
            SimpleCondition::Expr(value) => expression(value, out),
            SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
                condition_calls(lhs, out);
                condition_calls(rhs, out);
            }
            SimpleCondition::Not(value) => condition_calls(value, out),
        }
    }
    for statement in statements {
        match statement {
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
                condition_calls(condition, out);
                collect_calls(then_statements, out);
                if let Some(values) = else_statements {
                    collect_calls(values, out);
                }
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                collect_calls(std::slice::from_ref(init), out);
                condition_calls(condition, out);
                collect_calls(std::slice::from_ref(step), out);
                collect_calls(body_statements, out);
            }
            SimpleStmt::Foreach {
                body_statements, ..
            } => collect_calls(body_statements, out),
            SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_function(
    function: &FunctionMeta,
    hir: &FunctionHIR,
    constants: &BTreeMap<String, ConstantValue>,
    globals: &BTreeMap<String, u32>,
    global_types: &BTreeMap<String, TypeId>,
    memory: &BTreeMap<String, MemoryBinding>,
    struct_collections: &BTreeMap<String, StructCollectionBinding>,
    struct_scalars: &BTreeMap<String, StructScalarBinding>,
    named_structs: &crate::backend::emit::NamedStructFieldTypeMap,
    types: &TypeTable,
    imports: &BTreeMap<String, u32>,
    internals: &BTreeMap<String, u32>,
    signatures: &[Signature],
) -> Result<Vec<u8>, String> {
    let mut local_declarations = Vec::new();
    collect_locals(&hir.statements, &mut local_declarations)?;
    let mut locals = BTreeMap::new();
    let mut physical_cursor = 0u32;
    for (name, type_id) in function.param_names.iter().zip(function.params.iter()) {
        let struct_view =
            is_struct_view_type(*type_id, named_structs).then_some(StructViewBinding {
                index: physical_cursor + 1,
                len: physical_cursor + 2,
            });
        locals.insert(
            name.clone(),
            LocalBinding {
                index: physical_cursor,
                type_id: *type_id,
                struct_view,
            },
        );
        physical_cursor += if struct_view.is_some() { 3 } else { 1 };
    }
    for (name, type_id) in &local_declarations {
        if locals.contains_key(name) {
            return Err(format!("duplicate local '{name}' in '{}'", function.name));
        }
        locals.insert(
            name.clone(),
            LocalBinding {
                index: physical_cursor,
                type_id: *type_id,
                struct_view: is_struct_view_type(*type_id, named_structs).then_some(
                    StructViewBinding {
                        index: physical_cursor + 1,
                        len: physical_cursor + 2,
                    },
                ),
            },
        );
        physical_cursor += if is_struct_view_type(*type_id, named_structs) {
            3
        } else {
            1
        };
    }

    let mut local_types = Vec::new();
    for (_, type_id) in &local_declarations {
        if is_struct_view_type(*type_id, named_structs) {
            for _ in 0..3 {
                local_types.push(I32);
            }
        } else {
            local_types.push(wasm_value_type(*type_id)?);
        }
    }
    let scratch_index = physical_cursor;
    let scratch_i32 = scratch_index + 1;
    let scratch_i32_b = scratch_index + 2;
    let scratch_i32_c = scratch_index + 3;
    let scratch_f32 = scratch_index + 4;
    let scratch_f64 = scratch_index + 5;
    local_types.extend([I32, I32, I32, I32, F32, F64]);
    let mut body = Vec::new();
    encode_local_declarations(&local_types, &mut body);
    let context = EncodeContext {
        locals: &locals,
        globals,
        global_types,
        memory,
        struct_collections,
        struct_scalars,
        named_structs,
        types,
        constants,
        imports,
        internals,
        signatures,
        scratch_index,
        return_type: function.return_type,
        scratch_i32,
        scratch_i32_b,
        scratch_i32_c,
        scratch_f32,
        scratch_f64,
        foreach: BTreeMap::new(),
    };
    encode_statements(&hir.statements, &context, &mut body)?;
    // Structured statements use void block types, so only a direct return proves that
    // the function end does not need its declared result on the operand stack.
    if function.return_type != TYPE_ID_VOID && !ends_with_explicit_return(&hir.statements) {
        encode_zero(function.return_type, &mut body)?;
    }
    body.push(0x0b);
    Ok(body)
}

fn encode_local_declarations(types: &[u8], out: &mut Vec<u8>) {
    let mut runs = Vec::new();
    for &value_type in types {
        if let Some((count, previous)) = runs.last_mut() {
            if *previous == value_type {
                *count += 1;
                continue;
            }
        }
        runs.push((1u32, value_type));
    }
    uleb(runs.len() as u32, out);
    for (count, value_type) in runs {
        uleb(count, out);
        out.push(value_type);
    }
}

fn ends_with_explicit_return(statements: &[SimpleStmt]) -> bool {
    matches!(
        statements.last(),
        Some(SimpleStmt::Return(_) | SimpleStmt::ReturnVoid)
    )
}

fn collect_locals(
    statements: &[SimpleStmt],
    out: &mut Vec<(String, TypeId)>,
) -> Result<(), String> {
    for statement in statements {
        match statement {
            SimpleStmt::Let { name, type_id, .. } => {
                let type_id = type_id.ok_or_else(|| {
                    format!("web backend requires an explicit type for local '{name}'")
                })?;
                wasm_value_type(type_id)?;
                collect_local(name, type_id, out)?;
            }
            SimpleStmt::If {
                then_statements,
                else_statements,
                ..
            } => {
                collect_locals(then_statements, out)?;
                if let Some(values) = else_statements {
                    collect_locals(values, out)?;
                }
            }
            SimpleStmt::For {
                init,
                step,
                body_statements,
                ..
            } => {
                collect_locals(std::slice::from_ref(init), out)?;
                collect_locals(std::slice::from_ref(step), out)?;
                collect_locals(body_statements, out)?;
            }
            SimpleStmt::Foreach {
                item_name,
                index_name,
                body_statements,
                ..
            } => {
                let index_name = foreach_index_name(item_name, index_name.as_deref());
                collect_local(&index_name, TYPE_ID_I32, out)?;
                collect_locals(body_statements, out)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn collect_local(
    name: &str,
    type_id: TypeId,
    out: &mut Vec<(String, TypeId)>,
) -> Result<(), String> {
    if let Some((_, existing)) = out.iter().find(|(existing, _)| existing == name) {
        if *existing == type_id {
            return Ok(());
        }
        return Err(format!(
            "web local '{name}' is redeclared with conflicting types {existing} and {type_id}"
        ));
    }
    out.push((name.to_string(), type_id));
    Ok(())
}

#[derive(Clone)]
struct EncodeContext<'a> {
    locals: &'a BTreeMap<String, LocalBinding>,
    globals: &'a BTreeMap<String, u32>,
    global_types: &'a BTreeMap<String, TypeId>,
    memory: &'a BTreeMap<String, MemoryBinding>,
    struct_collections: &'a BTreeMap<String, StructCollectionBinding>,
    struct_scalars: &'a BTreeMap<String, StructScalarBinding>,
    named_structs: &'a crate::backend::emit::NamedStructFieldTypeMap,
    types: &'a TypeTable,
    constants: &'a BTreeMap<String, ConstantValue>,
    imports: &'a BTreeMap<String, u32>,
    internals: &'a BTreeMap<String, u32>,
    signatures: &'a [Signature],
    scratch_index: u32,
    return_type: TypeId,
    scratch_i32: u32,
    scratch_i32_b: u32,
    scratch_i32_c: u32,
    scratch_f32: u32,
    scratch_f64: u32,
    foreach: BTreeMap<String, WebForeachBinding>,
}

#[derive(Debug, Clone)]
struct WebForeachBinding {
    collection_path: String,
    index_name: String,
}

fn foreach_index_name(item_name: &str, index_name: Option<&str>) -> String {
    index_name
        .map(str::to_string)
        .unwrap_or_else(|| format!("__web_foreach_index_{item_name}"))
}

#[derive(Debug, Clone, Copy)]
struct LocalBinding {
    index: u32,
    type_id: TypeId,
    struct_view: Option<StructViewBinding>,
}

#[derive(Debug, Clone, Copy)]
struct StructViewBinding {
    index: u32,
    len: u32,
}

fn set_struct_view_locals(base: u32, view: StructViewBinding, out: &mut Vec<u8>) {
    for index in [view.len, view.index, base] {
        out.push(0x21);
        uleb(index, out);
    }
}

fn require_same_struct_type(expected: TypeId, actual: TypeId, context: &str) -> Result<(), String> {
    if expected == actual {
        Ok(())
    } else {
        Err(format!(
            "web {context} struct type mismatch: expected {expected}, found {actual}"
        ))
    }
}

fn encode_statements(
    statements: &[SimpleStmt],
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    for statement in statements {
        match statement {
            SimpleStmt::Noop => {}
            SimpleStmt::Let {
                name, expression, ..
            } => {
                let binding = local_binding(context, name)?;
                if let Some(view) = binding.struct_view {
                    let value_type = encode_struct_view_expr(expression, context, out)?;
                    require_same_struct_type(binding.type_id, value_type, "local initializer")?;
                    set_struct_view_locals(binding.index, view, out);
                    continue;
                }
                let value_type = encode_expr_as(expression, Some(binding.type_id), context, out)?;
                require_same_type(binding.type_id, value_type, "local initializer")?;
                out.push(0x21);
                uleb(binding.index, out);
            }
            SimpleStmt::Assign {
                target,
                op,
                expression,
            } => {
                if *op == AssignOp::Set {
                    if let AssignTarget::IndexedPath {
                        collection_path,
                        index,
                        suffix,
                    } = target
                    {
                        if suffix.is_empty()
                            && context.struct_collections.contains_key(collection_path)
                        {
                            encode_struct_collection_copy(
                                collection_path,
                                index,
                                expression,
                                context,
                                out,
                            )?;
                            continue;
                        }
                    }
                }
                let target_type = target_type(target, context)?;
                if *op == AssignOp::Set && is_struct_view_type(target_type, context.named_structs) {
                    let AssignTarget::Local(name) = target else {
                        return Err(
                            "web struct views can only be assigned to local bindings".to_string()
                        );
                    };
                    let binding = local_binding(context, name)?;
                    let view = binding.struct_view.ok_or_else(|| {
                        format!("web struct local '{name}' is missing view storage")
                    })?;
                    let value_type = encode_struct_view_expr(expression, context, out)?;
                    require_same_struct_type(target_type, value_type, "assignment")?;
                    set_struct_view_locals(binding.index, view, out);
                    continue;
                }
                if *op != AssignOp::Set {
                    encode_target_get(target, context, out)?;
                }
                let value_type = encode_expr_as(expression, Some(target_type), context, out)?;
                require_same_type(target_type, value_type, "assignment")?;
                if *op != AssignOp::Set {
                    out.push(arithmetic_opcode(*op, target_type)?);
                }
                encode_target_set(target, context, out)?;
            }
            SimpleStmt::Expr(expression) => {
                encode_expr(expression, context, out)?;
                if expression_returns_value(expression, context)? {
                    out.push(0x1a);
                }
            }
            SimpleStmt::Return(expression) => {
                encode_expr_as(expression, Some(context.return_type), context, out)?;
                out.push(0x0f);
            }
            SimpleStmt::ReturnVoid => out.push(0x0f),
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                encode_condition(condition, context, out)?;
                out.extend([0x04, 0x40]);
                encode_statements(then_statements, context, out)?;
                if let Some(values) = else_statements {
                    out.push(0x05);
                    encode_statements(values, context, out)?;
                }
                out.push(0x0b);
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                encode_statements(std::slice::from_ref(init), context, out)?;
                out.extend([0x02, 0x40, 0x03, 0x40]);
                encode_condition(condition, context, out)?;
                out.extend([0x45, 0x0d, 0x01]);
                encode_statements(body_statements, context, out)?;
                encode_statements(std::slice::from_ref(step), context, out)?;
                out.extend([0x0c, 0x00, 0x0b, 0x0b]);
            }
            SimpleStmt::Continue => {
                return Err("web scalar lane does not yet support continue".to_string())
            }
            SimpleStmt::Convert { target, source, .. } => {
                let target_type = target_type(target, context)?;
                let source_type = encode_expr(source, context, out)?;
                encode_conversion(source_type, target_type, out)?;
                encode_target_set(target, context, out)?;
            }
            SimpleStmt::Foreach {
                item_name,
                index_name,
                collection_path,
                body_statements,
            } => {
                let index_name = foreach_index_name(item_name, index_name.as_deref());
                let index = local_binding(context, &index_name)?;
                let len = collection_len(context, collection_path)?;
                out.extend([0x41, 0, 0x21]);
                uleb(index.index, out);
                out.extend([0x02, 0x40, 0x03, 0x40, 0x20]);
                uleb(index.index, out);
                out.push(0x41);
                sleb(len, out);
                out.extend([0x4e, 0x0d, 0x01]);
                let mut nested = context.clone();
                nested.foreach.insert(
                    item_name.clone(),
                    WebForeachBinding {
                        collection_path: collection_path.clone(),
                        index_name: index_name.clone(),
                    },
                );
                encode_statements(body_statements, &nested, out)?;
                out.extend([0x20]);
                uleb(index.index, out);
                out.extend([0x41, 1, 0x6a, 0x21]);
                uleb(index.index, out);
                out.extend([0x0c, 0x00, 0x0b, 0x0b]);
            }
        }
    }
    Ok(())
}

fn encode_conversion(from: TypeId, to: TypeId, out: &mut Vec<u8>) -> Result<(), String> {
    let from = wasm_value_type(from)?;
    let to = wasm_value_type(to)?;
    if from == to {
        return Ok(());
    }
    out.push(match (from, to) {
        (I32, F32) => 0xb2,
        (I32, F64) => 0xb7,
        (F32, I32) => 0xa8,
        (F64, I32) => 0xaa,
        (F32, F64) => 0xbb,
        (F64, F32) => 0xb6,
        _ => return Err("unsupported web conversion".to_string()),
    });
    Ok(())
}

fn arithmetic_opcode(op: AssignOp, type_id: TypeId) -> Result<u8, String> {
    match (op, wasm_value_type(type_id)?) {
        (AssignOp::Add, I32) => Ok(0x6a),
        (AssignOp::Sub, I32) => Ok(0x6b),
        (AssignOp::Mul, I32) => Ok(0x6c),
        (AssignOp::Div, I32) => Ok(0x6d),
        (AssignOp::Mod, I32) => Ok(0x6f),
        (AssignOp::Add, F32) => Ok(0x92),
        (AssignOp::Sub, F32) => Ok(0x93),
        (AssignOp::Mul, F32) => Ok(0x94),
        (AssignOp::Div, F32) => Ok(0x95),
        (AssignOp::Add, F64) => Ok(0xa0),
        (AssignOp::Sub, F64) => Ok(0xa1),
        (AssignOp::Mul, F64) => Ok(0xa2),
        (AssignOp::Div, F64) => Ok(0xa3),
        (AssignOp::Mod, F32 | F64) => Err("web float remainder is unsupported".to_string()),
        (AssignOp::Set, _) => Err("set has no arithmetic opcode".to_string()),
        _ => Err("unsupported web arithmetic lane".to_string()),
    }
}

fn encode_target_get(
    target: &AssignTarget,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<TypeId, String> {
    match target {
        AssignTarget::Local(name) => {
            if let Some((binding, suffix)) = foreach_path(context, name) {
                encode_foreach_load(binding, suffix, context, out)
            } else if let Some((binding, suffix)) = local_collection_meta(context, name) {
                let candidates = collection_meta_candidates(context, suffix);
                encode_collection_meta_load(binding.index, suffix, &candidates, out);
                Ok(TYPE_ID_I32)
            } else if let Some((binding, suffix)) = local_struct_path(context, name) {
                encode_struct_field_load(binding, suffix, context, out)
            } else if let Some(binding) = context.locals.get(name) {
                out.push(0x20);
                uleb(binding.index, out);
                Ok(binding.type_id)
            } else {
                out.push(0x23);
                uleb(global(context, name)?, out);
                global_type(context, name)
            }
        }
        AssignTarget::GlobalPath(name) => {
            if let Some((binding, suffix)) = foreach_path(context, name) {
                encode_foreach_load(binding, suffix, context, out)
            } else if let Some((binding, suffix)) = local_collection_meta(context, name) {
                let candidates = collection_meta_candidates(context, suffix);
                encode_collection_meta_load(binding.index, suffix, &candidates, out);
                Ok(TYPE_ID_I32)
            } else if let Some((binding, suffix)) = local_struct_path(context, name) {
                encode_struct_field_load(binding, suffix, context, out)
            } else {
                out.push(0x23);
                uleb(global(context, name)?, out);
                global_type(context, name)
            }
        }
        AssignTarget::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            if suffix.is_empty() {
                if let Some(local) = context.locals.get(collection_path).copied() {
                    let element_type = encode_local_collection_address(local, index, context, out)?;
                    encode_memory_load(element_type, out)?;
                    return Ok(element_type);
                }
            }
            let binding = memory_binding(context, collection_path, suffix)?;
            encode_memory_address(binding, index, context, out)?;
            encode_memory_load(binding.type_id, out)?;
            Ok(binding.type_id)
        }
    }
}

fn encode_target_set(
    target: &AssignTarget,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    match target {
        AssignTarget::Local(name) => {
            if let Some((binding, suffix)) = foreach_path(context, name) {
                encode_foreach_store(binding, suffix, context, out)
            } else if let Some((binding, suffix)) = local_collection_meta(context, name) {
                if suffix != "length" {
                    return Err("web collection max_length is read-only".to_string());
                }
                out.push(0x21);
                uleb(context.scratch_i32, out);
                let candidates = collection_meta_candidates(context, suffix);
                encode_collection_length_store(
                    binding.index,
                    context.scratch_i32,
                    &candidates,
                    out,
                );
                Ok(())
            } else if let Some((binding, suffix)) = local_struct_path(context, name) {
                let field_type = context.named_structs[&binding.type_id][suffix];
                let temp = scratch_local(context, field_type)?;
                out.push(0x21);
                uleb(temp, out);
                encode_struct_field_store_from_local(binding, suffix, temp, context, out)
            } else if let Some(binding) = context.locals.get(name) {
                out.push(0x21);
                uleb(binding.index, out);
                Ok(())
            } else {
                out.push(0x24);
                uleb(global(context, name)?, out);
                Ok(())
            }
        }
        AssignTarget::GlobalPath(name) => {
            if let Some((binding, suffix)) = foreach_path(context, name) {
                encode_foreach_store(binding, suffix, context, out)
            } else if let Some((binding, suffix)) = local_collection_meta(context, name) {
                if suffix != "length" {
                    return Err("web collection max_length is read-only".to_string());
                }
                out.push(0x21);
                uleb(context.scratch_i32, out);
                let candidates = collection_meta_candidates(context, suffix);
                encode_collection_length_store(
                    binding.index,
                    context.scratch_i32,
                    &candidates,
                    out,
                );
                Ok(())
            } else if let Some((binding, suffix)) = local_struct_path(context, name) {
                let field_type = context.named_structs[&binding.type_id][suffix];
                let temp = scratch_local(context, field_type)?;
                out.push(0x21);
                uleb(temp, out);
                encode_struct_field_store_from_local(binding, suffix, temp, context, out)
            } else {
                out.push(0x24);
                uleb(global(context, name)?, out);
                Ok(())
            }
        }
        AssignTarget::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            if suffix.is_empty() {
                if let Some(local) = context.locals.get(collection_path).copied() {
                    let element_type = context
                        .types
                        .indexed_element_type_id(local.type_id)
                        .ok_or_else(|| {
                            format!("web local type {} is not indexable", local.type_id)
                        })?;
                    let temp = scratch_local(context, element_type)?;
                    out.push(0x21);
                    uleb(temp, out);
                    encode_local_collection_address(local, index, context, out)?;
                    out.push(0x20);
                    uleb(temp, out);
                    return encode_memory_store(element_type, out);
                }
            }
            let binding = memory_binding(context, collection_path, suffix)?;
            let temp_index = scratch_local(context, binding.type_id)?;
            out.push(0x21);
            uleb(temp_index, out);
            encode_memory_address(binding, index, context, out)?;
            out.push(0x20);
            uleb(temp_index, out);
            encode_memory_store(binding.type_id, out)
        }
    }
}

fn scratch_local(context: &EncodeContext<'_>, type_id: TypeId) -> Result<u32, String> {
    match wasm_value_type(type_id)? {
        I32 => Ok(context.scratch_i32),
        F32 => Ok(context.scratch_f32),
        F64 => Ok(context.scratch_f64),
        _ => Err("unsupported web scratch type".to_string()),
    }
}

fn local_binding(context: &EncodeContext<'_>, name: &str) -> Result<LocalBinding, String> {
    context
        .locals
        .get(name)
        .copied()
        .ok_or_else(|| format!("unknown web local '{name}'"))
}

fn global(context: &EncodeContext<'_>, name: &str) -> Result<u32, String> {
    context
        .globals
        .get(name)
        .copied()
        .ok_or_else(|| format!("unknown web global '{name}'"))
}

fn global_type(context: &EncodeContext<'_>, name: &str) -> Result<TypeId, String> {
    context
        .global_types
        .get(name)
        .copied()
        .ok_or_else(|| format!("unknown web global type '{name}'"))
}

fn target_type(target: &AssignTarget, context: &EncodeContext<'_>) -> Result<TypeId, String> {
    match target {
        AssignTarget::Local(name) => {
            if let Some((binding, suffix)) = foreach_path(context, name) {
                Ok(memory_binding(context, &binding.collection_path, suffix)?.type_id)
            } else if local_collection_meta(context, name).is_some() {
                Ok(TYPE_ID_I32)
            } else if let Some((binding, suffix)) = local_struct_path(context, name) {
                context.named_structs[&binding.type_id]
                    .get(suffix)
                    .copied()
                    .ok_or_else(|| {
                        format!("unknown web struct field '{}.{suffix}'", binding.type_id)
                    })
            } else {
                context
                    .locals
                    .get(name)
                    .map(|binding| binding.type_id)
                    .or_else(|| context.global_types.get(name).copied())
                    .ok_or_else(|| format!("unknown web assignment target '{name}'"))
            }
        }
        AssignTarget::GlobalPath(name) => {
            if let Some((binding, suffix)) = foreach_path(context, name) {
                Ok(memory_binding(context, &binding.collection_path, suffix)?.type_id)
            } else if local_collection_meta(context, name).is_some() {
                Ok(TYPE_ID_I32)
            } else if let Some((binding, suffix)) = local_struct_path(context, name) {
                context.named_structs[&binding.type_id]
                    .get(suffix)
                    .copied()
                    .ok_or_else(|| {
                        format!("unknown web struct field '{}.{suffix}'", binding.type_id)
                    })
            } else {
                global_type(context, name)
            }
        }
        AssignTarget::IndexedPath {
            collection_path,
            suffix,
            ..
        } => {
            if suffix.is_empty() {
                if let Some(local) = context.locals.get(collection_path) {
                    return context
                        .types
                        .indexed_element_type_id(local.type_id)
                        .ok_or_else(|| {
                            format!("web local type {} is not indexable", local.type_id)
                        });
                }
            }
            Ok(memory_binding(context, collection_path, suffix)?.type_id)
        }
    }
}

fn memory_binding<'a>(
    context: &'a EncodeContext<'_>,
    collection_path: &str,
    suffix: &str,
) -> Result<&'a MemoryBinding, String> {
    let path = if suffix.is_empty() {
        collection_path.to_string()
    } else {
        format!("{collection_path}.{suffix}")
    };
    context
        .memory
        .get(&path)
        .ok_or_else(|| format!("unknown web collection storage '{path}'"))
}

fn collection_len(context: &EncodeContext<'_>, collection_path: &str) -> Result<i32, String> {
    context
        .memory
        .get(collection_path)
        .or_else(|| {
            let prefix = format!("{collection_path}.");
            context
                .memory
                .iter()
                .find_map(|(path, binding)| path.starts_with(&prefix).then_some(binding))
        })
        .map(|binding| binding.len)
        .ok_or_else(|| format!("unknown web foreach collection '{collection_path}'"))
}

fn foreach_path<'a>(
    context: &'a EncodeContext<'_>,
    path: &'a str,
) -> Option<(&'a WebForeachBinding, &'a str)> {
    let (alias, suffix) = path.split_once('.').unwrap_or((path, ""));
    context.foreach.get(alias).map(|binding| (binding, suffix))
}

fn local_struct_path<'a>(
    context: &'a EncodeContext<'_>,
    path: &'a str,
) -> Option<(&'a LocalBinding, &'a str)> {
    let (name, suffix) = path.split_once('.')?;
    let binding = context.locals.get(name)?;
    binding.struct_view.map(|_| (binding, suffix))
}

fn local_collection_meta<'a>(
    context: &'a EncodeContext<'_>,
    path: &'a str,
) -> Option<(&'a LocalBinding, &'a str)> {
    let (name, suffix) = path.split_once('.')?;
    if !matches!(suffix, "length" | "max_length") {
        return None;
    }
    let binding = context.locals.get(name)?;
    context
        .types
        .indexed_element_type_id(binding.type_id)
        .map(|_| (binding, suffix))
}

fn collection_meta_candidates<'a>(
    context: &'a EncodeContext<'_>,
    suffix: &str,
) -> Vec<(&'a MemoryBinding, Option<u32>)> {
    context
        .memory
        .iter()
        .filter(|(path, _)| {
            context
                .global_types
                .get(*path)
                .is_some_and(|type_id| context.types.indexed_element_type_id(*type_id).is_some())
        })
        .map(|(path, memory)| {
            (
                memory,
                (suffix == "length")
                    .then(|| context.globals.get(&format!("{path}.length")).copied())
                    .flatten(),
            )
        })
        .collect()
}

fn encode_collection_meta_load(
    base_local: u32,
    suffix: &str,
    candidates: &[(&MemoryBinding, Option<u32>)],
    out: &mut Vec<u8>,
) {
    let Some(((memory, global), rest)) = candidates.split_first() else {
        out.push(0x00);
        return;
    };
    out.push(0x20);
    uleb(base_local, out);
    out.push(0x41);
    sleb(memory.offset as i32, out);
    out.extend([0x46, 0x04, I32]);
    if suffix == "length" {
        if let Some(global) = global {
            out.push(0x23);
            uleb(*global, out);
        } else {
            out.push(0x41);
            sleb(memory.len, out);
        }
    } else {
        out.push(0x41);
        sleb(memory.len, out);
    }
    out.push(0x05);
    encode_collection_meta_load(base_local, suffix, rest, out);
    out.push(0x0b);
}

fn encode_collection_length_store(
    base_local: u32,
    value_local: u32,
    candidates: &[(&MemoryBinding, Option<u32>)],
    out: &mut Vec<u8>,
) {
    let Some(((memory, global), rest)) = candidates.split_first() else {
        out.push(0x00);
        return;
    };
    out.push(0x20);
    uleb(base_local, out);
    out.push(0x41);
    sleb(memory.offset as i32, out);
    out.extend([0x46, 0x04, 0x40]);
    if let Some(global) = global {
        out.push(0x20);
        uleb(value_local, out);
        out.push(0x24);
        uleb(*global, out);
    } else {
        out.push(0x00);
    }
    out.push(0x05);
    encode_collection_length_store(base_local, value_local, rest, out);
    out.push(0x0b);
}

fn encode_struct_view_expr(
    value: &SimpleExpr,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<TypeId, String> {
    match value {
        SimpleExpr::Identifier(name) => {
            if let Some(binding) = context.locals.get(name) {
                let view = binding
                    .struct_view
                    .ok_or_else(|| format!("web value '{name}' is not a struct view"))?;
                for index in [binding.index, view.index, view.len] {
                    out.push(0x20);
                    uleb(index, out);
                }
                return Ok(binding.type_id);
            }
            if let Some(binding) = context.foreach.get(name) {
                let collection = context
                    .struct_collections
                    .get(&binding.collection_path)
                    .ok_or_else(|| {
                        format!("web foreach '{name}' is not over a struct collection")
                    })?;
                out.push(0x41);
                sleb(collection.base, out);
                let index = local_binding(context, &binding.index_name)?;
                out.push(0x20);
                uleb(index.index, out);
                out.push(0x41);
                sleb(collection.len, out);
                return Ok(collection.type_id);
            }
            if let Some(binding) = context.struct_scalars.get(name) {
                out.push(0x41);
                sleb(binding.base, out);
                out.push(0x41);
                sleb(-1, out);
                out.extend([0x41, 0]);
                return Ok(binding.type_id);
            }
            Err(format!("unknown web struct view '{name}'"))
        }
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } if suffix.is_empty() => {
            let collection = context
                .struct_collections
                .get(collection_path)
                .ok_or_else(|| format!("unknown web struct collection '{collection_path}'"))?;
            out.push(0x41);
            sleb(collection.base, out);
            let index_type = encode_expr_as(index, Some(TYPE_ID_I32), context, out)?;
            if !is_web_index_type(index_type, context) {
                return Err("web struct collection index must be i32-compatible".to_string());
            }
            out.push(0x41);
            sleb(collection.len, out);
            Ok(collection.type_id)
        }
        _ => Err("web struct value must be a collection element or existing view".to_string()),
    }
}

fn encode_struct_collection_copy(
    collection_path: &str,
    target_index: &SimpleExpr,
    source: &SimpleExpr,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let collection = context
        .struct_collections
        .get(collection_path)
        .ok_or_else(|| format!("unknown web struct collection '{collection_path}'"))?;
    let source_type = encode_struct_view_expr(source, context, out)?;
    require_same_struct_type(collection.type_id, source_type, "collection assignment")?;
    for local in [
        context.scratch_i32_c,
        context.scratch_i32_b,
        context.scratch_i32,
    ] {
        out.push(0x21);
        uleb(local, out);
    }
    let target_type = encode_expr_as(target_index, Some(TYPE_ID_I32), context, out)?;
    if !is_web_index_type(target_type, context) {
        return Err("web struct collection index must be i32-compatible".to_string());
    }
    out.push(0x21);
    uleb(context.scratch_index, out);
    emit_index_bounds_check(context.scratch_index, collection.len, out);

    let source_binding = LocalBinding {
        index: context.scratch_i32,
        type_id: source_type,
        struct_view: Some(StructViewBinding {
            index: context.scratch_i32_b,
            len: context.scratch_i32_c,
        }),
    };
    for (suffix, target_field) in &collection.fields {
        out.push(0x41);
        sleb(target_field.offset as i32, out);
        out.push(0x20);
        uleb(context.scratch_index, out);
        out.push(0x41);
        sleb(target_field.stride as i32, out);
        out.extend([0x6c, 0x6a]);
        let source_field = encode_struct_field_address(&source_binding, suffix, context, out)?;
        encode_memory_load(source_field.type_id, out)?;
        encode_memory_store(target_field.type_id, out)?;
    }
    Ok(())
}

fn emit_index_bounds_check(index_local: u32, len: i32, out: &mut Vec<u8>) {
    out.push(0x20);
    uleb(index_local, out);
    out.push(0x41);
    sleb(len, out);
    out.extend([0x4f, 0x04, 0x40, 0x00, 0x0b]);
}

fn struct_field_binding<'a>(
    context: &'a EncodeContext<'_>,
    type_id: TypeId,
    suffix: &str,
) -> Result<Vec<&'a StructCollectionBinding>, String> {
    let field_type = context
        .named_structs
        .get(&type_id)
        .and_then(|fields| fields.get(suffix))
        .copied()
        .ok_or_else(|| format!("unknown web struct field type {type_id}.{suffix}"))?;
    let candidates = context
        .struct_collections
        .values()
        .filter(|collection| {
            collection.type_id == type_id
                && collection
                    .fields
                    .get(suffix)
                    .is_some_and(|field| field.type_id == field_type)
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(format!(
            "web struct field {type_id}.{suffix} has no reachable SoA storage"
        ));
    }
    Ok(candidates)
}

fn encode_selected_field_offset(
    base_local: u32,
    suffix: &str,
    candidates: &[&StructCollectionBinding],
    out: &mut Vec<u8>,
) -> Result<MemoryBinding, String> {
    let first = candidates[0]
        .fields
        .get(suffix)
        .cloned()
        .ok_or_else(|| format!("missing web SoA field '{suffix}'"))?;
    fn branch(
        base_local: u32,
        suffix: &str,
        candidates: &[&StructCollectionBinding],
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        let Some((candidate, rest)) = candidates.split_first() else {
            out.push(0x00);
            return Ok(());
        };
        out.push(0x20);
        uleb(base_local, out);
        out.push(0x41);
        sleb(candidate.base, out);
        out.extend([0x46, 0x04, I32]);
        out.push(0x41);
        sleb(candidate.fields[suffix].offset as i32, out);
        out.push(0x05);
        branch(base_local, suffix, rest, out)?;
        out.push(0x0b);
        Ok(())
    }
    branch(base_local, suffix, candidates, out)?;
    Ok(first)
}

fn encode_struct_field_address(
    binding: &LocalBinding,
    suffix: &str,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<MemoryBinding, String> {
    let view = binding
        .struct_view
        .ok_or_else(|| "web struct binding has no view metadata".to_string())?;
    out.push(0x20);
    uleb(view.index, out);
    out.extend([0x41, 0, 0x48, 0x04, 0x40, 0x00, 0x0b]);
    out.push(0x20);
    uleb(view.index, out);
    out.push(0x20);
    uleb(view.len, out);
    out.extend([0x4e, 0x04, 0x40, 0x00, 0x0b]);
    let candidates = struct_field_binding(context, binding.type_id, suffix)?;
    let field = encode_selected_field_offset(binding.index, suffix, &candidates, out)?;
    out.push(0x20);
    uleb(view.index, out);
    out.push(0x41);
    sleb(field.stride as i32, out);
    out.extend([0x6c, 0x6a]);
    Ok(field)
}

fn scalar_field_candidates<'a>(
    binding: &LocalBinding,
    suffix: &str,
    context: &'a EncodeContext<'_>,
) -> Vec<&'a StructScalarBinding> {
    context
        .struct_scalars
        .values()
        .filter(|scalar| scalar.type_id == binding.type_id && scalar.fields.contains_key(suffix))
        .collect()
}

fn encode_scalar_field_load(
    base_local: u32,
    suffix: &str,
    candidates: &[&StructScalarBinding],
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let Some((candidate, rest)) = candidates.split_first() else {
        out.push(0x00);
        return Ok(());
    };
    let (global_index, type_id) = candidate.fields[suffix];
    out.push(0x20);
    uleb(base_local, out);
    out.push(0x41);
    sleb(candidate.base, out);
    out.extend([0x46, 0x04, wasm_value_type(type_id)?]);
    out.push(0x23);
    uleb(global_index, out);
    out.push(0x05);
    encode_scalar_field_load(base_local, suffix, rest, out)?;
    out.push(0x0b);
    Ok(())
}

fn encode_struct_field_load(
    binding: &LocalBinding,
    suffix: &str,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<TypeId, String> {
    let field_type = context.named_structs[&binding.type_id][suffix];
    let view = binding
        .struct_view
        .ok_or_else(|| "web struct binding has no view metadata".to_string())?;
    out.push(0x20);
    uleb(view.index, out);
    out.extend([0x41, 0, 0x48, 0x04, wasm_value_type(field_type)?]);
    let scalar_candidates = scalar_field_candidates(binding, suffix, context);
    encode_scalar_field_load(binding.index, suffix, &scalar_candidates, out)?;
    out.push(0x05);
    let field = encode_struct_field_address(binding, suffix, context, out)?;
    encode_memory_load(field.type_id, out)?;
    out.push(0x0b);
    Ok(field_type)
}

fn encode_scalar_field_store(
    base_local: u32,
    suffix: &str,
    value_local: u32,
    candidates: &[&StructScalarBinding],
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let Some((candidate, rest)) = candidates.split_first() else {
        out.push(0x00);
        return Ok(());
    };
    let (global_index, _) = candidate.fields[suffix];
    out.push(0x20);
    uleb(base_local, out);
    out.push(0x41);
    sleb(candidate.base, out);
    out.extend([0x46, 0x04, 0x40, 0x20]);
    uleb(value_local, out);
    out.push(0x24);
    uleb(global_index, out);
    out.push(0x05);
    encode_scalar_field_store(base_local, suffix, value_local, rest, out)?;
    out.push(0x0b);
    Ok(())
}

fn encode_struct_field_store_from_local(
    binding: &LocalBinding,
    suffix: &str,
    value_local: u32,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let view = binding
        .struct_view
        .ok_or_else(|| "web struct binding has no view metadata".to_string())?;
    out.push(0x20);
    uleb(view.index, out);
    out.extend([0x41, 0, 0x48, 0x04, 0x40]);
    let scalar_candidates = scalar_field_candidates(binding, suffix, context);
    encode_scalar_field_store(binding.index, suffix, value_local, &scalar_candidates, out)?;
    out.push(0x05);
    let field = encode_struct_field_address(binding, suffix, context, out)?;
    out.push(0x20);
    uleb(value_local, out);
    encode_memory_store(field.type_id, out)?;
    out.push(0x0b);
    Ok(())
}

fn encode_foreach_load(
    binding: &WebForeachBinding,
    suffix: &str,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<TypeId, String> {
    let memory = memory_binding(context, &binding.collection_path, suffix)?;
    encode_memory_address(
        memory,
        &SimpleExpr::Identifier(binding.index_name.clone()),
        context,
        out,
    )?;
    encode_memory_load(memory.type_id, out)?;
    Ok(memory.type_id)
}

fn encode_foreach_store(
    binding: &WebForeachBinding,
    suffix: &str,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let memory = memory_binding(context, &binding.collection_path, suffix)?;
    let temp_index = scratch_local(context, memory.type_id)?;
    out.push(0x21);
    uleb(temp_index, out);
    encode_memory_address(
        memory,
        &SimpleExpr::Identifier(binding.index_name.clone()),
        context,
        out,
    )?;
    out.push(0x20);
    uleb(temp_index, out);
    encode_memory_store(memory.type_id, out)
}

fn encode_memory_address(
    binding: &MemoryBinding,
    index: &SimpleExpr,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let index_type = encode_expr(index, context, out)?;
    if !is_web_index_type(index_type, context) {
        return Err(format!(
            "web collection index must be i32-compatible, found type {index_type}"
        ));
    }
    out.push(0x21);
    uleb(context.scratch_index, out);
    emit_index_bounds_check(context.scratch_index, binding.len, out);
    out.push(0x41);
    sleb(binding.offset as i32, out);
    out.push(0x20);
    uleb(context.scratch_index, out);
    out.push(0x41);
    sleb(binding.width as i32, out);
    out.push(0x6c);
    out.push(0x6a);
    Ok(())
}

fn encode_local_collection_address(
    binding: LocalBinding,
    index: &SimpleExpr,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<TypeId, String> {
    let element_type = context
        .types
        .indexed_element_type_id(binding.type_id)
        .ok_or_else(|| format!("web local type {} is not indexable", binding.type_id))?;
    let width = storage_width(element_type, context.types, context.named_structs)?;
    let index_type = encode_expr_as(index, Some(TYPE_ID_I32), context, out)?;
    if !is_web_index_type(index_type, context) {
        return Err("web local collection index must be i32-compatible".to_string());
    }
    out.push(0x21);
    uleb(context.scratch_index, out);
    out.push(0x20);
    uleb(context.scratch_index, out);
    out.extend([0x41, 0, 0x48, 0x04, 0x40, 0x00, 0x0b]);
    out.push(0x20);
    uleb(binding.index, out);
    out.push(0x20);
    uleb(context.scratch_index, out);
    out.push(0x41);
    sleb(width as i32, out);
    out.extend([0x6c, 0x6a]);
    Ok(element_type)
}

fn encode_memory_load(type_id: TypeId, out: &mut Vec<u8>) -> Result<(), String> {
    let (opcode, align) = match type_id {
        TYPE_ID_BOOL | TYPE_ID_U8 => (0x2d, 0),
        TYPE_ID_U16 => (0x2f, 1),
        TYPE_ID_I32 | TYPE_ID_U32 => (0x28, 2),
        TYPE_ID_F32 => (0x2a, 2),
        TYPE_ID_F64 => (0x2b, 3),
        _ if wasm_value_type(type_id)? == I32 => (0x28, 2),
        _ => return Err(format!("unsupported web memory load type id {type_id}")),
    };
    out.push(opcode);
    uleb(align, out);
    uleb(0, out);
    Ok(())
}

fn encode_memory_store(type_id: TypeId, out: &mut Vec<u8>) -> Result<(), String> {
    let (opcode, align) = match type_id {
        TYPE_ID_BOOL | TYPE_ID_U8 => (0x3a, 0),
        TYPE_ID_U16 => (0x3b, 1),
        TYPE_ID_I32 | TYPE_ID_U32 => (0x36, 2),
        TYPE_ID_F32 => (0x38, 2),
        TYPE_ID_F64 => (0x39, 3),
        _ if wasm_value_type(type_id)? == I32 => (0x36, 2),
        _ => return Err(format!("unsupported web memory store type id {type_id}")),
    };
    out.push(opcode);
    uleb(align, out);
    uleb(0, out);
    Ok(())
}

fn require_same_type(expected: TypeId, actual: TypeId, context: &str) -> Result<(), String> {
    if wasm_value_type(expected)? == wasm_value_type(actual)? {
        Ok(())
    } else {
        Err(format!(
            "web {context} type mismatch: expected {expected}, found {actual}"
        ))
    }
}

fn encode_expr(
    value: &SimpleExpr,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<TypeId, String> {
    encode_expr_as(value, None, context, out)
}

fn encode_expr_as(
    value: &SimpleExpr,
    expected: Option<TypeId>,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<TypeId, String> {
    match value {
        SimpleExpr::Int(value) => {
            out.push(0x41);
            sleb(*value as i32, out);
            Ok(expected
                .filter(|type_id| is_i32_lane(*type_id))
                .unwrap_or(TYPE_ID_I32))
        }
        SimpleExpr::Float(value) => {
            let type_id = expected
                .filter(|type_id| matches!(*type_id, TYPE_ID_F32 | TYPE_ID_F64))
                .unwrap_or(TYPE_ID_F32);
            if type_id == TYPE_ID_F64 {
                out.push(0x44);
                out.extend(value.to_le_bytes());
            } else {
                out.push(0x43);
                out.extend((*value as f32).to_le_bytes());
            }
            Ok(type_id)
        }
        SimpleExpr::Bool(value) => {
            out.extend([0x41, u8::from(*value)]);
            Ok(TYPE_ID_BOOL)
        }
        SimpleExpr::StringLiteral(value) => {
            out.push(0x41);
            sleb(crate::backend::emit::hash_string_literal(value), out);
            Ok(expected.unwrap_or(TYPE_ID_I32))
        }
        SimpleExpr::Identifier(name) => {
            if let Some((binding, suffix)) = foreach_path(context, name) {
                encode_foreach_load(binding, suffix, context, out)
            } else if let Some((binding, suffix)) = local_collection_meta(context, name) {
                let candidates = collection_meta_candidates(context, suffix);
                encode_collection_meta_load(binding.index, suffix, &candidates, out);
                Ok(TYPE_ID_I32)
            } else if let Some((binding, suffix)) = local_struct_path(context, name) {
                encode_struct_field_load(binding, suffix, context, out)
            } else if let Some(binding) = context.locals.get(name) {
                if binding.struct_view.is_some() {
                    return Err(format!(
                        "web struct view '{name}' requires a struct parameter or field access"
                    ));
                }
                out.push(0x20);
                uleb(binding.index, out);
                Ok(binding.type_id)
            } else if let Some(binding) = context.memory.get(name) {
                out.push(0x41);
                sleb(binding.offset as i32, out);
                Ok(expected.unwrap_or(TYPE_ID_I32))
            } else if let Some(index) = context.globals.get(name) {
                out.push(0x23);
                uleb(*index, out);
                global_type(context, name)
            } else if let Some(value) = context.constants.get(name) {
                encode_constant(value, expected, out)
            } else {
                Err(format!("unknown web value '{name}'"))
            }
        }
        SimpleExpr::Call { target, args } => {
            if is_inline_intrinsic(target) {
                return encode_inline_intrinsic(target, args, context, out);
            }
            let index = context
                .imports
                .get(target)
                .or_else(|| context.internals.get(target))
                .copied()
                .ok_or_else(|| format!("unknown web call '{target}'"))?;
            let signature = context
                .signatures
                .get(index as usize)
                .ok_or_else(|| format!("missing web signature for '{target}'"))?;
            if args.len() != signature.params.len() {
                return Err(format!(
                    "web call '{target}' expected {} arguments, found {}",
                    signature.params.len(),
                    args.len()
                ));
            }
            for (arg, param_type) in args.iter().zip(signature.params.iter()) {
                if is_struct_view_type(*param_type, context.named_structs) {
                    let actual = encode_struct_view_expr(arg, context, out)?;
                    require_same_struct_type(*param_type, actual, "call argument")?;
                } else {
                    let actual = encode_expr_as(arg, Some(*param_type), context, out)?;
                    require_same_type(*param_type, actual, "call argument")?;
                }
            }
            out.push(0x10);
            uleb(index, out);
            Ok(signature.result)
        }
        SimpleExpr::Binary { lhs, op, rhs } => {
            let lhs_type = encode_expr_as(lhs, expected, context, out)?;
            let rhs_type = encode_expr_as(rhs, Some(lhs_type), context, out)?;
            require_same_type(lhs_type, rhs_type, "binary expression")?;
            let assign_op = match op {
                '+' => AssignOp::Add,
                '-' => AssignOp::Sub,
                '*' => AssignOp::Mul,
                '/' => AssignOp::Div,
                '%' => AssignOp::Mod,
                other => return Err(format!("unsupported web binary operator '{other}'")),
            };
            out.push(arithmetic_opcode(assign_op, lhs_type)?);
            Ok(lhs_type)
        }
        SimpleExpr::Condition(condition) => {
            encode_condition(condition, context, out)?;
            Ok(TYPE_ID_BOOL)
        }
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            if suffix.is_empty()
                && expected
                    .is_some_and(|type_id| is_struct_view_type(type_id, context.named_structs))
            {
                return Err(format!(
                    "web struct collection element '{collection_path}' requires view context"
                ));
            }
            if suffix.is_empty() {
                if let Some(local) = context.locals.get(collection_path).copied() {
                    let element_type = encode_local_collection_address(local, index, context, out)?;
                    encode_memory_load(element_type, out)?;
                    return Ok(element_type);
                }
            }
            let binding = memory_binding(context, collection_path, suffix)?;
            encode_memory_address(binding, index, context, out)?;
            encode_memory_load(binding.type_id, out)?;
            Ok(binding.type_id)
        }
    }
}

fn is_inline_intrinsic(target: &str) -> bool {
    matches!(
        target,
        "i32_to_f32"
            | "f32_to_i32"
            | "fixed32_from_i32"
            | "fixed32_to_i32"
            | "fixed32_mul"
            | "fixed32_div"
            | "fixed32_from_ratio"
    )
}

fn encode_inline_intrinsic(
    target: &str,
    args: &[SimpleExpr],
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<TypeId, String> {
    let expected = if matches!(target, "fixed32_mul" | "fixed32_div" | "fixed32_from_ratio") {
        2
    } else {
        1
    };
    if args.len() != expected {
        return Err(format!(
            "web intrinsic '{target}' expects {expected} argument(s), found {}",
            args.len()
        ));
    }
    match target {
        "i32_to_f32" => {
            let actual = encode_expr_as(&args[0], Some(TYPE_ID_I32), context, out)?;
            require_same_type(TYPE_ID_I32, actual, "intrinsic argument")?;
            out.push(0xb2);
            Ok(TYPE_ID_F32)
        }
        "f32_to_i32" => {
            let actual = encode_expr_as(&args[0], Some(TYPE_ID_F32), context, out)?;
            require_same_type(TYPE_ID_F32, actual, "intrinsic argument")?;
            out.push(0xa8);
            Ok(TYPE_ID_I32)
        }
        "fixed32_from_i32" => {
            encode_exact_i32_arg(&args[0], context, out)?;
            out.extend([0x41, 16, 0x74]);
            Ok(TYPE_ID_I32)
        }
        "fixed32_to_i32" => {
            encode_exact_i32_arg(&args[0], context, out)?;
            out.push(0x41);
            sleb(65_536, out);
            out.push(0x6d);
            Ok(TYPE_ID_I32)
        }
        "fixed32_mul" => {
            encode_exact_i32_arg(&args[0], context, out)?;
            out.push(0xac);
            encode_exact_i32_arg(&args[1], context, out)?;
            out.extend([0xac, 0x7e, 0x42]);
            sleb64(65_536, out);
            out.extend([0x7f, 0xa7]);
            Ok(TYPE_ID_I32)
        }
        "fixed32_div" | "fixed32_from_ratio" => {
            encode_exact_i32_arg(&args[0], context, out)?;
            out.extend([0xac, 0x42]);
            sleb64(16, out);
            out.push(0x86);
            encode_exact_i32_arg(&args[1], context, out)?;
            out.extend([0xac, 0x7f, 0xa7]);
            Ok(TYPE_ID_I32)
        }
        _ => Err(format!("unknown web intrinsic '{target}'")),
    }
}

fn encode_exact_i32_arg(
    value: &SimpleExpr,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let actual = encode_expr_as(value, Some(TYPE_ID_I32), context, out)?;
    if actual != TYPE_ID_I32 {
        return Err(format!(
            "web deterministic numeric intrinsic requires exact i32 argument, found {actual}"
        ));
    }
    Ok(())
}

fn encode_constant(
    value: &ConstantValue,
    expected: Option<TypeId>,
    out: &mut Vec<u8>,
) -> Result<TypeId, String> {
    match value {
        ConstantValue::I32 { value, type_id } => {
            out.push(0x41);
            sleb(*value, out);
            Ok(*type_id)
        }
        ConstantValue::Bool(value) => {
            out.extend([0x41, u8::from(*value)]);
            Ok(TYPE_ID_BOOL)
        }
        ConstantValue::F32(value) => {
            out.push(0x43);
            out.extend(value.to_le_bytes());
            Ok(TYPE_ID_F32)
        }
        ConstantValue::F64(value) => {
            out.push(0x44);
            out.extend(value.to_le_bytes());
            Ok(TYPE_ID_F64)
        }
        ConstantValue::String { value, type_id } => {
            out.push(0x41);
            sleb(crate::backend::emit::hash_string_literal(value), out);
            Ok(expected.unwrap_or(*type_id))
        }
    }
}

fn encode_condition(
    value: &SimpleCondition,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    match value {
        SimpleCondition::Comparison { lhs, op, rhs } => {
            let lhs_type = encode_expr(lhs, context, out)?;
            let rhs_type = encode_expr_as(rhs, Some(lhs_type), context, out)?;
            require_same_type(lhs_type, rhs_type, "comparison")?;
            out.push(comparison_opcode(*op, lhs_type)?);
        }
        SimpleCondition::Expr(value) => {
            encode_expr(value, context, out)?;
        }
        SimpleCondition::And(lhs, rhs) => {
            encode_condition(lhs, context, out)?;
            out.extend([0x04, I32]);
            encode_condition(rhs, context, out)?;
            out.extend([0x05, 0x41, 0, 0x0b]);
        }
        SimpleCondition::Or(lhs, rhs) => {
            encode_condition(lhs, context, out)?;
            out.extend([0x04, I32, 0x41, 1, 0x05]);
            encode_condition(rhs, context, out)?;
            out.push(0x0b);
        }
        SimpleCondition::Not(value) => {
            encode_condition(value, context, out)?;
            out.push(0x45);
        }
    }
    Ok(())
}

fn comparison_opcode(op: ComparisonOp, type_id: TypeId) -> Result<u8, String> {
    match (op, wasm_value_type(type_id)?) {
        (ComparisonOp::Eq, I32) => Ok(0x46),
        (ComparisonOp::Ne, I32) => Ok(0x47),
        (ComparisonOp::Lt, I32) => Ok(0x48),
        (ComparisonOp::Gt, I32) => Ok(0x4a),
        (ComparisonOp::Le, I32) => Ok(0x4c),
        (ComparisonOp::Ge, I32) => Ok(0x4e),
        (ComparisonOp::Eq, F32) => Ok(0x5b),
        (ComparisonOp::Ne, F32) => Ok(0x5c),
        (ComparisonOp::Lt, F32) => Ok(0x5d),
        (ComparisonOp::Gt, F32) => Ok(0x5e),
        (ComparisonOp::Le, F32) => Ok(0x5f),
        (ComparisonOp::Ge, F32) => Ok(0x60),
        (ComparisonOp::Eq, F64) => Ok(0x61),
        (ComparisonOp::Ne, F64) => Ok(0x62),
        (ComparisonOp::Lt, F64) => Ok(0x63),
        (ComparisonOp::Gt, F64) => Ok(0x64),
        (ComparisonOp::Le, F64) => Ok(0x65),
        (ComparisonOp::Ge, F64) => Ok(0x66),
        _ => Err("unsupported web comparison lane".to_string()),
    }
}

fn expression_returns_value(
    value: &SimpleExpr,
    context: &EncodeContext<'_>,
) -> Result<bool, String> {
    if let SimpleExpr::Call { target, .. } = value {
        let index = context
            .imports
            .get(target)
            .or_else(|| context.internals.get(target))
            .copied()
            .ok_or_else(|| format!("unknown web call '{target}'"))? as usize;
        return Ok(context.signatures[index].result != TYPE_ID_VOID);
    }
    Ok(true)
}

fn encode_zero(type_id: TypeId, out: &mut Vec<u8>) -> Result<(), String> {
    match wasm_value_type(type_id)? {
        I32 => out.extend([0x41, 0]),
        F32 => {
            out.push(0x43);
            out.extend(0.0f32.to_le_bytes());
        }
        F64 => {
            out.push(0x44);
            out.extend(0.0f64.to_le_bytes());
        }
        _ => return Err("unsupported web zero value".to_string()),
    }
    Ok(())
}

fn collect_string_literals(functions: &[(FunctionMeta, FunctionHIR)]) -> BTreeMap<i32, String> {
    fn expression(value: &SimpleExpr, out: &mut BTreeMap<i32, String>) {
        match value {
            SimpleExpr::StringLiteral(value) => {
                out.insert(
                    crate::backend::emit::hash_string_literal(value),
                    value.clone(),
                );
            }
            SimpleExpr::Condition(value) => condition(value, out),
            SimpleExpr::IndexedPath { index, .. } => expression(index, out),
            SimpleExpr::Call { args, .. } => {
                for arg in args {
                    expression(arg, out);
                }
            }
            SimpleExpr::Binary { lhs, rhs, .. } => {
                expression(lhs, out);
                expression(rhs, out);
            }
            _ => {}
        }
    }
    fn condition(value: &SimpleCondition, out: &mut BTreeMap<i32, String>) {
        match value {
            SimpleCondition::Comparison { lhs, rhs, .. } => {
                expression(lhs, out);
                expression(rhs, out);
            }
            SimpleCondition::Expr(value) => expression(value, out),
            SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
                condition(lhs, out);
                condition(rhs, out);
            }
            SimpleCondition::Not(value) => condition(value, out),
        }
    }
    fn statements(values: &[SimpleStmt], out: &mut BTreeMap<i32, String>) {
        for value in values {
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
                    condition: value,
                    then_statements,
                    else_statements,
                } => {
                    condition(value, out);
                    statements(then_statements, out);
                    if let Some(values) = else_statements {
                        statements(values, out);
                    }
                }
                SimpleStmt::For {
                    init,
                    condition: value,
                    step,
                    body_statements,
                } => {
                    statements(std::slice::from_ref(init), out);
                    condition(value, out);
                    statements(std::slice::from_ref(step), out);
                    statements(body_statements, out);
                }
                SimpleStmt::Foreach {
                    body_statements, ..
                } => statements(body_statements, out),
                SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
            }
        }
    }
    let mut out = BTreeMap::new();
    for (_, hir) in functions {
        statements(&hir.statements, &mut out);
    }
    out
}

fn section(id: u8, payload: Vec<u8>, module: &mut Vec<u8>) {
    if payload.is_empty() {
        return;
    }
    module.push(id);
    uleb(payload.len() as u32, module);
    module.extend(payload);
}

fn string(value: &str, out: &mut Vec<u8>) {
    uleb(value.len() as u32, out);
    out.extend(value.as_bytes());
}

fn uleb(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn sleb(mut value: i32, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        out.push(if done { byte } else { byte | 0x80 });
        if done {
            break;
        }
    }
}

fn sleb64(mut value: i64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        out.push(if done { byte } else { byte | 0x80 });
        if done {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_test_uleb(bytes: &[u8], cursor: &mut usize) -> u32 {
        let mut value = 0;
        let mut shift = 0;
        loop {
            let byte = bytes[*cursor];
            *cursor += 1;
            value |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
            shift += 7;
        }
    }

    fn type_section_entry_count(module: &[u8]) -> u32 {
        let mut cursor = 8;
        while cursor < module.len() {
            let section_id = module[cursor];
            cursor += 1;
            let section_len = read_test_uleb(module, &mut cursor) as usize;
            if section_id == 1 {
                return read_test_uleb(module, &mut cursor);
            }
            cursor += section_len;
        }
        panic!("missing Wasm type section");
    }

    #[test]
    fn encodes_one_unsigned_bounds_trap_for_both_invalid_regions() {
        let mut bytes = Vec::new();
        emit_index_bounds_check(3, 7, &mut bytes);
        assert_eq!(bytes, [0x20, 3, 0x41, 7, 0x4f, 0x04, 0x40, 0x00, 0x0b]);
        for index in [-1, 0, 6, 7, i32::MAX] {
            assert_eq!(
                (index as u32) >= 7,
                index < 0 || index >= 7,
                "unsigned comparison changed bounds semantics for {index}"
            );
        }
    }

    #[test]
    fn groups_adjacent_wasm_local_types() {
        let mut bytes = Vec::new();
        encode_local_declarations(&[I32, I32, I32, F32, F64, F64], &mut bytes);
        assert_eq!(bytes, [3, 3, I32, 1, F32, 2, F64]);
    }

    #[test]
    fn omits_fallback_only_after_an_explicit_return() {
        let return_zero = SimpleStmt::Return(SimpleExpr::Int(0));
        assert!(ends_with_explicit_return(std::slice::from_ref(
            &return_zero
        )));
        assert!(!ends_with_explicit_return(&[SimpleStmt::If {
            condition: SimpleCondition::Expr(SimpleExpr::Bool(true)),
            then_statements: vec![return_zero.clone()],
            else_statements: Some(vec![return_zero]),
        }]));
        assert!(!ends_with_explicit_return(&[SimpleStmt::If {
            condition: SimpleCondition::Expr(SimpleExpr::Bool(true)),
            then_statements: vec![SimpleStmt::Return(SimpleExpr::Int(0))],
            else_statements: None,
        }]));
    }

    #[test]
    fn emits_valid_wasm_header_and_real_entry_exports() {
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&["main".into(), "tick".into(), "render".into()]);
        process.upsert_file(
            "web.stasis",
            "global x: i32; function truth(value: bool): i32 { if (value) { return 1; } return 0; } function main(): i32 { print_i32(1); print_int(2); x = 3; return x + truth(true); } function tick(): i32 { x += 1; return x; } function render(): i32 { return x; }",
        );
        process.compile().expect("compile web module");
        assert!(process.module_bytes().starts_with(b"\0asm\x01\0\0\0"));
        assert_eq!(
            type_section_entry_count(process.module_bytes()),
            6,
            "equivalent import, internal, and accessor signatures should share Wasm types"
        );
        assert_eq!(
            process
                .program_snapshot()
                .expect("web ProgramSnapshot")
                .global_type_ids()["x"],
            TYPE_ID_I32
        );
        for name in ["main", "tick", "render"] {
            assert!(process
                .module_bytes()
                .windows(name.len())
                .any(|window| window == name.as_bytes()));
        }
    }

    #[test]
    fn release_wasm_keeps_internal_global_names_private() {
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&["main".into(), "tick".into(), "render".into()]);
        process.upsert_file(
            "release.stasis",
            "global internal_score: i32; function main(): i32 { internal_score = 4; return internal_score; } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
        );
        process.compile().expect("compile release web module");
        assert!(!process
            .module_bytes()
            .windows("internal_score".len())
            .any(|window| window == b"internal_score"));
        for exported in [
            "__stasis_global_get_i32",
            "__stasis_global_set_i32",
            "main",
            "tick",
            "render",
        ] {
            assert!(process
                .module_bytes()
                .windows(exported.len())
                .any(|window| window == exported.as_bytes()));
        }
    }

    #[test]
    fn passes_fixed_collection_offsets_to_array_view_host_imports() {
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&["main".into(), "tick".into(), "render".into()]);
        process.upsert_file(
            "audio.stasis",
            "global samples: f32[4]; extern function audio_push_f32_interleaved(values: f32[], frames: i32): i32; function main(): i32 { samples[0] = 0.25; return audio_push_f32_interleaved(samples, 2); } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
        );
        process.compile().expect("compile web audio view module");
        assert_eq!(process.memory_layout()["samples"].offset, 0);
        assert!(process
            .module_bytes()
            .windows("audio_push_f32_interleaved".len())
            .any(|window| window == b"audio_push_f32_interleaved"));
    }

    #[test]
    fn indexes_and_updates_internal_collection_views() {
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&["main".into(), "tick".into(), "render".into()]);
        process.upsert_file(
            "views.stasis",
            "global text: ascii[8]; function update(s: ascii[]): i32 { s[0] = 65; s.length = 1; return s[0] + s.length + s.max_length; } function main(): i32 { return update(text); } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
        );
        process.compile().expect("compile web collection views");
    }

    #[test]
    fn lowers_foreach_over_struct_collection_storage() {
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&["main".into(), "tick".into(), "render".into()]);
        process.upsert_file(
            "foreach.stasis",
            "struct Item { value: i32; active: bool; } global items: Item[4]; function main(): i32 { items[2].value = 7; items[2].active = true; return 0; } function tick(): i32 { foreach (let item, i in items) { if (item.active) { item.value += i; } } return items[2].value; } function render(): i32 { return 0; }",
        );
        process.compile().expect("compile web foreach module");
        assert_eq!(process.memory_layout()["items.value"].length, 4);
        assert_eq!(process.memory_layout()["items.active"].length, 4);
    }

    #[test]
    fn passes_struct_collection_elements_as_native_shape_views() {
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&["main".into(), "tick".into(), "render".into()]);
        process.upsert_file(
            "views.stasis",
            "struct Item { value: i32; active: bool; } global items: Item[4]; global chosen: Item; function read(item: Item): i32 { if (item.active) { return item.value; } return 0; } function main(): i32 { chosen.value = 3; chosen.active = true; items[2].value = 7; items[2].active = true; items[1] = items[2]; let selected: Item = items[1]; selected.value += read(chosen); return read(selected); } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
        );
        process.compile().expect("compile web struct views");
        assert_eq!(process.memory_layout()["items.value"].length, 4);
        assert!(process.module_bytes().starts_with(b"\0asm\x01\0\0\0"));
    }

    #[test]
    fn lowers_numeric_intrinsics_without_fake_host_calls() {
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&["main".into(), "tick".into(), "render".into()]);
        process.upsert_file(
            "intrinsics.stasis",
            "function main(): i32 { let scaled: i32 = fixed32_mul(fixed32_from_i32(3), fixed32_from_ratio(1, 2)); let value: f32 = i32_to_f32(fixed32_to_i32(scaled)); return f32_to_i32(value); } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
        );
        process.compile().expect("compile web intrinsic module");
        assert!(process.module_bytes().starts_with(b"\0asm\x01\0\0\0"));
    }

    #[test]
    fn reuses_same_typed_local_names_from_disjoint_scopes() {
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&["main".into(), "tick".into(), "render".into()]);
        process.upsert_file(
            "scopes.stasis",
            "function main(): i32 { if (true) { let row: i32 = 2; } else { let row: i32 = 3; } if (true) { let row: i32 = 4; return row; } return 0; } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
        );
        process.compile().expect("compile repeated scoped locals");
    }

    #[test]
    fn lowers_boolean_conditions_with_native_short_circuit_semantics() {
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&["main".into(), "tick".into(), "render".into()]);
        process.upsert_file(
            "short_circuit.stasis",
            "global values: i32[2]; function main(): i32 { let index: i32 = -1; if (index >= 0 && values[index] == 0) { return 1; } if (index < 0 || values[index] == 0) { return 2; } return 0; } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
        );
        process.compile().expect("compile short-circuit conditions");
    }
}
