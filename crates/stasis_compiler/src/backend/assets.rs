use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::backend::emit::ConstantValue;
use crate::compiler::{FunctionId, FunctionMeta, SourceFile};
use crate::frontend::lexer::{lex, Token, TokenKind};
use crate::frontend::parser::{
    parse_local_declarations, parse_string_literal_text, parse_top_level_extern_functions,
    parse_top_level_functions, ParsedFunctionAnnotationArgumentKind, ParsedLocalDeclaration,
};

const ASSET_ANNOTATION: &str = "asset_path";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AssetLoader {
    parameter_count: usize,
    path_parameter: usize,
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
    reachable: &BTreeSet<FunctionId>,
    constants: &BTreeMap<String, ConstantValue>,
) -> Result<Vec<AssetReference>, String> {
    let loaders = collect_asset_loaders(files)?;
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

fn collect_asset_loaders(
    files: &[SourceFile],
) -> Result<BTreeMap<String, Vec<AssetLoader>>, String> {
    let mut loaders = BTreeMap::new();
    for file in files {
        for declaration in parse_top_level_extern_functions(&file.content).map_err(|error| {
            format!(
                "failed parsing asset loader declarations in {}: {error}",
                file.path
            )
        })? {
            if let Some(index) = legacy_asset_parameter(&declaration.name) {
                if declaration
                    .params
                    .get(index)
                    .is_some_and(|parameter| parameter.type_name == "string")
                {
                    insert_loader(
                        &mut loaders,
                        &declaration.name,
                        declaration.params.len(),
                        index,
                    );
                }
            }
            for annotation in declaration
                .annotations
                .iter()
                .filter(|annotation| annotation.name == ASSET_ANNOTATION)
            {
                if !annotation.has_parentheses || annotation.arguments.len() != 1 {
                    return Err(format!(
                        "@asset_path on '{}' must name exactly one path parameter",
                        declaration.name
                    ));
                }
                let argument = &annotation.arguments[0];
                if argument.kind != ParsedFunctionAnnotationArgumentKind::Identifier {
                    return Err(format!(
                        "@asset_path on '{}' must use a parameter name",
                        declaration.name
                    ));
                }
                let index = declaration
                    .params
                    .iter()
                    .position(|parameter| parameter.name == argument.text)
                    .ok_or_else(|| {
                        format!(
                            "@asset_path on '{}' names unknown parameter '{}'",
                            declaration.name, argument.text
                        )
                    })?;
                insert_loader(
                    &mut loaders,
                    &declaration.name,
                    declaration.params.len(),
                    index,
                );
            }
        }
        for declaration in parse_top_level_functions(&file.content).map_err(|error| {
            format!(
                "failed parsing asset wrapper declarations in {}: {error}",
                file.path
            )
        })? {
            if let Some(index) = legacy_asset_parameter(&declaration.name) {
                if declaration
                    .params
                    .get(index)
                    .is_some_and(|parameter| parameter.type_name == "string")
                {
                    insert_loader(
                        &mut loaders,
                        &declaration.name,
                        declaration.params.len(),
                        index,
                    );
                }
            }
            for annotation in declaration
                .annotations
                .iter()
                .filter(|annotation| annotation.name == ASSET_ANNOTATION)
            {
                if !annotation.has_parentheses || annotation.arguments.len() != 1 {
                    return Err(format!(
                        "@asset_path on '{}' must name exactly one path parameter",
                        declaration.name
                    ));
                }
                let argument = &annotation.arguments[0];
                if argument.kind != ParsedFunctionAnnotationArgumentKind::Identifier {
                    return Err(format!(
                        "@asset_path on '{}' must use a parameter name",
                        declaration.name
                    ));
                }
                let index = declaration
                    .params
                    .iter()
                    .position(|parameter| parameter.name == argument.text)
                    .ok_or_else(|| {
                        format!(
                            "@asset_path on '{}' names unknown parameter '{}'",
                            declaration.name, argument.text
                        )
                    })?;
                insert_loader(
                    &mut loaders,
                    &declaration.name,
                    declaration.params.len(),
                    index,
                );
            }
        }
    }
    Ok(loaders)
}

fn insert_loader(
    loaders: &mut BTreeMap<String, Vec<AssetLoader>>,
    name: &str,
    parameter_count: usize,
    path_parameter: usize,
) {
    let entries = loaders.entry(name.to_string()).or_default();
    entries.push(AssetLoader {
        parameter_count,
        path_parameter,
    });
    entries.sort();
    entries.dedup();
}

fn legacy_asset_parameter(name: &str) -> Option<usize> {
    match name {
        "audio_load_effect" | "audio_load_music" | "audio_load_wav" | "gfx_load_sprite"
        | "load_font" => Some(0),
        "load_sprite_from" | "load_sprite_sheet_from" => Some(1),
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
    loaders: &BTreeMap<String, Vec<AssetLoader>>,
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
        let Some(loader_signatures) = loaders.get(name) else {
            continue;
        };
        let receiver_call = index > 0
            && token_text_is(body, tokens[index - 1], ".")
            && tokens
                .get(index + 1)
                .is_some_and(|next| next.kind == TokenKind::LParen);
        let bare_call = !receiver_call
            && tokens
                .get(index + 1)
                .is_some_and(|next| next.kind == TokenKind::LParen);
        if !receiver_call && !bare_call {
            continue;
        }
        let open = index + 1;
        let (arguments, _) = call_argument_ranges(&tokens, open)?;
        let path_operands = loader_signatures
            .iter()
            .filter_map(|loader| {
                if receiver_call {
                    (loader.parameter_count == arguments.len() + 1).then(|| {
                        if loader.path_parameter == 0 {
                            PathOperand::Receiver
                        } else {
                            PathOperand::Argument(loader.path_parameter - 1)
                        }
                    })
                } else {
                    (loader.parameter_count == arguments.len())
                        .then_some(PathOperand::Argument(loader.path_parameter))
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
        let logical_path = static_string(expression, constants, |name| {
            parameter_names.iter().any(|parameter| parameter == name)
                || local_declarations.iter().any(|declaration| {
                    declaration.function_name == function_name
                        && declaration.name == name
                        && declaration.visibility_range.contains(&expression_start)
                })
        })?;
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
        let files = vec![SourceFile {
            path: "custom.stasis".into(),
            content: source.into(),
            hash: 0,
            functions: Vec::new(),
        }];
        assert_eq!(
            collect_asset_loaders(&files)
                .expect("loader metadata")
                .get("custom_load"),
            Some(&vec![AssetLoader {
                parameter_count: 2,
                path_parameter: 1,
            }])
        );
    }

    #[test]
    fn built_in_wrapper_metadata_uses_the_path_parameter() {
        let source = "function load_sprite_sheet_from(self: i32, path: string, columns: i32): bool { return columns > 0; }";
        let files = vec![SourceFile {
            path: "custom.stasis".into(),
            content: source.into(),
            hash: 0,
            functions: Vec::new(),
        }];
        assert_eq!(
            collect_asset_loaders(&files)
                .expect("wrapper metadata")
                .get("load_sprite_sheet_from"),
            Some(&vec![AssetLoader {
                parameter_count: 3,
                path_parameter: 1,
            }])
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
    fn snapshot_discovers_only_reachable_annotated_asset_calls() {
        let source = r#"
extern function @asset_path(path) load_font(path: string, size: i32): i32;
const FONT_PATH: string = "assets/ui.ttf";
function helper(): i32 { return FONT_PATH.load_font(16); }
function unused(): i32 { return load_font("assets/unused.ttf", 16); }
function main(): i32 { return helper(); }
"#;
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", source);
        jit.compile().expect("compile asset fixture");
        let references = jit.program_snapshot().expect("snapshot").asset_references();
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].api, "load_font");
        assert_eq!(references[0].logical_path.as_deref(), Some("assets/ui.ttf"));
        assert_eq!(&source[references[0].start..references[0].end], "FONT_PATH");

        let mut aot = AotProcess::new();
        aot.upsert_file("main.stasis", source);
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
        jit.upsert_file("main.stasis", source);
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
        jit.upsert_file("main.stasis", source);
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
                api: "load_sprite_from".into(),
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
