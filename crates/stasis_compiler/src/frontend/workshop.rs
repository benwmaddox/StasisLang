use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::compiler::{source_workshop_items, Compiler};
use crate::data_flow::CompilerLocalType;
use crate::frontend::lexer::{is_inside_backtick_literal, lex, Token, TokenKind};
use crate::frontend::parser::{
    parse_local_declarations, parse_top_level_functions, parse_top_level_type_layout,
};
use crate::identity::{
    canonical_source_path, overload_discriminator, CanonicalSourcePath, SymbolId,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopSymbolKind {
    Struct,
    Function,
    Global,
    Constant,
    Test,
}

/// Discovery exposure for Workshop-facing compiler metadata.
///
/// This controls user-facing enumeration, not compilation or symbol access.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopExposure {
    #[default]
    Public,
    Internal,
}

impl WorkshopExposure {
    pub fn is_public(&self) -> bool {
        *self == Self::Public
    }
}

pub fn workshop_file_exposure(path: &str) -> WorkshopExposure {
    let normalized = format!("/{}", path.replace('\\', "/").trim_start_matches('/'));
    if normalized.contains("/stdlib/internal/") || normalized.contains("/stdlib/testing/") {
        WorkshopExposure::Internal
    } else {
        WorkshopExposure::Public
    }
}

pub fn workshop_declaration_exposure<'a>(
    path: &str,
    annotations: impl IntoIterator<Item = &'a str>,
) -> WorkshopExposure {
    if workshop_file_exposure(path) == WorkshopExposure::Internal
        || annotations.into_iter().any(|name| name == "internal")
    {
        WorkshopExposure::Internal
    } else {
        WorkshopExposure::Public
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopSymbolGroupKind {
    Main,
    Struct,
    Global,
    Constant,
    System,
    Root,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSourceFile {
    pub path: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSourceSpan {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSymbol {
    pub symbol_id: String,
    pub kind: WorkshopSymbolKind,
    pub name: String,
    pub owner: Option<String>,
    pub file: String,
    pub signature: String,
    pub source_span: WorkshopSourceSpan,
    pub source: String,
    #[serde(default, skip_serializing_if = "WorkshopExposure::is_public")]
    pub exposure: WorkshopExposure,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSymbolGroup {
    pub kind: WorkshopSymbolGroupKind,
    pub name: String,
    pub symbols: Vec<WorkshopSymbol>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSymbolTree {
    pub groups: Vec<WorkshopSymbolGroup>,
}

#[derive(Debug, Clone)]
struct PendingSymbol {
    group_kind: WorkshopSymbolGroupKind,
    group_name: String,
    symbol: WorkshopSymbol,
}

pub fn load_workshop_project(
    project_root: &Path,
    entry_file: &Path,
) -> Result<Vec<WorkshopSourceFile>, String> {
    load_workshop_project_with_diagnostic(project_root, entry_file)
        .map_err(|diagnostic| format!("workshop module graph failed: {}", diagnostic.message))
}

pub fn load_workshop_project_with_diagnostic(
    project_root: &Path,
    entry_file: &Path,
) -> Result<Vec<WorkshopSourceFile>, crate::SourceDiagnostic> {
    let (_, sources) =
        crate::frontend::module_graph::load_project_module_graph(project_root, entry_file)?;
    Ok(sources
        .into_iter()
        .map(|(path, source)| WorkshopSourceFile { path, source })
        .collect())
}

fn parse_workshop_import_paths(source: &str) -> Result<Vec<String>, String> {
    let tokens = lex(source)?;
    let mut imports = Vec::new();
    let mut cursor = 0usize;
    while cursor + 1 < tokens.len() {
        let token = tokens[cursor];
        if token.kind == TokenKind::Identifier && token_text(source, token) == "import" {
            let literal = tokens[cursor + 1];
            if literal.kind != TokenKind::StringLiteral {
                return Err("import must be followed by a string literal path".to_string());
            }
            imports.push(parse_workshop_string_literal(token_text(source, literal))?);
            cursor += 2;
            continue;
        }
        cursor += 1;
    }
    Ok(imports)
}

fn parse_workshop_string_literal(literal: &str) -> Result<String, String> {
    let bytes = literal.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'"' || *bytes.last().unwrap_or(&0) != b'"' {
        return Err(format!("invalid import string literal: {literal}"));
    }
    let mut out = String::new();
    let mut cursor = 1usize;
    while cursor + 1 < bytes.len() {
        let byte = bytes[cursor];
        if byte == b'\\' {
            let Some(escaped) = bytes.get(cursor + 1).copied() else {
                return Err("unterminated escape in import string literal".to_string());
            };
            let decoded = match escaped {
                b'\\' => '\\',
                b'"' => '"',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                other => {
                    return Err(format!(
                        "unsupported escape sequence '\\{}' in import string literal",
                        other as char
                    ));
                }
            };
            out.push(decoded);
            cursor += 2;
            continue;
        }
        out.push(byte as char);
        cursor += 1;
    }
    Ok(out)
}

fn normalize_filesystem_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn normalize_workshop_project_path(project_root: &Path, path: &Path) -> String {
    let normalized_root = normalize_filesystem_path(project_root);
    let normalized_path = normalize_filesystem_path(path);
    let relative = normalized_path
        .strip_prefix(&normalized_root)
        .unwrap_or(normalized_path.as_path());
    relative
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSymbolPlacementRequest {
    pub kind: WorkshopPlacementSymbolKind,
    pub name: String,
    #[serde(default)]
    pub params: Vec<WorkshopFunctionParam>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopPlacementSymbolKind {
    Struct,
    Function,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopFunctionParam {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSymbolPlacement {
    pub file: String,
    pub group: String,
    pub reason: String,
}

pub fn plan_workshop_symbol_placement(
    files: &[WorkshopSourceFile],
    request: &WorkshopSymbolPlacementRequest,
) -> Result<WorkshopSymbolPlacement, String> {
    let known_structs = collect_workshop_struct_names(files)?;
    match request.kind {
        WorkshopPlacementSymbolKind::Struct => Ok(WorkshopSymbolPlacement {
            file: format!("src/{}.stasis", snake_case(&request.name)),
            group: request.name.clone(),
            reason: "Struct definitions go in their own file.".to_string(),
        }),
        WorkshopPlacementSymbolKind::Function => {
            plan_workshop_function_placement(request, &known_structs)
        }
    }
}

fn plan_workshop_function_placement(
    request: &WorkshopSymbolPlacementRequest,
    known_structs: &BTreeSet<String>,
) -> Result<WorkshopSymbolPlacement, String> {
    if is_lifecycle_function(&request.name) {
        return Ok(WorkshopSymbolPlacement {
            file: "src/main.stasis".to_string(),
            group: "Main".to_string(),
            reason: "Lifecycle functions live in main.stasis.".to_string(),
        });
    }

    if let Some(system) = request
        .system
        .as_deref()
        .filter(|system| !system.trim().is_empty())
    {
        let system_name = system.trim();
        return Ok(WorkshopSymbolPlacement {
            file: format!("src/systems/{}.stasis", snake_case(system_name)),
            group: title_case_words(system_name),
            reason: "Cross-struct behavior lives in systems/<system>.stasis.".to_string(),
        });
    }

    if let Some(owner) = request
        .owner
        .as_deref()
        .filter(|owner| known_structs.contains(*owner))
    {
        return Ok(struct_owned_function_placement(
            owner,
            "Receiver-style functions live with their receiver type.",
        ));
    }

    if let Some(first_param) = request.params.first() {
        if known_structs.contains(&first_param.type_name) {
            return Ok(struct_owned_function_placement(
                &first_param.type_name,
                "Functions whose first parameter is a struct view live with that struct type.",
            ));
        }
    }

    if let Some(return_type) = request.return_type.as_deref() {
        if known_structs.contains(return_type) {
            return Ok(struct_owned_function_placement(
                return_type,
                "Functions that return or create a specific struct live with that struct type.",
            ));
        }
    }

    Ok(WorkshopSymbolPlacement {
        file: "src/root.stasis".to_string(),
        group: "Root".to_string(),
        reason: "No-owner utility functions live in root.stasis.".to_string(),
    })
}

fn struct_owned_function_placement(owner: &str, reason: &str) -> WorkshopSymbolPlacement {
    WorkshopSymbolPlacement {
        file: format!("src/{}.stasis", snake_case(owner)),
        group: owner.to_string(),
        reason: reason.to_string(),
    }
}

fn collect_workshop_struct_names(files: &[WorkshopSourceFile]) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    for file in files {
        let layout = source_workshop_items(&file.source)?.layout;
        for parsed in layout.structs {
            out.insert(parsed.name);
        }
    }
    Ok(out)
}

fn title_case_words(value: &str) -> String {
    value
        .split(|ch: char| ch == '_' || ch == '-' || ch.is_ascii_whitespace())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<String>()
}

pub fn build_workshop_symbol_tree(
    files: &[WorkshopSourceFile],
) -> Result<WorkshopSymbolTree, String> {
    for file in files {
        canonical_source_path(None, &file.path).map_err(|error| {
            format!(
                "invalid Workshop project-relative path '{}': {error}",
                file.path
            )
        })?;
    }
    let mut struct_names = BTreeSet::new();
    let mut structs_by_file: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for file in files {
        let layout = source_workshop_items(&file.source)?.layout;
        for parsed in layout.structs {
            struct_names.insert(parsed.name.clone());
            structs_by_file
                .entry(file.path.as_str())
                .or_default()
                .push(parsed.name);
        }
    }

    let mut pending = Vec::new();
    for file in files {
        pending.extend(index_file_symbols(
            file,
            &struct_names,
            structs_by_file
                .get(file.path.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )?);
    }

    let mut by_group: BTreeMap<(WorkshopSymbolGroupKind, String), Vec<WorkshopSymbol>> =
        BTreeMap::new();
    for pending_symbol in pending {
        by_group
            .entry((pending_symbol.group_kind, pending_symbol.group_name))
            .or_default()
            .push(pending_symbol.symbol);
    }

    let mut groups = Vec::new();
    for group_kind in [
        WorkshopSymbolGroupKind::Main,
        WorkshopSymbolGroupKind::Struct,
        WorkshopSymbolGroupKind::Global,
        WorkshopSymbolGroupKind::Constant,
        WorkshopSymbolGroupKind::System,
        WorkshopSymbolGroupKind::Root,
        WorkshopSymbolGroupKind::Test,
    ] {
        let keys = by_group
            .keys()
            .filter(|(kind, _)| *kind == group_kind)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            let mut symbols = by_group.remove(&key).unwrap_or_default();
            symbols.sort_by_key(|symbol| (symbol.file.clone(), symbol.source_span.start));
            groups.push(WorkshopSymbolGroup {
                kind: key.0,
                name: key.1,
                symbols,
            });
        }
    }

    Ok(WorkshopSymbolTree { groups })
}

fn index_file_symbols(
    file: &WorkshopSourceFile,
    struct_names: &BTreeSet<String>,
    file_structs: &[String],
) -> Result<Vec<PendingSymbol>, String> {
    let canonical_path = CanonicalSourcePath::project_relative(&file.path)?;
    let mut out = Vec::new();
    let records = source_workshop_items(&file.source)?;
    for parsed_struct in &records.structs {
        let source = source_for_range(&file.source, parsed_struct.definition_range.clone())?;
        out.push(PendingSymbol {
            group_kind: WorkshopSymbolGroupKind::Struct,
            group_name: parsed_struct.name.clone(),
            symbol: WorkshopSymbol {
                symbol_id: SymbolId::declaration(
                    "struct",
                    &canonical_path,
                    &parsed_struct.name,
                    "declaration",
                )
                .to_string(),
                kind: WorkshopSymbolKind::Struct,
                name: parsed_struct.name.clone(),
                owner: Some(parsed_struct.name.clone()),
                file: file.path.clone(),
                signature: format!("struct {}", parsed_struct.name),
                source_span: span_from_range(parsed_struct.definition_range.clone())?,
                source,
                exposure: workshop_file_exposure(&file.path),
            },
        });
    }

    for function in records.functions {
        let exposure = workshop_declaration_exposure(
            &file.path,
            function
                .annotations
                .iter()
                .map(|annotation| annotation.name.as_str()),
        );
        let full_range = function.signature_range.start..function.body_range.end;
        let source = source_for_range(&file.source, full_range.clone())?;
        let signature =
            format_function_signature(&function.name, &function.params, &function.return_type_name);
        let owner = function_owner(
            &file.path,
            &function.name,
            function
                .params
                .first()
                .map(|param| param.type_name.as_str()),
            &function.return_type_name,
            file_structs,
            struct_names,
        );
        let (group_kind, group_name) = function_group(&file.path, &function.name, owner.as_deref());
        out.push(PendingSymbol {
            group_kind,
            group_name,
            symbol: WorkshopSymbol {
                symbol_id: SymbolId::function(
                    &canonical_path,
                    &function.name,
                    &overload_discriminator(
                        &function
                            .params
                            .iter()
                            .map(|param| param.type_name.clone())
                            .collect::<Vec<_>>(),
                    ),
                )
                .to_string(),
                kind: WorkshopSymbolKind::Function,
                name: function.name,
                owner,
                file: file.path.clone(),
                signature,
                source_span: span_from_range(full_range)?,
                source,
                exposure,
            },
        });
    }

    for parsed in parse_simple_top_level_symbols(&file.source)? {
        let (group_kind, group_name, owner) = match parsed.kind {
            WorkshopSymbolKind::Global => (
                WorkshopSymbolGroupKind::Global,
                "Globals".to_string(),
                Some("Globals".to_string()),
            ),
            WorkshopSymbolKind::Constant => (
                WorkshopSymbolGroupKind::Constant,
                "Constants".to_string(),
                Some("Constants".to_string()),
            ),
            WorkshopSymbolKind::Test => (
                WorkshopSymbolGroupKind::Test,
                "Tests".to_string(),
                Some("Tests".to_string()),
            ),
            WorkshopSymbolKind::Struct | WorkshopSymbolKind::Function => continue,
        };
        out.push(PendingSymbol {
            group_kind,
            group_name,
            symbol: WorkshopSymbol {
                symbol_id: SymbolId::declaration(
                    match parsed.kind {
                        WorkshopSymbolKind::Global => "global",
                        WorkshopSymbolKind::Constant => "constant",
                        WorkshopSymbolKind::Test => "test",
                        WorkshopSymbolKind::Struct | WorkshopSymbolKind::Function => unreachable!(),
                    },
                    &canonical_path,
                    &parsed.name,
                    "declaration",
                )
                .to_string(),
                kind: parsed.kind,
                name: parsed.name,
                owner,
                file: file.path.clone(),
                signature: parsed.signature,
                source_span: span_from_range(parsed.range.clone())?,
                source: source_for_range(&file.source, parsed.range)?,
                exposure: workshop_file_exposure(&file.path),
            },
        });
    }

    Ok(out)
}

pub fn load_workshop_edit_workspace(
    project_root: &Path,
    entry_file: &Path,
) -> Result<Vec<WorkshopSourceFile>, String> {
    let mut files = load_workshop_project(project_root, entry_file)?;
    let mut known = files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    for directory in ["src", "tests"] {
        collect_workshop_source_files(
            project_root,
            &project_root.join(directory),
            &mut known,
            &mut files,
        )?;
    }
    files.sort_by_key(|file| file.path.clone());
    Ok(files)
}

pub fn load_workshop_source_workspace(
    project_root: &Path,
    entry_file: &Path,
) -> Result<Vec<WorkshopSourceFile>, String> {
    let mut files = Vec::new();
    let mut known = BTreeSet::new();
    let entry = if entry_file.is_absolute() {
        entry_file.to_path_buf()
    } else {
        project_root.join(entry_file)
    };
    if entry.is_file() {
        let relative = normalize_workshop_project_path(project_root, &entry);
        known.insert(relative.clone());
        files.push(WorkshopSourceFile {
            path: relative,
            source: fs::read_to_string(&entry)
                .map_err(|error| format!("failed reading {}: {error}", entry.display()))?,
        });
    }
    for directory in ["src", "tests"] {
        collect_workshop_source_files(
            project_root,
            &project_root.join(directory),
            &mut known,
            &mut files,
        )?;
    }
    files.sort_by_key(|file| file.path.clone());
    Ok(files)
}

pub fn workshop_reachable_files(
    files: &[WorkshopSourceFile],
    entry_file: &Path,
) -> Result<Vec<WorkshopSourceFile>, String> {
    let by_path = files
        .iter()
        .map(|file| (normalize_project_path_text(&file.path), file))
        .collect::<BTreeMap<_, _>>();
    let entry = normalize_project_path_text(&entry_file.to_string_lossy());
    let (graph, _) = crate::frontend::module_graph::ModuleGraph::load([entry], |path| {
        by_path
            .get(path)
            .map(|file| file.source.clone())
            .ok_or_else(|| format!("import graph file is not loaded: {path}"))
    })
    .map_err(|diagnostic| diagnostic.message)?;
    let mut out = graph
        .modules()
        .keys()
        .filter_map(|path| by_path.get(path).map(|file| (*file).clone()))
        .collect::<Vec<_>>();
    out.sort_by_key(|file| file.path.clone());
    Ok(out)
}

pub fn workshop_direct_import_files(
    files: &[WorkshopSourceFile],
    file_path: &Path,
) -> Result<Vec<String>, String> {
    let normalized = normalize_project_path_text(&file_path.to_string_lossy());
    let by_path = files
        .iter()
        .map(|file| (normalize_project_path_text(&file.path), file))
        .collect::<BTreeMap<_, _>>();
    let (graph, _) =
        crate::frontend::module_graph::ModuleGraph::load([normalized.clone()], |path| {
            by_path
                .get(path)
                .map(|file| file.source.clone())
                .ok_or_else(|| format!("import graph file is not loaded: {path}"))
        })
        .map_err(|diagnostic| diagnostic.message)?;
    Ok(graph
        .direct_dependencies(&normalized)
        .into_iter()
        .map(str::to_string)
        .collect())
}

fn collect_workshop_source_files(
    project_root: &Path,
    directory: &Path,
    known: &mut BTreeSet<String>,
    out: &mut Vec<WorkshopSourceFile>,
) -> Result<(), String> {
    if !directory.exists() {
        return Ok(());
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("failed reading {}: {error}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed enumerating {}: {error}", directory.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed inspecting {}: {error}", path.display()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_workshop_source_files(project_root, &path, known, out)?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("stasis") {
            continue;
        }
        let relative = normalize_workshop_project_path(project_root, &path);
        if known.insert(relative.clone()) {
            out.push(WorkshopSourceFile {
                path: relative,
                source: fs::read_to_string(&path)
                    .map_err(|error| format!("failed reading {}: {error}", path.display()))?,
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedSimpleSymbol {
    kind: WorkshopSymbolKind,
    name: String,
    signature: String,
    range: Range<usize>,
}

fn parse_simple_top_level_symbols(source: &str) -> Result<Vec<ParsedSimpleSymbol>, String> {
    let tokens = lex(source)?;
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut depth = 0usize;
    while cursor < tokens.len() {
        let token = tokens[cursor];
        match token.kind {
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => depth = depth.saturating_sub(1),
            TokenKind::Identifier if depth == 0 => {
                let keyword = token_text(source, token);
                let kind = match keyword {
                    "global" => WorkshopSymbolKind::Global,
                    "const" => WorkshopSymbolKind::Constant,
                    "test" => WorkshopSymbolKind::Test,
                    _ => {
                        cursor += 1;
                        continue;
                    }
                };
                let (name, signature, end_index) = match kind {
                    WorkshopSymbolKind::Global => {
                        let name_token = expect_token(&tokens, cursor + 1, TokenKind::Identifier)?;
                        let name = token_text(source, name_token).to_string();
                        if tokens
                            .get(cursor + 2)
                            .is_some_and(|token| token.kind == TokenKind::LBrace)
                        {
                            let open = cursor + 2;
                            let close = find_matching_rbrace(&tokens, open + 1, 1)?;
                            (name.clone(), format!("global {name}"), close)
                        } else {
                            let end = find_next_token(&tokens, cursor + 2, TokenKind::Semicolon)?;
                            let signature = source[token.start..tokens[end].end].trim().to_string();
                            (name, signature, end)
                        }
                    }
                    WorkshopSymbolKind::Constant => {
                        let name_token = expect_token(&tokens, cursor + 1, TokenKind::Identifier)?;
                        let end = find_next_token(&tokens, cursor + 2, TokenKind::Semicolon)?;
                        let name = token_text(source, name_token).to_string();
                        let signature = source[token.start..tokens[end].end].trim().to_string();
                        (name, signature, end)
                    }
                    WorkshopSymbolKind::Test => {
                        let open = find_next_token(&tokens, cursor + 1, TokenKind::LBrace)?;
                        let close = find_matching_rbrace(&tokens, open + 1, 1)?;
                        let header = &source[token.end..tokens[open].start];
                        let first_tick = header.find('`').ok_or_else(|| {
                            "test declaration must contain a backtick-quoted name".to_string()
                        })?;
                        let rest = &header[first_tick + 1..];
                        let second_tick = rest.find('`').ok_or_else(|| {
                            "test declaration must contain a closing backtick".to_string()
                        })?;
                        let name = rest[..second_tick].to_string();
                        let signature = format!("test `{name}`");
                        (name, signature, close)
                    }
                    WorkshopSymbolKind::Struct | WorkshopSymbolKind::Function => unreachable!(),
                };
                out.push(ParsedSimpleSymbol {
                    kind,
                    name,
                    signature,
                    range: token.start..tokens[end_index].end,
                });
                cursor = end_index + 1;
                continue;
            }
            _ => {}
        }
        cursor += 1;
    }
    Ok(out)
}

fn function_owner(
    path: &str,
    function_name: &str,
    first_param_type: Option<&str>,
    return_type: &str,
    file_structs: &[String],
    struct_names: &BTreeSet<String>,
) -> Option<String> {
    if is_lifecycle_function(function_name) || is_system_path(path) || is_root_path(path) {
        return None;
    }

    if let Some(param_type) = first_param_type {
        if struct_names.contains(param_type) {
            return Some(param_type.to_string());
        }
    }

    if file_structs.iter().any(|name| name == return_type) {
        return Some(return_type.to_string());
    }

    if file_structs.len() == 1 && stem_matches_struct(path, &file_structs[0]) {
        return Some(file_structs[0].clone());
    }

    None
}

fn function_group(
    path: &str,
    function_name: &str,
    owner: Option<&str>,
) -> (WorkshopSymbolGroupKind, String) {
    if is_lifecycle_function(function_name) || is_main_path(path) {
        return (WorkshopSymbolGroupKind::Main, "Main".to_string());
    }
    if let Some(owner) = owner {
        return (WorkshopSymbolGroupKind::Struct, owner.to_string());
    }
    if is_system_path(path) {
        return (
            WorkshopSymbolGroupKind::System,
            title_case_stem(path).unwrap_or_else(|| "System".to_string()),
        );
    }
    (WorkshopSymbolGroupKind::Root, "Root".to_string())
}

fn format_function_signature(
    name: &str,
    params: &[crate::frontend::parser::ParsedParam],
    return_type_name: &str,
) -> String {
    let params = params
        .iter()
        .map(|param| format!("{}: {}", param.name, param.type_name))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{name}({params}): {return_type_name}")
}

fn is_lifecycle_function(name: &str) -> bool {
    matches!(name, "main" | "init" | "tick" | "render" | "on_code_swap")
}

fn is_main_path(path: &str) -> bool {
    file_stem(path).is_some_and(|stem| stem == "main")
}

fn is_root_path(path: &str) -> bool {
    file_stem(path).is_some_and(|stem| stem == "root")
}

fn is_system_path(path: &str) -> bool {
    path.replace('\\', "/").contains("/systems/")
}

fn stem_matches_struct(path: &str, struct_name: &str) -> bool {
    file_stem(path).is_some_and(|stem| stem == snake_case(struct_name))
}

fn title_case_stem(path: &str) -> Option<String> {
    file_stem(path).map(|stem| {
        stem.split('_')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<String>()
    })
}

fn file_stem(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
}

fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn source_for_range(source: &str, range: Range<usize>) -> Result<String, String> {
    source
        .get(range)
        .map(str::to_string)
        .ok_or_else(|| "invalid workshop symbol source span".to_string())
}

fn span_from_range(range: Range<usize>) -> Result<WorkshopSourceSpan, String> {
    Ok(WorkshopSourceSpan {
        start: u32::try_from(range.start)
            .map_err(|_| "symbol span start exceeds u32".to_string())?,
        end: u32::try_from(range.end).map_err(|_| "symbol span end exceeds u32".to_string())?,
    })
}

fn expect_token(tokens: &[Token], cursor: usize, kind: TokenKind) -> Result<Token, String> {
    let token = tokens
        .get(cursor)
        .copied()
        .ok_or_else(|| format!("unexpected end of token stream, expected {kind:?}"))?;
    if token.kind != kind {
        return Err(format!(
            "expected token {kind:?} but found {:?}",
            token.kind
        ));
    }
    Ok(token)
}

fn find_next_token(tokens: &[Token], start: usize, kind: TokenKind) -> Result<usize, String> {
    tokens
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, token)| (token.kind == kind).then_some(index))
        .ok_or_else(|| format!("expected token {kind:?}"))
}

fn find_matching_rbrace(tokens: &[Token], start: usize, mut depth: usize) -> Result<usize, String> {
    let mut cursor = start;
    while cursor < tokens.len() {
        match tokens[cursor].kind {
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => {
                depth -= 1;
                if depth == 0 {
                    return Ok(cursor);
                }
            }
            TokenKind::Eof => break,
            _ => {}
        }
        cursor += 1;
    }
    Err("missing closing '}' for struct body".to_string())
}

fn token_text<'a>(source: &'a str, token: Token) -> &'a str {
    &source[token.start..token.end]
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSymbolSelector {
    /// Canonical v1 SymbolId. Legacy tuple fields are accepted only for schema v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_id: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<WorkshopSourceItemKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopSourceItemKind {
    Imports,
    Globals,
    Struct,
    Function,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSourceItem {
    pub symbol_id: String,
    pub kind: WorkshopSourceItemKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub file: String,
    pub signature: String,
    pub source_spans: Vec<WorkshopSourceSpan>,
    pub source: String,
    pub source_hash: String,
    #[serde(default, skip_serializing_if = "WorkshopExposure::is_public")]
    pub exposure: WorkshopExposure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopReferenceKind {
    Definition,
    Read,
    Write,
    Call,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopReference {
    pub symbol: String,
    pub kind: WorkshopReferenceKind,
    pub file: String,
    pub source_span: WorkshopSourceSpan,
    pub containing_kind: WorkshopSourceItemKind,
    pub containing_name: String,
    pub containing_signature: String,
    pub containing_source_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopRenameEdit {
    pub file: String,
    pub source_span: WorkshopSourceSpan,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopRenamePlan {
    pub old_name: String,
    pub new_name: String,
    pub kind: String,
    pub owner: Option<String>,
    pub request_span: WorkshopSourceSpan,
    pub edits: Vec<WorkshopRenameEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopRenamePreparation {
    pub name: String,
    pub kind: String,
    pub owner: Option<String>,
    pub request_span: WorkshopSourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopCompletionItem {
    pub text: String,
    pub kind: String,
    pub detail: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<WorkshopCompletionScope>,
    #[serde(default, skip_serializing_if = "WorkshopExposure::is_public")]
    pub exposure: WorkshopExposure,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopCompletionScope {
    pub owner: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_end: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration_from: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration_to: Option<usize>,
    pub visible_from: usize,
    pub visible_to: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopSemanticToken {
    pub text: String,
    pub kind: String,
    pub source_span: WorkshopSourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkshopInlayHintKind {
    Type,
    Parameter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopInlayHint {
    pub file: String,
    pub source_span: WorkshopSourceSpan,
    pub kind: WorkshopInlayHintKind,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopHierarchyItem {
    pub symbol_id: String,
    pub name: String,
    pub detail: String,
    pub file: String,
    pub source_span: WorkshopSourceSpan,
    pub selection_span: WorkshopSourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopCallHierarchyEdge {
    pub caller_symbol_id: String,
    pub callee_symbol_id: String,
    pub call_span: WorkshopSourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopTypeHierarchyEdge {
    pub container_symbol_id: String,
    pub component_symbol_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopFoldingRange {
    pub source_span: WorkshopSourceSpan,
}

pub fn workshop_folding_ranges(source: &str) -> Result<Vec<WorkshopFoldingRange>, String> {
    let mut ranges = workshop_delimiter_ranges(source)?
        .into_iter()
        .filter(|(delimiter, _)| *delimiter == '{')
        .map(|(_, range)| {
            Ok(WorkshopFoldingRange {
                source_span: span_from_range(range)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    ranges.sort_by_key(|range| (range.source_span.start, range.source_span.end));
    Ok(ranges)
}

pub fn workshop_selection_ranges(
    source: &str,
    byte_offset: usize,
) -> Result<Vec<WorkshopSourceSpan>, String> {
    if byte_offset > source.len() || !source.is_char_boundary(byte_offset) {
        return Err(format!("selection offset {byte_offset} is invalid"));
    }
    let tokens = lex(source)?;
    let mut ranges = tokens
        .iter()
        .filter(|token| token.kind != TokenKind::Eof)
        .filter(|token| token.start <= byte_offset && byte_offset < token.end)
        .map(|token| token.start..token.end)
        .collect::<Vec<_>>();
    ranges.extend(
        workshop_delimiter_ranges(source)?
            .into_iter()
            .map(|(_, range)| range)
            .filter(|range| range.start <= byte_offset && byte_offset <= range.end),
    );
    ranges.push(0..source.len());
    ranges.sort_by_key(|range| (range.end.saturating_sub(range.start), range.start));
    ranges.dedup();
    let mut nested = Vec::<Range<usize>>::new();
    for range in ranges {
        if nested.last().is_none_or(|child| {
            range.start <= child.start && child.end <= range.end && *child != range
        }) {
            nested.push(range);
        }
    }
    nested
        .into_iter()
        .map(span_from_range)
        .collect::<Result<Vec<_>, _>>()
}

fn workshop_delimiter_ranges(source: &str) -> Result<Vec<(char, Range<usize>)>, String> {
    let tokens = lex(source)?;
    let mut stack = Vec::<(char, usize)>::new();
    let mut ranges = Vec::new();
    for token in tokens {
        let text = token_text(source, token);
        let character = match token.kind {
            TokenKind::LBrace => Some('{'),
            TokenKind::LParen => Some('('),
            TokenKind::Other if text == "[" => Some('['),
            _ => None,
        };
        if let Some(character) = character {
            stack.push((character, token.start));
            continue;
        }
        let expected = match token.kind {
            TokenKind::RBrace => Some('{'),
            TokenKind::RParen => Some('('),
            TokenKind::Other if text == "]" => Some('['),
            _ => None,
        };
        let Some(expected) = expected else {
            continue;
        };
        if stack.last().is_some_and(|(open, _)| *open == expected) {
            let (_, start) = stack.pop().expect("matching delimiter remains on stack");
            ranges.push((expected, start..token.end));
        }
    }
    Ok(ranges)
}

pub fn workshop_call_hierarchy(
    files: &[WorkshopSourceFile],
) -> Result<(Vec<WorkshopHierarchyItem>, Vec<WorkshopCallHierarchyEdge>), String> {
    let symbols = workshop_symbols(files)?;
    let mut compiler = Compiler::new();
    for file in files {
        compiler.upsert_file(file.path.clone(), file.source.clone());
    }
    compiler
        .index_pass()
        .map_err(|error| format!("{error:?}"))?;

    let functions = compiler.functions();
    let by_id = functions
        .iter()
        .map(|function| (function.id, function.symbol_id.to_string()))
        .collect::<BTreeMap<_, _>>();
    let items = symbols
        .iter()
        .filter(|symbol| symbol.kind == WorkshopSymbolKind::Function)
        .map(|symbol| hierarchy_item(files, symbol))
        .collect::<Result<Vec<_>, _>>()?;
    let mut edges = Vec::new();
    for caller in functions {
        let caller_symbol_id = caller.symbol_id.to_string();
        for site in &caller.call_sites {
            let Some(callee_symbol_id) = by_id.get(&site.callee) else {
                continue;
            };
            edges.push(WorkshopCallHierarchyEdge {
                caller_symbol_id: caller_symbol_id.clone(),
                callee_symbol_id: callee_symbol_id.clone(),
                call_span: WorkshopSourceSpan {
                    start: site.source_range.start,
                    end: site.source_range.end,
                },
            });
        }
    }
    edges.sort_by_key(|edge| {
        (
            edge.caller_symbol_id.clone(),
            edge.call_span.start,
            edge.callee_symbol_id.clone(),
        )
    });
    edges.dedup();
    Ok((items, edges))
}

pub fn workshop_type_hierarchy(
    files: &[WorkshopSourceFile],
) -> Result<(Vec<WorkshopHierarchyItem>, Vec<WorkshopTypeHierarchyEdge>), String> {
    let symbols = workshop_symbols(files)?;
    let struct_symbols = symbols
        .iter()
        .filter(|symbol| symbol.kind == WorkshopSymbolKind::Struct)
        .map(|symbol| (symbol.name.clone(), symbol))
        .collect::<BTreeMap<_, _>>();
    let items = struct_symbols
        .values()
        .map(|symbol| hierarchy_item(files, symbol))
        .collect::<Result<Vec<_>, _>>()?;
    let mut edges = Vec::new();
    for file in files {
        for definition in source_workshop_items(&file.source)?.layout.structs {
            let Some(container) = struct_symbols.get(&definition.name) else {
                continue;
            };
            for field in definition.fields {
                let component_name = workshop_base_type(&field.type_name);
                let Some(component) = struct_symbols.get(component_name) else {
                    continue;
                };
                edges.push(WorkshopTypeHierarchyEdge {
                    container_symbol_id: container.symbol_id.clone(),
                    component_symbol_id: component.symbol_id.clone(),
                });
            }
        }
    }
    edges.sort_by_key(|edge| {
        (
            edge.container_symbol_id.clone(),
            edge.component_symbol_id.clone(),
        )
    });
    edges.dedup();
    Ok((items, edges))
}

fn hierarchy_item(
    files: &[WorkshopSourceFile],
    symbol: &WorkshopSymbol,
) -> Result<WorkshopHierarchyItem, String> {
    let file = files
        .iter()
        .find(|file| file.path == symbol.file)
        .ok_or_else(|| format!("hierarchy source file is missing: {}", symbol.file))?;
    let selection = lex(&file.source)?
        .into_iter()
        .find(|token| {
            token.kind == TokenKind::Identifier
                && symbol.source_span.start as usize <= token.start
                && token.end <= symbol.source_span.end as usize
                && token_text(&file.source, *token) == symbol.name
        })
        .ok_or_else(|| format!("hierarchy declaration name is missing: {}", symbol.name))?;
    Ok(WorkshopHierarchyItem {
        symbol_id: symbol.symbol_id.clone(),
        name: symbol.name.clone(),
        detail: symbol.signature.clone(),
        file: symbol.file.clone(),
        source_span: symbol.source_span.clone(),
        selection_span: span_from_range(selection.start..selection.end)?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopSemanticEditOperation {
    Add,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSemanticEdit {
    pub operation: WorkshopSemanticEditOperation,
    pub target: WorkshopSymbolSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_source_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSemanticEditBatch {
    #[serde(default = "semantic_edit_schema_version")]
    pub schema_version: u32,
    pub edits: Vec<WorkshopSemanticEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSemanticFileChange {
    pub file: String,
    pub before_source: String,
    pub after_source: String,
    pub before_hash: String,
    pub after_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopSemanticEditPlan {
    pub schema_version: u32,
    pub edits: Vec<WorkshopSemanticEdit>,
    pub changed_files: Vec<WorkshopSemanticFileChange>,
    pub reload: WorkshopReloadClassification,
}

pub fn organize_workshop_imports(
    files: &[WorkshopSourceFile],
    file_path: &str,
) -> Result<Option<WorkshopSemanticFileChange>, String> {
    let before = files
        .iter()
        .find(|file| file.path == file_path)
        .ok_or_else(|| format!("organize-imports file is not loaded: {file_path}"))?;
    let mut after = files.to_vec();
    prune_unused_workshop_imports(&mut after, &BTreeSet::from([file_path.to_string()]))?;

    let file_index = after
        .iter()
        .position(|file| file.path == file_path)
        .expect("organize-imports file remains loaded");
    let rendered = render_imports(parse_workshop_import_paths(&after[file_index].source)?);
    let import_item = workshop_source_items(&after)?
        .into_iter()
        .find(|item| item.file == file_path && item.kind == WorkshopSourceItemKind::Imports)
        .ok_or_else(|| format!("imports item not found for {file_path}"))?;
    if import_item.source != rendered {
        replace_source_item(&mut after, &import_item, &rendered)?;
    }

    let after = &after[file_index];
    if after.source == before.source {
        return Ok(None);
    }
    Ok(Some(WorkshopSemanticFileChange {
        file: file_path.to_string(),
        before_source: before.source.clone(),
        after_source: after.source.clone(),
        before_hash: workshop_source_hash(&before.source),
        after_hash: workshop_source_hash(&after.source),
    }))
}

const fn semantic_edit_schema_version() -> u32 {
    2
}

pub fn workshop_symbols(files: &[WorkshopSourceFile]) -> Result<Vec<WorkshopSymbol>, String> {
    let mut symbols = build_workshop_symbol_tree(files)?
        .groups
        .into_iter()
        .flat_map(|group| group.symbols)
        .collect::<Vec<_>>();
    symbols.sort_by_key(|symbol| {
        (
            symbol.file.clone(),
            symbol.source_span.start,
            symbol.name.clone(),
        )
    });
    Ok(symbols)
}

pub fn workshop_source_items(
    files: &[WorkshopSourceFile],
) -> Result<Vec<WorkshopSourceItem>, String> {
    let symbols = workshop_symbols(files)?;
    let mut items = Vec::new();
    for file in files {
        let canonical_path = CanonicalSourcePath::project_relative(&file.path)?;
        let imports = parse_import_spans(&file.source)?;
        items.push(source_item_from_ranges(
            file,
            WorkshopSourceItemKind::Imports,
            "imports",
            None,
            "imports",
            imports,
            false,
            Some(
                SymbolId::declaration("tool_group", &canonical_path, "imports", "group")
                    .to_string(),
            ),
        )?);

        let globals = parse_simple_top_level_symbols(&file.source)?
            .into_iter()
            .filter(|parsed| {
                matches!(
                    parsed.kind,
                    WorkshopSymbolKind::Global | WorkshopSymbolKind::Constant
                )
            })
            .map(|parsed| parsed.range)
            .collect::<Vec<_>>();
        items.push(source_item_from_ranges(
            file,
            WorkshopSourceItemKind::Globals,
            "globals",
            Some("Globals".to_string()),
            "globals",
            globals,
            true,
            Some(
                SymbolId::declaration("tool_group", &canonical_path, "globals", "group")
                    .to_string(),
            ),
        )?);

        for symbol in symbols.iter().filter(|symbol| symbol.file == file.path) {
            let kind = match symbol.kind {
                WorkshopSymbolKind::Struct => WorkshopSourceItemKind::Struct,
                WorkshopSymbolKind::Function => WorkshopSourceItemKind::Function,
                WorkshopSymbolKind::Test => WorkshopSourceItemKind::Test,
                WorkshopSymbolKind::Global | WorkshopSymbolKind::Constant => continue,
            };
            let mut item = source_item_from_ranges(
                file,
                kind,
                &symbol.name,
                symbol.owner.clone(),
                &symbol.signature,
                vec![symbol.source_span.start as usize..symbol.source_span.end as usize],
                matches!(
                    kind,
                    WorkshopSourceItemKind::Struct | WorkshopSourceItemKind::Function
                ),
                Some(symbol.symbol_id.clone()),
            )?;
            item.exposure = symbol.exposure;
            items.push(item);
        }
    }
    items.sort_by_key(|item| {
        let order = match item.kind {
            WorkshopSourceItemKind::Imports => 0,
            WorkshopSourceItemKind::Globals => 1,
            WorkshopSourceItemKind::Struct => 2,
            WorkshopSourceItemKind::Function => 3,
            WorkshopSourceItemKind::Test => 4,
        };
        let start = item
            .source_spans
            .first()
            .map(|span| span.start)
            .unwrap_or(0);
        (item.file.clone(), order, start, item.name.clone())
    });
    Ok(items)
}

pub fn find_workshop_references(
    files: &[WorkshopSourceFile],
    symbol: &str,
    limit: usize,
) -> Result<Vec<WorkshopReference>, String> {
    let segments = symbol.split('.').collect::<Vec<_>>();
    if segments.is_empty()
        || segments.len() > 8
        || segments
            .iter()
            .any(|segment| !is_workshop_identifier(segment))
    {
        return Err("reference symbol must be 1..=8 dot-separated identifiers".to_string());
    }
    let limit = limit.clamp(1, 256);
    let items = workshop_source_items(files)?;
    let definition = match field_definition_reference(files, &segments, &items)? {
        Some(reference) => Some(reference),
        None => global_definition_reference(files, &segments, &items)?,
    };
    let mut references = definition.into_iter().collect::<Vec<_>>();
    for file in files {
        let tokens = lex(&file.source)?;
        for start_index in 0..tokens.len() {
            let Some(end_index) =
                reference_match_end(&file.source, &tokens, start_index, &segments)
            else {
                continue;
            };
            let start = tokens[start_index].start;
            let end = tokens[end_index].end;
            let Some(item) = items
                .iter()
                .filter(|item| {
                    item.file == file.path
                        && item
                            .source_spans
                            .iter()
                            .any(|span| span.start as usize <= start && end <= span.end as usize)
                })
                .min_by_key(|item| {
                    item.source_spans
                        .iter()
                        .map(|span| span.end.saturating_sub(span.start))
                        .min()
                        .unwrap_or(u32::MAX)
                })
            else {
                continue;
            };
            let kind =
                classify_workshop_reference(&file.source, &tokens, end_index, item, symbol, start);
            if references.iter().any(|reference| {
                reference.file == file.path
                    && reference.source_span.start as usize == start
                    && reference.source_span.end as usize == end
            }) {
                continue;
            }
            references.push(WorkshopReference {
                symbol: symbol.to_string(),
                kind,
                file: file.path.clone(),
                source_span: WorkshopSourceSpan {
                    start: u32::try_from(start)
                        .map_err(|_| "reference start exceeds u32".to_string())?,
                    end: u32::try_from(end).map_err(|_| "reference end exceeds u32".to_string())?,
                },
                containing_kind: item.kind,
                containing_name: item.name.clone(),
                containing_signature: item.signature.clone(),
                containing_source_hash: item.source_hash.clone(),
            });
            if references.len() >= limit {
                return Ok(references);
            }
        }
    }
    Ok(references)
}

pub fn plan_workshop_rename(
    files: &[WorkshopSourceFile],
    request_file: &str,
    byte_offset: usize,
    new_name: &str,
) -> Result<(Vec<WorkshopSourceFile>, WorkshopRenamePlan), String> {
    if !is_workshop_identifier(new_name) {
        return Err("rename target must be an ASCII Stasis identifier".to_string());
    }
    let normalized_file = normalize_project_path_text(request_file);
    let file = files
        .iter()
        .find(|file| normalize_project_path_text(&file.path) == normalized_file)
        .ok_or_else(|| format!("rename file is not indexed: {request_file}"))?;
    let (request_token, semantic_path) = rename_symbol_at(&file.source, byte_offset)?;
    let old_name = token_text(&file.source, request_token).to_string();
    if old_name == new_name {
        return Err("rename target already has the requested name".to_string());
    }
    let catalog = workshop_completion_items(files)?;
    let target =
        resolve_workshop_rename_target(files, &catalog, file, request_token, &semantic_path)?;
    reject_workshop_rename_collision(&catalog, &target, new_name)?;
    let mut references = rename_target_references(files, &catalog, &target)?;
    references.insert((file.path.clone(), request_token.start, request_token.end));
    let edits = references
        .into_iter()
        .map(|(file, start, end)| {
            Ok(WorkshopRenameEdit {
                file,
                source_span: WorkshopSourceSpan {
                    start: u32::try_from(start)
                        .map_err(|_| "rename edit start exceeds u32".to_string())?,
                    end: u32::try_from(end)
                        .map_err(|_| "rename edit end exceeds u32".to_string())?,
                },
                new_text: new_name.to_string(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if edits.is_empty() {
        return Err("rename target has no compiler-owned source locations".to_string());
    }
    let plan = WorkshopRenamePlan {
        old_name,
        new_name: new_name.to_string(),
        kind: target.kind,
        owner: target.owner,
        request_span: WorkshopSourceSpan {
            start: u32::try_from(request_token.start)
                .map_err(|_| "rename request start exceeds u32".to_string())?,
            end: u32::try_from(request_token.end)
                .map_err(|_| "rename request end exceeds u32".to_string())?,
        },
        edits,
    };
    let mut after = files.to_vec();
    apply_workshop_rename_edits(&mut after, &plan)?;
    build_workshop_symbol_tree(&after)?;
    workshop_completion_items(&after)?;
    Ok((after, plan))
}

pub fn prepare_workshop_rename(
    files: &[WorkshopSourceFile],
    request_file: &str,
    byte_offset: usize,
) -> Result<WorkshopRenamePreparation, String> {
    let normalized_file = normalize_project_path_text(request_file);
    let file = files
        .iter()
        .find(|file| normalize_project_path_text(&file.path) == normalized_file)
        .ok_or_else(|| format!("rename file is not indexed: {request_file}"))?;
    let (request_token, semantic_path) = rename_symbol_at(&file.source, byte_offset)?;
    let catalog = workshop_completion_items(files)?;
    let target =
        resolve_workshop_rename_target(files, &catalog, file, request_token, &semantic_path)?;
    Ok(WorkshopRenamePreparation {
        name: token_text(&file.source, request_token).to_string(),
        kind: target.kind,
        owner: target.owner,
        request_span: WorkshopSourceSpan {
            start: u32::try_from(request_token.start)
                .map_err(|_| "rename request start exceeds u32".to_string())?,
            end: u32::try_from(request_token.end)
                .map_err(|_| "rename request end exceeds u32".to_string())?,
        },
    })
}

pub fn workshop_linked_edit_ranges(
    files: &[WorkshopSourceFile],
    request_file: &str,
    byte_offset: usize,
) -> Result<Option<Vec<WorkshopSourceSpan>>, String> {
    let normalized_file = normalize_project_path_text(request_file);
    let file = files
        .iter()
        .find(|file| normalize_project_path_text(&file.path) == normalized_file)
        .ok_or_else(|| format!("linked-edit file is not indexed: {request_file}"))?;
    let (request_token, semantic_path) = rename_symbol_at(&file.source, byte_offset)?;
    let catalog = workshop_completion_items(files)?;
    let target =
        resolve_workshop_rename_target(files, &catalog, file, request_token, &semantic_path)?;
    if !matches!(target.kind.as_str(), "local" | "parameter") {
        return Ok(None);
    }
    let mut references = rename_target_references(files, &catalog, &target)?;
    references.insert((file.path.clone(), request_token.start, request_token.end));
    let ranges = references
        .into_iter()
        .filter(|(path, _, _)| normalize_project_path_text(path) == normalized_file)
        .map(|(_, start, end)| span_from_range(start..end))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((ranges.len() > 1).then_some(ranges))
}

#[derive(Clone)]
struct WorkshopRenameTarget {
    kind: String,
    name: String,
    owner: Option<String>,
    file: String,
    scope: Option<WorkshopCompletionScope>,
    signature: Option<String>,
}

fn rename_symbol_at(source: &str, byte_offset: usize) -> Result<(Token, String), String> {
    if byte_offset > source.len() || !source.is_char_boundary(byte_offset) {
        return Err(format!("rename offset {byte_offset} is invalid"));
    }
    let tokens = lex(source)?;
    let token = tokens
        .iter()
        .copied()
        .find(|token| {
            token.kind == TokenKind::Identifier
                && token.start <= byte_offset
                && byte_offset <= token.end
        })
        .ok_or_else(|| "no renameable Stasis identifier at position".to_string())?;
    Ok((token, semantic_path_for_token(source, token)?))
}

fn semantic_path_for_token(source: &str, token: Token) -> Result<String, String> {
    let line_start = source[..token.start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let prefix = &source[line_start..token.end];
    let mut start = prefix.len();
    let mut bracket_depth = 0usize;
    for (index, character) in prefix.char_indices().rev() {
        match character {
            ']' => bracket_depth = bracket_depth.saturating_add(1),
            '[' if bracket_depth > 0 => bracket_depth -= 1,
            _ if bracket_depth > 0 => {}
            character
                if character.is_ascii_alphanumeric() || character == '_' || character == '.' => {}
            _ => {
                start = index + character.len_utf8();
                break;
            }
        }
        start = index;
    }
    let mut path = String::new();
    bracket_depth = 0;
    for character in prefix[start..].chars() {
        match character {
            '[' => bracket_depth = bracket_depth.saturating_add(1),
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ if bracket_depth > 0 => {}
            _ => path.push(character),
        }
    }
    if path
        .split('.')
        .any(|segment| !is_workshop_identifier(segment))
    {
        return Err("rename target is not a semantic identifier path".to_string());
    }
    Ok(path)
}

pub fn workshop_semantic_tokens(
    files: &[WorkshopSourceFile],
    request_file: &str,
) -> Result<Vec<WorkshopSemanticToken>, String> {
    let normalized_file = normalize_project_path_text(request_file);
    let file = files
        .iter()
        .find(|file| normalize_project_path_text(&file.path) == normalized_file)
        .ok_or_else(|| format!("semantic-token file is not indexed: {request_file}"))?;
    let catalog = workshop_completion_items(files)?;
    let tokens = lex(&file.source)?;
    let mut semantic_tokens = Vec::new();
    for token in tokens
        .into_iter()
        .filter(|token| token.kind == TokenKind::Identifier)
    {
        let Ok(semantic_path) = semantic_path_for_token(&file.source, token) else {
            continue;
        };
        let Ok(target) =
            resolve_workshop_rename_target(files, &catalog, file, token, &semantic_path)
        else {
            continue;
        };
        semantic_tokens.push(WorkshopSemanticToken {
            text: token_text(&file.source, token).to_string(),
            kind: target.kind,
            source_span: WorkshopSourceSpan {
                start: u32::try_from(token.start)
                    .map_err(|_| "semantic token start exceeds u32".to_string())?,
                end: u32::try_from(token.end)
                    .map_err(|_| "semantic token end exceeds u32".to_string())?,
            },
        });
    }
    Ok(semantic_tokens)
}

pub fn workshop_inlay_hints(
    files: &[WorkshopSourceFile],
) -> Result<Vec<WorkshopInlayHint>, String> {
    let mut compiler = Compiler::new();
    for file in files {
        compiler.upsert_file(file.path.clone(), file.source.clone());
    }
    let locals = compiler
        .local_types()
        .map_err(|error| format!("inlay type analysis failed: {error:?}"))?;
    workshop_inlay_hints_from_local_types(files, &locals)
}

pub fn workshop_inlay_hints_from_local_types(
    files: &[WorkshopSourceFile],
    locals: &[CompilerLocalType],
) -> Result<Vec<WorkshopInlayHint>, String> {
    let mut inferred = BTreeMap::<(String, String, String), VecDeque<String>>::new();
    for local in locals.iter().filter(|local| local.inferred) {
        inferred
            .entry((
                local.file.clone(),
                local.function.clone(),
                local.name.clone(),
            ))
            .or_default()
            .push_back(local.type_name.clone());
    }

    let catalog = workshop_completion_items(files)?;
    let mut hints = Vec::new();
    for file in files {
        let functions = parse_top_level_functions(&file.source)?;
        let tokens = lex(&file.source)?;
        for function in &functions {
            let mut cursor =
                tokens.partition_point(|token| token.start < function.body_range.start);
            while cursor + 2 < tokens.len() && tokens[cursor].start < function.body_range.end {
                let token = tokens[cursor];
                if token.kind == TokenKind::Identifier && token_text(&file.source, token) == "let" {
                    let name = tokens[cursor + 1];
                    let annotation = tokens[cursor + 2];
                    if name.kind == TokenKind::Identifier && annotation.kind != TokenKind::Colon {
                        let key = (
                            file.path.clone(),
                            function.name.clone(),
                            token_text(&file.source, name).to_string(),
                        );
                        if let Some(type_name) =
                            inferred.get_mut(&key).and_then(VecDeque::pop_front)
                        {
                            hints.push(WorkshopInlayHint {
                                file: file.path.clone(),
                                source_span: span_from_range(name.start..name.end)?,
                                kind: WorkshopInlayHintKind::Type,
                                label: format!(": {type_name}"),
                            });
                        }
                    }
                }
                cursor += 1;
            }
        }

        for (index, token) in tokens.iter().copied().enumerate() {
            if token.kind != TokenKind::Identifier
                || !tokens
                    .get(index + 1)
                    .is_some_and(|next| next.kind == TokenKind::LParen)
                || functions.iter().any(|function| {
                    function.name == token_text(&file.source, token)
                        && function.signature_range.start <= token.start
                        && token.end <= function.signature_range.end
                })
            {
                continue;
            }
            let Ok(semantic_path) = semantic_path_for_token(&file.source, token) else {
                continue;
            };
            let Ok(target) =
                resolve_workshop_rename_target(files, &catalog, file, token, &semantic_path)
            else {
                continue;
            };
            if !matches!(target.kind.as_str(), "function" | "method") {
                continue;
            }
            let Some(signature) = target.signature.as_deref() else {
                continue;
            };
            let Some(arguments) = call_argument_spans(&file.source, &tokens, index + 1) else {
                continue;
            };
            let mut parameters = signature_parameter_names(signature);
            if target.kind == "method" && parameters.len() == arguments.len() + 1 {
                parameters.remove(0);
            }
            for (argument, parameter) in arguments.into_iter().zip(parameters) {
                if file.source[argument.clone()].trim() == parameter {
                    continue;
                }
                hints.push(WorkshopInlayHint {
                    file: file.path.clone(),
                    source_span: span_from_range(argument)?,
                    kind: WorkshopInlayHintKind::Parameter,
                    label: format!("{parameter}:"),
                });
            }
        }
    }
    hints.sort_by_key(|hint| (hint.file.clone(), hint.source_span.start, hint.kind as u8));
    Ok(hints)
}

fn signature_parameter_names(signature: &str) -> Vec<String> {
    let Some(open) = signature.find('(') else {
        return Vec::new();
    };
    let Some(close) = signature[open + 1..]
        .find(')')
        .map(|index| open + 1 + index)
    else {
        return Vec::new();
    };
    signature[open + 1..close]
        .split(',')
        .filter_map(|parameter| parameter.split_once(':').map(|(name, _)| name.trim()))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

fn call_argument_spans(
    source: &str,
    tokens: &[Token],
    open_index: usize,
) -> Option<Vec<Range<usize>>> {
    let mut arguments = Vec::new();
    let mut argument_start = None;
    let mut argument_end = None;
    let mut paren_depth = 1usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    for token in tokens.iter().copied().skip(open_index + 1) {
        match token.kind {
            TokenKind::LParen => paren_depth += 1,
            TokenKind::RParen => {
                paren_depth = paren_depth.checked_sub(1)?;
                if paren_depth == 0 {
                    if let (Some(start), Some(end)) = (argument_start, argument_end) {
                        arguments.push(start..end);
                    }
                    return Some(arguments);
                }
            }
            TokenKind::LBrace => brace_depth += 1,
            TokenKind::RBrace => brace_depth = brace_depth.checked_sub(1)?,
            TokenKind::Other if token_text(source, token) == "[" => bracket_depth += 1,
            TokenKind::Other if token_text(source, token) == "]" => {
                bracket_depth = bracket_depth.checked_sub(1)?;
            }
            TokenKind::Comma if paren_depth == 1 && bracket_depth == 0 && brace_depth == 0 => {
                if let (Some(start), Some(end)) = (argument_start.take(), argument_end.take()) {
                    arguments.push(start..end);
                }
                continue;
            }
            _ => {}
        }
        if paren_depth > 0 {
            argument_start.get_or_insert(token.start);
            argument_end = Some(token.end);
        }
    }
    None
}

fn resolve_workshop_rename_target(
    files: &[WorkshopSourceFile],
    catalog: &[WorkshopCompletionItem],
    file: &WorkshopSourceFile,
    token: Token,
    semantic_path: &str,
) -> Result<WorkshopRenameTarget, String> {
    let token_name = token_text(&file.source, token);
    let mut candidates = catalog
        .iter()
        .filter(|item| {
            (item.text == semantic_path || item.text == token_name)
                && rename_completion_visible(files, item, file, token)
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|item| {
        let scope = item.scope.as_ref();
        (
            usize::from(item.text != semantic_path),
            usize::from(scope.is_none()),
            scope
                .map(|scope| scope.visible_to.saturating_sub(scope.visible_from))
                .unwrap_or(usize::MAX),
            std::cmp::Reverse(scope.map(|scope| scope.visible_from).unwrap_or(0)),
            item.kind.clone(),
        )
    });
    let primary = candidates
        .first()
        .copied()
        .ok_or_else(|| format!("compiler could not resolve rename target '{semantic_path}'"))?;
    Ok(WorkshopRenameTarget {
        kind: primary.kind.clone(),
        name: token_name.to_string(),
        owner: primary.owner.clone(),
        file: file.path.clone(),
        scope: primary.scope.clone(),
        signature: primary.signature.clone(),
    })
}

fn rename_completion_visible(
    files: &[WorkshopSourceFile],
    item: &WorkshopCompletionItem,
    file: &WorkshopSourceFile,
    token: Token,
) -> bool {
    let Some(scope) = item.scope.as_ref() else {
        return true;
    };
    if scope.file != file.path {
        return false;
    }
    if scope.visible_from <= token.start && token.end <= scope.visible_to {
        return true;
    }
    if item.kind == "local"
        && scope_declaration_range(scope)
            .is_some_and(|range| range.start <= token.start && token.end <= range.end)
    {
        return true;
    }
    if item.kind == "parameter" {
        return files
            .iter()
            .find(|source| source.path == scope.file)
            .and_then(|source| parse_top_level_functions(&source.source).ok())
            .is_some_and(|functions| {
                functions.into_iter().any(|function| {
                    function.name == scope.owner
                        && function.signature_range.start <= token.start
                        && token.end <= function.signature_range.end
                })
            });
    }
    false
}

fn reject_workshop_rename_collision(
    catalog: &[WorkshopCompletionItem],
    target: &WorkshopRenameTarget,
    new_name: &str,
) -> Result<(), String> {
    let collision = catalog.iter().any(|item| {
        item.text == new_name
            && match target.kind.as_str() {
                "local" | "parameter" => {
                    matches!(item.kind.as_str(), "local" | "parameter")
                        && item.file == target.file
                        && item.scope.as_ref().zip(target.scope.as_ref()).is_some_and(
                            |(left, right)| {
                                left.owner == right.owner
                                    && left.visible_from < right.visible_to
                                    && right.visible_from < left.visible_to
                            },
                        )
                }
                "field" | "state_path" => {
                    matches!(item.kind.as_str(), "field" | "state_path")
                        && item.owner == target.owner
                }
                _ => item.scope.is_none(),
            }
    });
    if collision {
        Err(format!(
            "rename would collide with existing symbol '{new_name}'"
        ))
    } else {
        Ok(())
    }
}

fn scope_declaration_range(scope: &WorkshopCompletionScope) -> Option<Range<usize>> {
    Some(scope.declaration_from?..scope.declaration_to?)
}

fn rename_target_references(
    files: &[WorkshopSourceFile],
    catalog: &[WorkshopCompletionItem],
    target: &WorkshopRenameTarget,
) -> Result<BTreeSet<(String, usize, usize)>, String> {
    if matches!(target.kind.as_str(), "local" | "parameter") {
        return scoped_binding_references(files, catalog, target);
    }
    let mut paths = BTreeSet::new();
    if matches!(target.kind.as_str(), "field" | "state_path") {
        for item in catalog.iter().filter(|item| {
            matches!(item.kind.as_str(), "field" | "state_path")
                && item.owner == target.owner
                && item.text.rsplit('.').next() == Some(target.name.as_str())
                && item.text.contains('.')
        }) {
            paths.insert(item.text.clone());
        }
        if let Some(owner) = &target.owner {
            paths.insert(format!("{owner}.{}", target.name));
        }
    } else {
        paths.insert(target.name.clone());
    }
    let mut locations = BTreeSet::new();
    for path in paths {
        let references = find_workshop_references(files, &path, 256)?;
        if references.len() == 256 {
            return Err(format!(
                "rename target '{path}' exceeds the 255-reference safety limit"
            ));
        }
        for reference in references {
            let end = reference.source_span.end as usize;
            let start = end.saturating_sub(target.name.len());
            locations.insert((reference.file, start, end));
        }
    }
    Ok(locations)
}

fn scoped_binding_references(
    files: &[WorkshopSourceFile],
    catalog: &[WorkshopCompletionItem],
    target: &WorkshopRenameTarget,
) -> Result<BTreeSet<(String, usize, usize)>, String> {
    let scope = target
        .scope
        .as_ref()
        .ok_or_else(|| "scoped rename target has no compiler scope".to_string())?;
    let file = files
        .iter()
        .find(|file| file.path == scope.file)
        .ok_or_else(|| format!("rename scope file is missing: {}", scope.file))?;
    let shadowed = catalog
        .iter()
        .filter(|item| {
            item.text == target.name
                && matches!(item.kind.as_str(), "local" | "parameter")
                && item.file == scope.file
        })
        .filter_map(|item| item.scope.as_ref())
        .filter(|other| {
            scope.visible_from < other.visible_from && other.visible_to <= scope.visible_to
        })
        .map(|other| other.visible_from..other.visible_to)
        .collect::<Vec<_>>();
    let tokens = lex(&file.source)?;
    let functions = (target.kind == "parameter")
        .then(|| parse_top_level_functions(&file.source))
        .transpose()?
        .unwrap_or_default();
    let mut locations = BTreeSet::new();
    for token in tokens {
        if token.kind != TokenKind::Identifier || token_text(&file.source, token) != target.name {
            continue;
        }
        let is_local_definition = target.kind == "local"
            && scope_declaration_range(scope)
                .is_some_and(|range| range.start <= token.start && token.end <= range.end);
        let is_parameter_definition = target.kind == "parameter"
            && token.end <= scope.visible_from
            && functions.iter().any(|function| {
                function.name == scope.owner
                    && function.signature_range.start <= token.start
                    && token.end <= function.signature_range.end
            });
        let is_visible_use = scope.visible_from <= token.start
            && token.end <= scope.visible_to
            && !shadowed
                .iter()
                .any(|range| range.start <= token.start && token.end <= range.end);
        if is_local_definition || is_parameter_definition || is_visible_use {
            locations.insert((file.path.clone(), token.start, token.end));
        }
    }
    Ok(locations)
}

fn apply_workshop_rename_edits(
    files: &mut [WorkshopSourceFile],
    plan: &WorkshopRenamePlan,
) -> Result<(), String> {
    let mut by_file = BTreeMap::<String, Vec<&WorkshopRenameEdit>>::new();
    for edit in &plan.edits {
        by_file.entry(edit.file.clone()).or_default().push(edit);
    }
    for (path, mut edits) in by_file {
        let file = files
            .iter_mut()
            .find(|file| file.path == path)
            .ok_or_else(|| format!("rename edit file is missing: {path}"))?;
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.source_span.start));
        for edit in edits {
            let range = edit.source_span.start as usize..edit.source_span.end as usize;
            if file.source.get(range.clone()) != Some(plan.old_name.as_str()) {
                return Err(format!(
                    "rename source changed at {}:{}..{}",
                    path, range.start, range.end
                ));
            }
            file.source.replace_range(range, &edit.new_text);
        }
    }
    Ok(())
}

fn global_definition_reference(
    files: &[WorkshopSourceFile],
    segments: &[&str],
    items: &[WorkshopSourceItem],
) -> Result<Option<WorkshopReference>, String> {
    let [name] = segments else {
        return Ok(None);
    };
    for file in files {
        let tokens = lex(&file.source)?;
        let Some(name_token) = tokens.windows(2).find_map(|pair| {
            (matches!(token_text(&file.source, pair[0]), "global" | "const")
                && pair[1].kind == TokenKind::Identifier
                && token_text(&file.source, pair[1]) == *name)
                .then_some(pair[1])
        }) else {
            continue;
        };
        let Some(container) = items
            .iter()
            .find(|item| item.file == file.path && item.kind == WorkshopSourceItemKind::Globals)
        else {
            continue;
        };
        return Ok(Some(WorkshopReference {
            symbol: name.to_string(),
            kind: WorkshopReferenceKind::Definition,
            file: file.path.clone(),
            source_span: WorkshopSourceSpan {
                start: u32::try_from(name_token.start)
                    .map_err(|_| "global definition start exceeds u32".to_string())?,
                end: u32::try_from(name_token.end)
                    .map_err(|_| "global definition end exceeds u32".to_string())?,
            },
            containing_kind: container.kind,
            containing_name: container.name.clone(),
            containing_signature: container.signature.clone(),
            containing_source_hash: container.source_hash.clone(),
        }));
    }
    Ok(None)
}

fn is_workshop_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn reference_match_end(
    source: &str,
    tokens: &[Token],
    start: usize,
    segments: &[&str],
) -> Option<usize> {
    let mut cursor = start;
    for (index, segment) in segments.iter().enumerate() {
        let token = *tokens.get(cursor)?;
        if token.kind != TokenKind::Identifier || token_text(source, token) != *segment {
            return None;
        }
        if index + 1 < segments.len() {
            cursor += 1;
            if tokens
                .get(cursor)
                .is_some_and(|token| token_text(source, *token) == "[")
            {
                let mut depth = 0usize;
                while let Some(token) = tokens.get(cursor).copied() {
                    match token_text(source, token) {
                        "[" => depth += 1,
                        "]" => {
                            depth = depth.checked_sub(1)?;
                            if depth == 0 {
                                cursor += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    cursor += 1;
                }
                if depth != 0 {
                    return None;
                }
            }
            let dot = *tokens.get(cursor)?;
            if dot.kind != TokenKind::Other || token_text(source, dot) != "." {
                return None;
            }
            cursor += 1;
        }
    }
    Some(cursor)
}

fn field_definition_reference(
    files: &[WorkshopSourceFile],
    segments: &[&str],
    items: &[WorkshopSourceItem],
) -> Result<Option<WorkshopReference>, String> {
    if segments.len() < 2 {
        return Ok(None);
    }
    let mut layouts = Vec::new();
    for file in files {
        layouts.push((file, source_workshop_items(&file.source)?.layout));
    }
    let mut type_name = layouts.iter().find_map(|(_, layout)| {
        layout
            .globals
            .iter()
            .find(|global| global.name == segments[0])
            .map(|global| global.type_name.clone())
    });
    if type_name.is_none()
        && layouts.iter().any(|(_, layout)| {
            layout
                .structs
                .iter()
                .any(|definition| definition.name == segments[0])
        })
    {
        type_name = Some(segments[0].to_string());
    }
    let mut owner = None;
    for field_name in &segments[1..] {
        let Some(current) = type_name.as_deref().map(workshop_base_type) else {
            return Ok(None);
        };
        let field = layouts.iter().find_map(|(_, layout)| {
            layout
                .structs
                .iter()
                .find(|definition| definition.name == current)
                .and_then(|definition| {
                    definition
                        .fields
                        .iter()
                        .find(|field| field.name == *field_name)
                        .map(|field| (definition.name.clone(), field.type_name.clone()))
                })
        });
        let Some((field_owner, field_type)) = field else {
            return Ok(None);
        };
        owner = Some(field_owner);
        type_name = Some(field_type);
    }
    let Some(owner) = owner else {
        return Ok(None);
    };
    let field_name = segments.last().copied().unwrap_or_default();
    let Some(item) = items
        .iter()
        .find(|item| item.kind == WorkshopSourceItemKind::Struct && item.name == owner)
    else {
        return Ok(None);
    };
    let Some(file) = files.iter().find(|file| file.path == item.file) else {
        return Ok(None);
    };
    let tokens = lex(&file.source)?;
    let Some(span) = item.source_spans.first() else {
        return Ok(None);
    };
    let Some(token) = tokens.iter().enumerate().find_map(|(index, token)| {
        (token.start >= span.start as usize
            && token.end <= span.end as usize
            && token.kind == TokenKind::Identifier
            && token_text(&file.source, *token) == field_name
            && tokens
                .get(index + 1)
                .is_some_and(|next| next.kind == TokenKind::Colon))
        .then_some(*token)
    }) else {
        return Ok(None);
    };
    Ok(Some(WorkshopReference {
        symbol: segments.join("."),
        kind: WorkshopReferenceKind::Definition,
        file: file.path.clone(),
        source_span: WorkshopSourceSpan {
            start: u32::try_from(token.start)
                .map_err(|_| "field definition start exceeds u32".to_string())?,
            end: u32::try_from(token.end)
                .map_err(|_| "field definition end exceeds u32".to_string())?,
        },
        containing_kind: item.kind,
        containing_name: item.name.clone(),
        containing_signature: item.signature.clone(),
        containing_source_hash: item.source_hash.clone(),
    }))
}

fn workshop_base_type(type_name: &str) -> &str {
    type_name.split('[').next().unwrap_or(type_name).trim()
}

fn classify_workshop_reference(
    source: &str,
    tokens: &[Token],
    end_index: usize,
    item: &WorkshopSourceItem,
    symbol: &str,
    start: usize,
) -> WorkshopReferenceKind {
    if !symbol.contains('.')
        && item.name == symbol
        && matches!(
            item.kind,
            WorkshopSourceItemKind::Function
                | WorkshopSourceItemKind::Struct
                | WorkshopSourceItemKind::Test
        )
        && item
            .source_spans
            .first()
            .is_some_and(|span| start < span.start as usize + item.signature.len() + 16)
    {
        return WorkshopReferenceKind::Definition;
    }
    let next = tokens.get(end_index + 1).copied();
    if next.is_some_and(|token| token.kind == TokenKind::LParen) {
        return WorkshopReferenceKind::Call;
    }
    let next_text = next.map(|token| token_text(source, token));
    let following_text = tokens
        .get(end_index + 2)
        .copied()
        .map(|token| token_text(source, token));
    if next_text == Some("=")
        || (matches!(
            next_text,
            Some("+") | Some("-") | Some("*") | Some("/") | Some("%")
        ) && following_text == Some("="))
    {
        WorkshopReferenceKind::Write
    } else {
        WorkshopReferenceKind::Read
    }
}

pub fn workshop_completion_items(
    files: &[WorkshopSourceFile],
) -> Result<Vec<WorkshopCompletionItem>, String> {
    let mut items = Vec::new();
    let mut struct_fields = BTreeMap::<String, Vec<(String, String)>>::new();
    let parsed_files = files
        .iter()
        .map(|file| {
            let records = source_workshop_items(&file.source)?;
            let local_declarations = parse_local_declarations(&file.source)?;
            Ok((
                file,
                records.layout,
                records.functions,
                records.typed_local_bindings,
                local_declarations,
                records.structs,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut struct_scopes = BTreeMap::<(String, String), WorkshopCompletionScope>::new();
    for (file, layout, _, _, _, ranges) in &parsed_files {
        for definition in &layout.structs {
            struct_fields.insert(
                definition.name.clone(),
                definition
                    .fields
                    .iter()
                    .map(|field| (field.name.clone(), field.type_name.clone()))
                    .collect(),
            );
        }
        for definition in ranges {
            struct_scopes.insert(
                (file.path.clone(), definition.name.clone()),
                WorkshopCompletionScope {
                    owner: definition.name.clone(),
                    file: file.path.clone(),
                    owner_signature: Some(format!("struct {}", definition.name)),
                    owner_end: Some(definition.definition_range.end),
                    declaration_from: None,
                    declaration_to: None,
                    visible_from: definition.definition_range.start,
                    visible_to: definition.definition_range.end,
                },
            );
        }
    }

    let source_items = workshop_source_items(files)?;
    let mut methods = BTreeMap::<String, Vec<(String, String, String, WorkshopExposure)>>::new();
    for item in source_items.iter().filter(|item| {
        matches!(
            item.kind,
            WorkshopSourceItemKind::Struct
                | WorkshopSourceItemKind::Function
                | WorkshopSourceItemKind::Test
        )
    }) {
        let kind = format!("{:?}", item.kind).to_ascii_lowercase();
        let mut completion = completion_catalog_item(
            &item.name,
            &kind,
            &format!("{} [{}]", item.signature, item.file),
            &item.file,
            item.owner.clone(),
        );
        completion.signature = Some(item.signature.clone());
        completion.exposure = item.exposure;
        items.push(completion);
        if item.kind == WorkshopSourceItemKind::Function {
            if let Some(owner) = item
                .owner
                .as_ref()
                .filter(|owner| struct_fields.contains_key(*owner))
            {
                methods.entry(owner.clone()).or_default().push((
                    item.name.clone(),
                    item.signature.clone(),
                    item.file.clone(),
                    item.exposure,
                ));
            }
        }
    }

    let mut typed_bindings = Vec::<WorkshopTypedBinding>::new();
    for (file, layout, functions, locals, local_declarations, _) in parsed_files {
        for definition in layout.structs {
            let struct_scope = struct_scopes
                .get(&(file.path.clone(), definition.name.clone()))
                .cloned();
            for field in definition.fields {
                items.push(scoped_completion_catalog_item(
                    &field.name,
                    "field",
                    &format!(
                        "{}.{}: {} [{}]",
                        definition.name, field.name, field.type_name, file.path
                    ),
                    &file.path,
                    Some(definition.name.clone()),
                    Some(&field.type_name),
                    struct_scope
                        .clone()
                        .expect("parsed struct has a source range"),
                ));
                items.push(typed_completion_catalog_item(
                    &format!("{}.{}", definition.name, field.name),
                    "field",
                    &format!("{} [{}]", field.type_name, file.path),
                    &file.path,
                    Some(definition.name.clone()),
                    &field.type_name,
                ));
            }
        }
        for definition in layout.enums {
            items.push(typed_completion_catalog_item(
                &definition.name,
                "enum",
                &format!("enum {} [{}]", definition.name, file.path),
                &file.path,
                None,
                &definition.name,
            ));
            for variant in definition.variants {
                items.push(typed_completion_catalog_item(
                    &format!("{}.{}", definition.name, variant.name),
                    "enum_variant",
                    &format!("{} [{}]", definition.name, file.path),
                    &file.path,
                    Some(definition.name.clone()),
                    &definition.name,
                ));
            }
        }
        for global in layout.globals {
            items.push(typed_completion_catalog_item(
                &global.name,
                "global",
                &format!("{} [{}]", global.type_name, file.path),
                &file.path,
                None,
                &global.type_name,
            ));
            typed_bindings.push(WorkshopTypedBinding {
                name: global.name,
                type_name: global.type_name,
                kind: "global".to_string(),
                scope_label: "global".to_string(),
                file: file.path.clone(),
                scope: None,
            });
        }
        for block in layout.global_blocks {
            for field in block.fields {
                let path = format!("{}.{}", block.name, field.name);
                items.push(typed_completion_catalog_item(
                    &path,
                    "state_path",
                    &format!("{} [{}]", field.type_name, file.path),
                    &file.path,
                    Some(block.name.clone()),
                    &field.type_name,
                ));
                typed_bindings.push(WorkshopTypedBinding {
                    name: path,
                    type_name: field.type_name,
                    kind: "state_path".to_string(),
                    scope_label: block.name.clone(),
                    file: file.path.clone(),
                    scope: None,
                });
            }
        }
        for constant in layout.constants {
            items.push(typed_completion_catalog_item(
                &constant.name,
                "constant",
                &format!("{} [{}]", constant.type_name, file.path),
                &file.path,
                None,
                &constant.type_name,
            ));
        }
        let function_scopes = functions
            .iter()
            .map(|function| {
                (
                    function.body_range.clone(),
                    format_function_signature(
                        &function.name,
                        &function.params,
                        &function.return_type_name,
                    ),
                )
            })
            .collect::<Vec<_>>();
        let inferred_local_declarations = local_declarations
            .into_iter()
            .filter(|declaration| {
                !locals.iter().any(|local| {
                    local.function_name == declaration.function_name
                        && local.name == declaration.name
                        && local.visibility_range == declaration.visibility_range
                })
            })
            .collect::<Vec<_>>();
        for function in functions {
            let owner_signature = format_function_signature(
                &function.name,
                &function.params,
                &function.return_type_name,
            );
            for parameter in function.params {
                let scope = WorkshopCompletionScope {
                    owner: function.name.clone(),
                    file: file.path.clone(),
                    owner_signature: Some(owner_signature.clone()),
                    owner_end: Some(function.body_range.end),
                    declaration_from: None,
                    declaration_to: None,
                    visible_from: function.body_range.start,
                    visible_to: function.body_range.end,
                };
                items.push(scoped_completion_catalog_item(
                    &parameter.name,
                    "parameter",
                    &format!(
                        "{} in {} [{}]",
                        parameter.type_name, function.name, file.path
                    ),
                    &file.path,
                    Some(function.name.clone()),
                    Some(&parameter.type_name),
                    scope.clone(),
                ));
                typed_bindings.push(WorkshopTypedBinding {
                    name: parameter.name,
                    type_name: parameter.type_name,
                    kind: "parameter".to_string(),
                    scope_label: function.name.clone(),
                    file: file.path.clone(),
                    scope: Some(scope),
                });
            }
        }
        for local in locals {
            let (owner_range, owner_signature) = function_scopes
                .iter()
                .find(|(range, _)| {
                    range.start <= local.visibility_range.start
                        && local.visibility_range.start <= range.end
                })
                .ok_or_else(|| {
                    format!(
                        "typed local {} has no containing function in {}",
                        local.name, file.path
                    )
                })?;
            let scope = WorkshopCompletionScope {
                owner: local.function_name.clone(),
                file: file.path.clone(),
                owner_signature: Some(owner_signature.clone()),
                owner_end: Some(owner_range.end),
                declaration_from: Some(local.name_range.start),
                declaration_to: Some(local.name_range.end),
                visible_from: local.visibility_range.start,
                visible_to: local.visibility_range.end,
            };
            items.push(scoped_completion_catalog_item(
                &local.name,
                "local",
                &format!(
                    "{} in {} [{}]",
                    local.type_name, local.function_name, file.path
                ),
                &file.path,
                Some(local.function_name.clone()),
                Some(&local.type_name),
                scope.clone(),
            ));
            typed_bindings.push(WorkshopTypedBinding {
                name: local.name,
                type_name: local.type_name,
                kind: "local".to_string(),
                scope_label: local.function_name,
                file: file.path.clone(),
                scope: Some(scope),
            });
        }
        for local in inferred_local_declarations {
            let (owner_range, owner_signature) = function_scopes
                .iter()
                .find(|(range, _)| {
                    range.start <= local.visibility_range.start
                        && local.visibility_range.start <= range.end
                })
                .ok_or_else(|| {
                    format!(
                        "local {} has no containing function in {}",
                        local.name, file.path
                    )
                })?;
            items.push(scoped_completion_catalog_item(
                &local.name,
                "local",
                &format!("local in {} [{}]", local.function_name, file.path),
                &file.path,
                Some(local.function_name.clone()),
                None,
                WorkshopCompletionScope {
                    owner: local.function_name,
                    file: file.path.clone(),
                    owner_signature: Some(owner_signature.clone()),
                    owner_end: Some(owner_range.end),
                    declaration_from: Some(local.name_range.start),
                    declaration_to: Some(local.name_range.end),
                    visible_from: local.visibility_range.start,
                    visible_to: local.visibility_range.end,
                },
            ));
        }
    }

    for binding in typed_bindings {
        if let Some(fields) = struct_fields.get(&binding.type_name) {
            for (field, field_type) in fields {
                let text = format!("{}.{field}", binding.name);
                let detail = format!(
                    "{field_type} via {} {}: {} in {} [{}]",
                    binding.kind,
                    binding.name,
                    binding.type_name,
                    binding.scope_label,
                    binding.file
                );
                let item = match binding.scope.clone() {
                    Some(scope) => scoped_completion_catalog_item(
                        &text,
                        "field",
                        &detail,
                        &binding.file,
                        Some(binding.type_name.clone()),
                        Some(field_type),
                        scope,
                    ),
                    None => typed_completion_catalog_item(
                        &text,
                        "field",
                        &detail,
                        &binding.file,
                        Some(binding.type_name.clone()),
                        field_type,
                    ),
                };
                items.push(item);
            }
        }
        if let Some(owner_methods) = methods.get(&binding.type_name) {
            for (method, signature, method_file, exposure) in owner_methods {
                let text = format!("{}.{method}", binding.name);
                let detail = format!(
                    "{signature} via {} {}: {} [{method_file}]",
                    binding.kind, binding.name, binding.type_name
                );
                let mut item = match binding.scope.clone() {
                    Some(scope) => scoped_completion_catalog_item(
                        &text,
                        "method",
                        &detail,
                        method_file,
                        Some(binding.type_name.clone()),
                        None,
                        scope,
                    ),
                    None => completion_catalog_item(
                        &text,
                        "method",
                        &detail,
                        method_file,
                        Some(binding.type_name.clone()),
                    ),
                };
                item.signature = Some(signature.clone());
                item.exposure = *exposure;
                items.push(item);
            }
        }
    }

    items.sort_by_key(|item| {
        (
            item.text.clone(),
            item.kind.clone(),
            item.detail.clone(),
            item.file.clone(),
            item.owner.clone(),
        )
    });
    items.dedup();
    Ok(items)
}

fn completion_catalog_item(
    text: &str,
    kind: &str,
    detail: &str,
    file: &str,
    owner: Option<String>,
) -> WorkshopCompletionItem {
    let truncated = detail.chars().count() > 256;
    let mut detail = if truncated {
        detail.chars().take(253).collect::<String>()
    } else {
        detail.to_string()
    };
    if truncated {
        detail.push_str("...");
    }
    WorkshopCompletionItem {
        text: text.to_string(),
        kind: kind.to_string(),
        detail,
        file: file.to_string(),
        owner,
        signature: None,
        type_name: None,
        scope: None,
        exposure: workshop_file_exposure(file),
    }
}

fn typed_completion_catalog_item(
    text: &str,
    kind: &str,
    detail: &str,
    file: &str,
    owner: Option<String>,
    type_name: &str,
) -> WorkshopCompletionItem {
    let mut item = completion_catalog_item(text, kind, detail, file, owner);
    item.type_name = Some(type_name.to_string());
    item
}

fn scoped_completion_catalog_item(
    text: &str,
    kind: &str,
    detail: &str,
    file: &str,
    owner: Option<String>,
    type_name: Option<&str>,
    scope: WorkshopCompletionScope,
) -> WorkshopCompletionItem {
    let mut item = completion_catalog_item(text, kind, detail, file, owner);
    item.type_name = type_name.map(str::to_string);
    item.scope = Some(scope);
    item
}

#[derive(Debug, Clone)]
struct WorkshopTypedBinding {
    name: String,
    type_name: String,
    kind: String,
    scope_label: String,
    file: String,
    scope: Option<WorkshopCompletionScope>,
}

fn source_item_from_ranges(
    file: &WorkshopSourceFile,
    kind: WorkshopSourceItemKind,
    name: &str,
    owner: Option<String>,
    signature: &str,
    ranges: Vec<Range<usize>>,
    include_comments: bool,
    symbol_id: Option<String>,
) -> Result<WorkshopSourceItem, String> {
    let mut ranges = ranges
        .into_iter()
        .map(|range| {
            if include_comments {
                expand_declaration_item_range(&file.source, range)
            } else {
                expand_range_through_newline(&file.source, range)
            }
        })
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);
    let source = ranges
        .iter()
        .map(|range| source_for_range(&file.source, range.clone()))
        .collect::<Result<Vec<_>, _>>()?
        .join("");
    let source_spans = ranges
        .into_iter()
        .map(span_from_range)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(WorkshopSourceItem {
        symbol_id: symbol_id
            .ok_or_else(|| "source item is missing canonical identity".to_string())?,
        kind,
        name: name.to_string(),
        owner,
        file: file.path.clone(),
        signature: signature.to_string(),
        source_hash: workshop_source_hash(&source),
        source,
        source_spans,
        exposure: workshop_file_exposure(&file.path),
    })
}

fn expand_range_through_newline(source: &str, range: Range<usize>) -> Range<usize> {
    let mut end = range.end.min(source.len());
    while end < source.len() && matches!(source.as_bytes()[end], b' ' | b'\t' | b'\r') {
        end += 1;
    }
    if end < source.len() && source.as_bytes()[end] == b'\n' {
        end += 1;
    }
    range.start..end
}

fn expand_declaration_item_range(source: &str, range: Range<usize>) -> Range<usize> {
    let declaration_line_start = source[..range.start.min(source.len())]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let prefix = &source[declaration_line_start..range.start.min(source.len())];
    let mut start = if prefix.trim().is_empty() {
        declaration_line_start
    } else {
        range.start
    };
    if start != declaration_line_start {
        return start..expand_range_through_newline(source, range).end;
    }
    while start > 0 {
        let previous_end = start - 1;
        let previous_start = source[..previous_end]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        let line = source[previous_start..previous_end].trim();
        if line.starts_with("//") {
            start = previous_start;
            continue;
        }
        break;
    }
    start..expand_range_through_newline(source, range).end
}

fn parse_import_spans(source: &str) -> Result<Vec<Range<usize>>, String> {
    parse_import_spans_with_depth(source, true)
}

fn parse_any_import_spans(source: &str) -> Result<Vec<Range<usize>>, String> {
    parse_import_spans_with_depth(source, false)
}

fn parse_import_spans_with_depth(
    source: &str,
    top_level_only: bool,
) -> Result<Vec<Range<usize>>, String> {
    let tokens = lex(source)?;
    let mut spans = Vec::new();
    let mut cursor = 0usize;
    let mut depth = 0usize;
    while cursor < tokens.len() {
        match tokens[cursor].kind {
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => depth = depth.saturating_sub(1),
            TokenKind::Identifier
                if (!top_level_only || depth == 0)
                    && token_text(source, tokens[cursor]) == "import"
                    && !is_inside_backtick_literal(source, tokens[cursor].start) =>
            {
                let literal = expect_token(&tokens, cursor + 1, TokenKind::StringLiteral)?;
                let semicolon = expect_token(&tokens, cursor + 2, TokenKind::Semicolon)?;
                if literal.end > semicolon.start {
                    return Err("invalid import declaration".to_string());
                }
                spans.push(tokens[cursor].start..semicolon.end);
                cursor += 3;
                continue;
            }
            _ => {}
        }
        cursor += 1;
    }
    Ok(spans)
}

pub fn find_workshop_symbols(
    files: &[WorkshopSourceFile],
    selector: &WorkshopSymbolSelector,
) -> Result<Vec<WorkshopSourceItem>, String> {
    let normalized_file = selector.file.as_deref().map(normalize_project_path_text);
    Ok(workshop_source_items(files)?
        .into_iter()
        .filter(|item| {
            if let Some(symbol_id) = selector.symbol_id.as_deref() {
                return item.symbol_id == symbol_id;
            }
            item.name == selector.name
                && selector.kind.is_none_or(|kind| item.kind == kind)
                && normalized_file
                    .as_deref()
                    .is_none_or(|file| normalize_project_path_text(&item.file) == file)
                && selector
                    .owner
                    .as_deref()
                    .is_none_or(|owner| item.owner.as_deref() == Some(owner))
                && selector
                    .signature
                    .as_deref()
                    .is_none_or(|signature| item.signature == signature)
        })
        .collect())
}

pub fn plan_workshop_semantic_edits(
    files: &[WorkshopSourceFile],
    batch: &WorkshopSemanticEditBatch,
) -> Result<(Vec<WorkshopSourceFile>, WorkshopSemanticEditPlan), String> {
    if !matches!(batch.schema_version, 1 | 2) {
        return Err(format!(
            "unsupported semantic edit schema_version {}; expected {}",
            batch.schema_version,
            semantic_edit_schema_version()
        ));
    }
    if batch.schema_version == 2
        && batch
            .edits
            .iter()
            .any(|edit| edit.target.symbol_id.is_none())
    {
        return Err("semantic edit schema_version 2 requires target.symbol_id".to_string());
    }
    if batch.edits.is_empty() {
        return Err("semantic edit batch must contain at least one edit".to_string());
    }
    let before = files.to_vec();
    let mut after = before.clone();
    for edit in &batch.edits {
        apply_one_semantic_edit(&mut after, edit)?;
    }
    let touched_files = before
        .iter()
        .filter_map(|before_file| {
            after
                .iter()
                .find(|after_file| after_file.path == before_file.path)
                .filter(|after_file| after_file.source != before_file.source)
                .map(|_| before_file.path.clone())
        })
        .collect::<BTreeSet<_>>();
    prune_unused_workshop_imports(&mut after, &touched_files)?;
    let mut changed_files = Vec::new();
    for before_file in &before {
        let after_file = after
            .iter()
            .find(|file| file.path == before_file.path)
            .ok_or_else(|| format!("edited project lost file {}", before_file.path))?;
        if after_file.source != before_file.source {
            changed_files.push(WorkshopSemanticFileChange {
                file: before_file.path.clone(),
                before_source: before_file.source.clone(),
                after_source: after_file.source.clone(),
                before_hash: workshop_source_hash(&before_file.source),
                after_hash: workshop_source_hash(&after_file.source),
            });
        }
    }
    if changed_files.is_empty() {
        return Err("semantic edit batch made no changes".to_string());
    }
    let reload = classify_workshop_reload(&before, &after)?;
    Ok((
        after,
        WorkshopSemanticEditPlan {
            schema_version: semantic_edit_schema_version(),
            edits: batch.edits.clone(),
            changed_files,
            reload,
        },
    ))
}

fn apply_one_semantic_edit(
    files: &mut [WorkshopSourceFile],
    edit: &WorkshopSemanticEdit,
) -> Result<(), String> {
    if edit.operation == WorkshopSemanticEditOperation::Delete
        && edit.target.kind == Some(WorkshopSourceItemKind::Globals)
        && edit.target.name != "globals"
    {
        return delete_global_member(files, edit);
    }
    match edit.operation {
        WorkshopSemanticEditOperation::Add => apply_add_semantic_edit(files, edit),
        WorkshopSemanticEditOperation::Update | WorkshopSemanticEditOperation::Delete => {
            let matches = find_workshop_symbols(files, &edit.target)?;
            let item = unique_semantic_target(&edit.target, matches)?;
            if let Some(expected) = edit.expected_source_hash.as_deref() {
                let actual = item.source_hash.clone();
                if actual != expected {
                    return Err(format!(
                        "stale semantic edit target {}; expected source hash {} but found {}",
                        item.name, expected, actual
                    ));
                }
            }
            let (replacement, embedded_imports) = match edit.operation {
                WorkshopSemanticEditOperation::Update => {
                    let source = required_edit_source(edit)?;
                    let (source, imports) = extract_embedded_imports(source)?;
                    validate_source_item_replacement(item.kind, &item.name, &source)?;
                    (source.trim().to_string(), imports)
                }
                WorkshopSemanticEditOperation::Delete => (String::new(), Vec::new()),
                WorkshopSemanticEditOperation::Add => unreachable!(),
            };
            replace_source_item(files, &item, &replacement)?;
            merge_workshop_imports(files, &item.file, &embedded_imports)?;
            build_workshop_symbol_tree(files)?;
            Ok(())
        }
    }
}

fn delete_global_member(
    files: &mut [WorkshopSourceFile],
    edit: &WorkshopSemanticEdit,
) -> Result<(), String> {
    let requested_file = edit
        .target
        .file
        .as_deref()
        .ok_or_else(|| "semantic global delete requires target.file".to_string())?;
    let normalized_file = normalize_project_path_text(requested_file);
    let globals = workshop_source_items(files)?
        .into_iter()
        .find(|item| {
            item.kind == WorkshopSourceItemKind::Globals
                && normalize_project_path_text(&item.file) == normalized_file
        })
        .ok_or_else(|| format!("globals item not found for {requested_file}"))?;
    if let Some(expected) = edit.expected_source_hash.as_deref() {
        if globals.source_hash != expected {
            return Err(format!(
                "stale semantic globals target {}; expected source hash {} but found {}",
                edit.target.name, expected, globals.source_hash
            ));
        }
    }
    let matches = workshop_symbols(files)?
        .into_iter()
        .filter(|symbol| {
            matches!(
                symbol.kind,
                WorkshopSymbolKind::Global | WorkshopSymbolKind::Constant
            ) && symbol.name == edit.target.name
                && normalize_project_path_text(&symbol.file) == normalized_file
        })
        .collect::<Vec<_>>();
    let symbol = match matches.as_slice() {
        [symbol] => symbol,
        [] => {
            return Err(format!(
                "semantic global not found: {} in {}",
                edit.target.name, requested_file
            ))
        }
        _ => {
            return Err(format!(
                "semantic global is ambiguous: {} in {}",
                edit.target.name, requested_file
            ))
        }
    };
    let file = files
        .iter_mut()
        .find(|file| normalize_project_path_text(&file.path) == normalized_file)
        .expect("global symbol file remains loaded");
    let range = expand_range_through_newline(
        &file.source,
        symbol.source_span.start as usize..symbol.source_span.end as usize,
    );
    file.source.replace_range(range, "");
    build_workshop_symbol_tree(files)?;
    Ok(())
}

fn apply_add_semantic_edit(
    files: &mut [WorkshopSourceFile],
    edit: &WorkshopSemanticEdit,
) -> Result<(), String> {
    let kind = edit
        .target
        .kind
        .ok_or_else(|| "semantic add requires target.kind".to_string())?;
    if !matches!(
        kind,
        WorkshopSourceItemKind::Imports | WorkshopSourceItemKind::Globals
    ) && !find_workshop_symbols(files, &edit.target)?.is_empty()
    {
        return Err(format!(
            "semantic add target already exists: {}",
            describe_selector(&edit.target)
        ));
    }
    let requested_file = edit
        .target
        .file
        .as_deref()
        .ok_or_else(|| "semantic add requires target.file".to_string())?;
    let normalized_file = normalize_project_path_text(requested_file);
    let (source, embedded_imports) = extract_embedded_imports(required_edit_source(edit)?)?;
    validate_source_item_replacement(kind, &edit.target.name, &source)?;
    let file_index = files
        .iter()
        .position(|file| normalize_project_path_text(&file.path) == normalized_file)
        .ok_or_else(|| {
            format!(
                "semantic add file is not in the loaded import graph: {}",
                requested_file
            )
        })?;
    if matches!(
        kind,
        WorkshopSourceItemKind::Imports | WorkshopSourceItemKind::Globals
    ) {
        let item = workshop_source_items(files)?
            .into_iter()
            .find(|item| item.file == files[file_index].path && item.kind == kind)
            .ok_or_else(|| format!("missing {:?} item for {}", kind, requested_file))?;
        let merged = if kind == WorkshopSourceItemKind::Imports {
            let mut imports = parse_workshop_import_paths(&files[file_index].source)?;
            imports.extend(parse_workshop_import_paths(&source)?);
            render_imports(imports)
        } else if item.source.trim().is_empty() {
            source.trim().to_string()
        } else {
            format!("{}\n{}", item.source.trim(), source.trim())
        };
        replace_source_item(files, &item, &merged)?;
    } else {
        let file = &mut files[file_index];
        if !file.source.ends_with('\n') {
            file.source.push('\n');
        }
        if !file.source.ends_with("\n\n") {
            file.source.push('\n');
        }
        file.source.push_str(source.trim());
        file.source.push('\n');
    }
    merge_workshop_imports(files, requested_file, &embedded_imports)?;
    let matches = find_workshop_symbols(files, &edit.target)?;
    unique_semantic_target(&edit.target, matches)?;
    Ok(())
}

fn unique_semantic_target(
    selector: &WorkshopSymbolSelector,
    matches: Vec<WorkshopSourceItem>,
) -> Result<WorkshopSourceItem, String> {
    match matches.len() {
        1 => Ok(matches.into_iter().next().expect("one semantic match")),
        0 => Err(format!(
            "semantic symbol not found: {}",
            describe_selector(selector)
        )),
        count => Err(format!(
            "semantic symbol is ambiguous ({} matches): {}; add --kind, --file, --owner, or --signature",
            count,
            describe_selector(selector)
        )),
    }
}

fn required_edit_source(edit: &WorkshopSemanticEdit) -> Result<&str, String> {
    edit.new_source
        .as_deref()
        .filter(|source| !source.trim().is_empty())
        .ok_or_else(|| format!("semantic {:?} requires new_source", edit.operation))
}

fn extract_embedded_imports(source: &str) -> Result<(String, Vec<String>), String> {
    let imports = parse_workshop_import_paths(source)?;
    let ranges = parse_any_import_spans(source)?
        .into_iter()
        .map(|range| expand_import_line_range(source, range))
        .collect::<Vec<_>>();
    let cleaned = remove_source_ranges(source, &ranges)?;
    Ok((cleaned.trim().to_string(), imports))
}

fn expand_import_line_range(source: &str, range: Range<usize>) -> Range<usize> {
    let line_start = source[..range.start.min(source.len())]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let line_end = source[range.end.min(source.len())..]
        .find('\n')
        .map(|offset| range.end + offset + 1)
        .unwrap_or(source.len());
    let before = &source[line_start..range.start];
    let after = &source[range.end..line_end];
    if before.trim().is_empty() && after.trim().is_empty() {
        line_start..line_end
    } else {
        range
    }
}

fn remove_source_ranges(source: &str, ranges: &[Range<usize>]) -> Result<String, String> {
    let mut out = source.to_string();
    let mut ranges = ranges.to_vec();
    ranges.sort_by_key(|range| range.start);
    for range in ranges.into_iter().rev() {
        if range.end > out.len()
            || range.start > range.end
            || !out.is_char_boundary(range.start)
            || !out.is_char_boundary(range.end)
        {
            return Err("source range is invalid".to_string());
        }
        out.replace_range(range, "");
    }
    Ok(out)
}

fn render_imports(mut imports: Vec<String>) -> String {
    imports.sort();
    imports.dedup();
    imports
        .into_iter()
        .map(|path| format!("import \"{}\";", escape_import_path(&path)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn escape_import_path(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"")
}

fn merge_workshop_imports(
    files: &mut [WorkshopSourceFile],
    file_path: &str,
    added: &[String],
) -> Result<(), String> {
    if added.is_empty() {
        return Ok(());
    }
    let normalized = normalize_project_path_text(file_path);
    let file = files
        .iter()
        .find(|file| normalize_project_path_text(&file.path) == normalized)
        .ok_or_else(|| format!("semantic import target file not loaded: {file_path}"))?;
    let mut imports = parse_workshop_import_paths(&file.source)?;
    imports.extend(added.iter().cloned());
    let item = workshop_source_items(files)?
        .into_iter()
        .find(|item| {
            item.kind == WorkshopSourceItemKind::Imports
                && normalize_project_path_text(&item.file) == normalized
        })
        .ok_or_else(|| format!("imports item not found for {file_path}"))?;
    replace_source_item(files, &item, &render_imports(imports))
}

fn prune_unused_workshop_imports(
    files: &mut [WorkshopSourceFile],
    touched_files: &BTreeSet<String>,
) -> Result<(), String> {
    let by_path = files
        .iter()
        .enumerate()
        .map(|(index, file)| (normalize_project_path_text(&file.path), index))
        .collect::<BTreeMap<_, _>>();
    let mut replacements = Vec::new();
    for file in files.iter() {
        if !touched_files.contains(&file.path) {
            continue;
        }
        let imports = parse_workshop_import_paths(&file.source)?;
        if imports.is_empty() {
            continue;
        }
        let import_ranges = parse_import_spans(&file.source)?;
        let body = remove_source_ranges(&file.source, &import_ranges)?;
        let identifiers = source_identifiers(&body)?;
        let mut kept = Vec::new();
        for import in imports {
            let imported_path = resolve_project_import_path(&file.path, &import)?;
            let Some(_) = by_path.get(&imported_path) else {
                kept.push(import);
                continue;
            };
            let mut visiting = BTreeSet::new();
            let exports = exported_identifiers(&imported_path, files, &by_path, &mut visiting)?;
            if exports.is_empty()
                || exports.iter().any(|name| identifiers.contains(name))
                || exports.iter().any(|name| {
                    matches!(name.as_str(), "main" | "tick" | "render" | "on_code_swap")
                })
            {
                kept.push(import);
            }
        }
        let rendered = render_imports(kept);
        let existing = render_imports(parse_workshop_import_paths(&file.source)?);
        if rendered != existing {
            replacements.push((file.path.clone(), rendered));
        }
    }
    for (file_path, rendered) in replacements {
        let item = workshop_source_items(files)?
            .into_iter()
            .find(|item| item.kind == WorkshopSourceItemKind::Imports && item.file == file_path)
            .ok_or_else(|| format!("imports item not found for {file_path}"))?;
        replace_source_item(files, &item, &rendered)?;
    }
    Ok(())
}

fn source_identifiers(source: &str) -> Result<BTreeSet<String>, String> {
    let tokens = lex(source)?;
    let mut declarations = BTreeSet::new();
    let mut scope_stack = Vec::new();
    let mut scope_at = vec![None; tokens.len()];
    let mut scope_ends = BTreeMap::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.kind == TokenKind::LBrace {
            scope_stack.push(index);
        }
        scope_at[index] = scope_stack.last().copied();
        if token.kind == TokenKind::RBrace {
            if let Some(start) = scope_stack.pop() {
                scope_ends.insert(start, index);
            }
        }
    }
    let mut local_bindings = Vec::new();
    for (function_index, token) in tokens.iter().enumerate() {
        if token.kind != TokenKind::FunctionKw {
            continue;
        }
        let Some(open_paren) = tokens[function_index + 1..]
            .iter()
            .position(|token| token.kind == TokenKind::LParen)
            .map(|offset| function_index + 1 + offset)
        else {
            continue;
        };
        let mut paren_depth = 0usize;
        let mut close_paren = None;
        for (index, token) in tokens.iter().enumerate().skip(open_paren) {
            match token.kind {
                TokenKind::LParen => paren_depth += 1,
                TokenKind::RParen => {
                    paren_depth = paren_depth.saturating_sub(1);
                    if paren_depth == 0 {
                        close_paren = Some(index);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close_paren) = close_paren else {
            continue;
        };
        let Some(body_start) = tokens[close_paren + 1..]
            .iter()
            .take_while(|token| token.kind != TokenKind::Semicolon)
            .position(|token| token.kind == TokenKind::LBrace)
            .map(|offset| close_paren + 1 + offset)
        else {
            continue;
        };
        let Some(body_end) = scope_ends.get(&body_start).copied() else {
            continue;
        };
        for index in open_paren + 1..close_paren {
            if tokens[index].kind == TokenKind::Identifier
                && tokens
                    .get(index + 1)
                    .is_some_and(|next| next.kind == TokenKind::Colon)
            {
                local_bindings.push((
                    token_text(source, tokens[index]).to_string(),
                    body_start,
                    body_end,
                ));
            }
        }
    }
    let mut brace_depth = 0usize;
    let mut pending_enum = false;
    let mut enum_depth = None;
    for (index, token) in tokens.iter().copied().enumerate() {
        if token.kind == TokenKind::Identifier && token_text(source, token) == "enum" {
            pending_enum = true;
        } else if token.kind == TokenKind::LBrace {
            brace_depth += 1;
            if pending_enum {
                enum_depth = Some(brace_depth);
                pending_enum = false;
            }
        } else if token.kind == TokenKind::RBrace {
            if enum_depth == Some(brace_depth) {
                enum_depth = None;
            }
            brace_depth = brace_depth.saturating_sub(1);
        }
        if token.kind != TokenKind::Identifier {
            continue;
        }
        let previous = index.checked_sub(1).and_then(|index| tokens.get(index));
        let next = tokens.get(index + 1);
        let follows_declaration_keyword = previous.is_some_and(|previous| {
            previous.kind == TokenKind::FunctionKw
                || (previous.kind == TokenKind::Identifier
                    && matches!(
                        token_text(source, *previous),
                        "struct" | "global" | "const" | "let" | "enum"
                    ))
        });
        let is_enum_variant = enum_depth == Some(brace_depth)
            && previous.is_some_and(|previous| {
                matches!(previous.kind, TokenKind::LBrace | TokenKind::Comma)
            });
        if follows_declaration_keyword
            || next.is_some_and(|next| next.kind == TokenKind::Colon)
            || is_enum_variant
        {
            declarations.insert(index);
        }
        if previous.is_some_and(|previous| {
            previous.kind == TokenKind::Identifier && token_text(source, *previous) == "let"
        }) {
            if let Some(scope_start) = scope_at[index] {
                if let Some(scope_end) = scope_ends.get(&scope_start) {
                    local_bindings.push((token_text(source, token).to_string(), index, *scope_end));
                }
            }
        }
    }
    Ok(tokens
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, token)| token.kind == TokenKind::Identifier)
        .filter_map(|(index, token)| {
            if declarations.contains(&index) {
                return None;
            }
            let name = token_text(source, token);
            if local_bindings
                .iter()
                .any(|(binding, start, end)| binding == name && index >= *start && index <= *end)
            {
                return None;
            }
            let previous_is_dot = index.checked_sub(1).is_some_and(|index| {
                tokens[index].kind == TokenKind::Other && token_text(source, tokens[index]) == "."
            });
            let next_is_call = tokens
                .get(index + 1)
                .is_some_and(|next| next.kind == TokenKind::LParen);
            if previous_is_dot && !next_is_call {
                return None;
            }
            Some(name.to_string())
        })
        .collect())
}

fn exported_identifiers(
    path: &str,
    files: &[WorkshopSourceFile],
    by_path: &BTreeMap<String, usize>,
    visiting: &mut BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    if !visiting.insert(path.to_string()) {
        return Ok(BTreeSet::new());
    }
    let Some(index) = by_path.get(path).copied() else {
        return Ok(BTreeSet::new());
    };
    let file = &files[index];
    let mut exports = workshop_symbols(std::slice::from_ref(file))?
        .into_iter()
        .filter(|symbol| symbol.kind != WorkshopSymbolKind::Test)
        .map(|symbol| symbol.name)
        .collect::<BTreeSet<_>>();
    for import in parse_workshop_import_paths(&file.source)? {
        let imported_path = resolve_project_import_path(&file.path, &import)?;
        exports.extend(exported_identifiers(
            &imported_path,
            files,
            by_path,
            visiting,
        )?);
    }
    visiting.remove(path);
    Ok(exports)
}

fn resolve_project_import_path(file: &str, import: &str) -> Result<String, String> {
    crate::frontend::module_graph::resolve_import_path(file, import)
}

fn validate_source_item_replacement(
    kind: WorkshopSourceItemKind,
    name: &str,
    source: &str,
) -> Result<(), String> {
    reject_rust_style_replacement("semantic", source)?;
    if kind == WorkshopSourceItemKind::Imports {
        let spans = parse_import_spans(source)?;
        let remainder = remove_source_ranges(source, &spans)?;
        if !remainder.trim().is_empty() {
            return Err("imports item may contain only import declarations".to_string());
        }
        return Ok(());
    }
    let file = WorkshopSourceFile {
        path: "src/semantic_validation.stasis".to_string(),
        source: source.trim().to_string(),
    };
    let items = workshop_source_items(&[file])?;
    if kind == WorkshopSourceItemKind::Globals {
        let globals = items
            .iter()
            .find(|item| item.kind == WorkshopSourceItemKind::Globals)
            .expect("globals item always exists");
        if globals.source.trim() != source.trim() {
            return Err("globals item may contain only const and global declarations".to_string());
        }
        return Ok(());
    }
    let declarations = items
        .into_iter()
        .filter(|item| {
            !matches!(
                item.kind,
                WorkshopSourceItemKind::Imports | WorkshopSourceItemKind::Globals
            ) && !item.source.trim().is_empty()
        })
        .collect::<Vec<_>>();
    if declarations.len() != 1 || declarations[0].kind != kind || declarations[0].name != name {
        return Err(format!(
            "semantic edit source must define exactly one {:?} named `{}`",
            kind, name
        ));
    }
    Ok(())
}

fn replace_source_item(
    files: &mut [WorkshopSourceFile],
    item: &WorkshopSourceItem,
    replacement: &str,
) -> Result<(), String> {
    let file = files
        .iter_mut()
        .find(|file| file.path == item.file)
        .ok_or_else(|| format!("semantic edit file not loaded: {}", item.file))?;
    let mut ranges = item
        .source_spans
        .iter()
        .map(|span| span.start as usize..span.end as usize)
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);
    let insertion = ranges.first().map(|range| range.start).unwrap_or_else(|| {
        if item.kind == WorkshopSourceItemKind::Imports {
            0
        } else {
            parse_import_spans(&file.source)
                .ok()
                .and_then(|spans| {
                    spans
                        .last()
                        .map(|span| expand_range_through_newline(&file.source, span.clone()).end)
                })
                .unwrap_or(0)
        }
    });
    for range in ranges.into_iter().rev() {
        if range.end > file.source.len()
            || range.start > range.end
            || !file.source.is_char_boundary(range.start)
            || !file.source.is_char_boundary(range.end)
        {
            return Err("semantic edit target span is invalid".to_string());
        }
        file.source.replace_range(range, "");
    }
    if !replacement.trim().is_empty() {
        let mut rendered = replacement.trim().to_string();
        rendered.push('\n');
        file.source
            .insert_str(insertion.min(file.source.len()), &rendered);
    }
    Ok(())
}

fn describe_selector(selector: &WorkshopSymbolSelector) -> String {
    format!(
        "kind={:?} file={:?} owner={:?} name={} signature={:?}",
        selector.kind, selector.file, selector.owner, selector.name, selector.signature
    )
}

fn normalize_project_path_text(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

pub fn workshop_source_hash(source: &str) -> String {
    format!("{:x}", Sha256::digest(source.as_bytes()))
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
}

fn atomic_write(path: &Path, source: &str) -> Result<(), String> {
    use std::io::Write;

    let parent = path
        .parent()
        .ok_or_else(|| format!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed creating {}: {error}", parent.display()))?;
    let mut file = atomic_write_file::AtomicWriteFile::open(path)
        .map_err(|error| format!("failed staging {}: {error}", path.display()))?;
    file.write_all(source.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("failed staging {}: {error}", path.display()))?;
    file.commit()
        .map_err(|error| format!("failed committing {}: {error}", path.display()))
}

pub fn write_workshop_semantic_receipt(
    project_root: &Path,
    relative_directory: &Path,
    plan: &WorkshopSemanticEditPlan,
) -> Result<PathBuf, String> {
    if !safe_relative_path(relative_directory) {
        return Err(format!(
            "unsafe semantic receipt directory: {}",
            relative_directory.display()
        ));
    }
    let serialized = serde_json::to_string(plan)
        .map_err(|error| format!("failed serializing semantic edit receipt: {error}"))?;
    let relative = relative_directory.join(format!("{}.json", workshop_source_hash(&serialized)));
    let path = project_root.join(&relative);
    let mut pretty = serde_json::to_string_pretty(plan)
        .map_err(|error| format!("failed serializing semantic edit receipt: {error}"))?;
    pretty.push('\n');
    atomic_write(&path, &pretty)?;
    Ok(relative)
}

pub fn write_workshop_semantic_plan(
    project_root: &Path,
    plan: &WorkshopSemanticEditPlan,
    restore: bool,
) -> Result<(), String> {
    write_workshop_semantic_plan_with(project_root, plan, restore, atomic_write)
}

fn write_workshop_semantic_plan_with(
    project_root: &Path,
    plan: &WorkshopSemanticEditPlan,
    restore: bool,
    mut write: impl FnMut(&Path, &str) -> Result<(), String>,
) -> Result<(), String> {
    for change in &plan.changed_files {
        let relative = Path::new(&change.file);
        if !safe_relative_path(relative) {
            return Err(format!("unsafe semantic edit path: {}", change.file));
        }
        let path = project_root.join(relative);
        let current = fs::read_to_string(&path)
            .map_err(|error| format!("failed reading {}: {error}", path.display()))?;
        let expected_hash = if restore {
            &change.after_hash
        } else {
            &change.before_hash
        };
        let current_hash = workshop_source_hash(&current);
        if current_hash != *expected_hash {
            return Err(format!(
                "refusing semantic {} for {}: expected current hash {} but found {}",
                if restore { "revert" } else { "apply" },
                change.file,
                expected_hash,
                current_hash
            ));
        }
    }
    let mut written = Vec::new();
    for change in &plan.changed_files {
        let path = project_root.join(&change.file);
        let source = if restore {
            &change.before_source
        } else {
            &change.after_source
        };
        if let Err(error) = write(&path, source) {
            let mut rollback_errors = Vec::new();
            for completed in written.into_iter().rev() {
                let prior = plan
                    .changed_files
                    .iter()
                    .find(|candidate| candidate.file == completed)
                    .expect("written semantic file remains in plan");
                let rollback_source = if restore {
                    &prior.after_source
                } else {
                    &prior.before_source
                };
                if let Err(rollback) = write(&project_root.join(&completed), rollback_source) {
                    rollback_errors.push(format!("{completed}: {rollback}"));
                }
            }
            let rollback = if rollback_errors.is_empty() {
                String::new()
            } else {
                format!("; rollback incomplete: {}", rollback_errors.join("; "))
            };
            return Err(format!(
                "failed writing {}: {error}{rollback}",
                path.display()
            ));
        }
        written.push(change.file.clone());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AiCodeRequest {
    pub user_prompt: String,
    pub selected_symbols: Vec<AiSelectedSymbol>,
    pub stasis_style_rules: StasisStyleRules,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AiSelectedSymbol {
    pub kind: AiSelectedSymbolKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub file: String,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiSelectedSymbolKind {
    Struct,
    Function,
    Global,
    Constant,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StasisStyleRules {
    pub use_function_keyword: bool,
    pub use_receiver_style_when_possible: bool,
    pub do_not_use_rust_references: bool,
    pub struct_functions_live_with_struct: bool,
    pub lifecycle_functions_live_in_main: bool,
    pub no_owner_functions_live_in_root: bool,
}

impl StasisStyleRules {
    pub fn workshop_default() -> Self {
        Self {
            use_function_keyword: true,
            use_receiver_style_when_possible: true,
            do_not_use_rust_references: true,
            struct_functions_live_with_struct: true,
            lifecycle_functions_live_in_main: true,
            no_owner_functions_live_in_root: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AiCodeResponse {
    pub summary: String,
    pub edits: Vec<AiCodeEdit>,
    pub expected_reload: ExpectedReload,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AiCodeEdit {
    pub kind: AiCodeEditKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub name: String,
    pub file: String,
    pub new_source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiCodeEditKind {
    ReplaceFunction,
    ReplaceStruct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExpectedReload {
    FastReload,
    ResetRequired,
}

pub fn selected_symbol_from_workshop_symbol(symbol: &WorkshopSymbol) -> AiSelectedSymbol {
    AiSelectedSymbol {
        kind: match symbol.kind {
            WorkshopSymbolKind::Struct => AiSelectedSymbolKind::Struct,
            WorkshopSymbolKind::Function => AiSelectedSymbolKind::Function,
            WorkshopSymbolKind::Global => AiSelectedSymbolKind::Global,
            WorkshopSymbolKind::Constant => AiSelectedSymbolKind::Constant,
            WorkshopSymbolKind::Test => AiSelectedSymbolKind::Test,
        },
        name: symbol.name.clone(),
        owner: symbol.owner.clone(),
        file: symbol.file.clone(),
        source: symbol.source.clone(),
    }
}

pub fn apply_ai_code_response_to_file(
    file_path: &str,
    source: &str,
    symbols: &[WorkshopSymbol],
    response: &AiCodeResponse,
) -> Result<String, String> {
    let mut replacements = Vec::new();
    for edit in &response.edits {
        if edit.file != file_path {
            continue;
        }
        let expected_kind = match edit.kind {
            AiCodeEditKind::ReplaceFunction => {
                validate_replacement_function_source(&edit.name, &edit.new_source)?;
                WorkshopSymbolKind::Function
            }
            AiCodeEditKind::ReplaceStruct => {
                validate_replacement_struct_source(&edit.name, &edit.new_source)?;
                WorkshopSymbolKind::Struct
            }
        };
        let symbol = symbols
            .iter()
            .find(|symbol| {
                symbol.kind == expected_kind
                    && symbol.name == edit.name
                    && (symbol.owner == edit.owner || edit.owner.is_none())
                    && symbol.file == edit.file
            })
            .ok_or_else(|| {
                format!(
                    "AI edit target not found: owner={:?} name={} file={}",
                    edit.owner, edit.name, edit.file
                )
            })?;
        replacements.push((
            symbol.source_span.start as usize..symbol.source_span.end as usize,
            edit.new_source.clone(),
        ));
    }

    if replacements.is_empty() {
        return Ok(source.to_string());
    }

    replacements.sort_by_key(|(range, _)| range.start);
    for pair in replacements.windows(2) {
        if pair[0].0.end > pair[1].0.start {
            return Err("AI edits overlap in source file".to_string());
        }
    }

    let mut updated = source.to_string();
    for (range, replacement) in replacements.into_iter().rev() {
        if range.end > updated.len()
            || range.start > range.end
            || !updated.is_char_boundary(range.start)
            || !updated.is_char_boundary(range.end)
        {
            return Err("AI edit target span is invalid for source file".to_string());
        }
        updated.replace_range(range, &replacement);
    }
    Ok(updated)
}

pub fn apply_ai_code_response_to_project(
    files: &[WorkshopSourceFile],
    response: &AiCodeResponse,
) -> Result<Vec<WorkshopSourceFile>, String> {
    let tree = build_workshop_symbol_tree(files)?;
    let symbols = tree
        .groups
        .iter()
        .flat_map(|group| group.symbols.iter().cloned())
        .collect::<Vec<_>>();
    let mut updated = Vec::with_capacity(files.len());
    for file in files {
        updated.push(WorkshopSourceFile {
            path: file.path.clone(),
            source: apply_ai_code_response_to_file(&file.path, &file.source, &symbols, response)?,
        });
    }
    Ok(updated)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopReload {
    InitialCompile,
    NoChange,
    FastReload,
    ResetRequired,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopReloadClassification {
    pub expected_reload: ExpectedReload,
    pub reason: String,
    pub changed_symbols: Vec<WorkshopChangedSymbol>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopChangedSymbol {
    pub kind: WorkshopSymbolKind,
    pub name: String,
    pub owner: Option<String>,
    pub file: String,
    pub signature: String,
    pub change: WorkshopSymbolChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopSymbolChange {
    Added,
    Modified,
    Removed,
}

pub fn classify_workshop_reload(
    before: &[WorkshopSourceFile],
    after: &[WorkshopSourceFile],
) -> Result<WorkshopReloadClassification, String> {
    let before_layout = layout_fingerprint(before)?;
    let after_layout = layout_fingerprint(after)?;
    let before_tree = build_workshop_symbol_tree(before)?;
    let after_tree = build_workshop_symbol_tree(after)?;
    let changed_symbols = changed_symbols_between(&before_tree, &after_tree);

    if before_layout != after_layout {
        return Ok(WorkshopReloadClassification {
            expected_reload: ExpectedReload::ResetRequired,
            reason: layout_change_reason(before, after)?,
            changed_symbols,
        });
    }

    if let Some(reason) = function_signature_change_reason(&before_tree, &after_tree) {
        return Ok(WorkshopReloadClassification {
            expected_reload: ExpectedReload::ResetRequired,
            reason,
            changed_symbols,
        });
    }

    if changed_symbols.is_empty() {
        return Ok(WorkshopReloadClassification {
            expected_reload: ExpectedReload::FastReload,
            reason: "No symbol changes detected.".to_string(),
            changed_symbols,
        });
    }

    Ok(WorkshopReloadClassification {
        expected_reload: ExpectedReload::FastReload,
        reason: "Only function bodies changed; layouts and function signatures are unchanged."
            .to_string(),
        changed_symbols,
    })
}

fn layout_fingerprint(files: &[WorkshopSourceFile]) -> Result<String, String> {
    let mut parts = Vec::new();
    for file in files {
        let layout = source_workshop_items(&file.source)?.layout;
        for parsed in layout.structs {
            let fields = parsed
                .fields
                .iter()
                .map(|field| format!("{}:{}", field.name, field.type_name))
                .collect::<Vec<_>>()
                .join(",");
            parts.push(format!("{}|struct|{}|{}", file.path, parsed.name, fields));
        }
        for parsed in layout.globals {
            parts.push(format!(
                "{}|global|{}|{}",
                file.path, parsed.name, parsed.type_name
            ));
        }
        for parsed in layout.global_blocks {
            let fields = parsed
                .fields
                .iter()
                .map(|field| format!("{}:{}", field.name, field.type_name))
                .collect::<Vec<_>>()
                .join(",");
            parts.push(format!(
                "{}|global_block|{}|{}",
                file.path, parsed.name, fields
            ));
        }
    }
    parts.sort();
    Ok(parts.join("\n"))
}

fn layout_change_reason(
    before: &[WorkshopSourceFile],
    after: &[WorkshopSourceFile],
) -> Result<String, String> {
    let before_structs = struct_layouts_by_name(before)?;
    let after_structs = struct_layouts_by_name(after)?;
    for (name, before_layout) in &before_structs {
        if after_structs
            .get(name)
            .is_some_and(|after_layout| after_layout != before_layout)
        {
            return Ok(format!(
                "{} layout changed. Global memory layout may need to be rebuilt.",
                name
            ));
        }
    }
    for name in after_structs.keys() {
        if !before_structs.contains_key(name) {
            return Ok(format!(
                "{} layout was added. Global memory layout may need to be rebuilt.",
                name
            ));
        }
    }
    for name in before_structs.keys() {
        if !after_structs.contains_key(name) {
            return Ok(format!(
                "{} layout was removed. Global memory layout may need to be rebuilt.",
                name
            ));
        }
    }
    Ok(
        "Global memory layout changed; current runtime state cannot be blindly preserved."
            .to_string(),
    )
}

fn struct_layouts_by_name(
    files: &[WorkshopSourceFile],
) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for file in files {
        let layout = source_workshop_items(&file.source)?.layout;
        for parsed in layout.structs {
            let fields = parsed
                .fields
                .iter()
                .map(|field| format!("{}:{}", field.name, field.type_name))
                .collect::<Vec<_>>()
                .join(",");
            out.insert(parsed.name, fields);
        }
    }
    Ok(out)
}

fn function_signature_change_reason(
    before_tree: &WorkshopSymbolTree,
    after_tree: &WorkshopSymbolTree,
) -> Option<String> {
    let before = function_symbols_by_identity(before_tree);
    let after = function_symbols_by_identity(after_tree);
    for (identity, before_symbol) in before {
        let after_symbol = after.get(&identity).copied().or_else(|| {
            let mut candidates = after.values().copied().filter(|candidate| {
                candidate.file == before_symbol.file
                    && candidate.owner == before_symbol.owner
                    && candidate.name == before_symbol.name
            });
            let candidate = candidates.next()?;
            candidates.next().is_none().then_some(candidate)
        });
        let Some(after_symbol) = after_symbol else {
            continue;
        };
        if before_symbol.signature != after_symbol.signature {
            return Some(format!(
                "{} signature changed from `{}` to `{}`.",
                before_symbol.name, before_symbol.signature, after_symbol.signature
            ));
        }
    }
    None
}

fn changed_symbols_between(
    before_tree: &WorkshopSymbolTree,
    after_tree: &WorkshopSymbolTree,
) -> Vec<WorkshopChangedSymbol> {
    let before = symbols_by_identity(before_tree);
    let after = symbols_by_identity(after_tree);
    let mut changed = Vec::new();

    for (identity, before_symbol) in &before {
        match after.get(identity) {
            Some(after_symbol) if after_symbol.source != before_symbol.source => {
                changed.push(changed_symbol_from(
                    after_symbol,
                    WorkshopSymbolChange::Modified,
                ));
            }
            None => changed.push(changed_symbol_from(
                before_symbol,
                WorkshopSymbolChange::Removed,
            )),
            _ => {}
        }
    }
    for (identity, after_symbol) in &after {
        if !before.contains_key(identity) {
            changed.push(changed_symbol_from(
                after_symbol,
                WorkshopSymbolChange::Added,
            ));
        }
    }
    changed.sort_by_key(|symbol| {
        (
            symbol.file.clone(),
            symbol.owner.clone(),
            symbol.name.clone(),
        )
    });
    changed
}

fn symbols_by_identity(tree: &WorkshopSymbolTree) -> BTreeMap<SymbolIdentity, &WorkshopSymbol> {
    let mut out = BTreeMap::new();
    for group in &tree.groups {
        for symbol in &group.symbols {
            out.insert(symbol_identity(symbol), symbol);
        }
    }
    out
}

fn function_symbols_by_identity(
    tree: &WorkshopSymbolTree,
) -> BTreeMap<SymbolIdentity, &WorkshopSymbol> {
    symbols_by_identity(tree)
        .into_iter()
        .filter(|(_, symbol)| symbol.kind == WorkshopSymbolKind::Function)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SymbolIdentity {
    canonical: String,
}

fn symbol_identity(symbol: &WorkshopSymbol) -> SymbolIdentity {
    SymbolIdentity {
        canonical: symbol.symbol_id.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopGitChangeSummary {
    pub changed_symbols: Vec<WorkshopChangedSymbolGroup>,
    pub changed_files: Vec<String>,
    pub raw_file_diffs: Vec<WorkshopRawFileDiff>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopChangedSymbolGroup {
    pub name: String,
    pub symbols: Vec<WorkshopChangedSymbolSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopChangedSymbolSummary {
    pub change: WorkshopSymbolChange,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub file: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkshopRawFileDiff {
    pub file: String,
    pub diff: String,
}

pub fn summarize_workshop_git_changes(
    changed_symbols: &[WorkshopChangedSymbol],
    raw_file_diffs: Vec<WorkshopRawFileDiff>,
) -> WorkshopGitChangeSummary {
    let mut grouped: BTreeMap<String, Vec<WorkshopChangedSymbolSummary>> = BTreeMap::new();
    let mut changed_files = BTreeSet::new();

    for symbol in changed_symbols {
        changed_files.insert(symbol.file.clone());
        let group_name = symbol
            .owner
            .clone()
            .unwrap_or_else(|| fallback_change_group(symbol));
        grouped
            .entry(group_name)
            .or_default()
            .push(WorkshopChangedSymbolSummary {
                change: symbol.change,
                name: symbol.name.clone(),
                owner: symbol.owner.clone(),
                file: symbol.file.clone(),
                signature: symbol.signature.clone(),
            });
    }
    for diff in &raw_file_diffs {
        changed_files.insert(diff.file.clone());
    }

    let mut changed_symbols = grouped
        .into_iter()
        .map(|(name, mut symbols)| {
            symbols.sort_by_key(|symbol| (symbol.file.clone(), symbol.name.clone()));
            WorkshopChangedSymbolGroup { name, symbols }
        })
        .collect::<Vec<_>>();
    changed_symbols.sort_by_key(|group| group.name.clone());

    WorkshopGitChangeSummary {
        changed_symbols,
        changed_files: changed_files.into_iter().collect(),
        raw_file_diffs,
    }
}

fn fallback_change_group(symbol: &WorkshopChangedSymbol) -> String {
    if is_main_path(&symbol.file) {
        return "Main".to_string();
    }
    if is_system_path(&symbol.file) {
        return title_case_stem(&symbol.file).unwrap_or_else(|| "Systems".to_string());
    }
    if is_root_path(&symbol.file) {
        return "Root".to_string();
    }
    "Root".to_string()
}

fn changed_symbol_from(
    symbol: &WorkshopSymbol,
    change: WorkshopSymbolChange,
) -> WorkshopChangedSymbol {
    WorkshopChangedSymbol {
        kind: symbol.kind,
        name: symbol.name.clone(),
        owner: symbol.owner.clone(),
        file: symbol.file.clone(),
        signature: symbol.signature.clone(),
        change,
    }
}

fn validate_replacement_function_source(expected_name: &str, source: &str) -> Result<(), String> {
    let trimmed = source.trim_start();
    if !trimmed.starts_with("function ") {
        return Err("replace_function edit must provide Stasis function source".to_string());
    }
    reject_rust_style_replacement("replace_function", trimmed)?;
    let functions = parse_top_level_functions(trimmed)?;
    if !functions
        .iter()
        .any(|function| function.name == expected_name)
    {
        return Err(format!(
            "replace_function source does not define expected function `{}`",
            expected_name
        ));
    }
    Ok(())
}

fn validate_replacement_struct_source(expected_name: &str, source: &str) -> Result<(), String> {
    let trimmed = source.trim_start();
    if !trimmed.starts_with("struct ") {
        return Err("replace_struct edit must provide Stasis struct source".to_string());
    }
    reject_rust_style_replacement("replace_struct", trimmed)?;
    let layout = parse_top_level_type_layout(trimmed)?;
    if !layout
        .structs
        .iter()
        .any(|parsed| parsed.name == expected_name)
    {
        return Err(format!(
            "replace_struct source does not define expected struct `{}`",
            expected_name
        ));
    }
    Ok(())
}

fn reject_rust_style_replacement(kind: &str, source: &str) -> Result<(), String> {
    if source.contains("&mut") || source.contains("->") {
        return Err(format!(
            "{} edit must use Stasis syntax, not Rust reference or arrow syntax",
            kind
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_android_workshop_example_symbols() {
        let files = vec![
            WorkshopSourceFile {
                path: "src/main.stasis".to_string(),
                source: r#"import "game_state.stasis";
function main(): void { init(); }
function init(): void { GameState.score = 0; }
function tick(): void { GameState.player.update(read_input()); }
function on_code_swap(): void { }
"#
                .to_string(),
            },
            WorkshopSourceFile {
                path: "src/player.stasis".to_string(),
                source: r#"struct Player {
    x: f32;
    y: f32;
    velocity_y: f32;
    jump_cooldown_ticks: i32;
}

function update(self: Player, input: InputState): void { self.y += self.velocity_y; }
function jump(self: Player): void { self.velocity_y = -8.5; }
function create_default_player(): Player { return GameState.player; }
"#
                .to_string(),
            },
            WorkshopSourceFile {
                path: "src/enemy.stasis".to_string(),
                source: r#"struct Enemy { x: f32; y: f32; hp: i32; active: bool; }
function update(self: Enemy): void { self.x -= 1.0; }
function damage(self: Enemy, amount: i32): void { self.hp -= amount; }
"#
                .to_string(),
            },
            WorkshopSourceFile {
                path: "src/systems/collision.stasis".to_string(),
                source: r#"function collision_update(): void { }
function player_overlaps_enemy(player: Player, enemy: Enemy): bool { return true; }
"#
                .to_string(),
            },
            WorkshopSourceFile {
                path: "src/root.stasis".to_string(),
                source: "function get_starting_level_index(): i32 { return 0; }\n".to_string(),
            },
        ];

        let tree = build_workshop_symbol_tree(&files).expect("symbol tree");
        let group_names = tree
            .groups
            .iter()
            .map(|group| (group.kind, group.name.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            group_names,
            vec![
                (WorkshopSymbolGroupKind::Main, "Main"),
                (WorkshopSymbolGroupKind::Struct, "Enemy"),
                (WorkshopSymbolGroupKind::Struct, "Player"),
                (WorkshopSymbolGroupKind::System, "Collision"),
                (WorkshopSymbolGroupKind::Root, "Root"),
            ]
        );

        let player = tree
            .groups
            .iter()
            .find(|group| group.name == "Player")
            .expect("player group");
        assert_eq!(
            player
                .symbols
                .iter()
                .map(|symbol| (symbol.kind, symbol.signature.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (WorkshopSymbolKind::Struct, "struct Player"),
                (
                    WorkshopSymbolKind::Function,
                    "update(self: Player, input: InputState): void"
                ),
                (WorkshopSymbolKind::Function, "jump(self: Player): void"),
                (
                    WorkshopSymbolKind::Function,
                    "create_default_player(): Player"
                ),
            ]
        );
        assert!(player.symbols[0].source.starts_with("struct Player"));
        assert!(player.symbols[1].source.starts_with("function update"));
        assert_eq!(player.symbols[1].owner.as_deref(), Some("Player"));
        assert_eq!(
            player.symbols[1].symbol_id,
            SymbolId::function(
                &CanonicalSourcePath::project_relative("src/player.stasis").unwrap(),
                "update",
                "(Player,InputState)"
            )
            .to_string()
        );

        let collision = tree
            .groups
            .iter()
            .find(|group| group.name == "Collision")
            .expect("collision group");
        assert_eq!(collision.symbols.len(), 2);
        assert!(collision
            .symbols
            .iter()
            .all(|symbol| symbol.owner.is_none()));
    }

    #[test]
    fn classifies_annotated_and_internal_directory_symbols_for_discovery() {
        let files = vec![
            WorkshopSourceFile {
                path: "src/stdlib/example.stasis".to_string(),
                source: concat!(
                    "struct Device { id: i32; }\n",
                    "global device: Device;\n",
                    "function public_wrapper(): i32 { return raw_helper(); }\n",
                    "function @internal raw_helper(): i32 { return 1; }\n",
                    "function @internal raw_method(self: Device): i32 { return self.id; }\n",
                )
                .to_string(),
            },
            WorkshopSourceFile {
                path: "src/stdlib/internal/host_frame_raw.stasis".to_string(),
                source: "function host_frame_raw(): i32 { return 0; }\n".to_string(),
            },
        ];
        let symbols = workshop_symbols(&files).expect("symbols");
        let exposure = |name: &str| {
            symbols
                .iter()
                .find(|symbol| symbol.name == name)
                .expect("symbol")
                .exposure
        };
        assert_eq!(exposure("public_wrapper"), WorkshopExposure::Public);
        assert_eq!(exposure("raw_helper"), WorkshopExposure::Internal);
        assert_eq!(exposure("host_frame_raw"), WorkshopExposure::Internal);

        let completions = workshop_completion_items(&files).expect("completions");
        assert_eq!(
            completions
                .iter()
                .find(|item| item.text == "raw_helper")
                .expect("raw helper completion")
                .exposure,
            WorkshopExposure::Internal
        );
        assert_eq!(
            completions
                .iter()
                .find(|item| item.text == "device.raw_method")
                .expect("raw receiver method completion")
                .exposure,
            WorkshopExposure::Internal
        );
    }
}

#[cfg(test)]
mod ai_tests {
    use super::*;

    #[test]
    fn serializes_android_ai_request_with_stasis_style_rules() {
        let symbol = WorkshopSymbol {
            symbol_id: SymbolId::function(
                &CanonicalSourcePath::project_relative("src/player.stasis").unwrap(),
                "jump",
                "(Player)",
            )
            .to_string(),
            kind: WorkshopSymbolKind::Function,
            name: "jump".to_string(),
            owner: Some("Player".to_string()),
            file: "src/player.stasis".to_string(),
            signature: "jump(self: Player): void".to_string(),
            source_span: WorkshopSourceSpan { start: 0, end: 52 },
            source: "function jump(self: Player): void { return; }".to_string(),
            exposure: WorkshopExposure::Public,
        };
        let request = AiCodeRequest {
            user_prompt: "Make the player jump higher but prevent repeated jumps.".to_string(),
            selected_symbols: vec![selected_symbol_from_workshop_symbol(&symbol)],
            stasis_style_rules: StasisStyleRules::workshop_default(),
        };

        let json = serde_json::to_string(&request).expect("serialize request");
        assert!(json.contains("\"use_function_keyword\":true"));
        assert!(json.contains("\"use_receiver_style_when_possible\":true"));
        assert!(json.contains("\"do_not_use_rust_references\":true"));
        assert!(json.contains("\"owner\":\"Player\""));
        assert!(!json.contains("&mut"));
    }

    #[test]
    fn applies_replace_function_edit_to_selected_symbol_span() {
        let source = "struct Player { velocity_y: f32; jump_cooldown_ticks: i32; }\n\nfunction jump(self: Player): void {\n    self.velocity_y = -8.5;\n}\n";
        let file = WorkshopSourceFile {
            path: "src/player.stasis".to_string(),
            source: source.to_string(),
        };
        let tree = build_workshop_symbol_tree(&[file]).expect("symbol tree");
        let player = tree
            .groups
            .iter()
            .find(|group| group.name == "Player")
            .expect("player group");
        let replacement = "function jump(self: Player): void {\n    self.velocity_y = -10.0;\n    self.jump_cooldown_ticks = 12;\n}";
        let response = AiCodeResponse {
            summary: "Increased jump strength and added a short cooldown.".to_string(),
            edits: vec![AiCodeEdit {
                kind: AiCodeEditKind::ReplaceFunction,
                owner: Some("Player".to_string()),
                name: "jump".to_string(),
                file: "src/player.stasis".to_string(),
                new_source: replacement.to_string(),
            }],
            expected_reload: ExpectedReload::FastReload,
            reason: "Only function bodies changed.".to_string(),
        };

        let updated =
            apply_ai_code_response_to_file("src/player.stasis", source, &player.symbols, &response)
                .expect("apply edit");
        assert!(updated.contains("self.velocity_y = -10.0;"));
        assert!(updated.contains("self.jump_cooldown_ticks = 12;"));
        assert!(!updated.contains("self.velocity_y = -8.5;"));
        assert!(updated.starts_with("struct Player"));
    }

    #[test]
    fn rejects_replace_function_edit_with_rust_style_source() {
        let source = "function jump(self: Player): void { return; }\n";
        let symbol = WorkshopSymbol {
            symbol_id: SymbolId::function(
                &CanonicalSourcePath::project_relative("src/player.stasis").unwrap(),
                "jump",
                "(Player)",
            )
            .to_string(),
            kind: WorkshopSymbolKind::Function,
            name: "jump".to_string(),
            owner: Some("Player".to_string()),
            file: "src/player.stasis".to_string(),
            signature: "jump(self: Player): void".to_string(),
            source_span: WorkshopSourceSpan {
                start: 0,
                end: source.len() as u32,
            },
            source: source.to_string(),
            exposure: WorkshopExposure::Public,
        };
        let response = AiCodeResponse {
            summary: "bad".to_string(),
            edits: vec![AiCodeEdit {
                kind: AiCodeEditKind::ReplaceFunction,
                owner: Some("Player".to_string()),
                name: "jump".to_string(),
                file: "src/player.stasis".to_string(),
                new_source: "fn jump(self: &mut Player) -> void { }".to_string(),
            }],
            expected_reload: ExpectedReload::FastReload,
            reason: "bad syntax".to_string(),
        };

        let error =
            apply_ai_code_response_to_file("src/player.stasis", source, &[symbol], &response)
                .expect_err("expected syntax rejection");
        assert!(error.contains("Stasis function source"));
    }
}

#[cfg(test)]
mod reload_tests {
    use super::*;

    fn player_file(source: &str) -> WorkshopSourceFile {
        WorkshopSourceFile {
            path: "src/player.stasis".to_string(),
            source: source.to_string(),
        }
    }

    #[test]
    fn classifies_function_body_change_as_fast_reload() {
        let before = vec![player_file(
            "struct Player { velocity_y: f32; jump_cooldown_ticks: i32; }\nfunction jump(self: Player): void { self.velocity_y = -8.5; }\n",
        )];
        let after = vec![player_file(
            "struct Player { velocity_y: f32; jump_cooldown_ticks: i32; }\nfunction jump(self: Player): void { self.velocity_y = -10.0; self.jump_cooldown_ticks = 12; }\n",
        )];

        let classified = classify_workshop_reload(&before, &after).expect("classify");
        assert_eq!(classified.expected_reload, ExpectedReload::FastReload);
        assert!(classified.reason.contains("function bodies changed"));
        assert_eq!(classified.changed_symbols.len(), 1);
        assert_eq!(classified.changed_symbols[0].name, "jump");
        assert_eq!(
            classified.changed_symbols[0].owner.as_deref(),
            Some("Player")
        );
        assert_eq!(
            classified.changed_symbols[0].change,
            WorkshopSymbolChange::Modified
        );
    }

    #[test]
    fn classifies_struct_layout_change_as_reset_required() {
        let before = vec![player_file(
            "struct Player { velocity_y: f32; jump_cooldown_ticks: i32; }\nfunction jump(self: Player): void { self.velocity_y = -8.5; }\n",
        )];
        let after = vec![player_file(
            "struct Player { velocity_y: f32; jump_cooldown_ticks: i32; dash_cooldown_ticks: i32; }\nfunction jump(self: Player): void { self.velocity_y = -8.5; }\n",
        )];

        let classified = classify_workshop_reload(&before, &after).expect("classify");
        assert_eq!(classified.expected_reload, ExpectedReload::ResetRequired);
        assert!(classified.reason.contains("Player layout changed"));
    }

    #[test]
    fn classifies_function_signature_change_as_reset_required() {
        let before = vec![player_file(
            "struct Player { velocity_y: f32; }\nfunction jump(self: Player): void { self.velocity_y = -8.5; }\n",
        )];
        let after = vec![player_file(
            "struct Player { velocity_y: f32; }\nfunction jump(self: Player, strength: f32): void { self.velocity_y = strength; }\n",
        )];

        let classified = classify_workshop_reload(&before, &after).expect("classify");
        assert_eq!(classified.expected_reload, ExpectedReload::ResetRequired);
        assert!(classified.reason.contains("jump signature changed"));
    }
}

#[cfg(test)]
mod git_summary_tests {
    use super::*;

    #[test]
    fn summarizes_changes_by_symbol_group_before_files_and_raw_diff() {
        let changed = vec![
            WorkshopChangedSymbol {
                kind: WorkshopSymbolKind::Function,
                name: "jump".to_string(),
                owner: Some("Player".to_string()),
                file: "src/player.stasis".to_string(),
                signature: "jump(self: Player): void".to_string(),
                change: WorkshopSymbolChange::Modified,
            },
            WorkshopChangedSymbol {
                kind: WorkshopSymbolKind::Function,
                name: "update".to_string(),
                owner: Some("Player".to_string()),
                file: "src/player.stasis".to_string(),
                signature: "update(self: Player, input: InputState): void".to_string(),
                change: WorkshopSymbolChange::Modified,
            },
        ];
        let summary = summarize_workshop_git_changes(
            &changed,
            vec![WorkshopRawFileDiff {
                file: "src/player.stasis".to_string(),
                diff: "@@ player diff @@".to_string(),
            }],
        );

        assert_eq!(summary.changed_symbols.len(), 1);
        assert_eq!(summary.changed_symbols[0].name, "Player");
        assert_eq!(
            summary.changed_symbols[0]
                .symbols
                .iter()
                .map(|symbol| (symbol.change, symbol.signature.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (WorkshopSymbolChange::Modified, "jump(self: Player): void"),
                (
                    WorkshopSymbolChange::Modified,
                    "update(self: Player, input: InputState): void"
                ),
            ]
        );
        assert_eq!(summary.changed_files, vec!["src/player.stasis".to_string()]);
        assert_eq!(summary.raw_file_diffs[0].diff, "@@ player diff @@");

        let json = serde_json::to_string(&summary).expect("serialize summary");
        let symbol_index = json.find("changed_symbols").expect("changed_symbols key");
        let files_index = json.find("changed_files").expect("changed_files key");
        let diffs_index = json.find("raw_file_diffs").expect("raw_file_diffs key");
        assert!(symbol_index < files_index);
        assert!(files_index < diffs_index);
    }
}

#[cfg(test)]
mod project_loader_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn loads_project_import_closure_with_normalized_paths() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_workshop_project_{stamp}"));
        let src = root.join("src");
        let systems = src.join("systems");
        fs::create_dir_all(&systems).expect("create project dirs");
        fs::write(
            src.join("main.stasis"),
            "import \"player.stasis\";\nimport \"systems/collision.stasis\";\nfunction main(): void { }\n",
        )
        .expect("write main");
        fs::write(
            src.join("player.stasis"),
            "struct Player { x: f32; }\nfunction jump(self: Player): void { }\n",
        )
        .expect("write player");
        fs::write(
            systems.join("collision.stasis"),
            "import \"../player.stasis\";\nfunction collision_update(): void { }\n",
        )
        .expect("write collision");
        fs::write(src.join("unused.stasis"), "function unused(): void { }\n")
            .expect("write unused");

        let files =
            load_workshop_project(&root, Path::new("src/main.stasis")).expect("load project");
        assert_eq!(
            files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "src/main.stasis",
                "src/player.stasis",
                "src/systems/collision.stasis",
            ]
        );
        let tree = build_workshop_symbol_tree(&files).expect("symbol tree");
        assert!(tree.groups.iter().any(|group| group.name == "Player"));
        assert!(tree.groups.iter().any(|group| group.name == "Collision"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn load_project_reports_missing_import_path() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_workshop_missing_import_{stamp}"));
        let src = root.join("src");
        fs::create_dir_all(&src).expect("create project dirs");
        fs::write(
            src.join("main.stasis"),
            "import \"missing.stasis\";\nfunction main(): void { }\n",
        )
        .expect("write main");

        let error = load_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("expected missing import failure");
        assert!(error.contains("missing.stasis"));
        let current = load_workshop_source_workspace(&root, Path::new("src/main.stasis"))
            .expect("raw source workspace remains available for editor recovery");
        assert_eq!(current.len(), 1);
        assert!(current[0].source.contains("missing.stasis"));
        fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod project_edit_tests {
    use super::*;

    #[test]
    fn applies_struct_and_function_edits_across_project_files() {
        let before = vec![
            WorkshopSourceFile {
                path: "src/player.stasis".to_string(),
                source: "struct Player { velocity_y: f32; }\nfunction jump(self: Player): void { self.velocity_y = -8.5; }\n".to_string(),
            },
            WorkshopSourceFile {
                path: "src/enemy.stasis".to_string(),
                source: "struct Enemy { hp: i32; }\nfunction damage(self: Enemy, amount: i32): void { self.hp -= amount; }\n".to_string(),
            },
        ];
        let response = AiCodeResponse {
            summary: "Add player jump cooldown and increase enemy damage.".to_string(),
            edits: vec![
                AiCodeEdit {
                    kind: AiCodeEditKind::ReplaceStruct,
                    owner: None,
                    name: "Player".to_string(),
                    file: "src/player.stasis".to_string(),
                    new_source: "struct Player { velocity_y: f32; jump_cooldown_ticks: i32; }"
                        .to_string(),
                },
                AiCodeEdit {
                    kind: AiCodeEditKind::ReplaceFunction,
                    owner: Some("Enemy".to_string()),
                    name: "damage".to_string(),
                    file: "src/enemy.stasis".to_string(),
                    new_source:
                        "function damage(self: Enemy, amount: i32): void { self.hp -= amount * 2; }"
                            .to_string(),
                },
            ],
            expected_reload: ExpectedReload::ResetRequired,
            reason: "Player layout changed.".to_string(),
        };

        let after =
            apply_ai_code_response_to_project(&before, &response).expect("apply project edits");
        let player = after
            .iter()
            .find(|file| file.path == "src/player.stasis")
            .expect("player file");
        let enemy = after
            .iter()
            .find(|file| file.path == "src/enemy.stasis")
            .expect("enemy file");
        assert!(player.source.contains("jump_cooldown_ticks: i32"));
        assert!(enemy.source.contains("amount * 2"));

        let classified = classify_workshop_reload(&before, &after).expect("classify");
        assert_eq!(classified.expected_reload, ExpectedReload::ResetRequired);
        assert!(classified.reason.contains("Player layout changed"));
    }

    #[test]
    fn rejects_struct_edit_that_targets_wrong_symbol_name() {
        let files = vec![WorkshopSourceFile {
            path: "src/player.stasis".to_string(),
            source: "struct Player { velocity_y: f32; }\n".to_string(),
        }];
        let response = AiCodeResponse {
            summary: "bad".to_string(),
            edits: vec![AiCodeEdit {
                kind: AiCodeEditKind::ReplaceStruct,
                owner: None,
                name: "Player".to_string(),
                file: "src/player.stasis".to_string(),
                new_source: "struct Enemy { hp: i32; }".to_string(),
            }],
            expected_reload: ExpectedReload::ResetRequired,
            reason: "bad".to_string(),
        };

        let error = apply_ai_code_response_to_project(&files, &response)
            .expect_err("expected target validation error");
        assert!(error.contains("expected struct `Player`"));
    }
}

#[cfg(test)]
mod placement_tests {
    use super::*;

    fn placement_files() -> Vec<WorkshopSourceFile> {
        vec![
            WorkshopSourceFile {
                path: "src/player.stasis".to_string(),
                source: "struct Player { x: f32; }\n".to_string(),
            },
            WorkshopSourceFile {
                path: "src/enemy.stasis".to_string(),
                source: "struct Enemy { hp: i32; }\n".to_string(),
            },
        ]
    }

    #[test]
    fn plans_lifecycle_and_root_function_files() {
        let files = placement_files();
        let tick = plan_workshop_symbol_placement(
            &files,
            &WorkshopSymbolPlacementRequest {
                kind: WorkshopPlacementSymbolKind::Function,
                name: "tick".to_string(),
                params: Vec::new(),
                return_type: Some("void".to_string()),
                owner: None,
                system: None,
            },
        )
        .expect("tick placement");
        assert_eq!(tick.file, "src/main.stasis");
        assert_eq!(tick.group, "Main");

        let utility = plan_workshop_symbol_placement(
            &files,
            &WorkshopSymbolPlacementRequest {
                kind: WorkshopPlacementSymbolKind::Function,
                name: "get_starting_level_index".to_string(),
                params: Vec::new(),
                return_type: Some("i32".to_string()),
                owner: None,
                system: None,
            },
        )
        .expect("root placement");
        assert_eq!(utility.file, "src/root.stasis");
        assert_eq!(utility.group, "Root");
    }

    #[test]
    fn plans_struct_owned_function_and_constructor_files() {
        let files = placement_files();
        let receiver = plan_workshop_symbol_placement(
            &files,
            &WorkshopSymbolPlacementRequest {
                kind: WorkshopPlacementSymbolKind::Function,
                name: "jump".to_string(),
                params: vec![WorkshopFunctionParam {
                    name: "self".to_string(),
                    type_name: "Player".to_string(),
                }],
                return_type: Some("void".to_string()),
                owner: None,
                system: None,
            },
        )
        .expect("receiver placement");
        assert_eq!(receiver.file, "src/player.stasis");
        assert_eq!(receiver.group, "Player");

        let constructor = plan_workshop_symbol_placement(
            &files,
            &WorkshopSymbolPlacementRequest {
                kind: WorkshopPlacementSymbolKind::Function,
                name: "create_default_player".to_string(),
                params: Vec::new(),
                return_type: Some("Player".to_string()),
                owner: None,
                system: None,
            },
        )
        .expect("constructor placement");
        assert_eq!(constructor.file, "src/player.stasis");
        assert!(constructor.reason.contains("return or create"));
    }

    #[test]
    fn plans_system_and_struct_definition_files() {
        let files = placement_files();
        let system = plan_workshop_symbol_placement(
            &files,
            &WorkshopSymbolPlacementRequest {
                kind: WorkshopPlacementSymbolKind::Function,
                name: "collision_update".to_string(),
                params: Vec::new(),
                return_type: Some("void".to_string()),
                owner: None,
                system: Some("collision".to_string()),
            },
        )
        .expect("system placement");
        assert_eq!(system.file, "src/systems/collision.stasis");
        assert_eq!(system.group, "Collision");

        let projectile = plan_workshop_symbol_placement(
            &files,
            &WorkshopSymbolPlacementRequest {
                kind: WorkshopPlacementSymbolKind::Struct,
                name: "Projectile".to_string(),
                params: Vec::new(),
                return_type: None,
                owner: None,
                system: None,
            },
        )
        .expect("struct placement");
        assert_eq!(projectile.file, "src/projectile.stasis");
        assert_eq!(projectile.group, "Projectile");
    }
}

#[cfg(test)]
mod workshop_contract_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_project(name: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_workshop_plan_{name}_{stamp}"));
        fs::create_dir_all(root.join("src")).expect("create temp project");
        root
    }

    fn write_project_file(root: &Path, relative: &str, source: &str) -> std::path::PathBuf {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&path, source).expect("write project file");
        path
    }

    #[test]
    fn semantic_tooling_resolves_project_root_imports_from_nested_files() {
        assert_eq!(
            resolve_project_import_path(
                "src/game/player.stasis",
                "/vendor/stasis/stdlib/graphics.stasis"
            )
            .expect("project-root import"),
            "vendor/stasis/stdlib/graphics.stasis"
        );
    }

    #[test]
    fn workshop_symbol_and_completion_records_match_multifile_program_snapshot() {
        let files = vec![
            WorkshopSourceFile {
                path: "src/main.stasis".to_string(),
                source: "import \"helper.stasis\"; function main(): i32 { return helper(); } function extra(): i32 { return 9; }"
                    .to_string(),
            },
            WorkshopSourceFile {
                path: "src/helper.stasis".to_string(),
                source: "struct Helper { value: i32; }\nfunction helper(): i32 { return 7; }"
                    .to_string(),
            },
        ];
        let mut process = crate::backend::jit::JitProcess::new();
        for file in &files {
            process.upsert_file(file.path.clone(), file.source.clone());
        }
        process.compile().expect("compile snapshot");
        let snapshot_functions = process
            .program_snapshot()
            .expect("snapshot")
            .functions()
            .iter()
            .map(|function| function.name.clone())
            .collect::<BTreeSet<_>>();

        let tree = build_workshop_symbol_tree(&files).expect("symbol tree");
        let tree_functions = tree
            .groups
            .iter()
            .flat_map(|group| group.symbols.iter())
            .filter(|symbol| symbol.kind == WorkshopSymbolKind::Function)
            .map(|symbol| symbol.name.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(tree_functions, snapshot_functions);

        let completions = workshop_completion_items(&files).expect("completion records");
        for function in &snapshot_functions {
            assert!(completions.iter().any(|item| item.text == *function));
        }
        assert!(
            tree_functions.contains("extra"),
            "same-line declaration retained"
        );
    }

    fn semantic_selector(
        name: &str,
        kind: WorkshopSourceItemKind,
        file: &str,
    ) -> WorkshopSymbolSelector {
        WorkshopSymbolSelector {
            symbol_id: None,
            name: name.to_string(),
            kind: Some(kind),
            file: Some(file.to_string()),
            owner: None,
            signature: None,
        }
    }

    fn semantic_edit(
        operation: WorkshopSemanticEditOperation,
        target: WorkshopSymbolSelector,
        new_source: Option<&str>,
    ) -> WorkshopSemanticEdit {
        WorkshopSemanticEdit {
            operation,
            target,
            new_source: new_source.map(str::to_string),
            expected_source_hash: None,
        }
    }

    #[test]
    fn canonical_symbol_selector_is_lossless_for_overloads_and_schema_v2_requires_it() {
        let files = vec![WorkshopSourceFile {
            path: "src/actions.stasis".to_string(),
            source: "struct Player { value: i32; } struct Enemy { value: i32; } function damage(self: Player, amount: i32): i32 { return amount; } function damage(self: Enemy, amount: i32): i32 { return amount + 1; }".to_string(),
        }];
        let overloads = workshop_source_items(&files)
            .expect("source items")
            .into_iter()
            .filter(|item| item.name == "damage")
            .collect::<Vec<_>>();
        assert_eq!(overloads.len(), 2);
        assert_ne!(overloads[0].symbol_id, overloads[1].symbol_id);
        let serialized = serde_json::to_string(&overloads[0]).expect("serialize source item");
        assert!(serialized.contains("\"symbol_id\":\"v1|function|src/actions.stasis|damage|"));

        let selected = find_workshop_symbols(
            &files,
            &WorkshopSymbolSelector {
                symbol_id: Some(overloads[1].symbol_id.clone()),
                name: "stale_display_name".to_string(),
                kind: None,
                file: None,
                owner: None,
                signature: None,
            },
        )
        .expect("canonical lookup");
        assert_eq!(selected, vec![overloads[1].clone()]);

        let error = plan_workshop_semantic_edits(
            &files,
            &WorkshopSemanticEditBatch {
                schema_version: 2,
                edits: vec![semantic_edit(
                    WorkshopSemanticEditOperation::Delete,
                    semantic_selector(
                        "damage",
                        WorkshopSourceItemKind::Function,
                        "src/actions.stasis",
                    ),
                    None,
                )],
            },
        )
        .expect_err("schema v2 must reject a lossy selector");
        assert!(error.contains("requires target.symbol_id"));
    }

    #[test]
    fn source_items_use_rust_parser_owned_sections_and_comment_boundaries() {
        let source = "import \"math.stasis\";\n\nconst SPEED: i32 = 2;\nglobal score: i32;\nglobal State { score: i32; }\n\n// Player state.\n// Kept with the struct.\nstruct Player { x: i32; }\n\n// Unrelated note.\n\n// Advances the player.\nfunction update(self: Player): void {\n    self.x += SPEED;\n}\n\nfunction main(): i32 { return State.score; }\n";
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: source.to_string(),
        }];
        let items = workshop_source_items(&files).expect("source items");
        let imports = items
            .iter()
            .find(|item| item.kind == WorkshopSourceItemKind::Imports)
            .expect("imports item");
        assert_eq!(imports.name, "imports");
        assert_eq!(imports.source, "import \"math.stasis\";\n");
        let globals = items
            .iter()
            .find(|item| item.kind == WorkshopSourceItemKind::Globals)
            .expect("globals item");
        assert!(globals.source.contains("const SPEED"));
        assert!(globals.source.contains("global score: i32;"));
        assert!(globals.source.contains("global State"));
        assert!(!globals.source.contains("function update"));
        let player = items
            .iter()
            .find(|item| item.kind == WorkshopSourceItemKind::Struct && item.name == "Player")
            .expect("Player item");
        assert!(player.source.starts_with("// Player state."));
        assert!(player.source.ends_with("}\n"));
        let update = items
            .iter()
            .find(|item| item.kind == WorkshopSourceItemKind::Function && item.name == "update")
            .expect("update item");
        assert!(update.source.starts_with("// Advances the player."));
        assert!(!update.source.contains("Unrelated note"));
        assert!(update.source.ends_with("}\n"));
    }

    #[test]
    fn import_scanning_ignores_backtick_test_names_and_keeps_real_imports() {
        let source = concat!(
            "    import \"shared.stasis\";\n",
            "test `legacy profile codes import into merged enemies`(): bool {\n",
            "    return true;\n",
            "}\n",
        );

        let spans = parse_import_spans(source).expect("top-level imports");
        assert_eq!(spans.len(), 1);
        assert_eq!(&source[spans[0].clone()], "import \"shared.stasis\";");
        let items = workshop_source_items(&[WorkshopSourceFile {
            path: "tests/legacy.test.stasis".to_string(),
            source: source.to_string(),
        }])
        .expect("workshop source items");
        assert!(items.iter().any(|item| {
            item.kind == WorkshopSourceItemKind::Test
                && item.name == "legacy profile codes import into merged enemies"
        }));
    }

    #[test]
    fn any_depth_import_scanning_keeps_nested_declarations() {
        let source = concat!(
            "function update(): void {\n",
            "    import \"nested.stasis\";\n",
            "    return;\n",
            "}\n",
        );

        assert!(parse_import_spans(source)
            .expect("top-level scan")
            .is_empty());
        let spans = parse_any_import_spans(source).expect("any-depth imports");
        assert_eq!(spans.len(), 1);
        assert_eq!(&source[spans[0].clone()], "import \"nested.stasis\";");
    }

    #[test]
    fn references_find_dot_qualified_reads_and_writes_by_containing_function() {
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: "global Game { score: i32; }\nfunction bump(): void { Game.score += 1; }\nfunction current(): i32 { return Game.score; }\n"
                .to_string(),
        }];

        let references = find_workshop_references(&files, "Game.score", 16).expect("references");

        assert_eq!(references.len(), 2);
        assert!(references.iter().any(|reference| {
            reference.kind == WorkshopReferenceKind::Write && reference.containing_name == "bump"
        }));
        assert!(references.iter().any(|reference| {
            reference.kind == WorkshopReferenceKind::Read && reference.containing_name == "current"
        }));
    }

    #[test]
    fn references_resolve_indexed_struct_field_definition_and_use() {
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: "struct Enemy { speed: i32; }\nstruct State { enemies: Enemy[2]; }\nglobal state: State;\nfunction main(): void { state.enemies[0].speed = 2; }\n"
                .to_string(),
        }];

        let references =
            find_workshop_references(&files, "state.enemies.speed", 16).expect("references");

        assert_eq!(references.len(), 2);
        assert!(references.iter().any(|reference| {
            reference.kind == WorkshopReferenceKind::Definition
                && reference.containing_name == "Enemy"
        }));
        assert!(references.iter().any(|reference| {
            reference.kind == WorkshopReferenceKind::Write && reference.containing_name == "main"
        }));
    }

    #[test]
    fn references_publish_exact_global_definition_without_duplicate_write() {
        let source = "struct State { x: i32; }\nglobal state: State;\nfunction main(): void { state.x = 2; }\n";
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: source.to_string(),
        }];

        let references = find_workshop_references(&files, "state", 16).expect("references");
        let definitions = references
            .iter()
            .filter(|reference| reference.kind == WorkshopReferenceKind::Definition)
            .collect::<Vec<_>>();
        assert_eq!(definitions.len(), 1);
        let definition = definitions[0];
        assert_eq!(
            &source[definition.source_span.start as usize..definition.source_span.end as usize],
            "state"
        );
        assert_eq!(
            references
                .iter()
                .filter(|reference| reference.source_span == definition.source_span)
                .count(),
            1
        );
    }

    fn rename_fixture() -> (Vec<WorkshopSourceFile>, String) {
        let source = "struct State { x: i32; y: i32; }\nglobal state: State;\nfunction helper(amount: i32): i32 { let value: i32 = amount; state.x = value; return value + amount; }\nfunction read(other: State): i32 { return other.x; }\nfunction other(): i32 { let value: i32 = 9; return value; }\nfunction main(): i32 { state.y = helper(3); return read(state); }\n".to_string();
        (
            vec![WorkshopSourceFile {
                path: "src/main.stasis".to_string(),
                source: source.clone(),
            }],
            source,
        )
    }

    #[test]
    fn semantic_tokens_use_compiler_bindings_for_state_fields_and_scopes() {
        let (files, source) = rename_fixture();
        let tokens = workshop_semantic_tokens(&files, "src/main.stasis").expect("semantic tokens");
        let token_at = |needle: &str, within: usize| {
            let offset = source.find(needle).expect("semantic token use") + within;
            tokens
                .iter()
                .find(|token| {
                    token.source_span.start as usize <= offset
                        && offset <= token.source_span.end as usize
                })
                .unwrap_or_else(|| panic!("missing semantic token for {needle}"))
        };
        assert_eq!(token_at("global state", 8).kind, "global");
        assert_eq!(token_at("state.x", 1).kind, "global");
        assert_eq!(token_at("state.x", 7).kind, "field");
        assert_eq!(token_at("amount; state", 2).kind, "parameter");
        assert_eq!(token_at("value + amount", 2).kind, "local");
        assert_eq!(token_at("helper(3)", 2).kind, "function");
        assert_eq!(token_at("State;", 2).kind, "struct");
    }

    #[test]
    fn inlay_hints_use_inferred_types_and_bound_parameter_names() {
        let source = "function add(amount: i32, bonus: i32): i32 { return amount + bonus; }\nfunction main(): i32 { let total = add(1, add(2, 3)); let amount = 4; return add(amount, 5); }\n";
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: source.to_string(),
        }];
        let hints = workshop_inlay_hints(&files).expect("inlay hints");
        let rendered = hints
            .iter()
            .map(|hint| {
                (
                    &source[hint.source_span.start as usize..hint.source_span.end as usize],
                    hint.kind,
                    hint.label.as_str(),
                )
            })
            .collect::<Vec<_>>();
        assert!(rendered.contains(&("total", WorkshopInlayHintKind::Type, ": i32")));
        assert!(rendered.contains(&("amount", WorkshopInlayHintKind::Type, ": i32")));
        assert!(rendered.contains(&("1", WorkshopInlayHintKind::Parameter, "amount:")));
        assert!(rendered.contains(&("5", WorkshopInlayHintKind::Parameter, "bonus:")));
        assert!(!rendered.contains(&("amount", WorkshopInlayHintKind::Parameter, "amount:")));
    }

    #[test]
    fn inlay_hints_align_receiver_and_function_call_arguments() {
        let source = "struct Player { score: i32; }\nglobal player: Player;\nfunction boost(self: Player, amount: i32, bonus: i32): void { self.score += amount + bonus; }\nfunction main(): i32 { player.boost(1, 2); boost(player, 3, 4); return player.score; }\n";
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: source.to_string(),
        }];
        let rendered = workshop_inlay_hints(&files)
            .expect("inlay hints")
            .into_iter()
            .filter(|hint| hint.kind == WorkshopInlayHintKind::Parameter)
            .map(|hint| {
                (
                    source[hint.source_span.start as usize..hint.source_span.end as usize]
                        .to_string(),
                    hint.label,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            vec![
                ("1".to_string(), "amount:".to_string()),
                ("2".to_string(), "bonus:".to_string()),
                ("player".to_string(), "self:".to_string()),
                ("3".to_string(), "amount:".to_string()),
                ("4".to_string(), "bonus:".to_string()),
            ]
        );
    }

    #[test]
    fn rename_plan_covers_declarations_references_and_scoped_bindings() {
        let cases = [
            ("helper(3)", 2usize, "assist", "function", 2usize),
            ("state.y =", 2, "world", "global", 3),
            ("State;", 2, "World", "struct", 3),
            ("other.x", 7, "speed", "field", 3),
            ("amount; state", 2, "delta", "parameter", 3),
            ("value + amount", 2, "total", "local", 3),
        ];
        for (needle, within, new_name, expected_kind, minimum_edits) in cases {
            let (files, source) = rename_fixture();
            let offset = source.find(needle).expect("rename use") + within;
            let (after, plan) = plan_workshop_rename(&files, "src/main.stasis", offset, new_name)
                .unwrap_or_else(|error| panic!("rename {needle} failed: {error}"));
            assert_eq!(plan.kind, expected_kind, "{needle}");
            assert!(plan.edits.len() >= minimum_edits, "{needle}: {plan:?}");
            assert!(!after[0].source.contains(match expected_kind {
                "function" => "helper(3)",
                "global" => "state.y",
                "struct" => "State;",
                "field" => ".x",
                "parameter" => "amount; state",
                "local" => "value + amount",
                _ => unreachable!(),
            }));
            let mut process = crate::backend::jit::JitProcess::new();
            process.upsert_file(after[0].path.clone(), after[0].source.clone());
            process.compile().unwrap_or_else(|error| {
                panic!("renamed {expected_kind} did not compile: {error:?}")
            });
            assert_eq!(process.execute_i32_noarg_by_name("main"), Ok(3));
        }
    }

    #[test]
    fn rename_plan_rejects_collisions_and_preserves_same_named_other_scope() {
        let (files, source) = rename_fixture();
        let field = source.find("other.x").expect("field") + "other.".len();
        let error = plan_workshop_rename(&files, "src/main.stasis", field, "y")
            .expect_err("field collision");
        assert!(error.contains("collide"), "{error}");

        let local = source.find("value + amount").expect("local") + 2;
        let (after, plan) =
            plan_workshop_rename(&files, "src/main.stasis", local, "total").expect("local rename");
        assert_eq!(plan.edits.len(), 3);
        assert!(after[0]
            .source
            .contains("function other(): i32 { let value: i32 = 9; return value; }"));
    }

    #[test]
    fn completion_catalog_includes_scoped_bindings_fields_and_receiver_methods() {
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: r#"
struct Player { hp: i32; speed: f32; }
enum Mode { Playing, Paused, }
global state { player: Player; }
function damage(player: Player, amount: i32): i32 { return player.hp - amount; }
function tick(): i32 {
    let hero: Player;
    hero.hp = 7;
    return damage(hero, 1);
}
"#
            .to_string(),
        }];
        let items = workshop_completion_items(&files).expect("completion catalog");
        let has = |text: &str, kind: &str| {
            items
                .iter()
                .any(|item| item.text == text && item.kind == kind)
        };
        assert!(has("amount", "parameter"));
        assert!(has("hero", "local"));
        assert!(has("Player.hp", "field"));
        assert!(has("player.hp", "field"));
        assert!(has("player.damage", "method"));
        assert!(has("hero.hp", "field"));
        assert!(has("hero.damage", "method"));
        assert!(has("state.player.hp", "field"));
        assert!(has("Mode.Paused", "enum_variant"));
        let hero_hp = items
            .iter()
            .find(|item| item.text == "hero.hp")
            .expect("scoped receiver field");
        assert_eq!(hero_hp.type_name.as_deref(), Some("i32"));
        assert_eq!(
            hero_hp.scope.as_ref().map(|scope| scope.owner.as_str()),
            Some("tick")
        );
        let parameter = items
            .iter()
            .find(|item| item.text == "amount" && item.kind == "parameter")
            .expect("parameter");
        assert_eq!(
            parameter.scope.as_ref().map(|scope| scope.owner.as_str()),
            Some("damage")
        );
        assert!(items
            .iter()
            .find(|item| item.text == "state.player.hp")
            .expect("global receiver field")
            .scope
            .is_none());
        assert!(items
            .iter()
            .all(|item| item.detail.contains("[src/main.stasis]")));
    }

    #[test]
    fn semantic_update_hoists_embedded_import_and_prunes_unused_import() {
        let files = vec![
            WorkshopSourceFile {
                path: "src/main.stasis".to_string(),
                source: "import \"old.stasis\";\n\nfunction main(): i32 { return tick(); }\n\n// old comment\nfunction tick(): i32 { return helper_old(); }\n"
                    .to_string(),
            },
            WorkshopSourceFile {
                path: "src/old.stasis".to_string(),
                source: "function helper_old(): i32 { return 1; }\n".to_string(),
            },
            WorkshopSourceFile {
                path: "src/new.stasis".to_string(),
                source: "function helper_new(): i32 { return 9; }\n".to_string(),
            },
        ];
        let batch = WorkshopSemanticEditBatch {
            schema_version: 1,
            edits: vec![WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Update,
                target: WorkshopSymbolSelector {
                    symbol_id: None,
                    name: "tick".to_string(),
                    kind: Some(WorkshopSourceItemKind::Function),
                    file: Some("src/main.stasis".to_string()),
                    owner: None,
                    signature: None,
                },
                new_source: Some(
                    "function tick(): i32 {\n    import \"new.stasis\";\n    return helper_new();\n}"
                        .to_string(),
                ),
                expected_source_hash: None,
            }],
        };
        let (after, plan) = plan_workshop_semantic_edits(&files, &batch).expect("plan edit");
        let main = after
            .iter()
            .find(|file| file.path == "src/main.stasis")
            .expect("main file");
        assert!(main.source.starts_with("import \"new.stasis\";\n"));
        assert!(!main.source.contains("old.stasis"));
        assert!(!main.source.contains("    import"));
        assert!(main.source.contains("return helper_new();"));
        assert_eq!(plan.changed_files.len(), 1);
        assert_eq!(plan.changed_files[0].file, "src/main.stasis");
    }

    #[test]
    fn semantic_items_do_not_cross_same_line_declarations() {
        let source = "function first(): i32 { return 1; } function second(): i32 { return 2; }\n";
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: source.to_string(),
        }];
        let second = workshop_source_items(&files)
            .expect("items")
            .into_iter()
            .find(|item| item.kind == WorkshopSourceItemKind::Function && item.name == "second")
            .expect("second item");
        assert_eq!(second.source, "function second(): i32 { return 2; }\n");
        let batch = WorkshopSemanticEditBatch {
            schema_version: 1,
            edits: vec![WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Delete,
                target: WorkshopSymbolSelector {
                    symbol_id: None,
                    name: "second".to_string(),
                    kind: Some(WorkshopSourceItemKind::Function),
                    file: Some("src/main.stasis".to_string()),
                    owner: None,
                    signature: None,
                },
                new_source: None,
                expected_source_hash: Some(second.source_hash),
            }],
        };
        let (after, _) = plan_workshop_semantic_edits(&files, &batch).expect("delete second");
        assert_eq!(after[0].source, "function first(): i32 { return 1; } ");
    }

    #[test]
    fn semantic_edit_preserves_imported_lifecycle_roots() {
        let files = vec![
            WorkshopSourceFile {
                path: "src/main.stasis".to_string(),
                source: "import \"game.stasis\";\nfunction main(): i32 { return 1; }\n".to_string(),
            },
            WorkshopSourceFile {
                path: "src/game.stasis".to_string(),
                source: "function tick(): void {}\n".to_string(),
            },
        ];
        let batch = WorkshopSemanticEditBatch {
            schema_version: 1,
            edits: vec![WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Update,
                target: WorkshopSymbolSelector {
                    symbol_id: None,
                    name: "main".to_string(),
                    kind: Some(WorkshopSourceItemKind::Function),
                    file: Some("src/main.stasis".to_string()),
                    owner: None,
                    signature: None,
                },
                new_source: Some("function main(): i32 { return 2; }".to_string()),
                expected_source_hash: None,
            }],
        };
        let (after, _) = plan_workshop_semantic_edits(&files, &batch).expect("update main");
        assert!(after[0].source.contains("import \"game.stasis\";"));
    }

    #[test]
    fn organize_imports_sorts_deduplicates_and_prunes_unused_modules() {
        let files = vec![
            WorkshopSourceFile {
                path: "src/main.stasis".to_string(),
                source: "import \"unused.stasis\";\nimport \"used.stasis\";\nimport \"used.stasis\";\nfunction main(): i32 { return helper(); }\n"
                    .to_string(),
            },
            WorkshopSourceFile {
                path: "src/used.stasis".to_string(),
                source: "function helper(): i32 { return 3; }\n".to_string(),
            },
            WorkshopSourceFile {
                path: "src/unused.stasis".to_string(),
                source: "function unrelated(): i32 { return 4; }\n".to_string(),
            },
        ];
        let change = organize_workshop_imports(&files, "src/main.stasis")
            .expect("organize imports")
            .expect("source change");
        assert_eq!(
            change.after_source,
            "import \"used.stasis\";\nfunction main(): i32 { return helper(); }\n"
        );
        assert_eq!(
            change.before_hash,
            workshop_source_hash(&change.before_source)
        );
        assert_eq!(
            change.after_hash,
            workshop_source_hash(&change.after_source)
        );
    }

    #[test]
    fn semantic_import_pruning_distinguishes_fields_from_receiver_calls() {
        let files = vec![
            WorkshopSourceFile {
                path: "src/main.stasis".to_string(),
                source: "import \"unused.stasis\";\nimport \"mixed.stasis\";\nimport \"combat.stasis\";\nenum Phase { Ready = 1, }\nstruct Player { value: i32; }\nglobal State { player: Player; }\nfunction main(): i32 { let value: i32 = 1; State.player.damage(1); return value; }\nfunction parameter(amount: i32): i32 { return amount; }\nfunction shadow(): i32 { let helper: i32 = 3; return helper; }\nfunction imported_call(): i32 { return helper(); }\n"
                    .to_string(),
            },
            WorkshopSourceFile {
                path: "src/unused.stasis".to_string(),
                source: "function value(): i32 { return 9; }\nfunction amount(): i32 { return 7; }\nfunction Ready(): i32 { return 10; }\n"
                    .to_string(),
            },
            WorkshopSourceFile {
                path: "src/mixed.stasis".to_string(),
                source: "function helper(): i32 { return 8; }\n".to_string(),
            },
            WorkshopSourceFile {
                path: "src/combat.stasis".to_string(),
                source: "function damage(self: Player, amount: i32): void {}\n".to_string(),
            },
        ];
        let batch = WorkshopSemanticEditBatch {
            schema_version: 1,
            edits: vec![semantic_edit(
                WorkshopSemanticEditOperation::Update,
                semantic_selector("main", WorkshopSourceItemKind::Function, "src/main.stasis"),
                Some("function main(): i32 { let value: i32 = 2; State.player.damage(2); return value; }"),
            )],
        };
        let (after, _) = plan_workshop_semantic_edits(&files, &batch).expect("update main");
        let source = &after
            .iter()
            .find(|file| file.path == "src/main.stasis")
            .expect("main")
            .source;
        assert!(!source.contains("unused.stasis"));
        assert!(source.contains("mixed.stasis"));
        assert!(source.contains("combat.stasis"));
    }

    #[test]
    fn semantic_global_delete_removes_only_the_parser_selected_declaration() {
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: "const FIRST: i32 = 1;\nconst SECOND: i32 = 1;\nfunction main(): i32 { return SECOND; }\n"
                .to_string(),
        }];
        let globals = workshop_source_items(&files)
            .expect("items")
            .into_iter()
            .find(|item| item.kind == WorkshopSourceItemKind::Globals)
            .expect("globals");
        let batch = WorkshopSemanticEditBatch {
            schema_version: 1,
            edits: vec![WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Delete,
                target: WorkshopSymbolSelector {
                    symbol_id: None,
                    name: "FIRST".to_string(),
                    kind: Some(WorkshopSourceItemKind::Globals),
                    file: Some("src/main.stasis".to_string()),
                    owner: Some("Globals".to_string()),
                    signature: None,
                },
                new_source: None,
                expected_source_hash: Some(globals.source_hash),
            }],
        };
        let (after, _) = plan_workshop_semantic_edits(&files, &batch).expect("delete FIRST");
        assert!(!after[0].source.contains("FIRST"));
        assert!(after[0].source.contains("const SECOND: i32 = 1;"));
    }

    #[test]
    fn semantic_plan_apply_and_revert_are_hash_guarded() {
        let root = temp_project("semantic_receipt");
        let path = write_project_file(
            &root,
            "src/main.stasis",
            "function main(): i32 { return 1; }\n",
        );
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: fs::read_to_string(&path).expect("read source"),
        }];
        let batch = WorkshopSemanticEditBatch {
            schema_version: 1,
            edits: vec![WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Update,
                target: WorkshopSymbolSelector {
                    symbol_id: None,
                    name: "main".to_string(),
                    kind: Some(WorkshopSourceItemKind::Function),
                    file: Some("src/main.stasis".to_string()),
                    owner: None,
                    signature: None,
                },
                new_source: Some("function main(): i32 { return 7; }".to_string()),
                expected_source_hash: None,
            }],
        };
        let (_, plan) = plan_workshop_semantic_edits(&files, &batch).expect("plan");
        write_workshop_semantic_plan(&root, &plan, false).expect("apply");
        assert!(fs::read_to_string(&path)
            .expect("read applied")
            .contains("return 7"));
        write_workshop_semantic_plan(&root, &plan, true).expect("revert");
        assert!(fs::read_to_string(&path)
            .expect("read reverted")
            .contains("return 1"));
        let stale = fs::write(&path, "function main(): i32 { return 3; }\n");
        stale.expect("write stale source");
        assert!(write_workshop_semantic_plan(&root, &plan, false)
            .expect_err("stale apply")
            .contains("expected current hash"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn semantic_item_hash_ignores_formatting_outside_the_item() {
        let first = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: "function main(): i32 { return tick(); }\n\n// Stable tick.\nfunction tick(): i32 { return 1; }\n"
                .to_string(),
        }];
        let second = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: "function main(): i32 {\n    return tick();\n}\n\n\n// Stable tick.\nfunction tick(): i32 { return 1; }\n\n"
                .to_string(),
        }];
        let tick = |files: &[WorkshopSourceFile]| {
            workshop_source_items(files)
                .expect("items")
                .into_iter()
                .find(|item| item.kind == WorkshopSourceItemKind::Function && item.name == "tick")
                .expect("tick")
        };
        let first_tick = tick(&first);
        let second_tick = tick(&second);
        assert_eq!(first_tick.source, second_tick.source);
        assert_eq!(first_tick.source_hash, second_tick.source_hash);
        assert_ne!(
            workshop_source_hash(&first[0].source),
            workshop_source_hash(&second[0].source)
        );
    }

    #[test]
    fn semantic_batch_reindexes_shifted_items_between_edits() {
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: "function first(): i32 { return 1; }\nfunction second(): i32 { return 2; }\n"
                .to_string(),
        }];
        let items = workshop_source_items(&files).expect("items");
        let item_hash = |name: &str| {
            items
                .iter()
                .find(|item| item.kind == WorkshopSourceItemKind::Function && item.name == name)
                .expect("function item")
                .source_hash
                .clone()
        };
        let mut first = semantic_edit(
            WorkshopSemanticEditOperation::Update,
            semantic_selector("first", WorkshopSourceItemKind::Function, "src/main.stasis"),
            Some("function first(): i32 {\n    return 11;\n}"),
        );
        first.expected_source_hash = Some(item_hash("first"));
        let mut second = semantic_edit(
            WorkshopSemanticEditOperation::Update,
            semantic_selector(
                "second",
                WorkshopSourceItemKind::Function,
                "src/main.stasis",
            ),
            Some("function second(): i32 { return 22; }"),
        );
        second.expected_source_hash = Some(item_hash("second"));
        let (after, plan) = plan_workshop_semantic_edits(
            &files,
            &WorkshopSemanticEditBatch {
                schema_version: 1,
                edits: vec![first, second],
            },
        )
        .expect("plan shifted edits");
        assert!(after[0].source.contains("return 11;"));
        assert!(after[0].source.contains("return 22;"));
        assert_eq!(plan.changed_files.len(), 1);
        assert_eq!(plan.edits.len(), 2);
    }

    #[test]
    fn semantic_multi_file_preflight_prevents_partial_writes() {
        let root = temp_project("semantic_preflight");
        let first_path = write_project_file(
            &root,
            "src/first.stasis",
            "function first(): i32 { return 1; }\n",
        );
        let second_path = write_project_file(
            &root,
            "src/second.stasis",
            "function second(): i32 { return 2; }\n",
        );
        let files = vec![
            WorkshopSourceFile {
                path: "src/first.stasis".to_string(),
                source: fs::read_to_string(&first_path).expect("first source"),
            },
            WorkshopSourceFile {
                path: "src/second.stasis".to_string(),
                source: fs::read_to_string(&second_path).expect("second source"),
            },
        ];
        let (_, plan) = plan_workshop_semantic_edits(
            &files,
            &WorkshopSemanticEditBatch {
                schema_version: 1,
                edits: vec![
                    semantic_edit(
                        WorkshopSemanticEditOperation::Update,
                        semantic_selector(
                            "first",
                            WorkshopSourceItemKind::Function,
                            "src/first.stasis",
                        ),
                        Some("function first(): i32 { return 11; }"),
                    ),
                    semantic_edit(
                        WorkshopSemanticEditOperation::Update,
                        semantic_selector(
                            "second",
                            WorkshopSourceItemKind::Function,
                            "src/second.stasis",
                        ),
                        Some("function second(): i32 { return 22; }"),
                    ),
                ],
            },
        )
        .expect("multi-file plan");
        fs::write(&second_path, "function second(): i32 { return 3; }\n")
            .expect("make second stale");
        assert!(write_workshop_semantic_plan(&root, &plan, false)
            .expect_err("stale plan")
            .contains("expected current hash"));
        assert_eq!(
            fs::read_to_string(&first_path).expect("unchanged first"),
            "function first(): i32 { return 1; }\n"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn semantic_multi_file_write_reports_incomplete_rollback() {
        let root = temp_project("semantic_rollback_failure");
        write_project_file(
            &root,
            "src/first.stasis",
            "function first(): i32 { return 1; }\n",
        );
        write_project_file(
            &root,
            "src/second.stasis",
            "function second(): i32 { return 2; }\n",
        );
        let files = vec![
            WorkshopSourceFile {
                path: "src/first.stasis".to_string(),
                source: "function first(): i32 { return 1; }\n".to_string(),
            },
            WorkshopSourceFile {
                path: "src/second.stasis".to_string(),
                source: "function second(): i32 { return 2; }\n".to_string(),
            },
        ];
        let (_, plan) = plan_workshop_semantic_edits(
            &files,
            &WorkshopSemanticEditBatch {
                schema_version: 1,
                edits: vec![
                    semantic_edit(
                        WorkshopSemanticEditOperation::Update,
                        semantic_selector(
                            "first",
                            WorkshopSourceItemKind::Function,
                            "src/first.stasis",
                        ),
                        Some("function first(): i32 { return 11; }"),
                    ),
                    semantic_edit(
                        WorkshopSemanticEditOperation::Update,
                        semantic_selector(
                            "second",
                            WorkshopSourceItemKind::Function,
                            "src/second.stasis",
                        ),
                        Some("function second(): i32 { return 22; }"),
                    ),
                ],
            },
        )
        .expect("plan");
        let mut writes = 0;
        let error = write_workshop_semantic_plan_with(&root, &plan, false, |_, _| {
            writes += 1;
            match writes {
                1 => Ok(()),
                2 => Err("injected write failure".to_string()),
                _ => Err("injected rollback failure".to_string()),
            }
        })
        .expect_err("write and rollback fail");
        assert!(error.contains("injected write failure"));
        assert!(error.contains("rollback incomplete"));
        assert!(error.contains("src/first.stasis: injected rollback failure"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn semantic_receipts_are_deterministic_and_reject_unsafe_directories() {
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: "function main(): i32 { return 1; }\n".to_string(),
        }];
        let (_, plan) = plan_workshop_semantic_edits(
            &files,
            &WorkshopSemanticEditBatch {
                schema_version: 1,
                edits: vec![semantic_edit(
                    WorkshopSemanticEditOperation::Update,
                    semantic_selector("main", WorkshopSourceItemKind::Function, "src/main.stasis"),
                    Some("function main(): i32 { return 2; }"),
                )],
            },
        )
        .expect("plan");
        let root = temp_project("semantic_deterministic_receipt");
        let first =
            write_workshop_semantic_receipt(&root, Path::new("build/semantic-edits"), &plan)
                .expect("first receipt");
        let first_source = fs::read_to_string(root.join(&first)).expect("first receipt source");
        let second =
            write_workshop_semantic_receipt(&root, Path::new("build/semantic-edits"), &plan)
                .expect("second receipt");
        assert_eq!(first, second);
        assert_eq!(
            first_source,
            fs::read_to_string(root.join(&second)).expect("second receipt source")
        );
        assert_eq!(workshop_source_hash("source").len(), 64);
        assert!(
            write_workshop_semantic_receipt(&root, Path::new("../outside"), &plan)
                .expect_err("unsafe receipt directory")
                .contains("unsafe semantic receipt directory")
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn semantic_delete_preserves_crlf_neighbor_boundaries() {
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: "// Remove me.\r\nfunction removed(): i32 { return 1; }\r\n// Keep me.\r\nfunction kept(): i32 { return 2; }\r\n"
                .to_string(),
        }];
        let removed = workshop_source_items(&files)
            .expect("items")
            .into_iter()
            .find(|item| item.kind == WorkshopSourceItemKind::Function && item.name == "removed")
            .expect("removed item");
        assert!(removed.source.ends_with("}\r\n"));
        let (after, _) = plan_workshop_semantic_edits(
            &files,
            &WorkshopSemanticEditBatch {
                schema_version: 1,
                edits: vec![semantic_edit(
                    WorkshopSemanticEditOperation::Delete,
                    semantic_selector(
                        "removed",
                        WorkshopSourceItemKind::Function,
                        "src/main.stasis",
                    ),
                    None,
                )],
            },
        )
        .expect("delete CRLF item");
        assert_eq!(
            after[0].source,
            "// Keep me.\r\nfunction kept(): i32 { return 2; }\r\n"
        );
    }

    #[test]
    fn semantic_batch_adds_each_item_kind_without_span_overlap() {
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: "function main(): i32 { return helper(); }\n".to_string(),
        }];
        let edits = vec![
            semantic_edit(
                WorkshopSemanticEditOperation::Add,
                semantic_selector(
                    "imports",
                    WorkshopSourceItemKind::Imports,
                    "src/main.stasis",
                ),
                Some("import \"external.stasis\";"),
            ),
            semantic_edit(
                WorkshopSemanticEditOperation::Add,
                semantic_selector(
                    "globals",
                    WorkshopSourceItemKind::Globals,
                    "src/main.stasis",
                ),
                Some("const LIMIT: i32 = 3;"),
            ),
            semantic_edit(
                WorkshopSemanticEditOperation::Add,
                semantic_selector("Config", WorkshopSourceItemKind::Struct, "src/main.stasis"),
                Some("// Configuration.\nstruct Config { value: i32; }"),
            ),
            semantic_edit(
                WorkshopSemanticEditOperation::Add,
                semantic_selector(
                    "helper",
                    WorkshopSourceItemKind::Function,
                    "src/main.stasis",
                ),
                Some("// Helper.\nfunction helper(): i32 { return LIMIT; }"),
            ),
        ];
        let (after, plan) = plan_workshop_semantic_edits(
            &files,
            &WorkshopSemanticEditBatch {
                schema_version: 1,
                edits,
            },
        )
        .expect("add mixed items");
        let source = &after[0].source;
        assert!(source.starts_with("import \"external.stasis\";\nconst LIMIT: i32 = 3;\n"));
        assert_eq!(source.matches("struct Config").count(), 1);
        assert_eq!(source.matches("function helper").count(), 1);
        assert_eq!(plan.edits.len(), 4);
        let items = workshop_source_items(&after).expect("re-index added items");
        assert!(items.iter().any(|item| item.name == "Config"));
        assert!(items.iter().any(|item| item.name == "helper"));
    }

    #[test]
    fn hierarchy_uses_compiler_call_edges_and_struct_composition() {
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: "struct Position { x: i32; }\nstruct Enemy { position: Position; }\nfunction a(): i32 { return 1; }\nfunction b(): i32 { return a(); }\n"
                .to_string(),
        }];
        let (call_items, calls) = workshop_call_hierarchy(&files).expect("call hierarchy");
        let a = call_items.iter().find(|item| item.name == "a").expect("a");
        let b = call_items.iter().find(|item| item.name == "b").expect("b");
        let edge = calls
            .iter()
            .find(|edge| {
                edge.caller_symbol_id == b.symbol_id && edge.callee_symbol_id == a.symbol_id
            })
            .expect("b calls a");
        assert_eq!(
            &files[0].source[edge.call_span.start as usize..edge.call_span.end as usize],
            "a"
        );

        let (type_items, types) = workshop_type_hierarchy(&files).expect("type hierarchy");
        let position = type_items
            .iter()
            .find(|item| item.name == "Position")
            .expect("Position");
        let enemy = type_items
            .iter()
            .find(|item| item.name == "Enemy")
            .expect("Enemy");
        assert!(types.iter().any(|edge| {
            edge.container_symbol_id == enemy.symbol_id
                && edge.component_symbol_id == position.symbol_id
        }));
    }

    #[test]
    fn folding_and_selection_ranges_recover_around_incomplete_delimiters() {
        let source = "function a(): i32 { if (true) { return 1; } }\nfunction b(): i32 { a(state.";
        let folds = workshop_folding_ranges(source).expect("folding ranges");
        assert_eq!(folds.len(), 2);
        assert!(folds.iter().all(|range| {
            &source[range.source_span.start as usize..range.source_span.end as usize]
                != "{ a(state."
        }));

        let offset = source.find("return 1").expect("return") + 2;
        let selections = workshop_selection_ranges(source, offset).expect("selection ranges");
        assert_eq!(
            &source[selections[0].start as usize..selections[0].end as usize],
            "return"
        );
        assert!(selections
            .windows(2)
            .all(|pair| { pair[1].start <= pair[0].start && pair[0].end <= pair[1].end }));
        assert_eq!(
            selections.last().expect("file selection").end as usize,
            source.len()
        );
    }

    #[test]
    fn linked_edits_are_limited_to_compiler_scoped_bindings() {
        let source = "global score: i32;\nfunction add(value: i32): i32 { let copy: i32 = value; copy += value; return copy; }\n";
        let files = vec![WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: source.to_string(),
        }];
        let copy_offset = source.find("copy: i32").expect("copy") + 1;
        let ranges = workshop_linked_edit_ranges(&files, "src/main.stasis", copy_offset)
            .expect("linked local")
            .expect("local ranges");
        assert_eq!(ranges.len(), 3);
        assert!(ranges
            .iter()
            .all(|range| { &source[range.start as usize..range.end as usize] == "copy" }));

        let global_offset = source.find("score").expect("score") + 1;
        assert!(
            workshop_linked_edit_ranges(&files, "src/main.stasis", global_offset)
                .expect("global linked edit")
                .is_none()
        );
    }
}
