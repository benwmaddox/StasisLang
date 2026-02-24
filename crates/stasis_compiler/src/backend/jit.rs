use crate::backend::EngineEntrypoints;
use crate::compiler::{CompileReport, CompileResult, Compiler, FunctionId, FunctionMeta};
use crate::frontend::types::{TYPE_ID_I32, TYPE_ID_VOID};
use crate::ir::hir::FunctionHIR;
use cranelift_codegen::ir::{types, AbiParam, InstBuilder, Value};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, Linkage, Module};
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
        compiler.compile_with(|meta, hir| {
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
        })
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
    let builder = JITBuilder::new(default_libcall_names())
        .map_err(|error| format!("failed to construct JIT builder: {error}"))?;
    let mut module = JITModule::new(builder);
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

    let mut function_builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut function_builder_context);
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
        let mut params_by_name = BTreeMap::new();
        let block_params: Vec<Value> = builder.block_params(entry).to_vec();
        for (index, name) in meta.param_names.iter().enumerate() {
            let Some(value) = block_params.get(index).copied() else {
                return Err(format!(
                    "missing block parameter {} for function '{}'",
                    index, meta.name
                ));
            };
            params_by_name.insert(name.clone(), value);
        }

        if meta.return_type == TYPE_ID_I32 {
            let expression = extract_return_expression(hir)?;
            let parsed = parse_simple_i32_expression(&expression)?;
            let value = emit_simple_i32_expression(&mut builder, &parsed, &params_by_name)?;
            builder.ins().return_(&[value]);
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

fn extract_return_expression(hir: &FunctionHIR) -> Result<String, String> {
    let Some(block) = hir.blocks.first() else {
        return Err("function body missing block text".to_string());
    };
    let source = block.source.as_str();
    let return_index = source.find("return").ok_or_else(|| {
        "expected i32 return expression but no return statement found".to_string()
    })?;
    let tail = &source[return_index + "return".len()..];
    let semicolon_index = tail
        .find(';')
        .ok_or_else(|| "expected i32 return expression ending in ';'".to_string())?;
    let expression = tail[..semicolon_index].trim();
    if expression.is_empty() {
        return Err("expected i32 return expression but expression was empty".to_string());
    }
    Ok(expression.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SimpleExpr {
    Int(i64),
    Identifier(String),
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
            ExprToken::Identifier(name) => Ok(SimpleExpr::Identifier(name)),
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
    params_by_name: &BTreeMap<String, Value>,
) -> Result<Value, String> {
    match expression {
        SimpleExpr::Int(value) => {
            let value = i32::try_from(*value).map_err(|_| {
                format!("integer literal out of i32 range in return expression: {value}")
            })?;
            Ok(builder.ins().iconst(types::I32, i64::from(value)))
        }
        SimpleExpr::Identifier(name) => params_by_name
            .get(name)
            .copied()
            .ok_or_else(|| format!("unknown identifier '{name}' in return expression")),
        SimpleExpr::Binary { lhs, op, rhs } => {
            let lhs_value = emit_simple_i32_expression(builder, lhs, params_by_name)?;
            let rhs_value = emit_simple_i32_expression(builder, rhs, params_by_name)?;
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
            "function main(): i32 { return helper(); }\n",
        );
        let error = process.compile().expect_err("expected compile error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("unexpected trailing tokens")
                        || message.contains("unsupported token"),
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
