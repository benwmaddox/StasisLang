use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::frontend::lexer::{lex, Token, TokenKind};
use crate::frontend::parser::{parse_top_level_functions, parse_top_level_type_layout};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopSymbolKind {
    Struct,
    Function,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkshopSymbolGroupKind {
    Main,
    Struct,
    System,
    Root,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopSourceFile {
    pub path: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopSourceSpan {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopSymbol {
    pub kind: WorkshopSymbolKind,
    pub name: String,
    pub owner: Option<String>,
    pub file: String,
    pub signature: String,
    pub source_span: WorkshopSourceSpan,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopSymbolGroup {
    pub kind: WorkshopSymbolGroupKind,
    pub name: String,
    pub symbols: Vec<WorkshopSymbol>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    let entry_path = if entry_file.is_absolute() {
        entry_file.to_path_buf()
    } else {
        project_root.join(entry_file)
    };
    let mut visited = BTreeSet::new();
    let mut out = Vec::new();
    load_workshop_project_file(project_root, &entry_path, &mut visited, &mut out)?;
    out.sort_by_key(|file| file.path.clone());
    Ok(out)
}

fn load_workshop_project_file(
    project_root: &Path,
    path: &Path,
    visited: &mut BTreeSet<PathBuf>,
    out: &mut Vec<WorkshopSourceFile>,
) -> Result<(), String> {
    let normalized_path = normalize_filesystem_path(path);
    if !visited.insert(normalized_path.clone()) {
        return Ok(());
    }

    let source = fs::read_to_string(&normalized_path)
        .map_err(|error| format!("failed reading {}: {error}", normalized_path.display()))?;
    let relative_path = normalize_workshop_project_path(project_root, &normalized_path);
    out.push(WorkshopSourceFile {
        path: relative_path,
        source: source.clone(),
    });

    for import_path in parse_workshop_import_paths(&source)? {
        let base_dir = normalized_path.parent().unwrap_or(project_root);
        let resolved = normalize_filesystem_path(&base_dir.join(import_path));
        if resolved
            .extension()
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("stasis"))
        {
            return Err(format!(
                "import resolved to non-stasis file: {}",
                resolved.display()
            ));
        }
        load_workshop_project_file(project_root, &resolved, visited, out)?;
    }
    Ok(())
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

pub fn build_workshop_symbol_tree(
    files: &[WorkshopSourceFile],
) -> Result<WorkshopSymbolTree, String> {
    let mut struct_names = BTreeSet::new();
    let mut structs_by_file: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for file in files {
        let layout = parse_top_level_type_layout(&file.source)?;
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
        WorkshopSymbolGroupKind::System,
        WorkshopSymbolGroupKind::Root,
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
    let mut out = Vec::new();
    for parsed_struct in parse_struct_spans(&file.source)? {
        let source = source_for_range(&file.source, parsed_struct.range.clone())?;
        out.push(PendingSymbol {
            group_kind: WorkshopSymbolGroupKind::Struct,
            group_name: parsed_struct.name.clone(),
            symbol: WorkshopSymbol {
                kind: WorkshopSymbolKind::Struct,
                name: parsed_struct.name.clone(),
                owner: Some(parsed_struct.name.clone()),
                file: file.path.clone(),
                signature: format!("struct {}", parsed_struct.name),
                source_span: span_from_range(parsed_struct.range.clone())?,
                source,
            },
        });
    }

    for function in parse_top_level_functions(&file.source)? {
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
                kind: WorkshopSymbolKind::Function,
                name: function.name,
                owner,
                file: file.path.clone(),
                signature,
                source_span: span_from_range(full_range)?,
                source,
            },
        });
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedStructSpan {
    name: String,
    range: Range<usize>,
}

fn parse_struct_spans(source: &str) -> Result<Vec<ParsedStructSpan>, String> {
    let tokens = lex(source)?;
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut depth = 0usize;
    while cursor < tokens.len() {
        let token = tokens[cursor];
        match token.kind {
            TokenKind::LBrace => {
                depth = depth.saturating_add(1);
                cursor += 1;
                continue;
            }
            TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
                cursor += 1;
                continue;
            }
            TokenKind::Identifier if depth == 0 && token_text(source, token) == "struct" => {
                let name_token = expect_token(&tokens, cursor + 1, TokenKind::Identifier)?;
                let open_index = cursor + 2;
                expect_token(&tokens, open_index, TokenKind::LBrace)?;
                let close_index = find_matching_rbrace(&tokens, open_index + 1, 1)?;
                let end = tokens[close_index].end;
                out.push(ParsedStructSpan {
                    name: token_text(source, name_token).to_string(),
                    range: token.start..end,
                });
                cursor = close_index + 1;
                continue;
            }
            _ => {}
        }
        cursor += 1;
    }
    Ok(out)
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
    pub fn android_default() -> Self {
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
        if edit.kind != AiCodeEditKind::ReplaceFunction {
            return Err(format!("unsupported AI edit kind for {}", edit.name));
        }
        validate_replacement_function_source(&edit.new_source)?;
        let symbol = symbols
            .iter()
            .find(|symbol| {
                symbol.kind == WorkshopSymbolKind::Function
                    && symbol.name == edit.name
                    && symbol.owner == edit.owner
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
        let layout = parse_top_level_type_layout(&file.source)?;
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
        let layout = parse_top_level_type_layout(&file.source)?;
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
        let Some(after_symbol) = after.get(&identity) else {
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
    kind: WorkshopSymbolKind,
    file: String,
    owner: Option<String>,
    name: String,
}

fn symbol_identity(symbol: &WorkshopSymbol) -> SymbolIdentity {
    SymbolIdentity {
        kind: symbol.kind,
        file: symbol.file.clone(),
        owner: symbol.owner.clone(),
        name: symbol.name.clone(),
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

fn validate_replacement_function_source(source: &str) -> Result<(), String> {
    let trimmed = source.trim_start();
    if !trimmed.starts_with("function ") {
        return Err("replace_function edit must provide Stasis function source".to_string());
    }
    if trimmed.contains("&mut") || trimmed.contains("->") {
        return Err(
            "replace_function edit must use Stasis syntax, not Rust reference or arrow syntax"
                .to_string(),
        );
    }
    parse_top_level_functions(trimmed)?;
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
}

#[cfg(test)]
mod ai_tests {
    use super::*;

    #[test]
    fn serializes_android_ai_request_with_stasis_style_rules() {
        let symbol = WorkshopSymbol {
            kind: WorkshopSymbolKind::Function,
            name: "jump".to_string(),
            owner: Some("Player".to_string()),
            file: "src/player.stasis".to_string(),
            signature: "jump(self: Player): void".to_string(),
            source_span: WorkshopSourceSpan { start: 0, end: 52 },
            source: "function jump(self: Player): void { return; }".to_string(),
        };
        let request = AiCodeRequest {
            user_prompt: "Make the player jump higher but prevent repeated jumps.".to_string(),
            selected_symbols: vec![selected_symbol_from_workshop_symbol(&symbol)],
            stasis_style_rules: StasisStyleRules::android_default(),
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
        fs::remove_dir_all(&root).ok();
    }
}
