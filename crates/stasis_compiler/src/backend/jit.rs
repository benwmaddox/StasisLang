use crate::backend::EngineEntrypoints;
use crate::compiler::{CompileReport, CompileResult, Compiler, FunctionId, FunctionMeta};
use crate::frontend::types::{TYPE_ID_I32, TYPE_ID_VOID};
use crate::ir::hir::FunctionHIR;
use cranelift_codegen::ir::{condcodes::IntCC, types, AbiParam, FuncRef, InstBuilder, Value};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, FuncId, Linkage, Module};
use std::collections::BTreeMap;

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
        }
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.compiler.upsert_file(path, content);
    }

    pub fn compile(&mut self) -> CompileResult<CompileReport> {
        let (compiler, next_slot, next_symbol_seq, artifacts, modules) = (
            &mut self.compiler,
            &mut self.next_slot,
            &mut self.next_symbol_seq,
            &mut self.artifacts,
            &mut self.modules,
        );
        let report = compiler.compile_with(|meta, hir| {
            let symbol = format!("jit_fn_{}_{}", meta.id, *next_symbol_seq);
            *next_symbol_seq = next_symbol_seq.saturating_add(1);
            let (module, code_ptr) = compile_function_to_jit_module(meta, hir, &symbol)?;
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
        self.refresh_runtime_dispatch_table();
        Ok(report)
    }

    pub fn artifacts(&self) -> &[JitArtifact] {
        &self.artifacts
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
        let mut entries = Vec::new();
        for function in self.compiler.functions() {
            if function.return_type != TYPE_ID_I32 {
                continue;
            }
            if !function.params.iter().all(|type_id| *type_id == TYPE_ID_I32) {
                continue;
            }
            let Ok(arity) = u8::try_from(function.params.len()) else {
                continue;
            };
            if arity > 2 {
                continue;
            }
            let Some(artifact) = self
                .artifacts
                .iter()
                .find(|artifact| artifact.function_id == function.id)
            else {
                continue;
            };
            entries.push((crate::hash_identifier(&function.name), arity, artifact.code_ptr as usize));
        }
        stasis_dynload::replace_jit_i32_dispatch_table(&entries);
    }
}

impl Default for JitProcess {
    fn default() -> Self {
        Self::new()
    }
}

fn compile_function_to_jit_module(
    meta: &FunctionMeta,
    hir: &FunctionHIR,
    symbol: &str,
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
    let mut module = JITModule::new(jit_builder);
    let mut context = module.make_context();
    context.func.signature = module.make_signature();
    for param_type in &meta.params {
        match *param_type {
            TYPE_ID_I32 => context
                .func
                .signature
                .params
                .push(AbiParam::new(types::I32)),
            other => {
                return Err(format!(
                    "unsupported JIT parameter type id {other} for function {}",
                    meta.name
                ));
            }
        }
    }
    match meta.return_type {
        TYPE_ID_VOID => {}
        TYPE_ID_I32 => context
            .func
            .signature
            .returns
            .push(AbiParam::new(types::I32)),
        other => {
            return Err(format!(
                "unsupported JIT return type id {other} for function {}",
                meta.name
            ));
        }
    }

    let function_id = module
        .declare_function(symbol, Linkage::Export, &context.func.signature)
        .map_err(|error| format!("failed to declare JIT function {symbol}: {error}"))?;
    let runtime_call_imports = RuntimeCallImportIds {
        call_i32_0: declare_i32_call_import(&mut module, "stasis_jit_call_i32_0", 1)?,
        call_i32_1: declare_i32_call_import(&mut module, "stasis_jit_call_i32_1", 2)?,
        call_i32_2: declare_i32_call_import(&mut module, "stasis_jit_call_i32_2", 3)?,
    };

    let mut function_builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut function_builder_context);
        let runtime_call_refs = RuntimeCallRefs {
            call_i32_0: module.declare_func_in_func(runtime_call_imports.call_i32_0, builder.func),
            call_i32_1: module.declare_func_in_func(runtime_call_imports.call_i32_1, builder.func),
            call_i32_2: module.declare_func_in_func(runtime_call_imports.call_i32_2, builder.func),
        };
        let entry = builder.create_block();
        for _ in &meta.params {
            builder.append_block_param(entry, types::I32);
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
        let mut values_by_name = BTreeMap::new();
        let block_params: Vec<Value> = builder.block_params(entry).to_vec();
        for (index, name) in meta.param_names.iter().enumerate() {
            let Some(value) = block_params.get(index).copied() else {
                return Err(format!(
                    "missing block parameter {} for function '{}'",
                    index, meta.name
                ));
            };
            values_by_name.insert(name.clone(), value);
        }

        if meta.return_type == TYPE_ID_I32 {
            let statements = parse_simple_statements(hir)?;
            let terminated = emit_simple_i32_statements(
                &mut builder,
                &statements,
                &mut values_by_name,
                &runtime_call_refs,
            )?;
            if !terminated {
                return Err(format!(
                    "i32 function '{}' must end with a return statement",
                    meta.name
                ));
            }
        } else {
            builder.ins().return_(&[]);
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
}

struct RuntimeCallRefs {
    call_i32_0: FuncRef,
    call_i32_1: FuncRef,
    call_i32_2: FuncRef,
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum SimpleStmt {
    Let {
        name: String,
        expression: SimpleExpr,
    },
    If {
        condition: SimpleCondition,
        then_statements: Vec<SimpleStmt>,
    },
    Return(SimpleExpr),
    ReturnVoid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SimpleCondition {
    lhs: SimpleExpr,
    op: ComparisonOp,
    rhs: SimpleExpr,
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

fn parse_simple_statements(hir: &FunctionHIR) -> Result<Vec<SimpleStmt>, String> {
    let body = extract_function_body(hir)?;
    parse_simple_statements_from_block(body)
}

fn extract_function_body(hir: &FunctionHIR) -> Result<&str, String> {
    let Some(block) = hir.blocks.first() else {
        return Err("function body missing block text".to_string());
    };
    Ok(block.source.as_str())
}

fn parse_simple_statements_from_block(block_text: &str) -> Result<Vec<SimpleStmt>, String> {
    let trimmed = block_text.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err("expected function body block enclosed in '{...}'".to_string());
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    let mut statements = Vec::new();
    let mut cursor = 0usize;
    while cursor < inner.len() {
        cursor = skip_ascii_whitespace(inner, cursor);
        if cursor >= inner.len() {
            break;
        }
        if starts_with_keyword(inner, cursor, "let") {
            let let_start = cursor;
            let semicolon = find_statement_terminator(inner, cursor)?;
            let statement_text = inner[let_start..semicolon].trim();
            statements.push(parse_let_statement(statement_text)?);
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
        if starts_with_keyword(inner, cursor, "if") {
            let (statement, next_cursor) = parse_if_statement(inner, cursor)?;
            statements.push(statement);
            cursor = next_cursor;
            continue;
        }
        return Err(format!(
            "unsupported statement in function body near '{}'",
            snippet_from(inner, cursor)
        ));
    }
    Ok(statements)
}

fn parse_let_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    let after_let = statement_text
        .strip_prefix("let")
        .ok_or_else(|| format!("invalid let statement '{statement_text}'"))?;
    let mut cursor = skip_ascii_whitespace(after_let, 0);
    let (name, next) = parse_identifier(after_let, cursor)?;
    cursor = skip_ascii_whitespace(after_let, next);
    cursor = expect_byte(after_let, cursor, b':', "':' in let statement")?;
    cursor = skip_ascii_whitespace(after_let, cursor);
    let (type_name, next) = parse_identifier(after_let, cursor)?;
    if type_name != "i32" {
        return Err(format!(
            "unsupported let type '{}' in statement '{}'",
            type_name, statement_text
        ));
    }
    cursor = skip_ascii_whitespace(after_let, next);
    cursor = expect_byte(after_let, cursor, b'=', "'=' in let statement")?;
    let expression_text = after_let[cursor..].trim();
    if expression_text.is_empty() {
        return Err(format!("missing expression in let statement '{statement_text}'"));
    }
    Ok(SimpleStmt::Let {
        name: name.to_string(),
        expression: parse_simple_i32_expression(expression_text)?,
    })
}

fn parse_return_statement(statement_text: &str) -> Result<SimpleStmt, String> {
    let after_return = statement_text
        .strip_prefix("return")
        .ok_or_else(|| format!("invalid return statement '{statement_text}'"))?;
    let expression_text = after_return.trim();
    if expression_text.is_empty() {
        return Ok(SimpleStmt::ReturnVoid);
    }
    Ok(SimpleStmt::Return(parse_simple_i32_expression(
        expression_text,
    )?))
}

fn parse_if_statement(source: &str, start: usize) -> Result<(SimpleStmt, usize), String> {
    let mut cursor = start + "if".len();
    cursor = skip_ascii_whitespace(source, cursor);
    cursor = expect_byte(source, cursor, b'(', "'(' after if")?;
    let condition_open = cursor - 1;
    let condition_close = find_matching_delimiter(source, condition_open, b'(', b')')
        .ok_or_else(|| "missing ')' for if condition".to_string())?;
    let condition_text = source[condition_open + 1..condition_close].trim();
    if condition_text.is_empty() {
        return Err("if condition expression cannot be empty".to_string());
    }
    let condition = parse_simple_condition(condition_text)?;

    cursor = skip_ascii_whitespace(source, condition_close + 1);
    cursor = expect_byte(source, cursor, b'{', "'{' after if condition")?;
    let then_open = cursor - 1;
    let then_close = find_matching_delimiter(source, then_open, b'{', b'}')
        .ok_or_else(|| "missing '}' for if body".to_string())?;
    let then_block = &source[then_open..=then_close];
    let then_statements = parse_simple_statements_from_block(then_block)?;
    let next_cursor = then_close + 1;

    Ok((
        SimpleStmt::If {
            condition,
            then_statements,
        },
        next_cursor,
    ))
}

fn parse_simple_condition(condition_text: &str) -> Result<SimpleCondition, String> {
    let (op, position, width) = find_condition_operator(condition_text).ok_or_else(|| {
        format!(
            "unsupported if condition '{}': expected one comparison operator",
            condition_text
        )
    })?;
    let lhs_text = condition_text[..position].trim();
    let rhs_text = condition_text[position + width..].trim();
    if lhs_text.is_empty() || rhs_text.is_empty() {
        return Err(format!(
            "invalid if condition '{}': both sides of comparison are required",
            condition_text
        ));
    }
    Ok(SimpleCondition {
        lhs: parse_simple_i32_expression(lhs_text)?,
        op,
        rhs: parse_simple_i32_expression(rhs_text)?,
    })
}

fn find_condition_operator(condition_text: &str) -> Option<(ComparisonOp, usize, usize)> {
    let bytes = condition_text.as_bytes();
    let mut depth = 0i32;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
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

fn find_statement_terminator(source: &str, start: usize) -> Result<usize, String> {
    let mut depth = 0i32;
    let mut index = start;
    while index < source.len() {
        match source.as_bytes()[index] {
            b'(' | b'{' => depth += 1,
            b')' | b'}' => depth -= 1,
            b';' if depth == 0 => return Ok(index),
            _ => {}
        }
        index += 1;
    }
    Err(format!(
        "missing ';' terminator near '{}'",
        snippet_from(source, start)
    ))
}

fn find_matching_delimiter(
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
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == open {
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

fn snippet_from(source: &str, cursor: usize) -> String {
    source
        .get(cursor..)
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect()
}

fn emit_simple_i32_statements(
    builder: &mut FunctionBuilder<'_>,
    statements: &[SimpleStmt],
    values_by_name: &mut BTreeMap<String, Value>,
    runtime_call_refs: &RuntimeCallRefs,
) -> Result<bool, String> {
    for statement in statements {
        match statement {
            SimpleStmt::Let { name, expression } => {
                let value = emit_simple_i32_expression(
                    builder,
                    expression,
                    values_by_name,
                    runtime_call_refs,
                )?;
                values_by_name.insert(name.clone(), value);
            }
            SimpleStmt::Return(expression) => {
                let value = emit_simple_i32_expression(
                    builder,
                    expression,
                    values_by_name,
                    runtime_call_refs,
                )?;
                builder.ins().return_(&[value]);
                return Ok(true);
            }
            SimpleStmt::ReturnVoid => {
                return Err("void return statement is not allowed in i32 function".to_string());
            }
            SimpleStmt::If {
                condition,
                then_statements,
            } => {
                let condition_value = emit_simple_condition(
                    builder,
                    condition,
                    values_by_name,
                    runtime_call_refs,
                )?;
                let then_block = builder.create_block();
                let continue_block = builder.create_block();
                builder
                    .ins()
                    .brif(condition_value, then_block, &[], continue_block, &[]);
                builder.seal_block(then_block);
                builder.switch_to_block(then_block);

                let mut then_values = values_by_name.clone();
                let then_terminated = emit_simple_i32_statements(
                    builder,
                    then_statements,
                    &mut then_values,
                    runtime_call_refs,
                )?;
                if !then_terminated {
                    builder.ins().jump(continue_block, &[]);
                }

                builder.seal_block(continue_block);
                builder.switch_to_block(continue_block);
            }
        }
    }
    Ok(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SimpleExpr {
    Int(i64),
    Identifier(String),
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

fn parse_simple_i32_expression(expression: &str) -> Result<SimpleExpr, String> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExprToken {
    Int(i64),
    Identifier(String),
    Op(char),
    Comma,
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
        if byte.is_ascii_digit() {
            let start = index;
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            let text = &expression[start..index];
            let value = text
                .parse::<i64>()
                .map_err(|error| format!("invalid integer literal '{text}': {error}"))?;
            tokens.push(ExprToken::Int(value));
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
            b'(' => {
                tokens.push(ExprToken::LParen);
                index += 1;
            }
            b')' => {
                tokens.push(ExprToken::RParen);
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
            ExprToken::Identifier(name) => {
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
                } else {
                    Ok(SimpleExpr::Identifier(name))
                }
            }
            ExprToken::Op('-') => {
                let rhs = self.parse_primary()?;
                Ok(SimpleExpr::Binary {
                    lhs: Box::new(SimpleExpr::Int(0)),
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

fn emit_simple_i32_expression(
    builder: &mut FunctionBuilder<'_>,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, Value>,
    runtime_call_refs: &RuntimeCallRefs,
) -> Result<Value, String> {
    match expression {
        SimpleExpr::Int(value) => {
            let value = i32::try_from(*value).map_err(|_| {
                format!("integer literal out of i32 range in return expression: {value}")
            })?;
            Ok(builder.ins().iconst(types::I32, i64::from(value)))
        }
        SimpleExpr::Identifier(name) => values_by_name
            .get(name)
            .copied()
            .ok_or_else(|| format!("unknown identifier '{name}' in return expression")),
        SimpleExpr::Call { target, args } => {
            let target_hash = builder
                .ins()
                .iconst(types::I32, i64::from(crate::hash_identifier(target)));
            let call = match args.len() {
                0 => builder.ins().call(runtime_call_refs.call_i32_0, &[target_hash]),
                1 => {
                    let arg0 =
                        emit_simple_i32_expression(builder, &args[0], values_by_name, runtime_call_refs)?;
                    builder
                        .ins()
                        .call(runtime_call_refs.call_i32_1, &[target_hash, arg0])
                }
                2 => {
                    let arg0 =
                        emit_simple_i32_expression(builder, &args[0], values_by_name, runtime_call_refs)?;
                    let arg1 =
                        emit_simple_i32_expression(builder, &args[1], values_by_name, runtime_call_refs)?;
                    builder
                        .ins()
                        .call(runtime_call_refs.call_i32_2, &[target_hash, arg0, arg1])
                }
                other => {
                    return Err(format!(
                        "unsupported call arity {} in return expression for target '{}'",
                        other, target
                    ))
                }
            };
            let results = builder.inst_results(call);
            results
                .first()
                .copied()
                .ok_or_else(|| format!("call to '{}' produced no value", target))
        }
        SimpleExpr::Binary { lhs, op, rhs } => {
            let lhs_value =
                emit_simple_i32_expression(builder, lhs, values_by_name, runtime_call_refs)?;
            let rhs_value =
                emit_simple_i32_expression(builder, rhs, values_by_name, runtime_call_refs)?;
            let value = match op {
                '+' => builder.ins().iadd(lhs_value, rhs_value),
                '-' => builder.ins().isub(lhs_value, rhs_value),
                '*' => builder.ins().imul(lhs_value, rhs_value),
                '/' => builder.ins().sdiv(lhs_value, rhs_value),
                '%' => builder.ins().srem(lhs_value, rhs_value),
                other => {
                    return Err(format!(
                        "unsupported binary operator '{other}' in return expression"
                    ))
                }
            };
            Ok(value)
        }
    }
}

fn emit_simple_condition(
    builder: &mut FunctionBuilder<'_>,
    condition: &SimpleCondition,
    values_by_name: &BTreeMap<String, Value>,
    runtime_call_refs: &RuntimeCallRefs,
) -> Result<Value, String> {
    let lhs = emit_simple_i32_expression(builder, &condition.lhs, values_by_name, runtime_call_refs)?;
    let rhs = emit_simple_i32_expression(builder, &condition.rhs, values_by_name, runtime_call_refs)?;
    let intcc = match condition.op {
        ComparisonOp::Eq => IntCC::Equal,
        ComparisonOp::Ne => IntCC::NotEqual,
        ComparisonOp::Lt => IntCC::SignedLessThan,
        ComparisonOp::Le => IntCC::SignedLessThanOrEqual,
        ComparisonOp::Gt => IntCC::SignedGreaterThan,
        ComparisonOp::Ge => IntCC::SignedGreaterThanOrEqual,
    };
    Ok(builder.ins().icmp(intcc, lhs, rhs))
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
        assert_eq!(report.emit.emitted_functions, 2);
        assert_eq!(process.artifacts().len(), 2);
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
                    message.contains("unsupported call arity 3"),
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

    #[test]
    fn jit_process_incremental_compile_emits_only_changed_function() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return 2; }\n",
        );
        let first = process.compile().expect("first compile");
        assert_eq!(first.emit.emitted_functions, 2);
        assert_eq!(process.artifacts().len(), 2);

        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 3; }\nfunction main(): i32 { return 2; }\n",
        );
        let second = process.compile().expect("second compile");
        assert_eq!(second.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 2);
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
    fn jit_process_exposes_symbol_code_ptr_map() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return 2; }\n",
        );
        process.compile().expect("compile");
        let map = process.symbol_code_ptrs();
        assert!(map.contains_key("helper"));
        assert!(map.contains_key("main"));
        assert!(map.get("helper").copied().unwrap_or(0) != 0);
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
