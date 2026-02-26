#![forbid(unsafe_code)]

pub mod backend;
pub mod compiler;
pub mod frontend;
pub mod ir;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalCompileOutput {
    pub status: i32,
    pub layout_hash: i32,
    pub hook_symbol: Option<String>,
    pub file_paths: Vec<String>,
    pub functions: Vec<FunctionMetric>,
    pub errors: Vec<ErrorMetric>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionMetric {
    pub file_index: usize,
    pub ordinal: usize,
    pub id_hash: i32,
    pub sig_hash: i32,
    pub body_hash: i32,
    pub return_type: String,
    pub param_count: i32,
    pub first_param_type_code: i32,
    pub simple_i32_return_expr: Option<SimpleI32ReturnExpr>,
    pub simple_i32_return_call_target_id_hash: Option<i32>,
    pub simple_i32_return_call_add_delta: Option<i32>,
    pub simple_i32_return_call_one_arg_target_id_hash: Option<i32>,
    pub simple_i32_return_call_one_arg_i32_literal: Option<i32>,
    pub simple_i32_return_call_one_arg_arg_call_target_id_hash: Option<i32>,
    pub simple_i32_return_two_call_left_target_id_hash: Option<i32>,
    pub simple_i32_return_two_call_right_target_id_hash: Option<i32>,
    pub simple_i32_return_two_call_op_code: Option<i32>,
    pub simple_void_print_i32_literal: Option<i32>,
    pub simple_void_print_i32_call_target_id_hash: Option<i32>,
    pub simple_void_print_i32_call_one_arg_arg_call_target_id_hash: Option<i32>,
    pub simple_void_print_i32_call_add_delta: Option<i32>,
    pub clif_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimpleI32ReturnExpr {
    Literal(i32),
    Add(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Sub(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Mul(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Div(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Mod(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Select(
        SimpleI32Condition,
        Box<SimpleI32ReturnExpr>,
        Box<SimpleI32ReturnExpr>,
    ),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimpleI32Condition {
    Eq(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Ne(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Le(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Ge(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Lt(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Gt(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    And(Box<SimpleI32Condition>, Box<SimpleI32Condition>),
    Or(Box<SimpleI32Condition>, Box<SimpleI32Condition>),
    Not(Box<SimpleI32Condition>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorMetric {
    pub code: i32,
    pub pos: i32,
    pub detail_a: i32,
    pub detail_b: i32,
}

#[derive(Debug, Clone)]
struct FileState {
    layout_hash: i32,
    main_decl_count: i32,
    main_valid_count: i32,
    main_invalid_count: i32,
    functions: Vec<ParsedFunction>,
}

#[derive(Debug, Clone)]
struct ParsedFunction {
    ordinal: usize,
    id_hash: i32,
    is_export: bool,
    sig_hash: i32,
    body_hash: i32,
    return_type: String,
    param_count: i32,
    first_param_type_code: i32,
    simple_i32_return_expr: Option<SimpleI32ReturnExpr>,
    simple_i32_return_call_target_id_hash: Option<i32>,
    simple_i32_return_call_add_delta: Option<i32>,
    simple_i32_return_call_one_arg_target_id_hash: Option<i32>,
    simple_i32_return_call_one_arg_i32_literal: Option<i32>,
    simple_i32_return_call_one_arg_arg_call_target_id_hash: Option<i32>,
    simple_i32_return_two_call_left_target_id_hash: Option<i32>,
    simple_i32_return_two_call_right_target_id_hash: Option<i32>,
    simple_i32_return_two_call_op_code: Option<i32>,
    simple_void_print_i32_literal: Option<i32>,
    simple_void_print_i32_call_target_id_hash: Option<i32>,
    simple_void_print_i32_call_one_arg_arg_call_target_id_hash: Option<i32>,
    simple_void_print_i32_call_add_delta: Option<i32>,
    call_target_id_hashes: Vec<i32>,
    clif_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FunctionKey {
    path: String,
    id_hash: i32,
    sig_hash: i32,
}

#[derive(Debug, Clone)]
struct AnalysisResult {
    functions: Vec<ParsedFunction>,
    layout_hash: i32,
    main_decl_count: i32,
    main_valid_count: i32,
    main_invalid_count: i32,
    errors: Vec<ErrorMetric>,
}

pub struct IncrementalCompilerHost {
    source_hash_by_path: BTreeMap<String, u64>,
    state_by_path: BTreeMap<String, FileState>,
    last_layout_hash_i32: i32,
    required_reachability_root_hashes: Vec<i32>,
    last_reachable_function_keys: BTreeSet<FunctionKey>,
}

impl IncrementalCompilerHost {
    pub fn new() -> Self {
        Self {
            source_hash_by_path: BTreeMap::new(),
            state_by_path: BTreeMap::new(),
            last_layout_hash_i32: 0,
            required_reachability_root_hashes: Vec::new(),
            last_reachable_function_keys: BTreeSet::new(),
        }
    }

    pub fn set_required_reachability_roots(&mut self, roots: &[&str]) {
        self.required_reachability_root_hashes.clear();
        for root in roots {
            let id_hash = hash_identifier(root);
            if !self.required_reachability_root_hashes.contains(&id_hash) {
                self.required_reachability_root_hashes.push(id_hash);
            }
        }
    }

    pub fn compile_changed_files(
        &mut self,
        changed_files: &[PathBuf],
    ) -> Result<IncrementalCompileOutput, String> {
        if changed_files.is_empty() {
            return Err("compile request had no changed files".to_string());
        }

        let mut files = changed_files.to_vec();
        files.sort();
        files.dedup();
        let previous_state_by_path = self.state_by_path.clone();

        let mut changed_sources: Vec<(String, String)> = Vec::new();
        let mut deleted_paths: Vec<String> = Vec::new();
        for path in files {
            let path_key = normalize_path_key(&path);
            match fs::read(&path) {
                Ok(bytes) => {
                    let source = String::from_utf8_lossy(&bytes).to_string();
                    let source_hash = hash_text(&source);
                    let changed = self
                        .source_hash_by_path
                        .get(&path_key)
                        .is_none_or(|existing| *existing != source_hash);
                    self.source_hash_by_path
                        .insert(path_key.clone(), source_hash);
                    if changed {
                        changed_sources.push((path_key, source));
                    }
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    let removed_hash = self.source_hash_by_path.remove(&path_key).is_some();
                    let removed_state = self.state_by_path.remove(&path_key).is_some();
                    if removed_hash || removed_state {
                        deleted_paths.push(path_key);
                    }
                }
                Err(error) => {
                    return Err(format!("failed reading {}: {error}", path.display()));
                }
            }
        }

        if changed_sources.is_empty() && deleted_paths.is_empty() {
            return Ok(IncrementalCompileOutput {
                status: 0,
                layout_hash: self.last_layout_hash_i32,
                hook_symbol: current_hook_symbol(&self.state_by_path),
                file_paths: Vec::new(),
                functions: Vec::new(),
                errors: Vec::new(),
            });
        }

        let mut from_expr_errors = Vec::new();
        for (_, source) in &changed_sources {
            from_expr_errors.extend(find_from_conversion_expression_errors(source));
        }
        if !from_expr_errors.is_empty() {
            return Ok(IncrementalCompileOutput {
                status: 2,
                layout_hash: self.last_layout_hash_i32,
                hook_symbol: current_hook_symbol(&self.state_by_path),
                file_paths: Vec::new(),
                functions: Vec::new(),
                errors: from_expr_errors,
            });
        }

        let mut file_paths = Vec::with_capacity(changed_sources.len());
        let mut functions: Vec<FunctionMetric> = Vec::new();
        let mut errors: Vec<ErrorMetric> = Vec::new();
        let analyzed_by_path = analyze_sources_in_process_parallel(&changed_sources)?;
        for (path_key, analyzed) in &analyzed_by_path {
            if !analyzed.errors.is_empty() {
                errors.extend(analyzed.errors.clone());
            }
            if !changed_sources
                .iter()
                .any(|(changed_path, _)| changed_path == path_key)
            {
                return Err(format!(
                    "analysis returned unexpected path key {}",
                    path_key
                ));
            }
        }

        if !errors.is_empty() {
            return Ok(IncrementalCompileOutput {
                status: 2,
                layout_hash: self.last_layout_hash_i32,
                hook_symbol: current_hook_symbol(&self.state_by_path),
                file_paths,
                functions,
                errors,
            });
        }

        for (path_key, analyzed) in &analyzed_by_path {
            self.state_by_path.insert(
                path_key.clone(),
                FileState {
                    layout_hash: analyzed.layout_hash,
                    main_decl_count: analyzed.main_decl_count,
                    main_valid_count: analyzed.main_valid_count,
                    main_invalid_count: analyzed.main_invalid_count,
                    functions: analyzed.functions.clone(),
                },
            );
        }

        let mut main_decl_total = 0;
        let mut main_valid_total = 0;
        let mut main_invalid_total = 0;
        for state in self.state_by_path.values() {
            main_decl_total += state.main_decl_count;
            main_valid_total += state.main_valid_count;
            main_invalid_total += state.main_invalid_count;
        }

        if main_decl_total == 0 {
            errors.push(ErrorMetric {
                code: 41,
                pos: 0,
                detail_a: 0,
                detail_b: 0,
            });
        } else if main_decl_total > 1 {
            errors.push(ErrorMetric {
                code: 43,
                pos: 0,
                detail_a: main_decl_total,
                detail_b: 0,
            });
        } else if main_valid_total != 1 || main_invalid_total > 0 {
            errors.push(ErrorMetric {
                code: 42,
                pos: 0,
                detail_a: main_valid_total,
                detail_b: main_invalid_total,
            });
        }

        if !errors.is_empty() {
            return Ok(IncrementalCompileOutput {
                status: 2,
                layout_hash: self.last_layout_hash_i32,
                hook_symbol: current_hook_symbol(&self.state_by_path),
                file_paths,
                functions,
                errors,
            });
        }

        let previous_reachable_keys = self.last_reachable_function_keys.clone();
        let current_reachable_keys = compute_reachable_function_keys_from_state(
            &self.state_by_path,
            &self.required_reachability_root_hashes,
        );
        let previous_body_hash_by_key = build_function_body_hash_by_key(&previous_state_by_path);
        let mut file_index_by_path: BTreeMap<String, usize> = BTreeMap::new();
        for (path_key, state) in &self.state_by_path {
            for parsed in &state.functions {
                let key = function_key_for(path_key, parsed);
                if !current_reachable_keys.contains(&key) {
                    continue;
                }
                let changed_definition = match previous_body_hash_by_key.get(&key) {
                    Some(previous_body_hash) => *previous_body_hash != parsed.body_hash,
                    None => true,
                };
                let previously_reachable = previous_reachable_keys.contains(&key);
                if !changed_definition && previously_reachable {
                    continue;
                }

                let file_index = if let Some(existing) = file_index_by_path.get(path_key) {
                    *existing
                } else {
                    let new_index = file_paths.len();
                    file_paths.push(path_key.clone());
                    file_index_by_path.insert(path_key.clone(), new_index);
                    new_index
                };
                functions.push(FunctionMetric {
                    file_index,
                    ordinal: parsed.ordinal,
                    id_hash: parsed.id_hash,
                    sig_hash: parsed.sig_hash,
                    body_hash: parsed.body_hash,
                    return_type: parsed.return_type.clone(),
                    param_count: parsed.param_count,
                    first_param_type_code: parsed.first_param_type_code,
                    simple_i32_return_expr: parsed.simple_i32_return_expr.clone(),
                    simple_i32_return_call_target_id_hash: parsed
                        .simple_i32_return_call_target_id_hash,
                    simple_i32_return_call_add_delta: parsed.simple_i32_return_call_add_delta,
                    simple_i32_return_call_one_arg_target_id_hash: parsed
                        .simple_i32_return_call_one_arg_target_id_hash,
                    simple_i32_return_call_one_arg_i32_literal: parsed
                        .simple_i32_return_call_one_arg_i32_literal,
                    simple_i32_return_call_one_arg_arg_call_target_id_hash: parsed
                        .simple_i32_return_call_one_arg_arg_call_target_id_hash,
                    simple_i32_return_two_call_left_target_id_hash: parsed
                        .simple_i32_return_two_call_left_target_id_hash,
                    simple_i32_return_two_call_right_target_id_hash: parsed
                        .simple_i32_return_two_call_right_target_id_hash,
                    simple_i32_return_two_call_op_code: parsed.simple_i32_return_two_call_op_code,
                    simple_void_print_i32_literal: parsed.simple_void_print_i32_literal,
                    simple_void_print_i32_call_target_id_hash: parsed
                        .simple_void_print_i32_call_target_id_hash,
                    simple_void_print_i32_call_one_arg_arg_call_target_id_hash: parsed
                        .simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
                    simple_void_print_i32_call_add_delta: parsed
                        .simple_void_print_i32_call_add_delta,
                    clif_text: parsed.clif_text.clone(),
                });
            }
        }

        let mut layout_acc = 216613626_i32;
        for state in self.state_by_path.values() {
            layout_acc = hash_mix(layout_acc, state.layout_hash);
        }
        self.last_layout_hash_i32 = layout_acc;
        self.last_reachable_function_keys = current_reachable_keys;

        Ok(IncrementalCompileOutput {
            status: 0,
            layout_hash: layout_acc,
            hook_symbol: current_hook_symbol(&self.state_by_path),
            file_paths,
            functions,
            errors,
        })
    }

    pub fn backend_name(&self) -> &'static str {
        "stasis-orchestrated"
    }
}

impl Default for IncrementalCompilerHost {
    fn default() -> Self {
        Self::new()
    }
}

fn analyze_sources_in_process_parallel(
    changed_sources: &[(String, String)],
) -> Result<BTreeMap<String, AnalysisResult>, String> {
    let mut handles = Vec::with_capacity(changed_sources.len());
    for (path_key, source) in changed_sources {
        let path_key = path_key.clone();
        let source = source.clone();
        let handle = thread::spawn(move || (path_key, analyze_source_in_process(&source)));
        handles.push(handle);
    }

    let mut analyzed_by_path = BTreeMap::new();
    for handle in handles {
        let (path_key, result) = handle
            .join()
            .map_err(|_| "analysis worker thread panicked".to_string())?;
        analyzed_by_path.insert(path_key, result?);
    }
    Ok(analyzed_by_path)
}

fn analyze_source_in_process(source: &str) -> Result<AnalysisResult, String> {
    let functions = parse_defined_functions(source)?;
    let mut parsed_functions = Vec::with_capacity(functions.len());
    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut return_exprs = Vec::with_capacity(functions.len());

    let mut main_decl_count = 0i32;
    let mut main_valid_count = 0i32;
    let mut main_invalid_count = 0i32;

    for function in &functions {
        let body_text = source
            .get(function.body_range.clone())
            .ok_or_else(|| "function body range out of bounds".to_string())?;
        let id_hash = hash_identifier(&function.name);
        let sig_hash = hash_signature_i32(
            &function.name,
            &function
                .params
                .iter()
                .map(|param| param.type_name.as_str())
                .collect::<Vec<_>>(),
            &function.return_type_name,
        );
        let body_hash = hash_i32(body_text);
        let first_param_type_code = function
            .params
            .first()
            .map(|param| type_code_from_name(&param.type_name))
            .unwrap_or_default();
        if function.name == "main" {
            main_decl_count += 1;
            if function.params.is_empty() && function.return_type_name == "i32" {
                main_valid_count += 1;
            } else {
                main_invalid_count += 1;
            }
        }

        let expression = if function.return_type_name == "i32" {
            parse_return_expression(body_text)
        } else {
            None
        };
        let simple_i32_return_expr = expression.as_ref().and_then(convert_eval_expr_to_simple);
        let (
            simple_i32_return_call_target_id_hash,
            simple_i32_return_call_add_delta,
            simple_i32_return_call_one_arg_target_id_hash,
            simple_i32_return_call_one_arg_i32_literal,
            simple_i32_return_call_one_arg_arg_call_target_id_hash,
            simple_i32_return_two_call_left_target_id_hash,
            simple_i32_return_two_call_right_target_id_hash,
            simple_i32_return_two_call_op_code,
        ) = if let Some(expr) = expression.as_ref() {
            analyze_simple_i32_return_call_metadata(expr)
        } else {
            (None, None, None, None, None, None, None, None)
        };

        let (
            simple_void_print_i32_literal,
            simple_void_print_i32_call_target_id_hash,
            simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
            simple_void_print_i32_call_add_delta,
        ) = if function.return_type_name == "void" {
            analyze_simple_void_print_i32_metadata(body_text)
        } else {
            (None, None, None, None)
        };

        let parsed_index = parsed_functions.len();
        parsed_functions.push(ParsedFunction {
            ordinal: function.ordinal,
            id_hash,
            is_export: function.is_export,
            sig_hash,
            body_hash,
            return_type: function.return_type_name.clone(),
            param_count: i32::try_from(function.params.len()).unwrap_or_default(),
            first_param_type_code,
            simple_i32_return_expr,
            simple_i32_return_call_target_id_hash,
            simple_i32_return_call_add_delta,
            simple_i32_return_call_one_arg_target_id_hash,
            simple_i32_return_call_one_arg_i32_literal,
            simple_i32_return_call_one_arg_arg_call_target_id_hash,
            simple_i32_return_two_call_left_target_id_hash,
            simple_i32_return_two_call_right_target_id_hash,
            simple_i32_return_two_call_op_code,
            simple_void_print_i32_literal,
            simple_void_print_i32_call_target_id_hash,
            simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
            simple_void_print_i32_call_add_delta,
            call_target_id_hashes: collect_call_target_id_hashes(body_text),
            clif_text: String::new(),
        });
        by_name
            .entry(function.name.clone())
            .or_default()
            .push(parsed_index);
        return_exprs.push(expression);
    }

    let mut memo = vec![None; parsed_functions.len()];
    let mut visiting = vec![false; parsed_functions.len()];
    for index in 0..parsed_functions.len() {
        let value = evaluate_function_i32(
            index,
            &parsed_functions,
            &return_exprs,
            &by_name,
            &mut memo,
            &mut visiting,
        );
        let fallback = parsed_functions[index].body_hash;
        parsed_functions[index].clif_text =
            build_stub_clif_text(&parsed_functions[index], value.unwrap_or(fallback));
    }

    Ok(AnalysisResult {
        functions: parsed_functions,
        layout_hash: hash_i32(source),
        main_decl_count,
        main_valid_count,
        main_invalid_count,
        errors: Vec::new(),
    })
}

#[derive(Debug, Clone)]
struct ParsedFunctionDecl {
    ordinal: usize,
    name: String,
    is_export: bool,
    params: Vec<ParsedParamDecl>,
    return_type_name: String,
    body_range: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
struct ParsedParamDecl {
    type_name: String,
}

#[derive(Debug, Clone)]
enum EvalExpr {
    Literal(i32),
    Add(Box<EvalExpr>, Box<EvalExpr>),
    Sub(Box<EvalExpr>, Box<EvalExpr>),
    Mul(Box<EvalExpr>, Box<EvalExpr>),
    Div(Box<EvalExpr>, Box<EvalExpr>),
    Mod(Box<EvalExpr>, Box<EvalExpr>),
    Call(String, Vec<EvalExpr>),
}

fn parse_defined_functions(source: &str) -> Result<Vec<ParsedFunctionDecl>, String> {
    use crate::frontend::lexer::TokenKind;

    let tokens = crate::frontend::lexer::lex(source)?;
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut ordinal = 0usize;

    while cursor < tokens.len() {
        if tokens[cursor].kind != TokenKind::FunctionKw {
            cursor += 1;
            continue;
        }
        let is_export = cursor
            .checked_sub(1)
            .and_then(|index| tokens.get(index))
            .is_some_and(|token| {
                token.kind == TokenKind::Identifier && source[token.start..token.end] == *"export"
            });
        cursor += 1;
        cursor = skip_legacy_function_annotations(source, &tokens, cursor)?;

        let name_token = tokens
            .get(cursor)
            .ok_or_else(|| "expected function name after 'function'".to_string())?;
        if name_token.kind != TokenKind::Identifier {
            return Err("expected function name after 'function'".to_string());
        }
        let name = source[name_token.start..name_token.end].to_string();
        cursor += 1;

        expect_token_kind(
            &tokens,
            cursor,
            TokenKind::LParen,
            "expected '(' after function name",
        )?;
        cursor += 1;

        let mut params = Vec::new();
        while tokens
            .get(cursor)
            .is_some_and(|token| token.kind != TokenKind::RParen)
        {
            expect_token_kind(
                &tokens,
                cursor,
                TokenKind::Identifier,
                "expected parameter name",
            )?;
            cursor += 1;
            expect_token_kind(
                &tokens,
                cursor,
                TokenKind::Colon,
                "expected ':' after parameter name",
            )?;
            cursor += 1;

            let type_start = tokens
                .get(cursor)
                .map(|token| token.start)
                .ok_or_else(|| "missing parameter type".to_string())?;
            while tokens.get(cursor).is_some_and(|token| {
                token.kind != TokenKind::Comma && token.kind != TokenKind::RParen
            }) {
                cursor += 1;
            }
            let type_end = tokens
                .get(cursor.saturating_sub(1))
                .map(|token| token.end)
                .ok_or_else(|| "missing parameter type body".to_string())?;
            params.push(ParsedParamDecl {
                type_name: source[type_start..type_end].trim().to_string(),
            });
            if tokens
                .get(cursor)
                .is_some_and(|token| token.kind == TokenKind::Comma)
            {
                cursor += 1;
            }
        }
        expect_token_kind(
            &tokens,
            cursor,
            TokenKind::RParen,
            "expected ')' after parameter list",
        )?;
        cursor += 1;

        let mut return_type_name = "void".to_string();
        if tokens
            .get(cursor)
            .is_some_and(|token| token.kind == TokenKind::Colon)
        {
            cursor += 1;
            let type_start = tokens
                .get(cursor)
                .map(|token| token.start)
                .ok_or_else(|| "missing return type".to_string())?;
            while tokens.get(cursor).is_some_and(|token| {
                token.kind != TokenKind::LBrace && token.kind != TokenKind::Semicolon
            }) {
                cursor += 1;
            }
            let type_end = tokens
                .get(cursor.saturating_sub(1))
                .map(|token| token.end)
                .ok_or_else(|| "missing return type body".to_string())?;
            return_type_name = source[type_start..type_end].trim().to_string();
        }

        if tokens
            .get(cursor)
            .is_some_and(|token| token.kind == TokenKind::Semicolon)
        {
            cursor += 1;
            continue;
        }

        expect_token_kind(
            &tokens,
            cursor,
            TokenKind::LBrace,
            "expected '{' for function body",
        )?;
        let body_start = tokens[cursor].start;
        cursor += 1;
        let mut depth = 1usize;
        while cursor < tokens.len() {
            match tokens[cursor].kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                TokenKind::Eof => break,
                _ => {}
            }
            cursor += 1;
        }
        if depth != 0 {
            return Err(format!("missing closing '}}' for function '{name}'"));
        }
        let body_end = tokens[cursor].end;
        out.push(ParsedFunctionDecl {
            ordinal,
            name,
            is_export,
            params,
            return_type_name,
            body_range: body_start..body_end,
        });
        ordinal += 1;
        cursor += 1;
    }

    Ok(out)
}

fn skip_legacy_function_annotations(
    source: &str,
    tokens: &[crate::frontend::lexer::Token],
    mut cursor: usize,
) -> Result<usize, String> {
    use crate::frontend::lexer::TokenKind;

    loop {
        let Some(token) = tokens.get(cursor) else {
            return Ok(cursor);
        };
        if token.kind != TokenKind::Other || source[token.start..token.end] != *"@" {
            return Ok(cursor);
        }
        cursor += 1;
        let annotation_name = tokens
            .get(cursor)
            .ok_or_else(|| "expected annotation name after '@'".to_string())?;
        if annotation_name.kind != TokenKind::Identifier {
            return Err("expected annotation name after '@'".to_string());
        }
        cursor += 1;
        if tokens
            .get(cursor)
            .is_some_and(|next| next.kind == TokenKind::LParen)
        {
            cursor = skip_legacy_parenthesized_tokens(tokens, cursor)?;
        }
    }
}

fn skip_legacy_parenthesized_tokens(
    tokens: &[crate::frontend::lexer::Token],
    open_cursor: usize,
) -> Result<usize, String> {
    use crate::frontend::lexer::TokenKind;

    if tokens
        .get(open_cursor)
        .is_none_or(|token| token.kind != TokenKind::LParen)
    {
        return Err("expected '(' after annotation name".to_string());
    }
    let mut cursor = open_cursor + 1;
    let mut depth = 1i32;
    while cursor < tokens.len() {
        match tokens[cursor].kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => {
                depth -= 1;
                if depth == 0 {
                    return Ok(cursor + 1);
                }
            }
            TokenKind::Eof => break,
            _ => {}
        }
        cursor += 1;
    }
    Err("missing ')' in function annotation".to_string())
}

fn expect_token_kind(
    tokens: &[crate::frontend::lexer::Token],
    cursor: usize,
    expected: crate::frontend::lexer::TokenKind,
    message: &str,
) -> Result<(), String> {
    let Some(token) = tokens.get(cursor) else {
        return Err(message.to_string());
    };
    if token.kind != expected {
        return Err(message.to_string());
    }
    Ok(())
}

fn hash_signature_i32(name: &str, param_types: &[&str], return_type: &str) -> i32 {
    let mut signature = String::new();
    signature.push_str(name);
    signature.push('(');
    for (index, param_type) in param_types.iter().enumerate() {
        if index > 0 {
            signature.push(',');
        }
        signature.push_str(param_type.trim());
    }
    signature.push(')');
    signature.push(':');
    signature.push_str(return_type.trim());
    hash_i32(&signature)
}

fn type_code_from_name(type_name: &str) -> i32 {
    if type_name.trim() == "i32" {
        1
    } else {
        0
    }
}

fn collect_call_target_id_hashes(body_text: &str) -> Vec<i32> {
    let mut hashes = BTreeSet::new();
    let bytes = body_text.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if is_identifier_start(bytes[cursor]) {
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len() && is_identifier_continue(bytes[cursor]) {
                cursor += 1;
            }
            let identifier = &body_text[start..cursor];
            let mut next = cursor;
            while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                next += 1;
            }
            if next < bytes.len() && bytes[next] == b'(' && !is_call_keyword(identifier) {
                hashes.insert(hash_identifier(identifier));
            }
            continue;
        }
        cursor += 1;
    }
    hashes.into_iter().collect()
}

fn is_call_keyword(identifier: &str) -> bool {
    matches!(identifier, "if" | "for" | "foreach" | "return" | "function")
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

fn parse_return_expression(body_text: &str) -> Option<EvalExpr> {
    let trimmed = body_text.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len().saturating_sub(1)].trim();
    let statements = split_top_level_statements(inner);
    if statements.len() != 1 {
        return None;
    }
    let statement = statements[0].trim();
    let expression = statement.strip_prefix("return")?.trim();
    if expression.is_empty() {
        return None;
    }
    parse_eval_expression(expression)
}

fn simple_call0_name(expression: &EvalExpr) -> Option<&str> {
    match expression {
        EvalExpr::Call(name, args) if args.is_empty() => Some(name.as_str()),
        _ => None,
    }
}

fn simple_i32_literal(expression: &EvalExpr) -> Option<i32> {
    match expression {
        EvalExpr::Literal(value) => Some(*value),
        _ => None,
    }
}

fn analyze_simple_i32_return_call_metadata(
    expression: &EvalExpr,
) -> (
    Option<i32>, // call_target_id_hash
    Option<i32>, // call_add_delta
    Option<i32>, // call_one_arg_target_id_hash
    Option<i32>, // call_one_arg_i32_literal
    Option<i32>, // call_one_arg_arg_call_target_id_hash
    Option<i32>, // two_call_left_target_id_hash
    Option<i32>, // two_call_right_target_id_hash
    Option<i32>, // two_call_op_code (1 add, 2 sub)
) {
    // direct call0()
    if let Some(name) = simple_call0_name(expression) {
        return (
            Some(hash_identifier(name)),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
    }

    // call0() +/- literal
    match expression {
        EvalExpr::Add(lhs, rhs) | EvalExpr::Sub(lhs, rhs) => {
            let op_is_sub = matches!(expression, EvalExpr::Sub(_, _));
            if let (Some(call_name), Some(lit)) = (simple_call0_name(lhs), simple_i32_literal(rhs))
            {
                let delta = if op_is_sub { -lit } else { lit };
                return (
                    Some(hash_identifier(call_name)),
                    Some(delta),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                );
            }
            if let (Some(lit), Some(call_name)) = (simple_i32_literal(lhs), simple_call0_name(rhs))
            {
                // literal +/- call0(): only representable as call0 + delta when op is add.
                if !op_is_sub {
                    return (
                        Some(hash_identifier(call_name)),
                        Some(lit),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                    );
                }
            }

            // two call0 ops
            if let (Some(left), Some(right)) = (simple_call0_name(lhs), simple_call0_name(rhs)) {
                return (
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(hash_identifier(left)),
                    Some(hash_identifier(right)),
                    Some(if op_is_sub { 2 } else { 1 }),
                );
            }
        }
        _ => {}
    }

    // call(args...)
    if let EvalExpr::Call(name, args) = expression {
        if args.is_empty() {
            return (
                Some(hash_identifier(name)),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            );
        }
        let mut call_one_arg_i32_literal: Option<i32> = None;
        let mut call_one_arg_arg_call_target_id_hash: Option<i32> = None;
        if let Some(first) = args.first() {
            if let Some(value) = simple_i32_literal(first) {
                call_one_arg_i32_literal = Some(value);
            } else if let Some(arg_call_name) = simple_call0_name(first) {
                call_one_arg_arg_call_target_id_hash = Some(hash_identifier(arg_call_name));
            }
        }
        return (
            None,
            None,
            Some(hash_identifier(name)),
            call_one_arg_i32_literal,
            call_one_arg_arg_call_target_id_hash,
            None,
            None,
            None,
        );
    }

    (None, None, None, None, None, None, None, None)
}

fn analyze_simple_void_print_i32_metadata(
    body_text: &str,
) -> (
    Option<i32>, // literal
    Option<i32>, // call_target_id_hash
    Option<i32>, // call_one_arg_arg_call_target_id_hash
    Option<i32>, // call_add_delta
) {
    // Very small, statement-level matcher for:
    // - print_i32(<literal>);
    // - print_i32(<call0>());
    // - print_i32(<call0>() +/- <literal>);
    let cleaned = strip_comments_for_from_conversion_scan(body_text);
    let trimmed = cleaned.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return (None, None, None, None);
    }
    let inner = trimmed[1..trimmed.len().saturating_sub(1)].trim();
    for statement in split_top_level_statements(inner) {
        let stmt = statement.trim();
        let Some(call) = stmt.strip_prefix("print_i32") else {
            continue;
        };
        let call = call.trim_start();
        if !call.starts_with('(') {
            continue;
        }
        let Some(close) = call.rfind(')') else {
            continue;
        };
        let arg_text = call[1..close].trim();
        if arg_text.is_empty() {
            continue;
        }
        let Some(expr) = parse_eval_expression(arg_text) else {
            continue;
        };

        if let Some(value) = simple_i32_literal(&expr) {
            return (Some(value), None, None, None);
        }
        if let Some(name) = simple_call0_name(&expr) {
            return (None, Some(hash_identifier(name)), None, None);
        }
        if let EvalExpr::Add(lhs, rhs) | EvalExpr::Sub(lhs, rhs) = &expr {
            let op_is_sub = matches!(&expr, EvalExpr::Sub(_, _));
            if let (Some(call_name), Some(lit)) = (simple_call0_name(lhs), simple_i32_literal(rhs))
            {
                let delta = if op_is_sub { -lit } else { lit };
                return (None, Some(hash_identifier(call_name)), None, Some(delta));
            }
        }
    }
    (None, None, None, None)
}

fn split_top_level_statements(body: &str) -> Vec<&str> {
    let bytes = body.as_bytes();
    let mut depth_paren = 0i32;
    let mut depth_brace = 0i32;
    let mut start = 0usize;
    let mut statements = Vec::new();
    for (index, byte) in bytes.iter().copied().enumerate() {
        match byte {
            b'(' => depth_paren += 1,
            b')' => depth_paren -= 1,
            b'{' => depth_brace += 1,
            b'}' => depth_brace -= 1,
            b';' if depth_paren == 0 && depth_brace == 0 => {
                statements.push(&body[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < body.len() {
        statements.push(&body[start..]);
    }
    statements
        .into_iter()
        .filter(|statement| !statement.trim().is_empty())
        .collect()
}

fn parse_eval_expression(expression: &str) -> Option<EvalExpr> {
    let mut parser = EvalExpressionParser::new(expression);
    let expression = parser.parse_expression()?;
    parser.skip_ws();
    if parser.is_done() {
        Some(expression)
    } else {
        None
    }
}

struct EvalExpressionParser<'a> {
    source: &'a str,
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> EvalExpressionParser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            cursor: 0,
        }
    }

    fn parse_expression(&mut self) -> Option<EvalExpr> {
        self.parse_add_sub()
    }

    fn parse_add_sub(&mut self) -> Option<EvalExpr> {
        let mut lhs = self.parse_mul_div()?;
        loop {
            self.skip_ws();
            let op = match self.peek_byte() {
                Some(b'+') => b'+',
                Some(b'-') => b'-',
                _ => break,
            };
            self.cursor += 1;
            let rhs = self.parse_mul_div()?;
            lhs = if op == b'+' {
                EvalExpr::Add(Box::new(lhs), Box::new(rhs))
            } else {
                EvalExpr::Sub(Box::new(lhs), Box::new(rhs))
            };
        }
        Some(lhs)
    }

    fn parse_mul_div(&mut self) -> Option<EvalExpr> {
        let mut lhs = self.parse_unary()?;
        loop {
            self.skip_ws();
            let op = match self.peek_byte() {
                Some(b'*') => b'*',
                Some(b'/') => b'/',
                Some(b'%') => b'%',
                _ => break,
            };
            self.cursor += 1;
            let rhs = self.parse_unary()?;
            lhs = match op {
                b'*' => EvalExpr::Mul(Box::new(lhs), Box::new(rhs)),
                b'/' => EvalExpr::Div(Box::new(lhs), Box::new(rhs)),
                _ => EvalExpr::Mod(Box::new(lhs), Box::new(rhs)),
            };
        }
        Some(lhs)
    }

    fn parse_unary(&mut self) -> Option<EvalExpr> {
        self.skip_ws();
        if self.peek_byte() == Some(b'+') {
            self.cursor += 1;
            return self.parse_unary();
        }
        if self.peek_byte() == Some(b'-') {
            self.cursor += 1;
            let expression = self.parse_unary()?;
            return Some(EvalExpr::Sub(
                Box::new(EvalExpr::Literal(0)),
                Box::new(expression),
            ));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Option<EvalExpr> {
        self.skip_ws();
        match self.peek_byte()? {
            b'0'..=b'9' => self.parse_integer().map(EvalExpr::Literal),
            byte if is_identifier_start(byte) => {
                let identifier = self.parse_identifier()?.to_string();
                self.skip_ws();
                if self.peek_byte() != Some(b'(') {
                    return None;
                }
                self.cursor += 1;
                let mut args = Vec::new();
                loop {
                    self.skip_ws();
                    if self.peek_byte() == Some(b')') {
                        self.cursor += 1;
                        break;
                    }
                    let arg = self.parse_expression()?;
                    args.push(arg);
                    self.skip_ws();
                    match self.peek_byte()? {
                        b',' => {
                            self.cursor += 1;
                            continue;
                        }
                        b')' => {
                            self.cursor += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                Some(EvalExpr::Call(identifier, args))
            }
            b'(' => {
                self.cursor += 1;
                let expression = self.parse_expression()?;
                self.skip_ws();
                if self.peek_byte()? != b')' {
                    return None;
                }
                self.cursor += 1;
                Some(expression)
            }
            _ => None,
        }
    }

    fn parse_integer(&mut self) -> Option<i32> {
        let start = self.cursor;
        while self.cursor < self.bytes.len() && self.bytes[self.cursor].is_ascii_digit() {
            self.cursor += 1;
        }
        self.source.get(start..self.cursor)?.parse::<i32>().ok()
    }

    fn parse_identifier(&mut self) -> Option<&'a str> {
        let start = self.cursor;
        if !is_identifier_start(*self.bytes.get(start)?) {
            return None;
        }
        self.cursor += 1;
        while self.cursor < self.bytes.len() && is_identifier_continue(self.bytes[self.cursor]) {
            self.cursor += 1;
        }
        self.source.get(start..self.cursor)
    }

    fn skip_ws(&mut self) {
        while self.cursor < self.bytes.len() && self.bytes[self.cursor].is_ascii_whitespace() {
            self.cursor += 1;
        }
    }

    fn peek_byte(&self) -> Option<u8> {
        self.bytes.get(self.cursor).copied()
    }

    fn is_done(&self) -> bool {
        self.cursor >= self.bytes.len()
    }
}

fn convert_eval_expr_to_simple(expression: &EvalExpr) -> Option<SimpleI32ReturnExpr> {
    match expression {
        EvalExpr::Literal(value) => Some(SimpleI32ReturnExpr::Literal(*value)),
        EvalExpr::Add(lhs, rhs) => Some(SimpleI32ReturnExpr::Add(
            Box::new(convert_eval_expr_to_simple(lhs)?),
            Box::new(convert_eval_expr_to_simple(rhs)?),
        )),
        EvalExpr::Sub(lhs, rhs) => Some(SimpleI32ReturnExpr::Sub(
            Box::new(convert_eval_expr_to_simple(lhs)?),
            Box::new(convert_eval_expr_to_simple(rhs)?),
        )),
        EvalExpr::Mul(lhs, rhs) => Some(SimpleI32ReturnExpr::Mul(
            Box::new(convert_eval_expr_to_simple(lhs)?),
            Box::new(convert_eval_expr_to_simple(rhs)?),
        )),
        EvalExpr::Div(lhs, rhs) => Some(SimpleI32ReturnExpr::Div(
            Box::new(convert_eval_expr_to_simple(lhs)?),
            Box::new(convert_eval_expr_to_simple(rhs)?),
        )),
        EvalExpr::Mod(lhs, rhs) => Some(SimpleI32ReturnExpr::Mod(
            Box::new(convert_eval_expr_to_simple(lhs)?),
            Box::new(convert_eval_expr_to_simple(rhs)?),
        )),
        EvalExpr::Call(_, _) => None,
    }
}

fn evaluate_function_i32(
    index: usize,
    functions: &[ParsedFunction],
    return_exprs: &[Option<EvalExpr>],
    by_name: &BTreeMap<String, Vec<usize>>,
    memo: &mut [Option<i32>],
    visiting: &mut [bool],
) -> Option<i32> {
    if let Some(value) = memo.get(index).and_then(|value| *value) {
        return Some(value);
    }
    if visiting.get(index).copied().unwrap_or(false) {
        return None;
    }
    let function = functions.get(index)?;
    if function.return_type != "i32" || function.param_count != 0 {
        return None;
    }
    let expression = return_exprs.get(index)?.as_ref()?;
    visiting[index] = true;
    let value = evaluate_expr_i32(expression, functions, return_exprs, by_name, memo, visiting);
    visiting[index] = false;
    if let Some(value) = value {
        memo[index] = Some(value);
    }
    value
}

fn evaluate_expr_i32(
    expression: &EvalExpr,
    functions: &[ParsedFunction],
    return_exprs: &[Option<EvalExpr>],
    by_name: &BTreeMap<String, Vec<usize>>,
    memo: &mut [Option<i32>],
    visiting: &mut [bool],
) -> Option<i32> {
    match expression {
        EvalExpr::Literal(value) => Some(*value),
        EvalExpr::Add(lhs, rhs) => Some(
            evaluate_expr_i32(lhs, functions, return_exprs, by_name, memo, visiting)?.wrapping_add(
                evaluate_expr_i32(rhs, functions, return_exprs, by_name, memo, visiting)?,
            ),
        ),
        EvalExpr::Sub(lhs, rhs) => Some(
            evaluate_expr_i32(lhs, functions, return_exprs, by_name, memo, visiting)?.wrapping_sub(
                evaluate_expr_i32(rhs, functions, return_exprs, by_name, memo, visiting)?,
            ),
        ),
        EvalExpr::Mul(lhs, rhs) => Some(
            evaluate_expr_i32(lhs, functions, return_exprs, by_name, memo, visiting)?.wrapping_mul(
                evaluate_expr_i32(rhs, functions, return_exprs, by_name, memo, visiting)?,
            ),
        ),
        EvalExpr::Div(lhs, rhs) => {
            let divisor = evaluate_expr_i32(rhs, functions, return_exprs, by_name, memo, visiting)?;
            if divisor == 0 {
                return None;
            }
            Some(
                evaluate_expr_i32(lhs, functions, return_exprs, by_name, memo, visiting)?
                    .wrapping_div(divisor),
            )
        }
        EvalExpr::Mod(lhs, rhs) => {
            let divisor = evaluate_expr_i32(rhs, functions, return_exprs, by_name, memo, visiting)?;
            if divisor == 0 {
                return None;
            }
            Some(
                evaluate_expr_i32(lhs, functions, return_exprs, by_name, memo, visiting)?
                    .wrapping_rem(divisor),
            )
        }
        EvalExpr::Call(name, args) => {
            if !args.is_empty() {
                return None;
            }
            let candidates = by_name.get(name)?;
            let mut selected = None;
            for candidate in candidates {
                let function = functions.get(*candidate)?;
                if function.return_type == "i32" && function.param_count == 0 {
                    if selected.is_some() {
                        return None;
                    }
                    selected = Some(*candidate);
                }
            }
            evaluate_function_i32(selected?, functions, return_exprs, by_name, memo, visiting)
        }
    }
}

fn build_stub_clif_text(function: &ParsedFunction, i32_return_value: i32) -> String {
    let symbol = format!(
        "fn_{}_{}_{}",
        function.id_hash.unsigned_abs(),
        function.sig_hash.unsigned_abs(),
        function.ordinal
    );
    let call_conv = if cfg!(windows) {
        "windows_fastcall"
    } else {
        "system_v"
    };
    if function.return_type == "void" {
        format!("function %{symbol}() {call_conv} {{\nblock0:\nreturn\n}}")
    } else {
        format!(
            "function %{symbol}() -> i32 {call_conv} {{\nblock0:\nv0 = iconst.i32 {i32_return_value}\nreturn v0\n}}"
        )
    }
}

fn find_from_conversion_expression_errors(source: &str) -> Vec<ErrorMetric> {
    let sanitized = strip_comments_for_from_conversion_scan(source);
    let mut errors = Vec::new();
    for statement in split_semantic_statements(&sanitized) {
        if is_invalid_from_conversion_statement(statement.trim()) {
            errors.push(ErrorMetric {
                code: 4001,
                pos: 0,
                detail_a: 0,
                detail_b: 0,
            });
        }
    }
    errors
}

fn is_invalid_from_conversion_statement(statement: &str) -> bool {
    let trimmed = statement.trim();
    if !trimmed.contains(".from_") {
        return false;
    }
    if is_for_statement(trimmed) {
        return !is_valid_for_header_from_conversion_usage(trimmed);
    }
    !is_valid_from_conversion_statement(trimmed)
}

fn is_valid_from_conversion_statement(statement: &str) -> bool {
    if statement.is_empty() {
        return true;
    }
    let bytes = statement.as_bytes();
    let Some(dot_pos) = find_top_level_from_method_dot(statement) else {
        return false;
    };
    let receiver = statement[..dot_pos].trim();
    if receiver.is_empty() || !is_valid_from_conversion_receiver(receiver) {
        return false;
    }
    let mut cursor = dot_pos + 1;
    let Some(after_method) = parse_ascii_identifier(bytes, cursor) else {
        return false;
    };
    let method_name = &statement[cursor..after_method];
    if !method_name.starts_with("from_") {
        return false;
    }
    cursor = skip_ascii_whitespace(statement, after_method);
    if bytes.get(cursor).copied() != Some(b'(') {
        return false;
    }
    let Some(after_args) = find_matching_paren(bytes, cursor) else {
        return false;
    };
    if statement[cursor + 1..after_args - 1].contains(".from_") {
        return false;
    }
    cursor = skip_ascii_whitespace(statement, after_args);
    cursor == bytes.len()
}

fn is_for_statement(statement: &str) -> bool {
    if !statement.starts_with("for") {
        return false;
    }
    statement
        .as_bytes()
        .get(3)
        .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
}

fn is_valid_for_header_from_conversion_usage(statement: &str) -> bool {
    let bytes = statement.as_bytes();
    let mut cursor = skip_ascii_whitespace(statement, 0);
    cursor += "for".len();
    cursor = skip_ascii_whitespace(statement, cursor);
    if bytes.get(cursor).copied() != Some(b'(') {
        return false;
    }
    let Some(header_end) = find_matching_paren(bytes, cursor) else {
        return false;
    };
    if header_end <= cursor + 1 {
        return false;
    }
    let header = &statement[cursor + 1..header_end - 1];
    let Some([init, condition, step]) = split_for_header_segments(header) else {
        return false;
    };

    let init = init.trim();
    if init.contains(".from_") && !is_valid_from_conversion_statement(init) {
        return false;
    }

    let condition = condition.trim();
    if condition.contains(".from_") {
        return false;
    }

    let step = step.trim();
    if step.contains(".from_") && !is_valid_from_conversion_statement(step) {
        return false;
    }

    true
}

fn split_for_header_segments(header: &str) -> Option<[&str; 3]> {
    let bytes = header.as_bytes();
    let mut sections = Vec::with_capacity(3);
    let mut start = 0usize;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            b';' if paren_depth == 0 && bracket_depth == 0 => {
                sections.push(&header[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    sections.push(&header[start..]);
    if sections.len() != 3 {
        return None;
    }
    Some([sections[0], sections[1], sections[2]])
}

fn strip_comments_for_from_conversion_scan(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut cleaned = String::with_capacity(source.len());
    let mut cursor = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if in_string {
            cleaned.push(byte as char);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            cursor += 1;
            continue;
        }

        if byte == b'"' {
            in_string = true;
            cleaned.push('"');
            cursor += 1;
            continue;
        }

        if byte == b'/' && cursor + 1 < bytes.len() {
            let next = bytes[cursor + 1];
            if next == b'/' {
                cursor += 2;
                while cursor < bytes.len() && bytes[cursor] != b'\n' {
                    cursor += 1;
                }
                if cursor < bytes.len() {
                    cleaned.push('\n');
                    cursor += 1;
                }
                continue;
            }
            if next == b'*' {
                cursor += 2;
                while cursor + 1 < bytes.len()
                    && !(bytes[cursor] == b'*' && bytes[cursor + 1] == b'/')
                {
                    if bytes[cursor] == b'\n' {
                        cleaned.push('\n');
                    }
                    cursor += 1;
                }
                if cursor + 1 < bytes.len() {
                    cursor += 2;
                }
                continue;
            }
        }

        cleaned.push(byte as char);
        cursor += 1;
    }
    cleaned
}

fn split_semantic_statements(source: &str) -> Vec<&str> {
    let bytes = source.as_bytes();
    let mut statements = Vec::new();
    let mut start = 0usize;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            b';' | b'{' | b'}' if paren_depth == 0 && bracket_depth == 0 => {
                statements.push(&source[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < source.len() {
        statements.push(&source[start..]);
    }
    statements
}

fn find_top_level_from_method_dot(statement: &str) -> Option<usize> {
    let bytes = statement.as_bytes();
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut result: Option<usize> = None;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            b'.' if paren_depth == 0 && bracket_depth == 0 => {
                if statement[index + 1..].starts_with("from_") {
                    if result.is_some() {
                        return None;
                    }
                    result = Some(index);
                }
            }
            _ => {}
        }
    }
    result
}

fn is_valid_from_conversion_receiver(receiver: &str) -> bool {
    let bytes = receiver.as_bytes();
    let mut cursor = skip_ascii_whitespace(receiver, 0);
    let Some(after_identifier) = parse_ascii_identifier(bytes, cursor) else {
        return false;
    };
    cursor = after_identifier;
    loop {
        cursor = skip_ascii_whitespace(receiver, cursor);
        match bytes.get(cursor).copied() {
            Some(b'.') => {
                cursor += 1;
                cursor = skip_ascii_whitespace(receiver, cursor);
                let Some(after_segment) = parse_ascii_identifier(bytes, cursor) else {
                    return false;
                };
                cursor = after_segment;
            }
            Some(b'[') => {
                let Some(after_index) = find_matching_bracket(bytes, cursor) else {
                    return false;
                };
                if receiver[cursor + 1..after_index - 1].trim().is_empty() {
                    return false;
                }
                cursor = after_index;
            }
            _ => break,
        }
    }
    cursor = skip_ascii_whitespace(receiver, cursor);
    cursor == bytes.len()
}

fn skip_ascii_whitespace(source: &str, mut cursor: usize) -> usize {
    let bytes = source.as_bytes();
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

fn parse_ascii_identifier(bytes: &[u8], start: usize) -> Option<usize> {
    let first = *bytes.get(start)?;
    if !first.is_ascii_alphabetic() && first != b'_' {
        return None;
    }
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if !byte.is_ascii_alphanumeric() && byte != b'_' {
            break;
        }
        cursor += 1;
    }
    Some(cursor)
}

fn find_matching_paren(bytes: &[u8], open_index: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut cursor = open_index;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(cursor + 1);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

fn find_matching_bracket(bytes: &[u8], open_index: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut cursor = open_index;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(cursor + 1);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

fn normalize_path_key(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut text = absolute.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = text.strip_prefix("//?/") {
        text = stripped.to_string();
    }
    text
}

fn hash_text(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn hash_mix(hash: i32, value: i32) -> i32 {
    hash.wrapping_mul(16777619)
        .wrapping_add(value.wrapping_add(1))
}

fn hash_i32(value: &str) -> i32 {
    let mut hash: i32 = 216613626;
    for byte in value.bytes() {
        hash = hash_mix(hash, i32::from(byte));
    }
    hash
}

fn hash_identifier(name: &str) -> i32 {
    hash_i32(name)
}

fn function_key_for(path: &str, function: &ParsedFunction) -> FunctionKey {
    FunctionKey {
        path: path.to_string(),
        id_hash: function.id_hash,
        sig_hash: function.sig_hash,
    }
}

fn build_function_body_hash_by_key(
    state_by_path: &BTreeMap<String, FileState>,
) -> BTreeMap<FunctionKey, i32> {
    let mut by_key = BTreeMap::new();
    for (path, state) in state_by_path {
        for function in &state.functions {
            by_key.insert(function_key_for(path, function), function.body_hash);
        }
    }
    by_key
}

fn all_reachability_root_hashes(required_roots: &[i32]) -> Vec<i32> {
    let mut roots = vec![
        hash_identifier("main"),
        hash_identifier("tick"),
        hash_identifier("on_code_swap"),
    ];
    for root in required_roots {
        if !roots.contains(root) {
            roots.push(*root);
        }
    }
    roots
}

fn compute_reachable_function_keys_from_state(
    state_by_path: &BTreeMap<String, FileState>,
    required_roots: &[i32],
) -> BTreeSet<FunctionKey> {
    let mut all_keys = Vec::new();
    let mut by_id_hash: BTreeMap<i32, Vec<FunctionKey>> = BTreeMap::new();
    let mut call_edges_by_key: BTreeMap<FunctionKey, Vec<i32>> = BTreeMap::new();

    for (path, state) in state_by_path {
        for function in &state.functions {
            let key = function_key_for(path, function);
            all_keys.push(key.clone());
            by_id_hash
                .entry(function.id_hash)
                .or_default()
                .push(key.clone());
            call_edges_by_key.insert(key, function.call_target_id_hashes.clone());
        }
    }

    if all_keys.is_empty() {
        return BTreeSet::new();
    }

    let mut roots = all_reachability_root_hashes(required_roots);
    for state in state_by_path.values() {
        for function in &state.functions {
            if function.is_export && !roots.contains(&function.id_hash) {
                roots.push(function.id_hash);
            }
        }
    }
    let mut reachable = BTreeSet::new();
    let mut queue = VecDeque::new();
    let mut found_root = false;

    for root_hash in roots {
        if let Some(keys) = by_id_hash.get(&root_hash) {
            found_root = true;
            for key in keys {
                if reachable.insert(key.clone()) {
                    queue.push_back(key.clone());
                }
            }
        }
    }

    if !found_root {
        return all_keys.into_iter().collect();
    }

    while let Some(current) = queue.pop_front() {
        if let Some(callee_hashes) = call_edges_by_key.get(&current) {
            for callee_hash in callee_hashes {
                if let Some(callee_keys) = by_id_hash.get(callee_hash) {
                    for callee_key in callee_keys {
                        if reachable.insert(callee_key.clone()) {
                            queue.push_back(callee_key.clone());
                        }
                    }
                }
            }
        }
    }

    reachable
}

fn current_hook_symbol(state_by_path: &BTreeMap<String, FileState>) -> Option<String> {
    let hook_hash = hash_identifier("on_code_swap");
    for state in state_by_path.values() {
        if state.functions.iter().any(|func| func.id_hash == hook_hash) {
            return Some("on_code_swap".to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_file_state(functions: Vec<ParsedFunction>) -> FileState {
        FileState {
            layout_hash: 0,
            main_decl_count: 0,
            main_valid_count: 0,
            main_invalid_count: 0,
            functions,
        }
    }

    fn test_parsed_function(name: &str, sig_hash: i32, callees: &[&str]) -> ParsedFunction {
        ParsedFunction {
            ordinal: 0,
            id_hash: hash_identifier(name),
            is_export: false,
            sig_hash,
            body_hash: sig_hash.wrapping_mul(31),
            return_type: "i32".to_string(),
            param_count: 0,
            first_param_type_code: 0,
            simple_i32_return_expr: None,
            simple_i32_return_call_target_id_hash: None,
            simple_i32_return_call_add_delta: None,
            simple_i32_return_call_one_arg_target_id_hash: None,
            simple_i32_return_call_one_arg_i32_literal: None,
            simple_i32_return_call_one_arg_arg_call_target_id_hash: None,
            simple_i32_return_two_call_left_target_id_hash: None,
            simple_i32_return_two_call_right_target_id_hash: None,
            simple_i32_return_two_call_op_code: None,
            simple_void_print_i32_literal: None,
            simple_void_print_i32_call_target_id_hash: None,
            simple_void_print_i32_call_one_arg_arg_call_target_id_hash: None,
            simple_void_print_i32_call_add_delta: None,
            call_target_id_hashes: callees
                .iter()
                .map(|callee| hash_identifier(callee))
                .collect(),
            clif_text: String::new(),
        }
    }

    #[test]
    fn backend_name_is_stasis_orchestrated() {
        let host = IncrementalCompilerHost::new();
        assert_eq!(host.backend_name(), "stasis-orchestrated");
    }

    #[test]
    fn compile_empty_change_set_is_error() {
        let mut host = IncrementalCompilerHost::new();
        let err = host.compile_changed_files(&[]).expect_err("expected error");
        assert!(err.contains("no changed files"));
    }

    #[test]
    fn in_memory_reachability_is_transitive_from_default_roots() {
        let path_a = "/tmp/a.stasis".to_string();
        let path_b = "/tmp/b.stasis".to_string();
        let mut state_by_path = BTreeMap::new();
        state_by_path.insert(
            path_a.clone(),
            test_file_state(vec![
                test_parsed_function("main", 11, &["bridge"]),
                test_parsed_function("dead", 12, &[]),
            ]),
        );
        state_by_path.insert(
            path_b.clone(),
            test_file_state(vec![
                test_parsed_function("bridge", 21, &["leaf"]),
                test_parsed_function("leaf", 22, &[]),
            ]),
        );

        let reachable = compute_reachable_function_keys_from_state(&state_by_path, &[]);
        assert!(reachable.contains(&FunctionKey {
            path: path_a.clone(),
            id_hash: hash_identifier("main"),
            sig_hash: 11,
        }));
        assert!(reachable.contains(&FunctionKey {
            path: path_b.clone(),
            id_hash: hash_identifier("bridge"),
            sig_hash: 21,
        }));
        assert!(reachable.contains(&FunctionKey {
            path: path_b,
            id_hash: hash_identifier("leaf"),
            sig_hash: 22,
        }));
        assert!(!reachable.contains(&FunctionKey {
            path: path_a,
            id_hash: hash_identifier("dead"),
            sig_hash: 12,
        }));
    }

    #[test]
    fn in_memory_reachability_keeps_all_when_no_roots_exist() {
        let path = "/tmp/helpers.stasis".to_string();
        let mut state_by_path = BTreeMap::new();
        state_by_path.insert(
            path.clone(),
            test_file_state(vec![
                test_parsed_function("helper_a", 31, &[]),
                test_parsed_function("helper_b", 32, &["helper_a"]),
            ]),
        );

        let reachable = compute_reachable_function_keys_from_state(&state_by_path, &[]);
        assert_eq!(reachable.len(), 2);
        assert!(reachable.contains(&FunctionKey {
            path: path.clone(),
            id_hash: hash_identifier("helper_a"),
            sig_hash: 31,
        }));
        assert!(reachable.contains(&FunctionKey {
            path,
            id_hash: hash_identifier("helper_b"),
            sig_hash: 32,
        }));
    }

    #[test]
    fn in_memory_reachability_honors_required_roots() {
        let path = "/tmp/required_root.stasis".to_string();
        let mut state_by_path = BTreeMap::new();
        state_by_path.insert(
            path.clone(),
            test_file_state(vec![
                test_parsed_function("main", 41, &[]),
                test_parsed_function("bridge_entry", 42, &[]),
            ]),
        );

        let required = [hash_identifier("bridge_entry")];
        let reachable = compute_reachable_function_keys_from_state(&state_by_path, &required);
        assert!(reachable.contains(&FunctionKey {
            path: path.clone(),
            id_hash: hash_identifier("main"),
            sig_hash: 41,
        }));
        assert!(reachable.contains(&FunctionKey {
            path,
            id_hash: hash_identifier("bridge_entry"),
            sig_hash: 42,
        }));
    }

    #[test]
    fn in_memory_reachability_includes_exported_functions_as_roots() {
        let path = "/tmp/exported_root.stasis".to_string();
        let mut state_by_path = BTreeMap::new();
        let mut helper = test_parsed_function("helper_exported", 51, &[]);
        helper.is_export = true;
        state_by_path.insert(
            path.clone(),
            test_file_state(vec![test_parsed_function("main", 50, &[]), helper]),
        );

        let reachable = compute_reachable_function_keys_from_state(&state_by_path, &[]);
        assert!(reachable.contains(&FunctionKey {
            path: path.clone(),
            id_hash: hash_identifier("main"),
            sig_hash: 50,
        }));
        assert!(reachable.contains(&FunctionKey {
            path,
            id_hash: hash_identifier("helper_exported"),
            sig_hash: 51,
        }));
    }

    #[test]
    fn compile_deleted_file_updates_state_without_read_error() {
        let temp = std::env::temp_dir().join(format!(
            "stasis_compiler_deleted_file_{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&temp);
        let source_path = temp.join("main.stasis");
        fs::write(&source_path, "function main(): i32 { return 0; }").expect("write source");

        let mut host = IncrementalCompilerHost::new();
        let first = host
            .compile_changed_files(std::slice::from_ref(&source_path))
            .expect("first compile should succeed");
        assert_eq!(first.status, 0);

        fs::remove_file(&source_path).expect("remove source");
        let deleted = host
            .compile_changed_files(std::slice::from_ref(&source_path))
            .expect("deleted file should not return read error");
        assert_eq!(deleted.status, 2);
        assert!(deleted.errors.iter().any(|error| error.code == 41));
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn compile_records_function_hashes_return_type_and_hook_symbol() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_metrics_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.hook_symbol.as_deref(), Some("on_code_swap"));
        assert_eq!(compile.functions.len(), 2);
        assert!(compile.functions.iter().any(|f| f.return_type == "i32"));
        assert!(compile.functions.iter().any(|f| f.return_type == "void"));
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(
            main.simple_i32_return_expr,
            Some(SimpleI32ReturnExpr::Literal(0))
        );
        assert_eq!(hook.simple_i32_return_expr, None);
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_i32_return_call_target_id_hash.is_none()));
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_i32_return_call_add_delta.is_none()));
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_i32_return_call_one_arg_target_id_hash.is_none()));
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_i32_return_call_one_arg_i32_literal.is_none()));
        assert!(compile.functions.iter().all(|f| f
            .simple_i32_return_call_one_arg_arg_call_target_id_hash
            .is_none()));
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_void_print_i32_literal.is_none()));
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_void_print_i32_call_target_id_hash.is_none()));
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_void_print_i32_call_add_delta.is_none()));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_ignores_extern_function_declarations() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_extern_decl_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_cli_arg_count(): i32;\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 1);
        assert_eq!(compile.functions[0].id_hash, hash_identifier("main"));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_folds_noarg_direct_call_into_emitted_stub_value() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_call_fold_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(): i32 { return 7; }\nfunction main(): i32 { return callee(); }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        let callee = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("callee"))
            .expect("callee metric");
        assert!(
            main.clif_text.contains("iconst.i32 7"),
            "expected folded call value in main clif: {}",
            main.clif_text
        );
        assert!(
            callee.clif_text.contains("iconst.i32 7"),
            "expected callee value in callee clif: {}",
            callee.clif_text
        );
        fs::remove_dir_all(&temp_root).ok();
    }
    #[test]
    fn second_compile_without_source_change_emits_no_functions() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_no_change_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction tick(): void { return; }\n",
        )
        .expect("write sample");

        let first = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("first compile");
        assert_eq!(first.status, 0);
        assert!(first.functions.len() >= 2);

        let second = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("second compile");
        assert_eq!(second.status, 0);
        assert_eq!(second.functions.len(), 0);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_emits_only_changed_functions_after_edit() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_changed_fn_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let baseline = "function main(): i32 { return 0; }\nfunction tick(): void { return; }\n";
        fs::write(&file, baseline).expect("write baseline");

        let first = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("first compile");
        assert_eq!(first.status, 0);
        assert_eq!(first.functions.len(), 2);

        let updated = "function main(): i32 { return 1; }\nfunction tick(): void { return; }\n";
        fs::write(&file, updated).expect("write updated");
        let second = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("second compile");
        assert_eq!(second.status, 0);
        assert_eq!(second.functions.len(), 1);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn reachability_prunes_unreachable_helper_functions_from_emission() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_reachability_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function helper(): i32 { return 9; }\nfunction tick(): void { return; }\nfunction main(): i32 { return 1; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile");
        assert_eq!(compile.status, 0);
        assert!(compile
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("main")));
        assert!(compile
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("tick")));
        assert!(!compile
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("helper")));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn host_required_reachability_root_keeps_otherwise_unreachable_function() {
        let mut host = IncrementalCompilerHost::new();
        host.set_required_reachability_roots(&["bridge_entry"]);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_required_root_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function bridge_entry(): i32 { return 9; }\nfunction main(): i32 { return 1; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile");
        assert_eq!(compile.status, 0);
        assert!(compile
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("main")));
        assert!(compile
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("bridge_entry")));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn cross_file_reachability_emits_newly_reachable_unchanged_callee() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_cross_file_reachability_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file_a = temp_root.join("a.stasis");
        let file_b = temp_root.join("b.stasis");
        fs::write(&file_a, "function main(): i32 { return 0; }\n").expect("write a baseline");
        fs::write(
            &file_b,
            "function helper(): i32 { return 7; }\nfunction dead(): i32 { return 0; }\n",
        )
        .expect("write b baseline");

        let first = host
            .compile_changed_files(&[file_a.clone(), file_b.clone()])
            .expect("first compile");
        assert_eq!(first.status, 0);
        assert!(first
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("main")));
        assert!(!first
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("helper")));

        fs::write(&file_a, "function main(): i32 { return helper(); }\n").expect("write a updated");
        let second = host
            .compile_changed_files(std::slice::from_ref(&file_a))
            .expect("second compile");
        assert_eq!(second.status, 0);
        assert!(second
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("main")));
        assert!(second
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("helper")));
        assert!(!second
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("dead")));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn receiver_overloads_produce_distinct_signature_hashes() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_receiver_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { damage(0, 1); return 0; }\nfunction damage(self: Enemy, amount: i32): void { return; }\nfunction damage(self: Hero, amount: i32): void { return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile");
        assert_eq!(compile.status, 0);
        let damage = compile
            .functions
            .iter()
            .filter(|f| f.id_hash == hash_identifier("damage"))
            .collect::<Vec<_>>();
        assert_eq!(damage.len(), 2);
        assert_ne!(damage[0].sig_hash, damage[1].sig_hash);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn struct_global_lowering_uses_single_owner_global_and_import_for_secondary_function() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_global_owner_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "struct Enemy { hp: i32; }\n\
             global State { score: i32; enemy: Enemy; }\n\
             function set_first(): i32 { State.enemy.hp = 3; return State.enemy.hp; }\n\
             function set_second(): i32 { State.score = 7; return State.score; }\n\
             function main(): i32 { set_first(); return set_second(); }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile");
        assert_eq!(compile.status, 0);

        let first = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("set_first"))
            .expect("set_first metric");
        let second = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("set_second"))
            .expect("set_second metric");

        assert!(!first.clif_text.is_empty(), "expected set_first clif");
        assert!(!second.clif_text.is_empty(), "expected set_second clif");
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn reachability_prunes_unreachable_struct_and_global_layout_from_emitted_arena() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_dead_struct_global_prune_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "struct Live { hp: i32; }\n\
             struct Dead { value: i32; }\n\
             global LiveState { enemy: Live; }\n\
             global DeadState { dead: Dead; }\n\
             function dead_write(): i32 { DeadState.dead.value = 9; return DeadState.dead.value; }\n\
             function main(): i32 { LiveState.enemy.hp = 7; return LiveState.enemy.hp; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile");
        assert_eq!(compile.status, 0);
        assert!(compile
            .functions
            .iter()
            .any(|function| function.id_hash == hash_identifier("main")));
        assert!(!compile
            .functions
            .iter()
            .any(|function| function.id_hash == hash_identifier("dead_write")));

        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert!(!main.clif_text.is_empty(), "expected main clif");
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn from_conversion_in_expression_is_semantic_error() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_from_expr_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let source =
            "function main(): i32 { let x: i32 = 0; let y: i32 = 1; let z: i32 = x.from_i32(y); return 0; }\n";
        fs::write(&file, source).expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 2);
        assert!(compile.errors.iter().any(|error| error.code == 4001));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn from_conversion_statements_inside_if_else_are_valid() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_from_stmt_if_else_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let source = "function main(): i32 {\n\
            let snap_i: i32 = 0;\n\
            let snap_f: f32 = 0.0;\n\
            let value: f32 = 1.0;\n\
            if (value >= 0.0) {\n\
                snap_i.from_f32(value + 0.5);\n\
            } else {\n\
                snap_i.from_f32(value - 0.5);\n\
            }\n\
            snap_f.from_i32(snap_i);\n\
            return 0;\n\
        }\n";
        fs::write(&file, source).expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert!(!compile.errors.iter().any(|error| error.code == 4001));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn from_conversion_statement_with_field_receiver_is_valid() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_from_stmt_field_receiver_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let source = "struct State { src: i32; dst: f32; }\n\
            global state: State;\n\
            function main(): i32 {\n\
                state.src = 9;\n\
                state.dst.from_i32(state.src);\n\
                let out: i32 = 0;\n\
                out.from_f32(state.dst);\n\
                return out;\n\
            }\n";
        fs::write(&file, source).expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert!(!compile.errors.iter().any(|error| error.code == 4001));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn from_conversion_statement_in_for_init_and_step_is_valid() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_from_for_stmt_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let source = "function mark(): void { return; }\n\
            function main(): i32 {\n\
                let i: f32 = 0.0;\n\
                let sum: i32 = 0;\n\
                for (mark(); i < 3.0; i.from_i32(sum)) {\n\
                    sum += 1;\n\
                }\n\
                return sum;\n\
            }\n";
        fs::write(&file, source).expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert!(!compile.errors.iter().any(|error| error.code == 4001));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn from_conversion_statement_in_for_init_and_step_with_global_and_indexed_targets_is_valid() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_from_for_stmt_global_indexed_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let source = "const COUNT: i32 = 2;\n\
            struct Node { value: f32; }\n\
            global nodes: Node[COUNT];\n\
            global State { snap: f32; }\n\
            function main(): i32 {\n\
                let sum: i32 = 0;\n\
                for (State.snap.from_i32(0); sum < 2; nodes[1].value.from_i32(sum)) {\n\
                    sum += 1;\n\
                }\n\
                return sum;\n\
            }\n";
        fs::write(&file, source).expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert!(!compile.errors.iter().any(|error| error.code == 4001));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn from_conversion_in_for_condition_is_semantic_error() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_from_for_condition_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let source = "function main(): i32 {\n\
            let value: f32 = 0.0;\n\
            let i: i32 = 0;\n\
            for (; value.from_i32(i); i += 1) {\n\
                return 0;\n\
            }\n\
            return 1;\n\
        }\n";
        fs::write(&file, source).expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 2);
        assert!(compile.errors.iter().any(|error| error.code == 4001));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_accepts_inline_function_annotation() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_inline_annotation_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let source =
            "function @inline helper(): i32 { return 5; }\nfunction main(): i32 { return helper(); }\n";
        fs::write(&file, source).expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert!(compile.errors.is_empty());
        assert!(compile
            .functions
            .iter()
            .any(|function| function.id_hash == hash_identifier("main")));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn from_conversion_text_inside_comments_is_ignored() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_from_comment_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let source = "function main(): i32 {\n\
            let value: f32 = 0.0;\n\
            // value.from_i32(7);\n\
            /* value.from_i32(8); */\n\
            value.from_i32(9);\n\
            let out: i32 = 0;\n\
            out.from_f32(value);\n\
            return out;\n\
        }\n";
        fs::write(&file, source).expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert!(!compile.errors.iter().any(|error| error.code == 4001));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn hook_symbol_persists_when_non_hook_function_changes() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_hook_symbol_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let baseline =
            "function main(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n";
        fs::write(&file, baseline).expect("write baseline");

        let first = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("first compile");
        assert_eq!(first.status, 0);
        assert_eq!(first.hook_symbol.as_deref(), Some("on_code_swap"));

        let updated =
            "function main(): i32 { return 1; }\nfunction on_code_swap(): void { return; }\n";
        fs::write(&file, updated).expect("write updated");
        let second = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("second compile");
        assert_eq!(second.status, 0);
        assert_eq!(second.hook_symbol.as_deref(), Some("on_code_swap"));
        assert_eq!(second.functions.len(), 1);
        assert_eq!(
            second.functions[0].simple_i32_return_expr,
            Some(SimpleI32ReturnExpr::Literal(1))
        );
        fs::remove_dir_all(&temp_root).ok();
    }
}
