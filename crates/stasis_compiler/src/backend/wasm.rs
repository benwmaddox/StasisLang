//! Browser WebAssembly emission for the scalar Stasis lane.
//!
//! This intentionally consumes the same parsed HIR as JIT/AOT. Unsupported
//! storage or expression shapes fail at package time instead of receiving a
//! target-specific substitute implementation.

use crate::backend::emit::{
    build_compile_analysis_cache, compute_files_fingerprint, resolve_extern_call_signatures_with,
    AssignOp, AssignTarget, ComparisonOp, ConstantValue, SimpleCondition, SimpleExpr, SimpleStmt,
};
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

#[derive(Debug, Clone, Default)]
pub struct WasmProcess {
    compiler: Compiler,
    required_roots: Vec<String>,
    module: Vec<u8>,
    string_literals: BTreeMap<i32, String>,
    memory_layout: BTreeMap<String, WasmMemoryLayout>,
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

    pub fn last_source_diagnostic(&self) -> Option<&crate::SourceDiagnostic> {
        self.compiler.last_source_diagnostic()
    }

    pub fn compile(&mut self) -> CompileResult<CompileReport> {
        let index = self.compiler.index_pass()?;
        let mut types = self.compiler.types().clone();
        let analysis = build_compile_analysis_cache(
            self.compiler.files(),
            self.compiler.functions(),
            &mut types,
            compute_files_fingerprint(self.compiler.files()),
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
            build_memory_bindings(&analysis).map_err(CompileError::Backend)?;
        self.memory_layout = memory_bindings
            .into_iter()
            .map(|(path, binding)| {
                (
                    path,
                    WasmMemoryLayout {
                        offset: binding.offset,
                        type_id: binding.type_id,
                        length: binding.len,
                    },
                )
            })
            .collect();
        self.module = encode_module(&lowered, &analysis, &types).map_err(CompileError::Backend)?;
        Ok(CompileReport { index, emit })
    }
}

#[derive(Clone)]
struct Signature {
    params: Vec<TypeId>,
    result: TypeId,
}

fn is_i32_lane(type_id: TypeId) -> bool {
    matches!(
        type_id,
        TYPE_ID_I32 | TYPE_ID_BOOL | TYPE_ID_U8 | TYPE_ID_U16 | TYPE_ID_U32
    )
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

#[derive(Debug, Clone)]
struct MemoryBinding {
    offset: u32,
    type_id: TypeId,
    len: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmMemoryLayout {
    pub offset: u32,
    pub type_id: TypeId,
    pub length: i32,
}

fn storage_width(type_id: TypeId) -> Result<u32, String> {
    match type_id {
        TYPE_ID_BOOL | TYPE_ID_U8 => Ok(1),
        TYPE_ID_U16 => Ok(2),
        TYPE_ID_I32 | TYPE_ID_U32 | TYPE_ID_F32 => Ok(4),
        TYPE_ID_F64 => Ok(8),
        _ => Err(format!(
            "web memory does not support element type id {type_id}"
        )),
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
) -> Result<(BTreeMap<String, MemoryBinding>, u32), String> {
    let mut offset = 0u32;
    let mut bindings = BTreeMap::new();
    for (path, collection) in &analysis.collection_infos {
        if let Some(type_id) = collection.element_type {
            let width = storage_width(type_id)?;
            offset = align_up(offset, width)?;
            bindings.insert(
                path.clone(),
                MemoryBinding {
                    offset,
                    type_id,
                    len: collection.len,
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
            let width = storage_width(*type_id)?;
            offset = align_up(offset, width)?;
            let field_path = format!("{path}.{field}");
            bindings.insert(
                field_path,
                MemoryBinding {
                    offset,
                    type_id: *type_id,
                    len: collection.len,
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

fn encode_module(
    functions: &[(FunctionMeta, FunctionHIR)],
    analysis: &crate::backend::emit::CompileAnalysisCache,
    types: &TypeTable,
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
    imports.sort_by(|left, right| left.0.cmp(&right.0));

    let imported_names = imports
        .iter()
        .map(|(name, _, _)| name.clone())
        .collect::<BTreeSet<_>>();
    for target in &called {
        if !internal_by_name.contains_key(target) && !imported_names.contains(target) {
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

    let (memory_bindings, memory_bytes) = build_memory_bindings(analysis)?;
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
        if info.category == TypeCategory::Named
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
    uleb(signatures.len() as u32, &mut type_section);
    for signature in &signatures {
        type_section.push(0x60);
        uleb(signature.params.len() as u32, &mut type_section);
        for type_id in &signature.params {
            type_section.push(wasm_value_type(*type_id)?);
        }
        if signature.result == TYPE_ID_VOID {
            type_section.push(0);
        } else {
            type_section.extend([1, wasm_value_type(signature.result)?]);
        }
    }
    section(1, type_section, &mut module);

    if !imports.is_empty() {
        let mut import_section = Vec::new();
        uleb(imports.len() as u32, &mut import_section);
        for (_, symbol, _) in &imports {
            string("env", &mut import_section);
            string(symbol, &mut import_section);
            import_section.push(0);
            uleb(
                import_section_type_index(&imports, symbol)?,
                &mut import_section,
            );
        }
        section(2, import_section, &mut module);
    }

    let mut function_section = Vec::new();
    uleb(functions.len() as u32, &mut function_section);
    for index in 0..functions.len() {
        uleb((imports.len() + index) as u32, &mut function_section);
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
            + globals.len() as u32
            + u32::from(memory_bytes > 0),
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
    for (index, (name, _, _)) in globals.iter().enumerate() {
        string(name, &mut export_section);
        export_section.push(3);
        uleb(index as u32, &mut export_section);
    }
    if memory_bytes > 0 {
        string("memory", &mut export_section);
        export_section.push(2);
        uleb(0, &mut export_section);
    }
    section(7, export_section, &mut module);

    let mut code_section = Vec::new();
    uleb(functions.len() as u32, &mut code_section);
    for (function, hir) in functions {
        let body = encode_function(
            function,
            hir,
            &analysis.constant_values,
            &global_indices,
            &analysis.global_path_types,
            &memory_bindings,
            &import_indices,
            &internal_indices,
            &signatures,
        )?;
        uleb(body.len() as u32, &mut code_section);
        code_section.extend(body);
    }
    section(10, code_section, &mut module);
    Ok(module)
}

fn import_section_type_index(
    imports: &[(String, String, Signature)],
    symbol: &str,
) -> Result<u32, String> {
    imports
        .iter()
        .position(|(_, candidate, _)| candidate == symbol)
        .map(|index| index as u32)
        .ok_or_else(|| format!("missing web import type for '{symbol}'"))
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
    imports: &BTreeMap<String, u32>,
    internals: &BTreeMap<String, u32>,
    signatures: &[Signature],
) -> Result<Vec<u8>, String> {
    let mut local_declarations = Vec::new();
    collect_locals(&hir.statements, &mut local_declarations)?;
    let mut locals = function
        .param_names
        .iter()
        .zip(function.params.iter())
        .enumerate()
        .map(|(index, (name, type_id))| {
            (
                name.clone(),
                LocalBinding {
                    index: index as u32,
                    type_id: *type_id,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    for (name, type_id) in &local_declarations {
        if locals.contains_key(name) {
            return Err(format!("duplicate local '{name}' in '{}'", function.name));
        }
        locals.insert(
            name.clone(),
            LocalBinding {
                index: locals.len() as u32,
                type_id: *type_id,
            },
        );
    }

    let mut body = Vec::new();
    uleb(local_declarations.len() as u32 + 4, &mut body);
    for (_, type_id) in &local_declarations {
        uleb(1, &mut body);
        body.push(wasm_value_type(*type_id)?);
    }
    let scratch_index = locals.len() as u32;
    let scratch_i32 = scratch_index + 1;
    let scratch_f32 = scratch_index + 2;
    let scratch_f64 = scratch_index + 3;
    for value_type in [I32, I32, F32, F64] {
        uleb(1, &mut body);
        body.push(value_type);
    }
    let context = EncodeContext {
        locals: &locals,
        globals,
        global_types,
        memory,
        constants,
        imports,
        internals,
        signatures,
        scratch_index,
        return_type: function.return_type,
        scratch_i32,
        scratch_f32,
        scratch_f64,
    };
    encode_statements(&hir.statements, &context, &mut body)?;
    if function.return_type != TYPE_ID_VOID {
        encode_zero(function.return_type, &mut body)?;
    }
    body.push(0x0b);
    Ok(body)
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
                out.push((name.clone(), type_id));
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
            SimpleStmt::Foreach { .. } => {
                return Err("web scalar lane does not yet support foreach".to_string())
            }
            _ => {}
        }
    }
    Ok(())
}

struct EncodeContext<'a> {
    locals: &'a BTreeMap<String, LocalBinding>,
    globals: &'a BTreeMap<String, u32>,
    global_types: &'a BTreeMap<String, TypeId>,
    memory: &'a BTreeMap<String, MemoryBinding>,
    constants: &'a BTreeMap<String, ConstantValue>,
    imports: &'a BTreeMap<String, u32>,
    internals: &'a BTreeMap<String, u32>,
    signatures: &'a [Signature],
    scratch_index: u32,
    return_type: TypeId,
    scratch_i32: u32,
    scratch_f32: u32,
    scratch_f64: u32,
}

#[derive(Debug, Clone, Copy)]
struct LocalBinding {
    index: u32,
    type_id: TypeId,
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
                let target_type = target_type(target, context)?;
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
            SimpleStmt::Foreach { .. } => {
                return Err("web scalar lane does not yet support foreach".to_string())
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
            if let Some(binding) = context.locals.get(name) {
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
            out.push(0x23);
            uleb(global(context, name)?, out);
            global_type(context, name)
        }
        AssignTarget::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
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
            if let Some(binding) = context.locals.get(name) {
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
            out.push(0x24);
            uleb(global(context, name)?, out);
            Ok(())
        }
        AssignTarget::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
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
        AssignTarget::Local(name) => context
            .locals
            .get(name)
            .map(|binding| binding.type_id)
            .or_else(|| context.global_types.get(name).copied())
            .ok_or_else(|| format!("unknown web assignment target '{name}'")),
        AssignTarget::GlobalPath(name) => global_type(context, name),
        AssignTarget::IndexedPath {
            collection_path,
            suffix,
            ..
        } => Ok(memory_binding(context, collection_path, suffix)?.type_id),
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

fn encode_memory_address(
    binding: &MemoryBinding,
    index: &SimpleExpr,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let index_type = encode_expr(index, context, out)?;
    if !is_i32_lane(index_type) {
        return Err("web collection index must be i32-compatible".to_string());
    }
    out.push(0x21);
    uleb(context.scratch_index, out);
    out.push(0x20);
    uleb(context.scratch_index, out);
    out.extend([0x41, 0, 0x48, 0x04, 0x40, 0x00, 0x0b]);
    out.push(0x20);
    uleb(context.scratch_index, out);
    out.push(0x41);
    sleb(binding.len, out);
    out.extend([0x4e, 0x04, 0x40, 0x00, 0x0b]);
    out.push(0x41);
    sleb(binding.offset as i32, out);
    out.push(0x20);
    uleb(context.scratch_index, out);
    out.push(0x41);
    sleb(storage_width(binding.type_id)? as i32, out);
    out.push(0x6c);
    out.push(0x6a);
    Ok(())
}

fn encode_memory_load(type_id: TypeId, out: &mut Vec<u8>) -> Result<(), String> {
    let (opcode, align) = match type_id {
        TYPE_ID_BOOL | TYPE_ID_U8 => (0x2d, 0),
        TYPE_ID_U16 => (0x2f, 1),
        TYPE_ID_I32 | TYPE_ID_U32 => (0x28, 2),
        TYPE_ID_F32 => (0x2a, 2),
        TYPE_ID_F64 => (0x2b, 3),
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
            if let Some(binding) = context.locals.get(name) {
                out.push(0x20);
                uleb(binding.index, out);
                Ok(binding.type_id)
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
                let actual = encode_expr_as(arg, Some(*param_type), context, out)?;
                require_same_type(*param_type, actual, "call argument")?;
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
            let binding = memory_binding(context, collection_path, suffix)?;
            encode_memory_address(binding, index, context, out)?;
            encode_memory_load(binding.type_id, out)?;
            Ok(binding.type_id)
        }
    }
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
            encode_condition(rhs, context, out)?;
            out.push(0x71);
        }
        SimpleCondition::Or(lhs, rhs) => {
            encode_condition(lhs, context, out)?;
            encode_condition(rhs, context, out)?;
            out.push(0x72);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_valid_wasm_header_and_real_entry_exports() {
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&["main".into(), "tick".into(), "render".into()]);
        process.upsert_file(
            "web.stasis",
            "global x: i32; function main(): i32 { x = 3; return x; } function tick(): i32 { x += 1; return x; } function render(): i32 { return x; }",
        );
        process.compile().expect("compile web module");
        assert!(process.module_bytes().starts_with(b"\0asm\x01\0\0\0"));
        for name in ["main", "tick", "render"] {
            assert!(process
                .module_bytes()
                .windows(name.len())
                .any(|window| window == name.as_bytes()));
        }
    }
}
