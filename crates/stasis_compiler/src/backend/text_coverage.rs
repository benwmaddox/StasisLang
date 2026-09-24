//! Conservative, bounded proof of Unicode scalars reaching compiler-owned text sinks.
//!
//! This is evidence only. It never transforms fonts and deliberately returns
//! `Unknown` when accepted HIR does not prove a finite value or font identity.

use std::collections::{BTreeMap, BTreeSet};

use crate::backend::compile_analysis::{
    resolve_call_signature, CallSignature, CompileAnalysisCache, ConstantValue,
};
use crate::compiler::{FunctionId, FunctionMeta, SourceFile};
use crate::frontend::types::{
    TypeCategory, TypeId, TypeTable, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_I32,
};
use crate::ir::hir::{
    AssignOp, AssignTarget, FunctionHIR, SimpleCondition, SimpleExpr, SimpleStmt,
};

const MAX_CALL_DEPTH: usize = 32;
const MAX_SCALARS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct TextCoverageFontEvidence {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct TextCoverageSinkEvidence {
    pub caller_function_id: FunctionId,
    pub sink_identity: String,
    pub font_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct TextCoverageUnknownReason {
    pub code: String,
    pub function_id: FunctionId,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TextCoverageProof {
    Finite {
        unicode_scalars: Vec<u32>,
        fonts: Vec<TextCoverageFontEvidence>,
        sinks: Vec<TextCoverageSinkEvidence>,
    },
    Unknown {
        reasons: Vec<TextCoverageUnknownReason>,
        fonts: Vec<TextCoverageFontEvidence>,
        sinks: Vec<TextCoverageSinkEvidence>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TextValue {
    scalars: Option<BTreeSet<u32>>,
    reason: Option<String>,
}

impl TextValue {
    fn literal(value: &str) -> Self {
        Self {
            scalars: Some(value.chars().map(u32::from).collect()),
            reason: None,
        }
    }

    fn finite(scalars: BTreeSet<u32>) -> Self {
        Self {
            scalars: Some(scalars),
            reason: None,
        }
    }

    fn unknown(reason: impl Into<String>) -> Self {
        Self {
            scalars: None,
            reason: Some(reason.into()),
        }
    }

    fn union(&mut self, other: Self) {
        match (&mut self.scalars, other.scalars) {
            (Some(left), Some(right)) => left.extend(right),
            _ => {
                self.scalars = None;
                if self.reason.is_none() {
                    self.reason = other.reason;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Environment {
    types: BTreeMap<String, TypeId>,
    text: BTreeMap<String, TextValue>,
    fonts: BTreeMap<String, Option<String>>,
    buffers: BTreeMap<String, TextValue>,
    globals: BTreeSet<String>,
}

struct Analyzer<'a> {
    functions: &'a [FunctionMeta],
    hirs: &'a BTreeMap<FunctionId, FunctionHIR>,
    files: &'a [SourceFile],
    analysis: &'a CompileAnalysisCache,
    types: &'a TypeTable,
    scalars: BTreeSet<u32>,
    fonts: BTreeSet<TextCoverageFontEvidence>,
    sinks: BTreeSet<TextCoverageSinkEvidence>,
    reasons: BTreeSet<TextCoverageUnknownReason>,
}

pub(crate) fn analyze_text_coverage(
    functions: &[FunctionMeta],
    hirs: &BTreeMap<FunctionId, FunctionHIR>,
    reachable: &BTreeSet<FunctionId>,
    files: &[SourceFile],
    analysis: &CompileAnalysisCache,
    types: &TypeTable,
) -> TextCoverageProof {
    let mut analyzer = Analyzer {
        functions,
        hirs,
        files,
        analysis,
        types,
        scalars: BTreeSet::new(),
        fonts: BTreeSet::new(),
        sinks: BTreeSet::new(),
        reasons: BTreeSet::new(),
    };
    let roots = reachable
        .iter()
        .copied()
        .filter(|function_id| {
            analyzer.function(*function_id).is_some_and(|function| {
                function.host_export.is_some()
                    || matches!(
                        function.name.as_str(),
                        "main" | "tick" | "render" | "on_code_swap"
                    )
                    || !function
                        .dependents
                        .iter()
                        .any(|dependent| reachable.contains(dependent))
            })
        })
        .collect::<Vec<_>>();
    for function_id in if roots.is_empty() {
        reachable.iter().copied().collect()
    } else {
        roots
    } {
        analyzer.visit_function(function_id);
    }
    let fonts = analyzer.fonts.into_iter().collect();
    let sinks = analyzer.sinks.into_iter().collect();
    if analyzer.reasons.is_empty() && analyzer.scalars.len() <= MAX_SCALARS {
        TextCoverageProof::Finite {
            unicode_scalars: analyzer.scalars.into_iter().collect(),
            fonts,
            sinks,
        }
    } else {
        if analyzer.scalars.len() > MAX_SCALARS {
            analyzer.reasons.insert(TextCoverageUnknownReason {
                code: "coverage_limit_exceeded".to_string(),
                function_id: 0,
                detail: format!("more than {MAX_SCALARS} Unicode scalars"),
            });
        }
        TextCoverageProof::Unknown {
            reasons: analyzer.reasons.into_iter().collect(),
            fonts,
            sinks,
        }
    }
}

impl Analyzer<'_> {
    fn function(&self, id: FunctionId) -> Option<&FunctionMeta> {
        self.functions.iter().find(|function| function.id == id)
    }

    fn visit_function(&mut self, function_id: FunctionId) {
        let Some(function) = self.function(function_id).cloned() else {
            return;
        };
        let Some(hir) = self.hirs.get(&function_id).cloned() else {
            return;
        };
        let mut env = Environment::default();
        for (name, type_id) in function.param_names.iter().zip(&function.params) {
            env.types.insert(name.clone(), *type_id);
            if self.is_text_type(*type_id) {
                env.text
                    .insert(name.clone(), TextValue::unknown("unbound string parameter"));
            }
        }
        self.visit_statements(
            function_id,
            &hir.statements,
            &mut env,
            &mut vec![function_id],
        );
    }

    fn visit_statements(
        &mut self,
        function_id: FunctionId,
        statements: &[SimpleStmt],
        env: &mut Environment,
        stack: &mut Vec<FunctionId>,
    ) {
        for statement in statements {
            match statement {
                SimpleStmt::Let {
                    name,
                    type_id,
                    expression,
                } => {
                    if let Some(type_id) = type_id.or_else(|| self.infer_type(expression, env)) {
                        env.types.insert(name.clone(), type_id);
                    }
                    if let SimpleExpr::Identifier(source) = expression {
                        if env.buffers.contains_key(source) {
                            env.text.remove(name);
                            env.buffers
                                .insert(name.clone(), TextValue::unknown("aliased text buffer"));
                        } else if self.is_text_expr(expression, env) {
                            env.text.insert(
                                name.clone(),
                                self.eval_text(function_id, expression, env, stack),
                            );
                        }
                    } else if self.is_text_expr(expression, env) {
                        env.text.insert(
                            name.clone(),
                            self.eval_text(function_id, expression, env, stack),
                        );
                    }
                    if let Some(font) = self.eval_font(expression, env) {
                        env.fonts.insert(name.clone(), font);
                    }
                    self.visit_expr(function_id, expression, env, stack);
                }
                SimpleStmt::Assign {
                    target,
                    op,
                    expression,
                } => {
                    self.visit_expr(function_id, expression, env, stack);
                    if let AssignTarget::GlobalPath(name) = target {
                        env.globals.insert(name.clone());
                    }
                    match target {
                        AssignTarget::Local(name) | AssignTarget::GlobalPath(name) => {
                            let buffer_alias = matches!(
                                expression,
                                SimpleExpr::Identifier(source) if env.buffers.contains_key(source)
                            );
                            if buffer_alias {
                                env.text.remove(name);
                                env.buffers.insert(
                                    name.clone(),
                                    TextValue::unknown("aliased text buffer"),
                                );
                            } else if env.text.contains_key(name)
                                || self.is_text_expr(expression, env)
                            {
                                env.text.insert(
                                    name.clone(),
                                    if *op == AssignOp::Set {
                                        self.eval_text(function_id, expression, env, stack)
                                    } else {
                                        TextValue::unknown("mutable text assignment")
                                    },
                                );
                            }
                            if let Some(font) = self.eval_font(expression, env) {
                                env.fonts.insert(name.clone(), font);
                            } else if env.fonts.contains_key(name) {
                                env.fonts.insert(name.clone(), None);
                            }
                            if env.buffers.contains_key(name) {
                                env.buffers.insert(
                                    name.clone(),
                                    TextValue::unknown("direct buffer assignment"),
                                );
                            }
                        }
                        AssignTarget::IndexedPath {
                            collection_path,
                            index,
                            ..
                        } => {
                            self.visit_expr(function_id, index, env, stack);
                            self.poison_buffer(env, collection_path, "indexed buffer assignment");
                        }
                    }
                }
                SimpleStmt::Convert { target, source, .. } => {
                    self.visit_expr(function_id, source, env, stack);
                    self.poison_assign_target(env, target, "converted buffer assignment");
                }
                SimpleStmt::Expr(source) => self.visit_expr(function_id, source, env, stack),
                SimpleStmt::If {
                    condition,
                    then_statements,
                    else_statements,
                } => {
                    self.visit_condition(function_id, condition, env, stack);
                    let mut then_env = env.clone();
                    self.visit_statements(function_id, then_statements, &mut then_env, stack);
                    let mut else_env = env.clone();
                    if let Some(else_statements) = else_statements {
                        self.visit_statements(function_id, else_statements, &mut else_env, stack);
                    }
                    merge_environment(env, then_env, else_env);
                }
                SimpleStmt::For {
                    init,
                    condition,
                    step,
                    body_statements,
                } => {
                    self.visit_statements(function_id, std::slice::from_ref(init), env, stack);
                    poison_loop_carried_values(env);
                    self.visit_condition(function_id, condition, env, stack);
                    let mut loop_env = env.clone();
                    self.visit_statements(function_id, body_statements, &mut loop_env, stack);
                    self.visit_statements(
                        function_id,
                        std::slice::from_ref(step),
                        &mut loop_env,
                        stack,
                    );
                    invalidate_changed_values(env, &loop_env);
                }
                SimpleStmt::Foreach {
                    body_statements, ..
                } => {
                    poison_loop_carried_values(env);
                    let mut loop_env = env.clone();
                    self.visit_statements(function_id, body_statements, &mut loop_env, stack);
                    invalidate_changed_values(env, &loop_env);
                }
                SimpleStmt::Return(expression) => {
                    self.visit_expr(function_id, expression, env, stack)
                }
                SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
            }
        }
    }

    fn visit_condition(
        &mut self,
        function_id: FunctionId,
        condition: &SimpleCondition,
        env: &mut Environment,
        stack: &mut Vec<FunctionId>,
    ) {
        match condition {
            SimpleCondition::Comparison { lhs, rhs, .. } => {
                self.visit_expr(function_id, lhs, env, stack);
                self.visit_expr(function_id, rhs, env, stack);
            }
            SimpleCondition::Expr(expression) => {
                self.visit_expr(function_id, expression, env, stack)
            }
            SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
                self.visit_condition(function_id, lhs, env, stack);
                self.visit_condition(function_id, rhs, env, stack);
            }
            SimpleCondition::Not(inner) => self.visit_condition(function_id, inner, env, stack),
        }
    }

    fn visit_expr(
        &mut self,
        function_id: FunctionId,
        expression: &SimpleExpr,
        env: &mut Environment,
        stack: &mut Vec<FunctionId>,
    ) {
        match expression {
            SimpleExpr::Call { target, args } => {
                for arg in args {
                    self.visit_expr(function_id, arg, env, stack);
                }
                let signature = self.resolve_call(target, args, env).cloned();
                if let Some(signature) = signature {
                    let sink = self.sink_contract(&signature);
                    if let Some((identity, font_index, text_index)) = sink.clone() {
                        self.record_sink(
                            function_id,
                            identity,
                            args.get(font_index),
                            args.get(text_index),
                            env,
                            stack,
                        );
                    }
                    let digit_chain = self.apply_digit_chain(&signature, args, env, function_id);
                    if sink.is_none() && !digit_chain {
                        self.poison_buffer_arguments(
                            args,
                            env,
                            "arbitrary call may mutate text buffer",
                        );
                        if signature.extern_symbol.is_some()
                            && !self.is_trusted_resolved_extern(&signature)
                            && args.iter().any(|arg| self.is_text_expr(arg, env))
                        {
                            self.unknown(
                                function_id,
                                "custom_text_extern",
                                signature
                                    .extern_symbol
                                    .clone()
                                    .unwrap_or_else(|| target.clone()),
                            );
                        }
                        self.visit_internal_call(function_id, target, &signature, args, env, stack);
                    }
                } else if args.iter().any(|arg| self.is_text_expr(arg, env)) {
                    self.unknown(function_id, "unresolved_text_call", target.clone());
                } else if args.iter().any(contains_indexed_expr) {
                    self.unknown(function_id, "unresolved_indexed_text_call", target.clone());
                }
            }
            SimpleExpr::Binary { lhs, rhs, .. } => {
                self.visit_expr(function_id, lhs, env, stack);
                self.visit_expr(function_id, rhs, env, stack);
            }
            SimpleExpr::Condition(condition) => {
                self.visit_condition(function_id, condition, env, stack)
            }
            SimpleExpr::IndexedPath { index, .. } => {
                self.visit_expr(function_id, index, env, stack)
            }
            SimpleExpr::DefaultValue(_)
            | SimpleExpr::Int(_)
            | SimpleExpr::Float(_)
            | SimpleExpr::Bool(_)
            | SimpleExpr::StringLiteral(_)
            | SimpleExpr::Identifier(_) => {}
        }
    }

    fn record_sink(
        &mut self,
        function_id: FunctionId,
        identity: String,
        font: Option<&SimpleExpr>,
        text: Option<&SimpleExpr>,
        env: &Environment,
        stack: &mut Vec<FunctionId>,
    ) {
        let font_path = font.and_then(|font| self.eval_font(font, env).flatten());
        if font_path.is_none() {
            self.unknown(function_id, "unknown_font", identity.clone());
        } else if let Some(font_path) = &font_path {
            self.fonts.insert(TextCoverageFontEvidence {
                path: font_path.clone(),
            });
            self.sinks.insert(TextCoverageSinkEvidence {
                caller_function_id: function_id,
                sink_identity: identity.clone(),
                font_path: font_path.clone(),
            });
        }
        let value = text.map_or_else(
            || TextValue::unknown("missing text argument"),
            |text| self.eval_text(function_id, text, env, stack),
        );
        if value.scalars.is_none() {
            self.unknown(
                function_id,
                "unknown_text_value",
                value.reason.clone().unwrap_or_else(|| identity.clone()),
            );
        }
        let Some(scalars) = value.scalars else {
            return;
        };
        self.scalars.extend(scalars);
    }

    fn eval_text(
        &self,
        function_id: FunctionId,
        expression: &SimpleExpr,
        env: &Environment,
        stack: &mut Vec<FunctionId>,
    ) -> TextValue {
        match expression {
            SimpleExpr::StringLiteral(value) => TextValue::literal(value),
            SimpleExpr::Identifier(name) => env
                .text
                .get(name)
                .cloned()
                .or_else(|| env.buffers.get(name).cloned())
                .or_else(|| match self.analysis.constant_values.get(name) {
                    Some(ConstantValue::String { value, .. }) => Some(TextValue::literal(value)),
                    _ => None,
                })
                .unwrap_or_else(|| TextValue::unknown(format!("unproven text identifier {name}"))),
            SimpleExpr::Call { target, args } => {
                let Some(signature) = self.resolve_call(target, args, env) else {
                    return TextValue::unknown(format!("unresolved helper {target}"));
                };
                let Some(callee_id) = signature.function_id else {
                    return TextValue::unknown(format!("external text helper {target}"));
                };
                if stack.len() >= MAX_CALL_DEPTH || stack.contains(&callee_id) {
                    return TextValue::unknown(format!("recursive text helper {target}"));
                }
                let Some(callee) = self.function(callee_id) else {
                    return TextValue::unknown(format!("missing text helper {target}"));
                };
                let Some(hir) = self.hirs.get(&callee_id) else {
                    return TextValue::unknown(format!("missing text HIR {target}"));
                };
                let mut callee_env = Environment::default();
                for ((name, type_id), argument) in
                    callee.param_names.iter().zip(&callee.params).zip(args)
                {
                    callee_env.types.insert(name.clone(), *type_id);
                    if self.is_text_type(*type_id) {
                        callee_env.text.insert(
                            name.clone(),
                            self.eval_text(function_id, argument, env, stack),
                        );
                    }
                }
                stack.push(callee_id);
                let value = self.eval_returns(callee_id, &hir.statements, &mut callee_env, stack);
                stack.pop();
                value
            }
            SimpleExpr::IndexedPath { .. } => TextValue::unknown("indexed text alias"),
            _ => TextValue::unknown("non-text expression"),
        }
    }

    fn eval_returns(
        &self,
        function_id: FunctionId,
        statements: &[SimpleStmt],
        env: &mut Environment,
        stack: &mut Vec<FunctionId>,
    ) -> TextValue {
        let mut result: Option<TextValue> = None;
        for statement in statements {
            match statement {
                SimpleStmt::Let {
                    name,
                    type_id,
                    expression,
                } => {
                    if let Some(type_id) = type_id.or_else(|| self.infer_type(expression, env)) {
                        env.types.insert(name.clone(), type_id);
                    }
                    if self.is_text_expr(expression, env) {
                        env.text.insert(
                            name.clone(),
                            self.eval_text(function_id, expression, env, stack),
                        );
                    }
                }
                SimpleStmt::Assign { target, .. } => {
                    if let AssignTarget::Local(name) | AssignTarget::GlobalPath(name) = target {
                        env.text
                            .insert(name.clone(), TextValue::unknown("mutable helper text"));
                    }
                }
                SimpleStmt::Return(expression) => {
                    let value = self.eval_text(function_id, expression, env, stack);
                    if let Some(result) = &mut result {
                        result.union(value);
                    } else {
                        result = Some(value);
                    }
                }
                SimpleStmt::If {
                    then_statements,
                    else_statements,
                    ..
                } => {
                    let mut then_env = env.clone();
                    let mut branch =
                        self.eval_returns(function_id, then_statements, &mut then_env, stack);
                    if let Some(else_statements) = else_statements {
                        let mut else_env = env.clone();
                        branch.union(self.eval_returns(
                            function_id,
                            else_statements,
                            &mut else_env,
                            stack,
                        ));
                    }
                    if let Some(result) = &mut result {
                        result.union(branch);
                    } else {
                        result = Some(branch);
                    }
                }
                SimpleStmt::For { .. } | SimpleStmt::Foreach { .. } => {
                    return TextValue::unknown("loop in text helper")
                }
                _ => {}
            }
        }
        result.unwrap_or_else(|| TextValue::unknown("helper has no finite return"))
    }

    fn eval_font(&self, expression: &SimpleExpr, env: &Environment) -> Option<Option<String>> {
        match expression {
            SimpleExpr::Identifier(name) => env.fonts.get(name).cloned(),
            SimpleExpr::Call { target, args } => {
                let signature = self.resolve_call(target, args, env)?;
                if !self.is_trusted_resolved_extern(signature)
                    || !matches!(
                        signature.extern_symbol.as_deref(),
                        Some("load_font" | "stasis_load_font" | "stasis_jit_load_font")
                    )
                {
                    return None;
                }
                let path = args.first().and_then(|arg| self.literal_or_constant(arg));
                Some(path)
            }
            _ => None,
        }
    }

    fn literal_or_constant(&self, expression: &SimpleExpr) -> Option<String> {
        match expression {
            SimpleExpr::StringLiteral(value) => Some(value.clone()),
            SimpleExpr::Identifier(name) => match self.analysis.constant_values.get(name) {
                Some(ConstantValue::String { value, .. }) => Some(value.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    fn resolve_call(
        &self,
        target: &str,
        args: &[SimpleExpr],
        env: &Environment,
    ) -> Option<&CallSignature> {
        let arg_types = args
            .iter()
            .map(|arg| self.infer_type(arg, env))
            .collect::<Option<Vec<_>>>()?;
        resolve_call_signature(
            target,
            &arg_types,
            &self.analysis.call_signatures,
            self.types,
            &self.analysis.named_struct_field_types,
        )
        .ok()
    }

    fn infer_type(&self, expression: &SimpleExpr, env: &Environment) -> Option<TypeId> {
        match expression {
            SimpleExpr::DefaultValue(type_id) => Some(*type_id),
            SimpleExpr::Int(_) => Some(TYPE_ID_I32),
            SimpleExpr::Float(_) => Some(TYPE_ID_F32),
            SimpleExpr::Bool(_) | SimpleExpr::Condition(_) => Some(TYPE_ID_BOOL),
            SimpleExpr::StringLiteral(_) => self.types.string_literal_type_id(),
            SimpleExpr::Identifier(name) => env
                .types
                .get(name)
                .copied()
                .or_else(|| self.analysis.global_path_types.get(name).copied())
                .or_else(|| match self.analysis.constant_values.get(name) {
                    Some(ConstantValue::String { type_id, .. })
                    | Some(ConstantValue::I32 { type_id, .. }) => Some(*type_id),
                    Some(ConstantValue::F32(_)) => Some(TYPE_ID_F32),
                    Some(ConstantValue::F64(_)) => Some(crate::frontend::types::TYPE_ID_F64),
                    Some(ConstantValue::Bool(_)) => Some(TYPE_ID_BOOL),
                    None => None,
                }),
            SimpleExpr::Call { target, args } => self
                .resolve_call(target, args, env)
                .map(|signature| signature.return_type),
            SimpleExpr::Binary { lhs, rhs, .. } => self
                .infer_type(lhs, env)
                .or_else(|| self.infer_type(rhs, env)),
            SimpleExpr::IndexedPath { .. } => None,
        }
    }

    fn is_text_expr(&self, expression: &SimpleExpr, env: &Environment) -> bool {
        self.infer_type(expression, env)
            .is_some_and(|type_id| self.is_text_type(type_id))
            || matches!(expression, SimpleExpr::Identifier(name) if env.buffers.contains_key(name))
    }

    fn is_text_type(&self, type_id: TypeId) -> bool {
        self.types.type_info(type_id).is_some_and(|info| {
            matches!(
                info.category,
                TypeCategory::AsciiFixed
                    | TypeCategory::AsciiView
                    | TypeCategory::Utf8Fixed
                    | TypeCategory::Utf8View
            )
        })
    }

    fn sink_contract(&self, signature: &CallSignature) -> Option<(String, usize, usize)> {
        if let Some(symbol) = signature.extern_symbol.as_deref() {
            if !self.is_trusted_resolved_extern(signature) {
                return None;
            }
            return match symbol {
                "measure_text" | "stasis_measure_text" | "stasis_jit_measure_text" => {
                    Some(("stasis_jit_measure_text".to_string(), 0, 1))
                }
                "stasis_jit_text_run_load_from" | "stasis_jit_text_run_replace_from" => {
                    Some((symbol.to_string(), 1, 2))
                }
                _ => None,
            };
        }
        let function = self.function(signature.function_id?)?;
        (function.name == "draw_text" && self.is_trusted_graphics_function(function))
            .then(|| (function.symbol_id.to_string(), 0, 1))
    }

    fn apply_digit_chain(
        &mut self,
        signature: &CallSignature,
        args: &[SimpleExpr],
        env: &mut Environment,
        _function_id: FunctionId,
    ) -> bool {
        let Some(callee_id) = signature.function_id else {
            return false;
        };
        let Some(callee) = self.function(callee_id) else {
            return false;
        };
        if !self.is_compiler_owned_stdlib_function(callee) {
            return false;
        }
        match callee.name.as_str() {
            "ascii_clear" => {
                if let Some(SimpleExpr::Identifier(dst)) = args.first() {
                    if self.analysis.global_path_types.contains_key(dst) {
                        env.globals.insert(dst.clone());
                    }
                    env.buffers
                        .insert(dst.clone(), TextValue::finite(BTreeSet::new()));
                }
                true
            }
            "ascii_push_i32" => {
                if let Some(SimpleExpr::Identifier(dst)) = args.first() {
                    if self.analysis.global_path_types.contains_key(dst) {
                        env.globals.insert(dst.clone());
                    }
                    let mut chars = (b'0'..=b'9').map(u32::from).collect::<BTreeSet<_>>();
                    chars.insert(u32::from(b'-'));
                    env.buffers
                        .entry(dst.clone())
                        .or_insert_with(|| TextValue::unknown("digit buffer was not cleared"))
                        .union(TextValue::finite(chars));
                }
                true
            }
            "utf8_from_ascii" => {
                if let (Some(SimpleExpr::Identifier(dst)), Some(SimpleExpr::Identifier(src))) =
                    (args.first(), args.get(1))
                {
                    if self.analysis.global_path_types.contains_key(dst) {
                        env.globals.insert(dst.clone());
                    }
                    let value = env.buffers.get(src).cloned().unwrap_or_else(|| {
                        TextValue::unknown("ASCII source has unknown mutation history")
                    });
                    env.buffers.insert(dst.clone(), value);
                }
                true
            }
            _ => false,
        }
    }

    fn visit_internal_call(
        &mut self,
        caller_id: FunctionId,
        target: &str,
        signature: &CallSignature,
        args: &[SimpleExpr],
        caller_env: &mut Environment,
        stack: &mut Vec<FunctionId>,
    ) {
        let Some(callee_id) = signature.function_id else {
            return;
        };
        if stack.len() >= MAX_CALL_DEPTH || stack.contains(&callee_id) {
            self.unknown(caller_id, "recursive_call", target.to_string());
            return;
        }
        let Some(callee) = self.function(callee_id).cloned() else {
            self.unknown(caller_id, "missing_helper", target.to_string());
            return;
        };
        let Some(hir) = self.hirs.get(&callee_id).cloned() else {
            return;
        };
        let caller_snapshot = caller_env.clone();
        let mut callee_env = Environment::default();
        for name in &caller_snapshot.globals {
            callee_env.globals.insert(name.clone());
            if let Some(type_id) = caller_snapshot.types.get(name) {
                callee_env.types.insert(name.clone(), *type_id);
            }
            if let Some(value) = caller_snapshot.text.get(name) {
                callee_env.text.insert(name.clone(), value.clone());
            }
            if let Some(value) = caller_snapshot.fonts.get(name) {
                callee_env.fonts.insert(name.clone(), value.clone());
            }
            if let Some(value) = caller_snapshot.buffers.get(name) {
                callee_env.buffers.insert(name.clone(), value.clone());
            }
        }
        for ((name, type_id), argument) in callee.param_names.iter().zip(&callee.params).zip(args) {
            callee_env.types.insert(name.clone(), *type_id);
            if self.is_text_type(*type_id) {
                callee_env.text.insert(
                    name.clone(),
                    self.eval_text(caller_id, argument, &caller_snapshot, stack),
                );
            }
            if let Some(font) = self.eval_font(argument, &caller_snapshot) {
                callee_env.fonts.insert(name.clone(), font);
            }
            if let SimpleExpr::Identifier(argument_name) = argument {
                if let Some(buffer) = caller_snapshot.buffers.get(argument_name) {
                    callee_env.buffers.insert(name.clone(), buffer.clone());
                }
            }
        }
        stack.push(callee_id);
        self.visit_statements(callee_id, &hir.statements, &mut callee_env, stack);
        stack.pop();
        poison_tracked_globals(caller_env);
    }

    fn is_trusted_resolved_extern(&self, signature: &CallSignature) -> bool {
        let Some(symbol) = signature.extern_symbol.as_deref() else {
            return false;
        };
        self.analysis
            .resolved_extern_signatures
            .iter()
            .any(|resolved| {
                resolved.symbol == symbol
                    && resolved.params == signature.params
                    && resolved.return_type == signature.return_type
                    && resolved.trusted_graphics_source
            })
    }

    fn is_trusted_graphics_function(&self, function: &FunctionMeta) -> bool {
        self.files
            .get(function.file_id as usize)
            .is_some_and(|file| {
                crate::frontend::module_graph::is_recognized_graphics_implementation_source(
                    &file.path,
                    &file.original_content,
                ) || crate::frontend::module_graph::is_explicit_graphics_test_seam_path(&file.path)
            })
    }

    fn is_compiler_owned_stdlib_function(&self, function: &FunctionMeta) -> bool {
        let Some(file) = self.files.get(function.file_id as usize) else {
            return false;
        };
        let path = file.path.replace('\\', "/");
        let recognized_path = path == "src/stdlib/stdlib.stasis"
            || path == ".stasis_cache/toolchain/src/stdlib/stdlib.stasis"
            || path.ends_with("/vendor/stasis/stdlib/stdlib.stasis")
            || path.ends_with("/vendor/stasis/src/stdlib/stdlib.stasis");
        let expected = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../src/stdlib/stdlib.stasis"
        ));
        recognized_path
            && (file.original_content == expected
                || file.original_content.replace("\r\n", "\n") == expected.replace("\r\n", "\n"))
    }

    fn poison_buffer_arguments(&self, args: &[SimpleExpr], env: &mut Environment, reason: &str) {
        for arg in args {
            if let SimpleExpr::Identifier(name) = arg {
                self.poison_buffer(env, name, reason);
            }
        }
    }

    fn poison_assign_target(&self, env: &mut Environment, target: &AssignTarget, reason: &str) {
        match target {
            AssignTarget::Local(name) | AssignTarget::GlobalPath(name) => {
                self.poison_buffer(env, name, reason)
            }
            AssignTarget::IndexedPath {
                collection_path, ..
            } => self.poison_buffer(env, collection_path, reason),
        }
    }

    fn poison_buffer(&self, env: &mut Environment, name: &str, reason: &str) {
        if env.buffers.contains_key(name) {
            env.buffers
                .insert(name.to_string(), TextValue::unknown(reason));
        }
    }

    fn unknown(&mut self, function_id: FunctionId, code: &str, detail: String) {
        self.reasons.insert(TextCoverageUnknownReason {
            code: code.to_string(),
            function_id,
            detail,
        });
    }
}

fn merge_environment(base: &mut Environment, left: Environment, right: Environment) {
    base.globals.extend(left.globals.iter().cloned());
    base.globals.extend(right.globals.iter().cloned());
    for name in left.text.keys().chain(right.text.keys()) {
        let mut value = left
            .text
            .get(name)
            .cloned()
            .unwrap_or_else(|| TextValue::unknown("missing branch value"));
        value.union(
            right
                .text
                .get(name)
                .cloned()
                .unwrap_or_else(|| TextValue::unknown("missing branch value")),
        );
        base.text.insert(name.clone(), value);
    }
    for name in left.fonts.keys().chain(right.fonts.keys()) {
        let value = match (left.fonts.get(name), right.fonts.get(name)) {
            (Some(left), Some(right)) if left == right => left.clone(),
            _ => None,
        };
        base.fonts.insert(name.clone(), value);
    }
    for name in left.buffers.keys().chain(right.buffers.keys()) {
        let mut value = left
            .buffers
            .get(name)
            .cloned()
            .unwrap_or_else(|| TextValue::unknown("missing branch buffer"));
        value.union(
            right
                .buffers
                .get(name)
                .cloned()
                .unwrap_or_else(|| TextValue::unknown("missing branch buffer")),
        );
        base.buffers.insert(name.clone(), value);
    }
}

fn contains_indexed_expr(expression: &SimpleExpr) -> bool {
    match expression {
        SimpleExpr::IndexedPath { .. } => true,
        SimpleExpr::Call { args, .. } => args.iter().any(contains_indexed_expr),
        SimpleExpr::Binary { lhs, rhs, .. } => {
            contains_indexed_expr(lhs) || contains_indexed_expr(rhs)
        }
        SimpleExpr::Condition(condition) => contains_indexed_condition(condition),
        _ => false,
    }
}

fn contains_indexed_condition(condition: &SimpleCondition) -> bool {
    match condition {
        SimpleCondition::Comparison { lhs, rhs, .. } => {
            contains_indexed_expr(lhs) || contains_indexed_expr(rhs)
        }
        SimpleCondition::Expr(expression) => contains_indexed_expr(expression),
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            contains_indexed_condition(lhs) || contains_indexed_condition(rhs)
        }
        SimpleCondition::Not(inner) => contains_indexed_condition(inner),
    }
}

fn poison_tracked_globals(env: &mut Environment) {
    for name in env.globals.clone() {
        if env.text.contains_key(&name) {
            env.text.insert(
                name.clone(),
                TextValue::unknown("global text may be mutated"),
            );
        }
        if env.buffers.contains_key(&name) {
            env.buffers.insert(
                name.clone(),
                TextValue::unknown("global text buffer may be mutated"),
            );
        }
        if env.fonts.contains_key(&name) {
            env.fonts.insert(name, None);
        }
    }
}

fn poison_loop_carried_values(env: &mut Environment) {
    for value in env.text.values_mut() {
        *value = TextValue::unknown("loop-carried text");
    }
    for value in env.buffers.values_mut() {
        *value = TextValue::unknown("loop-carried text buffer");
    }
    for value in env.fonts.values_mut() {
        *value = None;
    }
}

fn invalidate_changed_values(base: &mut Environment, changed: &Environment) {
    for (name, value) in &changed.text {
        if base.text.get(name) != Some(value) {
            base.text
                .insert(name.clone(), TextValue::unknown("loop-mutated text"));
        }
    }
    for (name, value) in &changed.buffers {
        if base.buffers.get(name) != Some(value) {
            base.buffers
                .insert(name.clone(), TextValue::unknown("loop-mutated text buffer"));
        }
    }
    for (name, value) in &changed.fonts {
        if base.fonts.get(name) != Some(value) {
            base.fonts.insert(name.clone(), None);
        }
    }
}
