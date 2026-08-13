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
    TypeId, TypeTable, TYPE_ID_BOOL, TYPE_ID_I32, TYPE_ID_U16, TYPE_ID_U32, TYPE_ID_U8,
    TYPE_ID_VOID,
};
use crate::ir::hir::FunctionHIR;
use std::collections::{BTreeMap, BTreeSet};

const I32: u8 = 0x7f;

#[derive(Debug, Clone, Default)]
pub struct WasmProcess {
    compiler: Compiler,
    required_roots: Vec<String>,
    module: Vec<u8>,
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

        let function_ids = self
            .compiler
            .functions()
            .iter()
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

fn validate_signature(name: &str, signature: &Signature) -> Result<(), String> {
    if signature.params.iter().any(|value| !is_i32_lane(*value))
        || (signature.result != TYPE_ID_VOID && !is_i32_lane(signature.result))
    {
        return Err(format!(
            "web scalar lane does not yet support non-i32 signature for '{name}'"
        ));
    }
    Ok(())
}

fn encode_module(
    functions: &[(FunctionMeta, FunctionHIR)],
    analysis: &crate::backend::emit::CompileAnalysisCache,
    types: &TypeTable,
) -> Result<Vec<u8>, String> {
    let mut internal_by_name = BTreeMap::new();
    for (index, (function, _)) in functions.iter().enumerate() {
        if internal_by_name
            .insert(function.name.clone(), index)
            .is_some()
        {
            return Err(format!(
                "web scalar lane requires unique function names; '{}' is overloaded",
                function.name
            ));
        }
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

    let mut globals = Vec::new();
    for (name, type_id) in &analysis.global_path_types {
        if name.contains('.') {
            return Err(format!(
                "web scalar lane does not yet support structured state path '{name}'"
            ));
        }
        if !is_i32_lane(*type_id) {
            return Err(format!(
                "web scalar lane does not yet support global '{name}' with type {}",
                types
                    .type_info(*type_id)
                    .map_or("unknown", |info| info.name.as_str())
            ));
        }
        globals.push((name.clone(), *type_id));
    }
    let global_indices = globals
        .iter()
        .enumerate()
        .map(|(index, (name, _))| (name.clone(), index as u32))
        .collect::<BTreeMap<_, _>>();

    let import_indices = imports
        .iter()
        .enumerate()
        .map(|(index, (name, _, _))| (name.clone(), index as u32))
        .collect::<BTreeMap<_, _>>();
    let internal_indices = functions
        .iter()
        .enumerate()
        .map(|(index, (function, _))| (function.name.clone(), (imports.len() + index) as u32))
        .collect::<BTreeMap<_, _>>();

    let mut module = b"\0asm\x01\0\0\0".to_vec();

    let mut type_section = Vec::new();
    uleb(signatures.len() as u32, &mut type_section);
    for signature in &signatures {
        type_section.push(0x60);
        uleb(signature.params.len() as u32, &mut type_section);
        type_section.extend(std::iter::repeat_n(I32, signature.params.len()));
        if signature.result == TYPE_ID_VOID {
            type_section.push(0);
        } else {
            type_section.extend([1, I32]);
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

    if !globals.is_empty() {
        let mut global_section = Vec::new();
        uleb(globals.len() as u32, &mut global_section);
        for _ in &globals {
            global_section.extend([I32, 1, 0x41, 0, 0x0b]);
        }
        section(6, global_section, &mut module);
    }

    let mut export_section = Vec::new();
    uleb(
        functions.len() as u32 + globals.len() as u32,
        &mut export_section,
    );
    for (index, (function, _)) in functions.iter().enumerate() {
        string(&function.name, &mut export_section);
        export_section.push(0);
        uleb((imports.len() + index) as u32, &mut export_section);
    }
    for (index, (name, _)) in globals.iter().enumerate() {
        string(name, &mut export_section);
        export_section.push(3);
        uleb(index as u32, &mut export_section);
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
    imports: &BTreeMap<String, u32>,
    internals: &BTreeMap<String, u32>,
    signatures: &[Signature],
) -> Result<Vec<u8>, String> {
    let mut local_names = Vec::new();
    collect_locals(&hir.statements, &mut local_names)?;
    let mut locals = function
        .param_names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index as u32))
        .collect::<BTreeMap<_, _>>();
    for name in &local_names {
        if locals.contains_key(name) {
            return Err(format!("duplicate local '{name}' in '{}'", function.name));
        }
        locals.insert(name.clone(), locals.len() as u32);
    }

    let mut body = Vec::new();
    if local_names.is_empty() {
        body.push(0);
    } else {
        body.push(1);
        uleb(local_names.len() as u32, &mut body);
        body.push(I32);
    }
    let context = EncodeContext {
        locals: &locals,
        globals,
        constants,
        imports,
        internals,
        signatures,
    };
    encode_statements(&hir.statements, &context, &mut body)?;
    if function.return_type != TYPE_ID_VOID {
        body.extend([0x41, 0]);
    }
    body.push(0x0b);
    Ok(body)
}

fn collect_locals(statements: &[SimpleStmt], out: &mut Vec<String>) -> Result<(), String> {
    for statement in statements {
        match statement {
            SimpleStmt::Let { name, type_id, .. } => {
                if type_id.is_some_and(|value| !is_i32_lane(value)) {
                    return Err(format!(
                        "web scalar lane requires i32-compatible local '{name}'"
                    ));
                }
                out.push(name.clone());
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
    locals: &'a BTreeMap<String, u32>,
    globals: &'a BTreeMap<String, u32>,
    constants: &'a BTreeMap<String, ConstantValue>,
    imports: &'a BTreeMap<String, u32>,
    internals: &'a BTreeMap<String, u32>,
    signatures: &'a [Signature],
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
                encode_expr(expression, context, out)?;
                out.push(0x21);
                uleb(local(context, name)?, out);
            }
            SimpleStmt::Assign {
                target,
                op,
                expression,
            } => {
                if *op != AssignOp::Set {
                    encode_target_get(target, context, out)?;
                }
                encode_expr(expression, context, out)?;
                if *op != AssignOp::Set {
                    out.push(arithmetic_opcode(*op)?);
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
                encode_expr(expression, context, out)?;
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
            SimpleStmt::Convert { .. } => {
                return Err("web scalar lane does not yet support conversions".to_string())
            }
            SimpleStmt::Foreach { .. } => {
                return Err("web scalar lane does not yet support foreach".to_string())
            }
        }
    }
    Ok(())
}

fn arithmetic_opcode(op: AssignOp) -> Result<u8, String> {
    match op {
        AssignOp::Add => Ok(0x6a),
        AssignOp::Sub => Ok(0x6b),
        AssignOp::Mul => Ok(0x6c),
        AssignOp::Div => Ok(0x6d),
        AssignOp::Mod => Ok(0x6f),
        AssignOp::Set => Err("set has no arithmetic opcode".to_string()),
    }
}

fn encode_target_get(
    target: &AssignTarget,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    match target {
        AssignTarget::Local(name) => {
            if let Some(index) = context.locals.get(name) {
                out.push(0x20);
                uleb(*index, out);
                Ok(())
            } else {
                out.push(0x23);
                uleb(global(context, name)?, out);
                Ok(())
            }
        }
        AssignTarget::GlobalPath(name) => {
            out.push(0x23);
            uleb(global(context, name)?, out);
            Ok(())
        }
        AssignTarget::IndexedPath { .. } => {
            Err("web scalar lane does not yet support indexed assignment".to_string())
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
            if let Some(index) = context.locals.get(name) {
                out.push(0x21);
                uleb(*index, out);
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
        AssignTarget::IndexedPath { .. } => {
            Err("web scalar lane does not yet support indexed assignment".to_string())
        }
    }
}

fn local(context: &EncodeContext<'_>, name: &str) -> Result<u32, String> {
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

fn encode_expr(
    value: &SimpleExpr,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    match value {
        SimpleExpr::Int(value) => {
            out.push(0x41);
            sleb(*value as i32, out);
        }
        SimpleExpr::Bool(value) => out.extend([0x41, u8::from(*value)]),
        SimpleExpr::Identifier(name) => {
            if let Some(index) = context.locals.get(name) {
                out.push(0x20);
                uleb(*index, out);
            } else if let Some(index) = context.globals.get(name) {
                out.push(0x23);
                uleb(*index, out);
            } else if let Some(value) = context.constants.get(name) {
                encode_constant(value, out)?;
            } else {
                return Err(format!("unknown web value '{name}'"));
            }
        }
        SimpleExpr::Call { target, args } => {
            for arg in args {
                encode_expr(arg, context, out)?;
            }
            out.push(0x10);
            let index = context
                .imports
                .get(target)
                .or_else(|| context.internals.get(target))
                .copied()
                .ok_or_else(|| format!("unknown web call '{target}'"))?;
            uleb(index, out);
        }
        SimpleExpr::Binary { lhs, op, rhs } => {
            encode_expr(lhs, context, out)?;
            encode_expr(rhs, context, out)?;
            out.push(match op {
                '+' => 0x6a,
                '-' => 0x6b,
                '*' => 0x6c,
                '/' => 0x6d,
                '%' => 0x6f,
                other => return Err(format!("unsupported web binary operator '{other}'")),
            });
        }
        SimpleExpr::Condition(condition) => encode_condition(condition, context, out)?,
        SimpleExpr::Float(_) => {
            return Err("web scalar lane does not yet support float expressions".to_string())
        }
        SimpleExpr::StringLiteral(_) => {
            return Err("web scalar lane does not yet support string expressions".to_string())
        }
        SimpleExpr::IndexedPath { .. } => {
            return Err("web scalar lane does not yet support indexed expressions".to_string())
        }
    }
    Ok(())
}

fn encode_constant(value: &ConstantValue, out: &mut Vec<u8>) -> Result<(), String> {
    let value = match value {
        ConstantValue::I32 { value, .. } => *value,
        ConstantValue::Bool(value) => i32::from(*value),
        _ => return Err("web scalar lane only supports i32-compatible constants".to_string()),
    };
    out.push(0x41);
    sleb(value, out);
    Ok(())
}

fn encode_condition(
    value: &SimpleCondition,
    context: &EncodeContext<'_>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    match value {
        SimpleCondition::Comparison { lhs, op, rhs } => {
            encode_expr(lhs, context, out)?;
            encode_expr(rhs, context, out)?;
            out.push(match op {
                ComparisonOp::Eq => 0x46,
                ComparisonOp::Ne => 0x47,
                ComparisonOp::Lt => 0x48,
                ComparisonOp::Le => 0x4c,
                ComparisonOp::Gt => 0x4a,
                ComparisonOp::Ge => 0x4e,
            });
        }
        SimpleCondition::Expr(value) => encode_expr(value, context, out)?,
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
