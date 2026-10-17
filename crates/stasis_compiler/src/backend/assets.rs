use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::backend::compile_analysis::{ConstantValue, ResolvedExternCallSignature};
use crate::compiler::{FunctionId, FunctionMeta, SourceFile};
use crate::frontend::lexer::{lex, Token, TokenKind};
use crate::frontend::parser::{
    parse_local_declarations, parse_string_literal_text, parse_top_level_extern_functions,
    parse_top_level_functions, ParsedFunctionAnnotation, ParsedFunctionAnnotationArgumentKind,
    ParsedLocalDeclaration, ParsedParam,
};
use crate::frontend::types::{TypeCategory, TypeId, TypeTable, TYPE_ID_VOID};
use crate::ir::hir::{
    AssignTarget, ExternCallIdentity, FunctionHIR, ResolvedCallTarget, ResolvedConstantValue,
    SimpleCondition, SimpleExpr, SimpleStmt,
};

const ASSET_ANNOTATION: &str = "asset_path";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AssetLoader {
    parameter_count: usize,
    path_parameter: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum AssetCallee {
    Function(FunctionId),
    Extern(ExternCallIdentity),
}

#[derive(Debug, Clone)]
struct ResolvedAssetCall<'hir> {
    callee: AssetCallee,
    args: &'hir [SimpleExpr],
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AssetLoaderCatalog {
    by_callee: BTreeMap<AssetCallee, Vec<AssetLoader>>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SpriteRenderLoaderTargets {
    pub(crate) sprites: HashSet<ResolvedCallTarget>,
    pub(crate) sheets: HashSet<ResolvedCallTarget>,
    pub(crate) sprite_draws: HashSet<ResolvedCallTarget>,
    pub(crate) sheet_draws: HashSet<ResolvedCallTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PathOperand {
    Receiver,
    Argument(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssetReference {
    pub api: String,
    pub source_path: String,
    pub start: usize,
    pub end: usize,
    pub logical_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedAssetReference {
    #[serde(flatten)]
    pub reference: AssetReference,
    pub project_path: String,
    pub absolute_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssetDiagnostic {
    pub code: String,
    pub api: String,
    pub source_path: String,
    pub start: usize,
    pub end: usize,
    pub logical_path: Option<String>,
    pub attempted_paths: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AssetValidationResult {
    pub references: Vec<ResolvedAssetReference>,
    pub resolved_paths: BTreeSet<String>,
    pub diagnostics: Vec<AssetDiagnostic>,
}

pub(crate) fn discover_asset_references(
    files: &[SourceFile],
    functions: &[FunctionMeta],
    source_function_hirs: &BTreeMap<FunctionId, FunctionHIR>,
    loaders: &AssetLoaderCatalog,
    reachable: &BTreeSet<FunctionId>,
    constants: &BTreeMap<String, ConstantValue>,
) -> Result<Vec<AssetReference>, String> {
    let local_declarations = files
        .iter()
        .map(|file| {
            parse_local_declarations(&file.content).map_err(|error| {
                format!(
                    "failed parsing scoped bindings for asset discovery in {}: {error}",
                    file.path
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut references = Vec::new();
    for function in functions
        .iter()
        .filter(|function| reachable.contains(&function.id))
    {
        let file = files
            .get(function.file_id as usize)
            .ok_or_else(|| format!("asset discovery missing source file {}", function.file_id))?;
        let body_start = function.source_range.start as usize;
        let body_end = function.source_range.end as usize;
        let body = file.content.get(body_start..body_end).ok_or_else(|| {
            format!(
                "invalid function range for asset discovery in {}",
                file.path
            )
        })?;
        discover_function_references(
            &file.path,
            body,
            body_start,
            &function.name,
            &function.param_names,
            &local_declarations[function.file_id as usize],
            &resolved_asset_calls(function, source_function_hirs.get(&function.id)),
            &loaders,
            constants,
            &mut references,
        )?;
    }
    references.sort_by(|left, right| {
        (&left.source_path, left.start, left.end, &left.api).cmp(&(
            &right.source_path,
            right.start,
            right.end,
            &right.api,
        ))
    });
    references.dedup();
    Ok(references)
}

pub(crate) fn collect_asset_loaders(
    files: &[SourceFile],
    functions: &[FunctionMeta],
    extern_signatures: &[ResolvedExternCallSignature],
) -> Result<AssetLoaderCatalog, String> {
    let mut loaders = AssetLoaderCatalog::default();
    for (file_id, file) in files.iter().enumerate() {
        for declaration in parse_top_level_extern_functions(&file.content).map_err(|error| {
            format!(
                "failed parsing asset loader declarations in {}: {error}",
                file.path
            )
        })? {
            let metadata = asset_loader_metadata(
                &declaration.name,
                &declaration.params,
                &declaration.annotations,
            )?;
            if metadata.is_empty() {
                continue;
            }
            let function = functions.iter().find(|function| {
                function.file_id as usize == file_id
                    && function.name == declaration.name
                    && function.signature_range.start as usize <= declaration.name_range.start
                    && declaration.name_range.end <= function.signature_range.end as usize
            });
            let extern_callees = extern_signatures
                .iter()
                .filter(|signature| {
                    signature.source_path == file.path
                        && signature.source_start == declaration.name_range.start
                        && signature.source_end == declaration.name_range.end
                })
                .map(|signature| AssetCallee::Extern(signature.identity.clone()))
                .collect::<BTreeSet<_>>();
            let mut callees = extern_callees;
            if let Some(function) = function {
                callees.insert(AssetCallee::Function(function.id));
            }
            if callees.is_empty() {
                return Err(format!(
                    "asset loader declaration '{}' in {} has no resolved callee identity",
                    declaration.name, file.path
                ));
            }
            for callee in callees {
                for loader in &metadata {
                    insert_loader(&mut loaders, callee.clone(), *loader);
                }
            }
        }
        for declaration in parse_top_level_functions(&file.content).map_err(|error| {
            format!(
                "failed parsing asset wrapper declarations in {}: {error}",
                file.path
            )
        })? {
            let metadata = asset_loader_metadata(
                &declaration.name,
                &declaration.params,
                &declaration.annotations,
            )?;
            if metadata.is_empty() {
                continue;
            }
            let function = functions.iter().find(|function| {
                function.file_id as usize == file_id
                    && function.name == declaration.name
                    && function.signature_range.start as usize == declaration.signature_range.start
                    && function.signature_range.end as usize == declaration.signature_range.end
            });
            let Some(function) = function else {
                return Err(format!(
                    "asset loader function '{}' in {} has no indexed function identity",
                    declaration.name, file.path
                ));
            };
            for loader in metadata {
                insert_loader(&mut loaders, AssetCallee::Function(function.id), loader);
            }
        }
    }
    Ok(loaders)
}

fn asset_loader_metadata(
    name: &str,
    params: &[ParsedParam],
    annotations: &[ParsedFunctionAnnotation],
) -> Result<Vec<AssetLoader>, String> {
    let mut loaders = Vec::new();
    if let Some(index) = legacy_asset_parameter(name) {
        if params
            .get(index)
            .is_some_and(|parameter| parameter.type_name == "string")
        {
            loaders.push(AssetLoader {
                parameter_count: params.len(),
                path_parameter: index,
            });
        }
    }
    for annotation in annotations
        .iter()
        .filter(|annotation| annotation.name == ASSET_ANNOTATION)
    {
        if !annotation.has_parentheses || annotation.arguments.len() != 1 {
            return Err(format!(
                "@asset_path on '{name}' must name exactly one path parameter"
            ));
        }
        let argument = &annotation.arguments[0];
        if argument.kind != ParsedFunctionAnnotationArgumentKind::Identifier {
            return Err(format!("@asset_path on '{name}' must use a parameter name"));
        }
        let index = params
            .iter()
            .position(|parameter| parameter.name == argument.text)
            .ok_or_else(|| {
                format!(
                    "@asset_path on '{name}' names unknown parameter '{}'",
                    argument.text
                )
            })?;
        loaders.push(AssetLoader {
            parameter_count: params.len(),
            path_parameter: index,
        });
    }
    loaders.sort();
    loaders.dedup();
    Ok(loaders)
}

fn insert_loader(loaders: &mut AssetLoaderCatalog, callee: AssetCallee, loader: AssetLoader) {
    let entries = loaders.by_callee.entry(callee).or_default();
    entries.push(loader);
    entries.sort();
    entries.dedup();
}

pub(crate) fn sprite_render_loader_targets(
    catalog: &AssetLoaderCatalog,
    files: &[SourceFile],
    functions: &[FunctionMeta],
    types: &TypeTable,
    extern_signatures: &[ResolvedExternCallSignature],
) -> Result<SpriteRenderLoaderTargets, String> {
    let mut targets = SpriteRenderLoaderTargets::default();

    for (callee, loaders) in &catalog.by_callee {
        let params = match callee {
            AssetCallee::Function(function_id) => functions
                .iter()
                .find(|function| function.id == *function_id)
                .map(|function| function.params.as_slice()),
            AssetCallee::Extern(identity) => extern_signatures
                .iter()
                .find(|signature| signature.identity == *identity)
                .map(|signature| signature.params.as_slice()),
        };
        let Some(params) = params else {
            continue;
        };
        let valid_loader = loaders
            .iter()
            .any(|loader| loader.path_parameter == 1 && loader.parameter_count == params.len());
        if !valid_loader || params.is_empty() {
            continue;
        }

        let target = resolved_call_target(callee);
        match params.len() {
            4 if is_canonical_render_type(
                types,
                files,
                params[0],
                "Sprite",
                "/asset_tasks.stasis",
            ) || is_sprite_load_abi(types, &callee, extern_signatures, 4, "Sprite") =>
            {
                targets.sprites.insert(target);
            }
            6 if is_canonical_render_type(
                types,
                files,
                params[0],
                "SpriteSheet",
                "/graphics.stasis",
            ) || is_sprite_load_abi(types, &callee, extern_signatures, 6, "SpriteSheet") =>
            {
                targets.sheets.insert(target);
            }
            _ => {}
        }
    }

    // Compiler tests and embedding callers may declare the stable host ABI
    // directly, without importing the stdlib declaration carrying
    // @asset_path. The exact symbol and full lane signature identify that seam.
    for signature in extern_signatures {
        let callee = AssetCallee::Extern(signature.identity.clone());
        if is_sprite_load_abi(types, &callee, extern_signatures, 4, "Sprite") {
            targets.sprites.insert(resolved_call_target(&callee));
        }
        if is_sprite_load_abi(types, &callee, extern_signatures, 6, "SpriteSheet") {
            targets.sheets.insert(resolved_call_target(&callee));
        }
        if is_sprite_draw_abi(types, files, signature, "Sprite") {
            targets
                .sprite_draws
                .insert(ResolvedCallTarget::Extern(signature.identity.clone()));
        }
        if is_sprite_draw_abi(types, files, signature, "SpriteSheet") {
            targets
                .sheet_draws
                .insert(ResolvedCallTarget::Extern(signature.identity.clone()));
        }
    }

    Ok(targets)
}

fn resolved_call_target(callee: &AssetCallee) -> ResolvedCallTarget {
    match callee {
        AssetCallee::Function(function_id) => ResolvedCallTarget::Function(*function_id),
        AssetCallee::Extern(identity) => ResolvedCallTarget::Extern(identity.clone()),
    }
}

fn is_sprite_load_abi(
    types: &TypeTable,
    callee: &AssetCallee,
    extern_signatures: &[ResolvedExternCallSignature],
    parameter_count: usize,
    receiver_name: &str,
) -> bool {
    let AssetCallee::Extern(identity) = callee else {
        return false;
    };
    let Some(string_type) = types.resolve("string") else {
        return false;
    };
    let Some(i32_type) = types.resolve("i32") else {
        return false;
    };
    let Some(bool_type) = types.resolve("bool") else {
        return false;
    };
    let Some(signature) = extern_signatures.iter().find(|signature| {
        signature.identity == *identity
            && signature.symbol == "stasis_jit_sprite_load_from"
            && signature.source_path == identity.source_path
    }) else {
        return false;
    };
    signature.params.len() == parameter_count
        && signature.params.first().is_some_and(|receiver| {
            types.type_info(*receiver).is_some_and(|info| {
                info.category == TypeCategory::Named && info.name == receiver_name
            })
        })
        && signature.params.get(1) == Some(&string_type)
        && signature.params[2..].iter().all(|param| *param == i32_type)
        && signature.return_type == bool_type
}

fn is_sprite_draw_abi(
    types: &TypeTable,
    files: &[SourceFile],
    signature: &ResolvedExternCallSignature,
    receiver_name: &str,
) -> bool {
    let (expected_name, expected_symbol, expected_path_suffix) = match receiver_name {
        "Sprite" => ("draw", "stasis_jit_sprite_draw", "/graphics.stasis"),
        "SpriteSheet" => (
            "draw_frame",
            "stasis_jit_sprite_draw_frame",
            "/graphics.stasis",
        ),
        _ => return false,
    };
    if !signature.trusted_graphics_source
        || signature.name != expected_name
        || signature.identity.name != expected_name
        || signature.identity.source_path != signature.source_path
        || signature.symbol != expected_symbol
        || !signature
            .identity
            .symbol_candidates
            .iter()
            .any(|candidate| candidate == &signature.symbol)
        || signature.return_type != TYPE_ID_VOID
    {
        return false;
    }

    let Some(receiver_type) = signature.params.first().copied() else {
        return false;
    };
    let Some(receiver_info) = types.type_info(receiver_type) else {
        return false;
    };
    if receiver_info.category != TypeCategory::Named
        || !is_trusted_render_receiver_type(
            types,
            files,
            receiver_type,
            receiver_name,
            expected_path_suffix,
            &signature.source_path,
        )
    {
        return false;
    }

    let Some(f32_type) = types.resolve("f32") else {
        return false;
    };
    let Some(i32_type) = types.resolve("i32") else {
        return false;
    };
    let expected_params = if receiver_name == "Sprite" {
        vec![receiver_type, f32_type, f32_type, i32_type, i32_type]
    } else {
        vec![
            receiver_type,
            i32_type,
            f32_type,
            f32_type,
            i32_type,
            i32_type,
        ]
    };
    signature.params == expected_params
}

fn is_trusted_render_receiver_type(
    types: &TypeTable,
    files: &[SourceFile],
    type_id: TypeId,
    expected_name: &str,
    expected_path_suffix: &str,
    signature_source_path: &str,
) -> bool {
    if is_canonical_render_type(types, files, type_id, expected_name, expected_path_suffix) {
        return true;
    }

    let Some(origin) = types.source_type_origin(type_id) else {
        return false;
    };
    let normalize = |path: &str| path.replace('\\', "/");
    origin.name == expected_name
        && normalize(&origin.path) == normalize(signature_source_path)
        && crate::frontend::module_graph::is_explicit_graphics_test_seam_path(signature_source_path)
}

fn is_canonical_render_type(
    types: &TypeTable,
    files: &[SourceFile],
    type_id: TypeId,
    expected_name: &str,
    expected_path_suffix: &str,
) -> bool {
    let Some(origin) = types.source_type_origin(type_id) else {
        return false;
    };
    if origin.name != expected_name
        || !origin
            .path
            .replace('\\', "/")
            .ends_with(expected_path_suffix)
    {
        return false;
    }
    files
        .iter()
        .find(|file| file.path == origin.path)
        .is_some_and(|file| {
            crate::frontend::module_graph::is_recognized_graphics_implementation_source(
                &origin.path,
                &file.content,
            )
        })
}

fn legacy_asset_parameter(name: &str) -> Option<usize> {
    match name {
        "audio_load_effect" | "audio_load_music" | "audio_load_wav" | "gfx_load_sprite"
        | "load_font" => Some(0),
        _ => None,
    }
}

fn discover_function_references(
    source_path: &str,
    body: &str,
    body_offset: usize,
    function_name: &str,
    parameter_names: &[String],
    local_declarations: &[ParsedLocalDeclaration],
    resolved_calls: &BTreeMap<(usize, usize), ResolvedAssetCall<'_>>,
    loaders: &AssetLoaderCatalog,
    constants: &BTreeMap<String, ConstantValue>,
    out: &mut Vec<AssetReference>,
) -> Result<(), String> {
    let tokens = lex(body)?;
    for index in 0..tokens.len() {
        let token = tokens[index];
        if token.kind != TokenKind::Identifier {
            continue;
        }
        let name = &body[token.start..token.end];
        let Some(resolved_call) = resolved_calls.get(&(token.start, token.end)) else {
            continue;
        };
        let Some(loader_signatures) = loaders.by_callee.get(&resolved_call.callee) else {
            continue;
        };
        let dotted_call = index > 0
            && token_text_is(body, tokens[index - 1], ".")
            && tokens
                .get(index + 1)
                .is_some_and(|next| next.kind == TokenKind::LParen);
        let bare_call = !dotted_call
            && tokens
                .get(index + 1)
                .is_some_and(|next| next.kind == TokenKind::LParen);
        if !dotted_call && !bare_call {
            continue;
        }
        let open = index + 1;
        let (arguments, _) = call_argument_ranges(&tokens, open)?;
        let path_operands = loader_signatures
            .iter()
            .filter_map(|loader| {
                if dotted_call && loader.parameter_count == arguments.len() + 1 {
                    Some(if loader.path_parameter == 0 {
                        PathOperand::Receiver
                    } else {
                        PathOperand::Argument(loader.path_parameter - 1)
                    })
                } else if loader.parameter_count == arguments.len() {
                    Some(PathOperand::Argument(loader.path_parameter))
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();
        let mut path_operands = path_operands.into_iter();
        let Some(path_operand) = path_operands.next() else {
            continue;
        };
        if path_operands.next().is_some() {
            return Err(format!(
                "ambiguous asset path metadata for call target '{name}' with {} argument(s)",
                arguments.len()
            ));
        }
        let range = match path_operand {
            PathOperand::Receiver => receiver_expression_range(body, &tokens, index)?,
            PathOperand::Argument(explicit_index) => {
                let Some(range) = arguments.get(explicit_index).cloned() else {
                    continue;
                };
                range
            }
        };
        let expression = body[range.clone()].trim();
        let leading = body[range.clone()].len() - body[range.clone()].trim_start().len();
        let start = range.start + leading;
        let end = start + expression.len();
        let expression_start = body_offset + start;
        let logical_path = if let Some(path) =
            resolved_string_operand(resolved_call, path_operand, arguments.len())
        {
            Some(path.to_string())
        } else {
            static_string(expression, constants, |name| {
                parameter_names.iter().any(|parameter| parameter == name)
                    || local_declarations.iter().any(|declaration| {
                        declaration.function_name == function_name
                            && declaration.name == name
                            && declaration.visibility_range.contains(&expression_start)
                    })
            })?
        };
        out.push(AssetReference {
            api: name.to_string(),
            source_path: source_path.to_string(),
            start: body_offset + start,
            end: body_offset + end,
            logical_path,
        });
    }
    Ok(())
}

fn resolved_asset_calls<'hir>(
    function: &FunctionMeta,
    hir: Option<&'hir FunctionHIR>,
) -> BTreeMap<(usize, usize), ResolvedAssetCall<'hir>> {
    let mut calls = BTreeMap::new();
    let Some(hir) = hir else {
        return calls;
    };
    for statement in &hir.statements {
        collect_calls_in_statement(statement, &mut calls);
    }
    // HIR call spans are relative to the cached function body. Validate that
    // the span still indexes the same function range before the scanner uses it.
    let body_len = function
        .source_range
        .end
        .saturating_sub(function.source_range.start) as usize;
    calls.retain(|(start, end), _| *start <= *end && *end <= body_len);
    calls
}

fn collect_calls_in_statement<'hir>(
    statement: &'hir SimpleStmt,
    calls: &mut BTreeMap<(usize, usize), ResolvedAssetCall<'hir>>,
) {
    match statement {
        SimpleStmt::Let { expression, .. }
        | SimpleStmt::Expr(expression)
        | SimpleStmt::Return(expression) => collect_calls_in_expression(expression, calls),
        SimpleStmt::Assign {
            target, expression, ..
        } => {
            collect_calls_in_assignment_target(target, calls);
            collect_calls_in_expression(expression, calls);
        }
        SimpleStmt::Convert { target, source, .. } => {
            collect_calls_in_assignment_target(target, calls);
            collect_calls_in_expression(source, calls);
        }
        SimpleStmt::If {
            condition,
            then_statements,
            else_statements,
        } => {
            collect_calls_in_condition(condition, calls);
            for statement in then_statements {
                collect_calls_in_statement(statement, calls);
            }
            if let Some(statements) = else_statements {
                for statement in statements {
                    collect_calls_in_statement(statement, calls);
                }
            }
        }
        SimpleStmt::For {
            init,
            condition,
            step,
            body_statements,
        } => {
            collect_calls_in_statement(init, calls);
            collect_calls_in_condition(condition, calls);
            collect_calls_in_statement(step, calls);
            for statement in body_statements {
                collect_calls_in_statement(statement, calls);
            }
        }
        SimpleStmt::Foreach {
            body_statements, ..
        } => {
            for statement in body_statements {
                collect_calls_in_statement(statement, calls);
            }
        }
        SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
    }
}

fn collect_calls_in_assignment_target<'hir>(
    target: &'hir AssignTarget,
    calls: &mut BTreeMap<(usize, usize), ResolvedAssetCall<'hir>>,
) {
    if let AssignTarget::IndexedPath {
        index,
        nested_index,
        ..
    } = target
    {
        collect_calls_in_expression(index, calls);
        if let Some(nested_index) = nested_index {
            collect_calls_in_expression(nested_index, calls);
        }
    }
}

fn collect_calls_in_condition<'hir>(
    condition: &'hir SimpleCondition,
    calls: &mut BTreeMap<(usize, usize), ResolvedAssetCall<'hir>>,
) {
    match condition {
        SimpleCondition::Comparison { lhs, rhs, .. } => {
            collect_calls_in_expression(lhs, calls);
            collect_calls_in_expression(rhs, calls);
        }
        SimpleCondition::Expr(expression) => collect_calls_in_expression(expression, calls),
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            collect_calls_in_condition(lhs, calls);
            collect_calls_in_condition(rhs, calls);
        }
        SimpleCondition::Not(inner) => collect_calls_in_condition(inner, calls),
    }
}

fn collect_calls_in_expression<'hir>(
    expression: &'hir SimpleExpr,
    calls: &mut BTreeMap<(usize, usize), ResolvedAssetCall<'hir>>,
) {
    match expression {
        SimpleExpr::Condition(condition) => collect_calls_in_condition(condition, calls),
        SimpleExpr::IndexedPath {
            index,
            nested_index,
            ..
        } => {
            collect_calls_in_expression(index, calls);
            if let Some(nested_index) = nested_index {
                collect_calls_in_expression(nested_index, calls);
            }
        }
        SimpleExpr::Call {
            args,
            source_span,
            resolved_target,
            ..
        } => {
            if let (Some(source_span), Some(resolved_target)) = (source_span, resolved_target) {
                let callee = match resolved_target {
                    ResolvedCallTarget::Function(function) => AssetCallee::Function(*function),
                    ResolvedCallTarget::Extern(identity) => AssetCallee::Extern(identity.clone()),
                };
                calls.insert(
                    (source_span.start as usize, source_span.end as usize),
                    ResolvedAssetCall { callee, args },
                );
            }
            for argument in args {
                collect_calls_in_expression(argument, calls);
            }
        }
        SimpleExpr::Unary { operand, .. } => collect_calls_in_expression(operand, calls),
        SimpleExpr::Binary { lhs, rhs, .. } => {
            collect_calls_in_expression(lhs, calls);
            collect_calls_in_expression(rhs, calls);
        }
        SimpleExpr::DefaultValue(_)
        | SimpleExpr::Int(_)
        | SimpleExpr::Float(_)
        | SimpleExpr::Bool(_)
        | SimpleExpr::StringLiteral(_)
        | SimpleExpr::Identifier(_)
        | SimpleExpr::ResolvedConstant { .. } => {}
    }
}

fn resolved_string_operand<'hir>(
    call: &ResolvedAssetCall<'hir>,
    path_operand: PathOperand,
    source_argument_count: usize,
) -> Option<&'hir str> {
    let implicit_argument_count = call.args.len().checked_sub(source_argument_count)?;
    if implicit_argument_count > 1 {
        return None;
    }
    let index = match path_operand {
        PathOperand::Receiver if implicit_argument_count == 1 => 0,
        PathOperand::Receiver => return None,
        PathOperand::Argument(index) => index.checked_add(implicit_argument_count)?,
    };
    match call.args.get(index)? {
        SimpleExpr::ResolvedConstant {
            value: ResolvedConstantValue::String(value),
            ..
        } => Some(value),
        _ => None,
    }
}

fn receiver_expression_range(
    source: &str,
    tokens: &[Token],
    method_index: usize,
) -> Result<Range<usize>, String> {
    let dot_index = method_index
        .checked_sub(1)
        .ok_or_else(|| "asset receiver is missing its separator".to_string())?;
    let receiver_index = dot_index
        .checked_sub(1)
        .ok_or_else(|| "asset receiver expression is missing".to_string())?;
    let start_index = postfix_expression_start(source, tokens, receiver_index)?;
    Ok(tokens[start_index].start..tokens[dot_index].start)
}

fn postfix_expression_start(
    source: &str,
    tokens: &[Token],
    end_index: usize,
) -> Result<usize, String> {
    let token = tokens[end_index];
    let mut start = match token.kind {
        TokenKind::Identifier | TokenKind::StringLiteral | TokenKind::Integer => end_index,
        TokenKind::RParen => {
            let open = matching_delimiter(source, tokens, end_index, "(", ")")?;
            if open > 0 && can_end_receiver_callee(source, tokens[open - 1]) {
                postfix_expression_start(source, tokens, open - 1)?
            } else {
                open
            }
        }
        TokenKind::Other if token_text_is(source, token, "]") => {
            let open = matching_delimiter(source, tokens, end_index, "[", "]")?;
            let base = open
                .checked_sub(1)
                .ok_or_else(|| "asset receiver index is missing its base expression".to_string())?;
            postfix_expression_start(source, tokens, base)?
        }
        _ => {
            return Err("asset receiver has an unsupported expression shape".to_string());
        }
    };
    while start >= 2 && token_text_is(source, tokens[start - 1], ".") {
        start = postfix_expression_start(source, tokens, start - 2)?;
    }
    Ok(start)
}

fn can_end_receiver_callee(source: &str, token: Token) -> bool {
    match token.kind {
        TokenKind::Identifier => !matches!(
            &source[token.start..token.end],
            "return" | "if" | "else" | "while" | "for" | "foreach" | "let"
        ),
        TokenKind::RParen => true,
        TokenKind::Other => token_text_is(source, token, "]"),
        _ => false,
    }
}

fn matching_delimiter(
    source: &str,
    tokens: &[Token],
    close_index: usize,
    open: &str,
    close: &str,
) -> Result<usize, String> {
    let mut depth = 0usize;
    for index in (0..=close_index).rev() {
        if token_text_is(source, tokens[index], close) {
            depth += 1;
        } else if token_text_is(source, tokens[index], open) {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Ok(index);
            }
        }
    }
    Err(format!("unbalanced '{open}{close}' in asset receiver"))
}

fn call_argument_ranges(
    tokens: &[Token],
    open: usize,
) -> Result<(Vec<Range<usize>>, usize), String> {
    let open_token = tokens
        .get(open)
        .ok_or_else(|| "missing asset call opening parenthesis".to_string())?;
    let mut depth = 1usize;
    let mut argument_start = open_token.end;
    let mut arguments = Vec::new();
    let mut cursor = open + 1;
    while let Some(token) = tokens.get(cursor).copied() {
        match token.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if token.start > argument_start {
                        arguments.push(argument_start..token.start);
                    }
                    return Ok((arguments, cursor));
                }
            }
            TokenKind::Comma if depth == 1 => {
                arguments.push(argument_start..token.start);
                argument_start = token.end;
            }
            _ => {}
        }
        cursor += 1;
    }
    Err("unterminated asset loader call".to_string())
}

fn static_string(
    expression: &str,
    constants: &BTreeMap<String, ConstantValue>,
    is_scoped_binding: impl Fn(&str) -> bool,
) -> Result<Option<String>, String> {
    let expression = strip_parenthesized_expression(expression)?;
    let tokens = lex(expression)?;
    let tokens = tokens
        .into_iter()
        .filter(|token| token.kind != TokenKind::Eof)
        .collect::<Vec<_>>();
    if tokens.len() != 1 {
        return Ok(None);
    }
    let token = tokens[0];
    match token.kind {
        TokenKind::StringLiteral => {
            parse_string_literal_text(&expression[token.start..token.end]).map(Some)
        }
        TokenKind::Identifier => {
            let name = &expression[token.start..token.end];
            if is_scoped_binding(name) {
                return Ok(None);
            }
            Ok(constants.get(name).and_then(|value| match value {
                ConstantValue::String { value, .. } => Some(value.clone()),
                _ => None,
            }))
        }
        _ => Ok(None),
    }
}

fn strip_parenthesized_expression(mut expression: &str) -> Result<&str, String> {
    loop {
        expression = expression.trim();
        let tokens = lex(expression)?
            .into_iter()
            .filter(|token| token.kind != TokenKind::Eof)
            .collect::<Vec<_>>();
        if tokens.len() < 2
            || tokens[0].kind != TokenKind::LParen
            || tokens
                .last()
                .is_none_or(|token| token.kind != TokenKind::RParen)
            || matching_delimiter(expression, &tokens, tokens.len() - 1, "(", ")")? != 0
        {
            return Ok(expression);
        }
        expression = &expression[tokens[0].end..tokens[tokens.len() - 1].start];
    }
}

fn token_text_is(source: &str, token: Token, expected: &str) -> bool {
    source.get(token.start..token.end) == Some(expected)
}

pub fn validate_asset_references(
    project_root: &Path,
    source_base_dirs: &[PathBuf],
    references: &[AssetReference],
    manifest_paths: Option<&BTreeSet<String>>,
    dynamic_paths: &BTreeSet<String>,
) -> AssetValidationResult {
    let mut result = AssetValidationResult::default();
    let project_root = match project_root.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            for reference in references {
                result.diagnostics.push(diagnostic(
                    reference,
                    "asset_project_root_unavailable",
                    Vec::new(),
                    format!("project root is unavailable: {error}"),
                ));
            }
            return result;
        }
    };
    let asset_root = project_root
        .join("assets")
        .canonicalize()
        .unwrap_or_else(|_| project_root.join("assets"));
    for reference in references {
        let Some(logical_path) = reference.logical_path.as_deref() else {
            if dynamic_paths.is_empty() {
                result.diagnostics.push(diagnostic(
                    reference,
                    "asset_dynamic_path_undeclared",
                    Vec::new(),
                    "dynamic asset paths require assets/manifest.json dynamic_assets declarations"
                        .to_string(),
                ));
            } else {
                for path in dynamic_paths {
                    resolve_one(
                        &project_root,
                        &asset_root,
                        source_base_dirs,
                        reference,
                        path,
                        manifest_paths,
                        &mut result,
                    );
                }
            }
            continue;
        };
        resolve_one(
            &project_root,
            &asset_root,
            source_base_dirs,
            reference,
            logical_path,
            manifest_paths,
            &mut result,
        );
    }
    result.references.sort_by(|left, right| {
        (
            &left.reference.source_path,
            left.reference.start,
            &left.project_path,
        )
            .cmp(&(
                &right.reference.source_path,
                right.reference.start,
                &right.project_path,
            ))
    });
    result.references.dedup();
    result.diagnostics.sort_by(|left, right| {
        (&left.source_path, left.start, &left.code).cmp(&(
            &right.source_path,
            right.start,
            &right.code,
        ))
    });
    result
}

fn resolve_one(
    project_root: &Path,
    asset_root: &Path,
    source_base_dirs: &[PathBuf],
    reference: &AssetReference,
    logical_path: &str,
    manifest_paths: Option<&BTreeSet<String>>,
    result: &mut AssetValidationResult,
) {
    if logical_path.contains('\\') {
        result.diagnostics.push(diagnostic(
            reference,
            "asset_path_nonportable_separator",
            vec![logical_path.to_string()],
            "asset paths must use forward slashes; backslash separators are not portable"
                .to_string(),
        ));
        return;
    }
    if looks_absolute_or_uri(logical_path) && !is_virtual_asset_path(logical_path) {
        result.diagnostics.push(diagnostic(
            reference,
            "asset_path_absolute_or_uri",
            vec![logical_path.to_string()],
            "asset paths must be relative filesystem paths or use the /assets/... project-root spelling".to_string(),
        ));
        return;
    }
    let rooted_project_path = is_virtual_asset_path(logical_path)
        .then(|| logical_path.strip_prefix('/').unwrap_or(logical_path));
    let source = PathBuf::from(&reference.source_path);
    let declaring_dir = if source.is_absolute() {
        source
            .parent()
            .unwrap_or(project_root)
            .canonicalize()
            .unwrap_or_else(|_| source.parent().unwrap_or(project_root).to_path_buf())
    } else {
        project_root
            .join(source)
            .parent()
            .unwrap_or(project_root)
            .to_path_buf()
    };
    let mut candidates = Vec::new();
    if let Some(rooted_project_path) = rooted_project_path {
        // `/assets/...` is a virtual project-root spelling. It must not depend on
        // the declaring source module or any caller-provided source base.
        candidates.push(project_root.join(rooted_project_path));
    } else {
        for source_base_dir in source_base_dirs {
            let source_base_dir = if source_base_dir.is_absolute() {
                source_base_dir.clone()
            } else {
                project_root.join(source_base_dir)
            };
            let candidate = source_base_dir.join(logical_path);
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
        for candidate in [
            declaring_dir.join(logical_path),
            project_root.join(logical_path),
        ] {
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    let attempted = candidates
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let mut case_mismatch = false;
    for candidate in candidates {
        let Some(normalized) = normalize_path(&candidate) else {
            continue;
        };
        if !normalized.starts_with(asset_root) {
            continue;
        }
        match exact_case_file(project_root, &normalized) {
            Ok(true) => {
                let Ok(canonical) = normalized.canonicalize() else {
                    continue;
                };
                if !canonical.starts_with(asset_root) || !canonical.is_file() {
                    continue;
                }
                let Ok(relative) = normalized.strip_prefix(project_root) else {
                    continue;
                };
                let project_path = relative.to_string_lossy().replace('\\', "/");
                if manifest_paths.is_some_and(|paths| !paths.contains(&project_path)) {
                    result.diagnostics.push(diagnostic(
                        reference,
                        "asset_not_declared",
                        attempted,
                        format!("resolved asset '{project_path}' is not declared in the manifest"),
                    ));
                    return;
                }
                result.resolved_paths.insert(project_path.clone());
                result.references.push(ResolvedAssetReference {
                    reference: reference.clone(),
                    project_path,
                    absolute_path: canonical,
                });
                return;
            }
            Ok(false) => {}
            Err(()) => case_mismatch = true,
        }
    }
    result.diagnostics.push(diagnostic(
        reference,
        if case_mismatch {
            "asset_path_case_mismatch"
        } else {
            "asset_path_missing_or_outside_boundary"
        },
        attempted,
        if case_mismatch {
            "asset path casing does not exactly match disk".to_string()
        } else {
            "asset does not exist as a regular file inside the project assets directory".to_string()
        },
    ));
}

fn diagnostic(
    reference: &AssetReference,
    code: &str,
    attempted_paths: Vec<String>,
    reason: String,
) -> AssetDiagnostic {
    AssetDiagnostic {
        code: code.to_string(),
        api: reference.api.clone(),
        source_path: reference.source_path.clone(),
        start: reference.start,
        end: reference.end,
        logical_path: reference.logical_path.clone(),
        attempted_paths,
        reason,
    }
}

fn looks_absolute_or_uri(path: &str) -> bool {
    path.contains("://")
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.as_bytes().get(1).is_some_and(|byte| *byte == b':')
}

fn is_virtual_asset_path(path: &str) -> bool {
    path == "/assets" || path.starts_with("/assets/")
}

fn normalize_path(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::Normal(segment) => out.push(segment),
        }
    }
    Some(out)
}

fn exact_case_file(project_root: &Path, target: &Path) -> Result<bool, ()> {
    let relative = target.strip_prefix(project_root).map_err(|_| ())?;
    let mut current = project_root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(expected) = component else {
            return Ok(false);
        };
        let entries = fs::read_dir(&current).map_err(|_| ())?;
        let mut insensitive = false;
        let mut exact = false;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy() == expected.to_string_lossy() {
                exact = true;
                break;
            }
            if name
                .to_string_lossy()
                .eq_ignore_ascii_case(&expected.to_string_lossy())
            {
                insensitive = true;
            }
        }
        if !exact {
            return if insensitive { Err(()) } else { Ok(false) };
        }
        current.push(expected);
    }
    Ok(current.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::aot::AotProcess;
    use crate::backend::jit::JitProcess;

    #[test]
    fn custom_loader_metadata_uses_the_named_parameter() {
        let source = "function @asset_path(file) @extern(\"custom_load\") custom_load(self: i32, file: string): i32;";
        let declaration = parse_top_level_extern_functions(source)
            .expect("parse external loader")
            .remove(0);
        assert_eq!(
            asset_loader_metadata(
                &declaration.name,
                &declaration.params,
                &declaration.annotations,
            )
            .expect("loader metadata"),
            vec![AssetLoader {
                parameter_count: 2,
                path_parameter: 1,
            }],
        );
    }

    #[test]
    fn sprite_abi_requires_the_selected_physical_symbol() {
        let mut types = TypeTable::new();
        let sprite_type = types.resolve_or_intern("Sprite").expect("Sprite type");
        let string_type = types.resolve_or_intern("string").expect("string type");
        let i32_type = types.resolve("i32").expect("i32 type");
        let bool_type = types.resolve("bool").expect("bool type");
        let identity = ExternCallIdentity {
            source_path: "game/graphics.stasis".to_string(),
            name: "load_from".to_string(),
            symbol_candidates: vec![
                "custom_sprite_load_from".to_string(),
                "stasis_jit_sprite_load_from".to_string(),
            ],
            declared_params: vec![
                "Sprite".to_string(),
                "string".to_string(),
                "i32".to_string(),
                "i32".to_string(),
            ],
            declared_return_type: "bool".to_string(),
        };
        let callee = AssetCallee::Extern(identity.clone());
        let mut selected = ResolvedExternCallSignature {
            identity: identity.clone(),
            name: identity.name.clone(),
            symbol: "custom_sprite_load_from".to_string(),
            source_path: identity.source_path.clone(),
            trusted_graphics_source: false,
            source_start: 0,
            source_end: 0,
            params: vec![sprite_type, string_type, i32_type, i32_type],
            return_type: bool_type,
        };

        assert!(!is_sprite_load_abi(
            &types,
            &callee,
            std::slice::from_ref(&selected),
            4,
            "Sprite",
        ));

        selected.symbol = "stasis_jit_sprite_load_from".to_string();
        assert!(is_sprite_load_abi(
            &types,
            &callee,
            &[selected],
            4,
            "Sprite",
        ));
    }

    #[test]
    fn direct_draw_targets_require_trusted_source_exact_lanes_and_bound_symbols() {
        use crate::frontend::types::SourceTypeOrigin;

        let source_path = "tests/stasis/hot_render.stasis";
        let mut types = TypeTable::new();
        let sprite_type = types.resolve_or_intern("Sprite").expect("Sprite type");
        let sheet_type = types
            .resolve_or_intern("SpriteSheet")
            .expect("SpriteSheet type");
        let f32_type = types.resolve("f32").expect("f32 type");
        let i32_type = types.resolve("i32").expect("i32 type");
        types.replace_source_type_origins(BTreeMap::from([
            (
                "Sprite".to_string(),
                SourceTypeOrigin {
                    path: source_path.to_string(),
                    name: "Sprite".to_string(),
                },
            ),
            (
                "SpriteSheet".to_string(),
                SourceTypeOrigin {
                    path: source_path.to_string(),
                    name: "SpriteSheet".to_string(),
                },
            ),
        ]));

        let make_signature =
            |name: &str, symbol: &str, declared_params: &[&str], params: Vec<TypeId>| {
                let identity = ExternCallIdentity {
                    source_path: source_path.to_string(),
                    name: name.to_string(),
                    symbol_candidates: vec![symbol.to_string()],
                    declared_params: declared_params
                        .iter()
                        .map(|param| param.to_string())
                        .collect(),
                    declared_return_type: "void".to_string(),
                };
                ResolvedExternCallSignature {
                    identity,
                    name: name.to_string(),
                    symbol: symbol.to_string(),
                    source_path: source_path.to_string(),
                    trusted_graphics_source: true,
                    source_start: 0,
                    source_end: name.len(),
                    params,
                    return_type: TYPE_ID_VOID,
                }
            };
        let sprite_draw = make_signature(
            "draw",
            "stasis_jit_sprite_draw",
            &["Sprite", "f32", "f32", "i32", "i32"],
            vec![sprite_type, f32_type, f32_type, i32_type, i32_type],
        );
        let sheet_draw = make_signature(
            "draw_frame",
            "stasis_jit_sprite_draw_frame",
            &["SpriteSheet", "i32", "f32", "f32", "i32", "i32"],
            vec![sheet_type, i32_type, f32_type, f32_type, i32_type, i32_type],
        );
        let mut wrong_selected_symbol = sprite_draw.clone();
        wrong_selected_symbol.identity.symbol_candidates = vec![
            "custom_sprite_draw".to_string(),
            "stasis_jit_sprite_draw".to_string(),
        ];
        wrong_selected_symbol.symbol = "custom_sprite_draw".to_string();
        let mut untrusted_declaration = sprite_draw.clone();
        untrusted_declaration.trusted_graphics_source = false;
        let mut wrong_lane = sprite_draw.clone();
        wrong_lane.params[1] = i32_type;
        wrong_lane.identity.declared_params[1] = "i32".to_string();

        let signatures = vec![
            sprite_draw.clone(),
            sheet_draw.clone(),
            wrong_selected_symbol,
            untrusted_declaration,
            wrong_lane,
        ];
        let targets = sprite_render_loader_targets(
            &AssetLoaderCatalog::default(),
            &[],
            &[],
            &types,
            &signatures,
        )
        .expect("render target catalog");

        assert!(targets
            .sprite_draws
            .contains(&ResolvedCallTarget::Extern(sprite_draw.identity)));
        assert!(targets
            .sheet_draws
            .contains(&ResolvedCallTarget::Extern(sheet_draw.identity)));
        assert_eq!(targets.sprite_draws.len(), 1);
        assert_eq!(targets.sheet_draws.len(), 1);
    }

    #[test]
    fn built_in_wrapper_metadata_uses_the_path_parameter() {
        let source = concat!(
            "struct Sprite { handle: i32; width: i32; height: i32; }\n",
            "struct SpriteSheet { handle: i32; }\n",
            "function @asset_path(path) load_from(self: Sprite, path: string, width: i32, height: i32): bool { return width > 0 && height > 0; }\n",
            "function @asset_path(path) load_from(self: SpriteSheet, path: string, columns: i32, rows: i32, cell_width: i32, cell_height: i32): bool { return columns > 0 && rows > 0 && cell_width > 0 && cell_height > 0; }",
        );
        let declarations = parse_top_level_functions(source).expect("parse wrappers");
        let metadata = declarations
            .iter()
            .map(|declaration| {
                asset_loader_metadata(
                    &declaration.name,
                    &declaration.params,
                    &declaration.annotations,
                )
                .expect("wrapper metadata")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            metadata,
            vec![
                vec![AssetLoader {
                    parameter_count: 4,
                    path_parameter: 1,
                },],
                vec![AssetLoader {
                    parameter_count: 6,
                    path_parameter: 1,
                },],
            ]
        );
    }

    #[test]
    fn resolved_load_from_overloads_exclude_unannotated_same_arity_calls() {
        let source = r#"
struct Sprite { loaded: bool; }
struct SpriteSheet { loaded: bool; }
struct Custom { loaded: bool; }
function @asset_path(path) @extern("stasis_jit_sprite_load_from") load_from(self: Sprite, path: string, width: i32, height: i32): bool;
function @asset_path(path) load_from(self: SpriteSheet, path: string, columns: i32, rows: i32, cell_width: i32, cell_height: i32): bool {
    return columns > 0 && rows > 0 && cell_width > 0 && cell_height > 0;
}
function load_from(self: Custom, path: string, width: i32, height: i32): bool {
    return width > 0 && height > 0;
}
function path_for(width: i32): string { return "assets/nested.png"; }
global sprite: Sprite;
global sheet: SpriteSheet;
global custom: Custom;
global sprites: Sprite[1];
function main(): i32 {
    load_from(sprite, "assets/sprite.png", 32, 24);
    sprite.load_from(path_for(32), 32, 24);
    sheet.load_from("assets/sheet.png", 2, 2, 16, 16);
    load_from(sheet, "assets/sheet_function.png", 2, 2, 16, 16);
    sprites[0].load_from("assets/indexed.png", 32, 24);
    load_from(custom, "assets/custom_function.png", 32, 24);
    custom.load_from("assets/custom_receiver.png", 32, 24);
    return 0;
}
"#;
        let mut jit = JitProcess::new();
        let path = "tests/stasis/assets/load_from_overloads.stasis";
        jit.upsert_file(path, source);
        jit.compile().expect("compile typed loader overloads");
        let references = jit.program_snapshot().expect("snapshot").asset_references();

        assert_eq!(references.len(), 5);
        assert!(references
            .iter()
            .all(|reference| reference.api == "load_from"));
        let sprite = references
            .iter()
            .find(|reference| reference.logical_path.as_deref() == Some("assets/sprite.png"))
            .expect("function-form Sprite loader reference");
        let sheet = references
            .iter()
            .find(|reference| reference.logical_path.as_deref() == Some("assets/sheet.png"))
            .expect("receiver-form SpriteSheet loader reference");
        let sheet_function = references
            .iter()
            .find(|reference| {
                reference.logical_path.as_deref() == Some("assets/sheet_function.png")
            })
            .expect("function-form SpriteSheet loader reference");
        let indexed = references
            .iter()
            .find(|reference| reference.logical_path.as_deref() == Some("assets/indexed.png"))
            .expect("indexed Sprite receiver reference");
        let nested = references
            .iter()
            .find(|reference| reference.logical_path.is_none())
            .expect("nested dynamic path reference");
        assert_eq!(sprite.logical_path.as_deref(), Some("assets/sprite.png"));
        assert_eq!(sheet.logical_path.as_deref(), Some("assets/sheet.png"));
        assert_eq!(
            sheet_function.logical_path.as_deref(),
            Some("assets/sheet_function.png")
        );
        assert_eq!(
            &source[indexed.start..indexed.end],
            "\"assets/indexed.png\""
        );
        assert_eq!(&source[nested.start..nested.end], "path_for(32)");
        assert!(references.iter().all(|reference| {
            !matches!(
                reference.logical_path.as_deref(),
                Some("assets/custom_function.png" | "assets/custom_receiver.png")
            )
        }));

        let old_paths = references
            .iter()
            .map(|reference| reference.logical_path.clone())
            .collect::<Vec<_>>();
        let moved_source = format!(
            "// shift the function body ranges while keeping the cached HIR bodies unchanged\nfunction earlier(): i32 {{ return 1; }}\n{source}"
        );
        jit.upsert_file(path, &moved_source);
        jit.compile().expect("compile after source prefix movement");
        let moved_references = jit
            .program_snapshot()
            .expect("moved snapshot")
            .asset_references();
        assert_eq!(
            moved_references
                .iter()
                .map(|reference| reference.logical_path.clone())
                .collect::<Vec<_>>(),
            old_paths
        );
        assert!(moved_references.iter().all(|reference| {
            let expression = &moved_source[reference.start..reference.end];
            match reference.logical_path.as_deref() {
                Some("assets/sprite.png") => expression == "\"assets/sprite.png\"",
                Some("assets/sheet.png") => expression == "\"assets/sheet.png\"",
                Some("assets/sheet_function.png") => expression == "\"assets/sheet_function.png\"",
                Some("assets/indexed.png") => expression == "\"assets/indexed.png\"",
                None => expression == "path_for(32)",
                _ => false,
            }
        }));

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let project_root = std::env::temp_dir().join(format!("stasis_sheet_asset_{stamp}"));
        fs::create_dir_all(project_root.join("assets")).expect("asset directory");
        fs::write(project_root.join("assets/sheet.png"), "sheet").expect("sheet asset");
        let sheet_reference = moved_references
            .iter()
            .find(|reference| reference.logical_path.as_deref() == Some("assets/sheet.png"))
            .expect("SpriteSheet package reference");
        let packaged = validate_asset_references(
            &project_root,
            &[],
            std::slice::from_ref(sheet_reference),
            Some(&BTreeSet::from(["assets/sheet.png".to_string()])),
            &BTreeSet::new(),
        );
        assert_eq!(packaged.references.len(), 1);
        assert_eq!(packaged.references[0].project_path, "assets/sheet.png");
        fs::remove_dir_all(project_root).ok();
    }

    #[test]
    fn snapshot_discovers_imported_group_asset_path() {
        let main = r#"
import "font_paths.stasis";
extern function @asset_path(path) load_font(path: string, size: i32): i32;
function main(): i32 {
    font_paths.ASSETS.FONT_PATH.load_font(16);
    return load_font(font_paths.ASSETS.FONT_PATH, 16);
}
"#;
        let font_paths = r#"
const ASSETS {
    FONT_PATH: string = "assets/ui.ttf";
}
"#;
        let main_path = "tests/stasis/assets/grouped_font_main.stasis";
        let mut jit = JitProcess::new();
        jit.upsert_file(main_path, main);
        jit.upsert_file("tests/stasis/assets/font_paths.stasis", font_paths);
        jit.compile().expect("compile grouped font asset fixture");
        let references = jit.program_snapshot().expect("snapshot").asset_references();

        assert_eq!(references.len(), 2);
        assert!(references
            .iter()
            .all(|reference| reference.source_path == main_path));
        assert!(references
            .iter()
            .all(|reference| reference.api == "load_font"));
        assert!(references
            .iter()
            .all(|reference| { reference.logical_path.as_deref() == Some("assets/ui.ttf") }));
        assert_eq!(
            &main[references[0].start..references[0].end],
            "font_paths.ASSETS.FONT_PATH"
        );
        assert_eq!(
            &main[references[1].start..references[1].end],
            "font_paths.ASSETS.FONT_PATH"
        );

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let project_root = std::env::temp_dir().join(format!("stasis_group_asset_{stamp}"));
        fs::create_dir_all(project_root.join("assets")).expect("asset directory");
        fs::write(project_root.join("assets/ui.ttf"), "font").expect("font asset");
        let packaged = validate_asset_references(
            &project_root,
            &[],
            references,
            Some(&BTreeSet::from(["assets/ui.ttf".to_string()])),
            &BTreeSet::new(),
        );
        assert_eq!(packaged.references.len(), 2);
        assert!(packaged
            .references
            .iter()
            .all(|reference| reference.project_path == "assets/ui.ttf"));
        assert!(packaged.resolved_paths.contains("assets/ui.ttf"));
        assert!(packaged.diagnostics.is_empty());
        fs::remove_dir_all(project_root).ok();

        let mut aot = AotProcess::new();
        aot.upsert_file(main_path, main);
        aot.upsert_file("tests/stasis/assets/font_paths.stasis", font_paths);
        aot.compile()
            .expect("compile AOT grouped font asset fixture");
        assert_eq!(
            references,
            aot.program_snapshot()
                .expect("AOT snapshot")
                .asset_references()
        );
    }

    #[test]
    fn module_qualified_asset_call_uses_the_resolved_function_identity() {
        let main = r#"
import "asset_loader.stasis";
global sprite: ModuleSprite;
function main(): i32 {
    asset_loader.load_from(sprite, "assets/module_sprite.png", 32, 24);
    sprite.load_from("assets/receiver_sprite.png", 32, 24);
    return 0;
}
"#;
        let loader = r#"
struct ModuleSprite { loaded: bool; }
function @asset_path(path) @extern("stasis_jit_sprite_load_from") load_from(self: ModuleSprite, path: string, width: i32, height: i32): bool;
"#;
        let mut jit = JitProcess::new();
        jit.upsert_file("tests/stasis/assets/module_main.stasis", main);
        jit.upsert_file("tests/stasis/assets/asset_loader.stasis", loader);
        jit.compile().expect("compile qualified asset calls");
        let references = jit.program_snapshot().expect("snapshot").asset_references();

        assert_eq!(references.len(), 2);
        assert_eq!(
            references
                .iter()
                .map(|reference| reference.logical_path.as_deref())
                .collect::<Vec<_>>(),
            vec![
                Some("assets/module_sprite.png"),
                Some("assets/receiver_sprite.png"),
            ]
        );
    }

    #[test]
    fn inlined_loader_calls_keep_their_original_source_identity_and_range() {
        let source_prefix = r#"
struct Sprite { loaded: bool; }
struct Custom { loaded: bool; }
function @asset_path(path) @extern("stasis_jit_sprite_load_from") load_from(self: Sprite, path: string, width: i32, height: i32): bool;
function load_from(self: Custom, path: string, width: i32, height: i32): bool { return width > 0 && height > 0; }
global sprite: Sprite;
global custom: Custom;
function @inline load_sprite(): bool { return sprite.load_from("assets/inline.png", 32, 24); }
"#;
        let wrapper_body_start = source_prefix
            .find("function @inline load_sprite")
            .and_then(|start| {
                source_prefix[start..]
                    .find('{')
                    .map(|open| start + open + 1)
            })
            .expect("inline wrapper body");
        let inlined_callee_start = source_prefix[wrapper_body_start..]
            .find("load_from(")
            .map(|start| wrapper_body_start + start)
            .expect("annotated loader call");
        let relative_callee_start = inlined_callee_start - wrapper_body_start;
        let custom_callee_prefix = "custom.";
        let padding = " ".repeat(relative_callee_start - custom_callee_prefix.len());
        // This deliberate span collision catches callee-body coordinates being
        // interpreted as a caller range after expression inlining.
        let main = format!(
            "function main(): i32 {{{padding}custom.load_from(\"assets/custom.png\", 32, 24); load_sprite(); return 0; }}\n"
        );
        let source = format!("{source_prefix}{main}");

        let mut jit = JitProcess::new();
        jit.upsert_file("tests/stasis/assets/inlined_loader.stasis", &source);
        jit.compile().expect("compile inline loader fixture");
        let references = jit.program_snapshot().expect("snapshot").asset_references();

        assert_eq!(references.len(), 1);
        assert_eq!(references[0].api, "load_from");
        assert_eq!(
            references[0].logical_path.as_deref(),
            Some("assets/inline.png")
        );
        assert_eq!(
            &source[references[0].start..references[0].end],
            "\"assets/inline.png\""
        );
    }

    #[test]
    fn annotated_asset_task_wrapper_owns_packaging_for_its_raw_host_call() {
        let source = r#"
function @extern("stasis_jit_asset_request_audio") asset_request_audio(path: string): i32;
function @asset_path(path) request_audio_load(path: string): i32 {
    return asset_request_audio(path);
}
function main(): i32 { return request_audio_load("assets/voice.mp3"); }
"#;
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", source);
        jit.compile().expect("compile asset task wrapper fixture");
        let references = jit.program_snapshot().expect("snapshot").asset_references();
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].api, "request_audio_load");
        assert_eq!(
            references[0].logical_path.as_deref(),
            Some("assets/voice.mp3")
        );
    }

    #[test]
    fn snapshot_discovers_reachable_annotated_and_legacy_asset_calls() {
        let source = r#"
extern function @asset_path(path) load_font(path: string, size: i32): i32;
extern function gfx_load_sprite(path: string, max_w: i32, max_h: i32): i32;
const FONT_PATH: string = "assets/ui.ttf";
function helper(): i32 { return FONT_PATH.load_font(16) + gfx_load_sprite("assets/raw_sprite.png", 32, 32); }
function unused(): i32 { return load_font("assets/unused.ttf", 16); }
function main(): i32 { return helper(); }
"#;
        let mut jit = JitProcess::new();
        jit.upsert_file("tests/stasis/assets/reachable.stasis", source);
        jit.compile().expect("compile asset fixture");
        let references = jit.program_snapshot().expect("snapshot").asset_references();
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].api, "load_font");
        assert_eq!(references[0].logical_path.as_deref(), Some("assets/ui.ttf"));
        assert_eq!(&source[references[0].start..references[0].end], "FONT_PATH");
        assert_eq!(references[1].api, "gfx_load_sprite");
        assert_eq!(
            references[1].logical_path.as_deref(),
            Some("assets/raw_sprite.png")
        );
        assert_eq!(
            &source[references[1].start..references[1].end],
            "\"assets/raw_sprite.png\""
        );

        let mut aot = AotProcess::new();
        aot.upsert_file("tests/stasis/assets/reachable.stasis", source);
        aot.compile().expect("compile AOT asset fixture");
        assert_eq!(
            references,
            aot.program_snapshot()
                .expect("AOT snapshot")
                .asset_references()
        );
    }

    #[test]
    fn receiver_form_reports_dynamic_path_expression_span() {
        let source = r#"
extern function @asset_path(path) load_font(path: string, size: i32): i32;
function main(path: string): i32 { return path.load_font(16); }
"#;
        let mut jit = JitProcess::new();
        jit.upsert_file("tests/stasis/assets/dynamic_receiver.stasis", source);
        jit.compile().expect("compile dynamic receiver fixture");
        let references = jit.program_snapshot().expect("snapshot").asset_references();
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].logical_path, None);
        assert_eq!(&source[references[0].start..references[0].end], "path");
    }

    #[test]
    fn compound_receiver_spans_include_calls_and_indexes() {
        for (source, expected) in [
            ("(FONT_PATH).load_font(16)", "(FONT_PATH)"),
            ("choose_path().load_font(16)", "choose_path()"),
            ("choose_path ().load_font(16)", "choose_path ()"),
            ("paths[0].load_font(16)", "paths[0]"),
        ] {
            let tokens = lex(source).expect("receiver tokens");
            let method_index = tokens
                .iter()
                .position(|token| {
                    token.kind == TokenKind::Identifier
                        && &source[token.start..token.end] == "load_font"
                })
                .expect("method token");
            let range = receiver_expression_range(source, &tokens, method_index)
                .expect("receiver expression range");
            assert_eq!(&source[range], expected);
        }
        let constants = BTreeMap::from([(
            "FONT_PATH".to_string(),
            ConstantValue::String {
                value: "assets/ui.ttf".to_string(),
                type_id: 0,
            },
        )]);
        assert_eq!(
            static_string("(FONT_PATH)", &constants, |_| false).expect("parenthesized constant"),
            Some("assets/ui.ttf".to_string())
        );
    }

    #[test]
    fn scoped_bindings_shadow_global_asset_constants_at_the_call_site() {
        let source = r#"
extern function @asset_path(path) load_font(path: string, size: i32): i32;
const FONT_PATH: string = "assets/global.ttf";
function from_parameter(FONT_PATH: string): i32 { return load_font(FONT_PATH, 16); }
function from_local(): i32 {
    let result: i32 = 0;
    if (result == 0) {
        let FONT_PATH: string = "assets/local.ttf";
        return load_font(FONT_PATH, 16);
    }
    return load_font(FONT_PATH, 16);
}
function from_initializer(): i32 {
    let FONT_PATH: i32 = load_font(FONT_PATH, 16);
    return FONT_PATH;
}
function main(): i32 {
    return from_parameter("assets/runtime.ttf") + from_local() + from_initializer();
}
"#;
        let mut jit = JitProcess::new();
        jit.upsert_file("tests/stasis/assets/scoped_bindings.stasis", source);
        jit.compile().expect("compile scoped asset fixture");
        let references = jit.program_snapshot().expect("snapshot").asset_references();
        let paths = references
            .iter()
            .map(|reference| reference.logical_path.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                None,
                None,
                Some("assets/global.ttf"),
                Some("assets/global.ttf")
            ]
        );
    }

    #[test]
    fn validation_rejects_asset_symlinks_that_escape_the_asset_root() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_asset_symlink_{stamp}"));
        fs::create_dir_all(root.join("src")).expect("source dir");
        fs::create_dir_all(root.join("assets")).expect("asset dir");
        fs::write(root.join("outside.svg"), "outside").expect("outside fixture");
        let link = root.join("assets/escape.svg");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(root.join("outside.svg"), &link).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(root.join("outside.svg"), &link).is_ok();
        if !linked {
            fs::remove_dir_all(root).ok();
            return;
        }
        let references = vec![AssetReference {
            api: "load_font".into(),
            source_path: root.join("src/main.stasis").to_string_lossy().into_owned(),
            start: 0,
            end: 19,
            logical_path: Some("assets/escape.svg".into()),
        }];
        let result = validate_asset_references(
            &root,
            &[],
            &references,
            Some(&BTreeSet::from(["assets/escape.svg".to_string()])),
            &BTreeSet::new(),
        );
        assert!(result.references.is_empty());
        assert_eq!(
            result.diagnostics[0].code,
            "asset_path_missing_or_outside_boundary"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn validation_reports_case_dynamic_and_manifest_failures() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_asset_validation_{stamp}"));
        fs::create_dir_all(root.join("src")).expect("source dir");
        fs::create_dir_all(root.join("assets/svg")).expect("asset dir");
        fs::write(root.join("assets/svg/hero.svg"), "svg").expect("asset");
        let references = vec![
            AssetReference {
                api: "load_from".into(),
                source_path: root.join("src/main.stasis").to_string_lossy().into_owned(),
                start: 10,
                end: 20,
                logical_path: Some("../assets/Svg/hero.svg".into()),
            },
            AssetReference {
                api: "load_font".into(),
                source_path: root.join("src/main.stasis").to_string_lossy().into_owned(),
                start: 30,
                end: 34,
                logical_path: None,
            },
        ];
        let result = validate_asset_references(
            &root,
            &[],
            &references,
            Some(&BTreeSet::from(["assets/other.svg".to_string()])),
            &BTreeSet::new(),
        );
        assert_eq!(
            result
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["asset_dynamic_path_undeclared", "asset_path_case_mismatch"])
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn validation_resolves_declaring_module_and_reports_every_path_contract() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_asset_contract_{stamp}"));
        fs::create_dir_all(root.join("src/ui")).expect("source dir");
        fs::create_dir_all(root.join("assets/svg")).expect("asset dir");
        fs::write(root.join("assets/svg/hero.svg"), "svg").expect("asset");
        fs::write(root.join("outside.svg"), "outside").expect("outside fixture");
        let source_path = root
            .join("src/ui/menu.stasis")
            .to_string_lossy()
            .into_owned();
        let reference = |start, logical_path: &str| AssetReference {
            api: "custom_asset_loader".into(),
            source_path: source_path.clone(),
            start,
            end: start + logical_path.len(),
            logical_path: Some(logical_path.into()),
        };
        let references = vec![
            reference(0, "../../assets/svg/hero.svg"),
            reference(10, "assets/svg/hero.svg"),
            reference(20, "/assets/svg/hero.svg"),
            reference(30, "../../outside.svg"),
            reference(40, "assets/svg/missing.svg"),
            reference(50, "https://example.com/hero.svg"),
            reference(60, "C:/assets/hero.svg"),
        ];
        let result = validate_asset_references(
            &root,
            &[],
            &references,
            Some(&BTreeSet::from(["assets/svg/hero.svg".to_string()])),
            &BTreeSet::new(),
        );
        assert_eq!(result.references.len(), 3);
        assert_eq!(
            result.resolved_paths,
            BTreeSet::from(["assets/svg/hero.svg".to_string()])
        );
        assert_eq!(
            result
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect::<Vec<_>>(),
            vec![
                "asset_path_missing_or_outside_boundary",
                "asset_path_missing_or_outside_boundary",
                "asset_path_absolute_or_uri",
                "asset_path_absolute_or_uri",
            ]
        );

        let undeclared = validate_asset_references(
            &root,
            &[],
            &[reference(70, "assets/svg/hero.svg")],
            Some(&BTreeSet::new()),
            &BTreeSet::new(),
        );
        assert_eq!(undeclared.diagnostics[0].code, "asset_not_declared");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn validation_rejects_nonportable_asset_path_spellings() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_asset_portability_{stamp}"));
        fs::create_dir_all(root.join("assets/fonts")).expect("asset dir");
        fs::write(root.join("assets/fonts/ui.ttf"), "font").expect("asset fixture");
        let source_path = root.join("main.stasis").to_string_lossy().into_owned();
        let reference = |start, logical_path: &str| AssetReference {
            api: "load_font".into(),
            source_path: source_path.clone(),
            start,
            end: start + logical_path.len(),
            logical_path: Some(logical_path.into()),
        };
        let result = validate_asset_references(
            &root,
            &[],
            &[
                reference(0, "/assets/fonts/ui.ttf"),
                reference(1, "/tmp/ui.ttf"),
                reference(2, "C:/assets/fonts/ui.ttf"),
                reference(3, "assets\\fonts\\ui.ttf"),
                reference(4, "\\\\server\\share\\ui.ttf"),
                reference(5, "https://example.com/ui.ttf"),
            ],
            None,
            &BTreeSet::new(),
        );
        assert_eq!(result.references.len(), 1);
        assert_eq!(result.references[0].project_path, "assets/fonts/ui.ttf");
        assert_eq!(
            result
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect::<Vec<_>>(),
            vec![
                "asset_path_absolute_or_uri",
                "asset_path_absolute_or_uri",
                "asset_path_nonportable_separator",
                "asset_path_nonportable_separator",
                "asset_path_absolute_or_uri",
            ]
        );
        fs::remove_dir_all(root).ok();
    }
}
