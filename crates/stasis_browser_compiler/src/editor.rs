use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use stasis_compiler::frontend::lexer::{lex, lex_with_diagnostic, Token, TokenKind};
use stasis_compiler::frontend::module_graph::{
    workshop_file_exposure, ModuleGraph, ModuleImport, WorkshopExposure,
};
use stasis_compiler::frontend::parser::{
    parse_local_declarations, parse_top_level_function_signatures_with_diagnostic,
    parse_top_level_functions_with_diagnostic, parse_top_level_type_layout,
    parse_typed_local_bindings, ParsedFunctionSignature, ParsedGenericParameter,
    ParsedGenericParameterKind, ParsedLocalBinding, ParsedLocalDeclaration, ParsedTypeLayout,
};

pub(super) const MAX_EDITOR_JSON_BYTES: usize = 8 * 1024 * 1024;
const MAX_EDITOR_TOKENS: usize = 100_000;
const MAX_EDITOR_COMPLETIONS: usize = 100;
const MAX_DISPLAY_TEXT_CHARS: usize = 512;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnalyzeEditorRequest {
    path: String,
    source: String,
    #[serde(default)]
    files: Vec<EditorSourceFile>,
    #[serde(default)]
    cursor: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EditorSourceFile {
    pub path: String,
    pub source: String,
}

#[derive(Debug, Clone, Default)]
pub(super) struct EditorCatalog {
    files: BTreeMap<String, EditorFileCatalog>,
}

#[derive(Debug, Clone, Default)]
struct EditorFileCatalog {
    path: String,
    module_alias: String,
    exposure: WorkshopExposure,
    imports: Vec<EditorImport>,
    functions: Vec<EditorFunction>,
    types: Vec<EditorType>,
    values: Vec<EditorValue>,
}

#[derive(Debug, Clone)]
struct EditorImport {
    alias: String,
    target: String,
}

#[derive(Debug, Clone)]
struct EditorFunction {
    parsed: ParsedFunctionSignature,
    signature: String,
    receiver_type: Option<String>,
}

#[derive(Debug, Clone)]
struct EditorType {
    path: String,
    name: String,
    kind: &'static str,
    detail: String,
    members: Vec<EditorMember>,
}

#[derive(Debug, Clone)]
struct EditorMember {
    name: String,
    type_name: Option<String>,
    kind: &'static str,
}

#[derive(Debug, Clone)]
struct EditorValue {
    name: String,
    type_name: Option<String>,
    kind: &'static str,
    detail: String,
}

#[derive(Debug, Clone, Default)]
struct EditorDraft {
    functions: Vec<ParsedFunctionSignature>,
    functions_complete: bool,
    layout: Option<ParsedTypeLayout>,
    locals: Vec<ParsedLocalDeclaration>,
    typed_locals: Vec<ParsedLocalBinding>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EditorAnalysis {
    path: String,
    tokens: Vec<EditorToken>,
    tokens_truncated: bool,
    completions: Vec<EditorCompletion>,
    replacement_range: Option<EditorRange>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EditorToken {
    start: u32,
    end: u32,
    kind: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EditorCompletion {
    label: String,
    insert_text: String,
    kind: &'static str,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EditorRange {
    start: u32,
    end: u32,
}

#[derive(Debug, Clone)]
struct RawEditorToken {
    start: usize,
    end: usize,
    kind: RawEditorTokenKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RawEditorTokenKind {
    Compiler(TokenKind),
    Comment,
    UnterminatedString,
    UnterminatedComment,
}

#[derive(Debug)]
struct DraftLex {
    tokens: Vec<RawEditorToken>,
    unterminated: Option<RawEditorTokenKind>,
}

#[derive(Debug, Clone)]
struct CompletionCandidate {
    label: String,
    kind: &'static str,
    detail: String,
    signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeIdentity {
    path: String,
    name: String,
}

#[derive(Debug, Clone)]
struct ResolvedTypeReceiver {
    identity: TypeIdentity,
    is_type: bool,
}

impl EditorCatalog {
    pub(super) fn from_sources(files: &[EditorSourceFile]) -> Self {
        Self::build(files, None)
    }

    fn build(files: &[EditorSourceFile], previous: Option<&EditorCatalog>) -> Self {
        let sources = files
            .iter()
            .map(|file| (file.path.clone(), file.source.clone()))
            .collect::<BTreeMap<_, _>>();
        let graph = build_module_graph(&sources);
        let mut catalog = Self::default();

        for file in files {
            let draft = parse_editor_draft(&file.source);
            let previous_file = previous.and_then(|catalog| catalog.files.get(&file.path));
            let mut record = file_catalog(file, &draft, graph.as_ref(), previous_file);
            record.path = file.path.clone();
            catalog.files.insert(file.path.clone(), record);
        }
        catalog
    }

    fn types(&self) -> BTreeSet<String> {
        let mut names = builtin_type_names();
        for file in self.files.values() {
            names.extend(file.types.iter().map(|item| item.name.clone()));
        }
        names
    }

    fn functions(&self) -> BTreeSet<String> {
        self.files
            .values()
            .flat_map(|file| file.functions.iter())
            .map(|function| function.parsed.name.clone())
            .collect()
    }

    fn analyze(
        &self,
        path: &str,
        source: &str,
        cursor: Option<u32>,
    ) -> Result<EditorAnalysis, String> {
        let draft = parse_editor_draft(source);
        let lexed = lex_editor_source(source);
        let type_names = self.types();
        let function_names = self.functions();
        let (tokens, tokens_truncated) =
            render_tokens(source, &lexed, &type_names, &function_names)?;

        let Some(cursor) = cursor else {
            return Ok(EditorAnalysis {
                path: path.to_string(),
                tokens,
                tokens_truncated,
                completions: Vec::new(),
                replacement_range: None,
            });
        };
        let cursor_byte = byte_offset_from_utf16(source, cursor as usize)?;
        let (replacement_bytes, receiver) = completion_context(source, cursor_byte, &lexed)?;
        let replacement_range = Some(EditorRange {
            start: utf16_offset_at(source, replacement_bytes.start)? as u32,
            end: utf16_offset_at(source, replacement_bytes.end)? as u32,
        });
        let completions = if lexed.unterminated.is_some_and(|_| {
            lexed.tokens.iter().any(|token| {
                matches!(
                    token.kind,
                    RawEditorTokenKind::UnterminatedString
                        | RawEditorTokenKind::UnterminatedComment
                ) && token.start <= cursor_byte
                    && cursor_byte <= token.end
            })
        }) || is_inside_complete_literal(&lexed, cursor_byte)
            || is_inside_comment(&lexed, cursor_byte)
        {
            Vec::new()
        } else {
            complete(
                self,
                path,
                &draft,
                source,
                cursor_byte,
                &replacement_bytes,
                receiver,
            )
        };

        Ok(EditorAnalysis {
            path: path.to_string(),
            tokens,
            tokens_truncated,
            completions,
            replacement_range,
        })
    }
}

pub(super) fn analyze_editor_json(
    input: &[u8],
    last_good: Option<&EditorCatalog>,
) -> Result<Vec<u8>, String> {
    if input.is_empty() || input.len() > MAX_EDITOR_JSON_BYTES {
        return Err(format!(
            "analyze-editor JSON must contain 1..={MAX_EDITOR_JSON_BYTES} bytes"
        ));
    }
    let text =
        std::str::from_utf8(input).map_err(|error| format!("request is not UTF-8: {error}"))?;
    let request: AnalyzeEditorRequest = serde_json::from_str(text)
        .map_err(|error| format!("invalid analyze-editor request: {error}"))?;
    super::validate_project_path(&request.path)?;

    if request.files.len() > super::MAX_PROJECT_FILES {
        return Err(format!(
            "analyze-editor must include at most {} project files",
            super::MAX_PROJECT_FILES
        ));
    }

    let mut sources = BTreeMap::<String, String>::new();
    for file in request.files {
        super::validate_project_path(&file.path)?;
        validate_source_size(&file.path, &file.source)?;
        if file.path != request.path && sources.insert(file.path.clone(), file.source).is_some() {
            return Err(format!("duplicate editor project path '{}'", file.path));
        }
    }
    validate_source_size(&request.path, &request.source)?;
    sources.insert(request.path.clone(), request.source.clone());
    if sources.len() > super::MAX_PROJECT_FILES {
        return Err(format!(
            "analyze-editor must include 1..={} project files",
            super::MAX_PROJECT_FILES
        ));
    }
    let total_source_bytes = sources.values().try_fold(0usize, |total, source| {
        total
            .checked_add(source.len())
            .ok_or_else(|| "editor project source byte count overflowed".to_string())
    })?;
    if total_source_bytes > super::MAX_PROJECT_SOURCE_BYTES {
        return Err(format!(
            "editor project sources exceed the {}-byte total limit",
            super::MAX_PROJECT_SOURCE_BYTES
        ));
    }

    let files = sources
        .into_iter()
        .map(|(path, source)| EditorSourceFile { path, source })
        .collect::<Vec<_>>();
    let catalog = EditorCatalog::build(&files, last_good);
    let analysis = catalog.analyze(&request.path, &request.source, request.cursor)?;
    let output = serde_json::to_vec(&analysis)
        .map_err(|error| format!("could not serialize editor analysis: {error}"))?;
    super::validate_output_size(
        "editor analysis",
        output.len(),
        super::MAX_OUTPUT_METADATA_BYTES,
    )?;
    Ok(output)
}

fn validate_source_size(path: &str, source: &str) -> Result<(), String> {
    if source.len() > super::MAX_SOURCE_FILE_BYTES {
        return Err(format!(
            "editor source file '{path}' exceeds the {}-byte per-file limit",
            super::MAX_SOURCE_FILE_BYTES
        ));
    }
    Ok(())
}

fn build_module_graph(sources: &BTreeMap<String, String>) -> Option<ModuleGraph> {
    let roots = sources.keys().cloned().collect::<Vec<_>>();
    ModuleGraph::load(roots, |path| {
        sources
            .get(path)
            .cloned()
            .ok_or_else(|| format!("editor source file '{path}' is not loaded"))
    })
    .ok()
    .map(|(graph, _)| graph)
}

fn file_catalog(
    file: &EditorSourceFile,
    draft: &EditorDraft,
    graph: Option<&ModuleGraph>,
    previous: Option<&EditorFileCatalog>,
) -> EditorFileCatalog {
    let module = graph.and_then(|graph| graph.module(&file.path));
    let imports = module
        .map(|module| module.imports.iter().map(EditorImport::from).collect())
        .or_else(|| previous.map(|previous| previous.imports.clone()))
        .unwrap_or_default();
    let module_alias = module
        .map(|module| module.alias.clone())
        .or_else(|| previous.map(|previous| previous.module_alias.clone()))
        .unwrap_or_default();

    let mut functions = draft
        .functions
        .iter()
        .cloned()
        .map(EditorFunction::new)
        .collect::<Vec<_>>();
    if !draft.functions_complete {
        if let Some(previous) = previous {
            for function in &previous.functions {
                if !functions
                    .iter()
                    .any(|current| current.parsed.name == function.parsed.name)
                {
                    functions.push(function.clone());
                }
            }
        }
    }
    functions.sort_by(|left, right| {
        left.parsed
            .name
            .cmp(&right.parsed.name)
            .then_with(|| left.signature.cmp(&right.signature))
    });

    let (types, values) = if let Some(layout) = draft.layout.as_ref() {
        catalog_types_and_values(&file.path, layout)
    } else {
        previous.map_or_else(
            || (Vec::new(), Vec::new()),
            |previous| (previous.types.clone(), previous.values.clone()),
        )
    };

    EditorFileCatalog {
        path: file.path.clone(),
        module_alias,
        exposure: workshop_file_exposure(&file.path),
        imports,
        functions,
        types,
        values,
    }
}

impl From<&ModuleImport> for EditorImport {
    fn from(import: &ModuleImport) -> Self {
        Self {
            alias: import.alias.clone(),
            target: import.target.clone(),
        }
    }
}

impl EditorFunction {
    fn new(parsed: ParsedFunctionSignature) -> Self {
        let receiver_type = parsed
            .params
            .first()
            .filter(|parameter| parameter.name == "self")
            .map(|parameter| base_type_name(&parameter.type_name).to_string());
        let signature = format_function_signature(&parsed, receiver_type.is_some());
        Self {
            parsed,
            signature,
            receiver_type,
        }
    }
}

fn catalog_types_and_values(
    path: &str,
    layout: &ParsedTypeLayout,
) -> (Vec<EditorType>, Vec<EditorValue>) {
    let mut types = Vec::new();
    let mut values = Vec::new();
    for definition in &layout.structs {
        types.push(EditorType {
            path: path.to_string(),
            name: definition.name.clone(),
            kind: "type",
            detail: format!("struct {}", definition.name),
            members: definition
                .fields
                .iter()
                .map(|field| EditorMember {
                    name: field.name.clone(),
                    type_name: Some(field.type_name.clone()),
                    kind: "field",
                })
                .collect(),
        });
    }
    for definition in &layout.enums {
        types.push(EditorType {
            path: path.to_string(),
            name: definition.name.clone(),
            kind: "type",
            detail: format!("enum {}", definition.name),
            members: definition
                .variants
                .iter()
                .map(|variant| EditorMember {
                    name: variant.name.clone(),
                    type_name: Some(definition.name.clone()),
                    kind: "enum_variant",
                })
                .collect(),
        });
    }
    for definition in &layout.global_blocks {
        types.push(EditorType {
            path: path.to_string(),
            name: definition.name.clone(),
            kind: "type",
            detail: format!("global block {}", definition.name),
            members: definition
                .fields
                .iter()
                .map(|field| EditorMember {
                    name: field.name.clone(),
                    type_name: Some(field.type_name.clone()),
                    kind: "field",
                })
                .collect(),
        });
    }
    for definition in &layout.globals {
        values.push(EditorValue {
            name: definition.name.clone(),
            type_name: Some(definition.type_name.clone()),
            kind: "global",
            detail: format!("global: {}", definition.type_name),
        });
    }
    for definition in &layout.constants {
        values.push(EditorValue {
            name: definition.name.clone(),
            type_name: Some(definition.type_name.clone()),
            kind: "constant",
            detail: format!("const: {}", definition.type_name),
        });
    }
    types.sort_by(|left, right| left.name.cmp(&right.name));
    values.sort_by(|left, right| left.name.cmp(&right.name));
    (types, values)
}

fn format_function_signature(function: &ParsedFunctionSignature, method: bool) -> String {
    let generics = format_generic_parameters(&function.generic_parameters);
    let params = function
        .params
        .iter()
        .filter(|parameter| !(method && parameter.name == "self"))
        .map(|parameter| format!("{}: {}", parameter.name, parameter.type_name))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{}{}({params}): {}",
        function.name, generics, function.return_type_name
    )
}

fn format_generic_parameters(parameters: &[ParsedGenericParameter]) -> String {
    if parameters.is_empty() {
        return String::new();
    }
    let parameters = parameters
        .iter()
        .map(|parameter| match parameter.kind {
            ParsedGenericParameterKind::Type => parameter.name.clone(),
            ParsedGenericParameterKind::I32 => format!("{}: i32", parameter.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("<{parameters}>")
}

fn parse_editor_draft(source: &str) -> EditorDraft {
    let parser_source = source_for_parser(source);
    let (functions, functions_complete) = parse_functions_best_effort(&parser_source);
    let layout = parse_top_level_type_layout(&parser_source).ok();
    let (locals, typed_locals) =
        if parse_top_level_functions_with_diagnostic(&parser_source).is_ok() {
            (
                parse_local_declarations(&parser_source).unwrap_or_default(),
                parse_typed_local_bindings(&parser_source).unwrap_or_default(),
            )
        } else {
            (Vec::new(), Vec::new())
        };
    EditorDraft {
        functions,
        functions_complete,
        layout,
        locals,
        typed_locals,
    }
}

fn source_for_parser(source: &str) -> String {
    let prefix_end = match lex_with_diagnostic(source) {
        Ok(_) => source.len(),
        Err(diagnostic) => floor_char_boundary(source, diagnostic.offset.min(source.len())),
    };
    let prefix = &source[..prefix_end];
    let mut depth = 0usize;
    if let Ok(tokens) = lex(prefix) {
        for token in tokens {
            match token.kind {
                TokenKind::LBrace => depth = depth.saturating_add(1),
                TokenKind::RBrace => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    let mut parser_source = prefix.to_string();
    parser_source.extend(std::iter::repeat('}').take(depth));
    parser_source
}

fn parse_functions_best_effort(source: &str) -> (Vec<ParsedFunctionSignature>, bool) {
    match parse_top_level_function_signatures_with_diagnostic(source) {
        Ok(functions) => return (functions, true),
        Err(_) => {}
    }

    let mut end = source.len();
    for _ in 0..64 {
        let diagnostic = match parse_top_level_function_signatures_with_diagnostic(&source[..end]) {
            Ok(functions) => return (functions, false),
            Err(diagnostic) => diagnostic,
        };
        let mut next_end = floor_char_boundary(source, diagnostic.start.min(end));
        if next_end >= end {
            next_end = previous_token_start(&source[..end]).unwrap_or(0);
        }
        if next_end == 0 || next_end >= end {
            break;
        }
        end = next_end;
    }
    (Vec::new(), false)
}

fn previous_token_start(source: &str) -> Option<usize> {
    lex(source)
        .ok()?
        .into_iter()
        .filter(|token| token.kind != TokenKind::Eof && token.start < source.len())
        .map(|token| token.start)
        .last()
}

fn floor_char_boundary(source: &str, mut offset: usize) -> usize {
    while !source.is_char_boundary(offset) {
        offset = offset.saturating_sub(1);
    }
    offset
}

fn builtin_type_names() -> BTreeSet<String> {
    ["void", "i32", "f32", "bool", "f64", "u8", "u16", "u32"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn base_type_name(type_name: &str) -> &str {
    let type_name = type_name.trim();
    let end = type_name
        .find(|ch| ch == '<' || ch == '[')
        .unwrap_or(type_name.len());
    type_name[..end].trim()
}

fn lex_editor_source(source: &str) -> DraftLex {
    let (prefix_end, unterminated) = match lex_with_diagnostic(source) {
        Ok(_) => (source.len(), None),
        Err(diagnostic) => {
            let kind = if diagnostic.message.contains("block comment") {
                RawEditorTokenKind::UnterminatedComment
            } else {
                RawEditorTokenKind::UnterminatedString
            };
            (
                floor_char_boundary(source, diagnostic.offset.min(source.len())),
                Some(kind),
            )
        }
    };
    let prefix = &source[..prefix_end];
    let compiler_tokens = lex(prefix).unwrap_or_default();
    let mut tokens = Vec::new();
    let mut cursor = 0usize;
    let mut index = 0usize;
    while index < compiler_tokens.len() {
        let token = compiler_tokens[index];
        if token.kind == TokenKind::Eof {
            append_comments(prefix, cursor, token.start, &mut tokens);
            break;
        }
        append_comments(prefix, cursor, token.start, &mut tokens);
        let mut end = token.end;
        if token.kind == TokenKind::Other {
            while compiler_tokens
                .get(index + 1)
                .is_some_and(|next| next.kind == TokenKind::Other && next.start == end)
            {
                index += 1;
                end = compiler_tokens[index].end;
            }
        }
        tokens.push(RawEditorToken {
            start: token.start,
            end,
            kind: RawEditorTokenKind::Compiler(token.kind),
        });
        cursor = end;
        index += 1;
    }
    if let Some(kind) = unterminated {
        tokens.push(RawEditorToken {
            start: prefix_end,
            end: source.len(),
            kind,
        });
    }
    DraftLex {
        tokens,
        unterminated,
    }
}

fn append_comments(source: &str, start: usize, end: usize, tokens: &mut Vec<RawEditorToken>) {
    let Some(gap) = source.get(start..end) else {
        return;
    };
    let bytes = gap.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
            continue;
        }
        if bytes.get(cursor..cursor + 2) == Some(b"//") {
            let mut comment_end = cursor + 2;
            while comment_end < bytes.len() && bytes[comment_end] != b'\n' {
                comment_end += 1;
            }
            tokens.push(RawEditorToken {
                start: start + cursor,
                end: start + comment_end,
                kind: RawEditorTokenKind::Comment,
            });
            cursor = comment_end;
            continue;
        }
        if bytes.get(cursor..cursor + 2) == Some(b"/*") {
            let comment_end = gap[cursor + 2..]
                .find("*/")
                .map_or(bytes.len(), |close| cursor + 2 + close + 2);
            tokens.push(RawEditorToken {
                start: start + cursor,
                end: start + comment_end,
                kind: RawEditorTokenKind::Comment,
            });
            cursor = comment_end;
            continue;
        }
        let ch = gap[cursor..].chars().next().expect("cursor is inside gap");
        cursor += ch.len_utf8();
    }
}

fn render_tokens(
    source: &str,
    lexed: &DraftLex,
    type_names: &BTreeSet<String>,
    function_names: &BTreeSet<String>,
) -> Result<(Vec<EditorToken>, bool), String> {
    let offsets = Utf16Offsets::new(source);
    let mut tokens = Vec::with_capacity(lexed.tokens.len().min(MAX_EDITOR_TOKENS));
    let mut truncated = false;
    for token in &lexed.tokens {
        if tokens.len() == MAX_EDITOR_TOKENS {
            truncated = true;
            break;
        }
        let Some(text) = source.get(token.start..token.end) else {
            continue;
        };
        let kind = match token.kind {
            RawEditorTokenKind::Comment | RawEditorTokenKind::UnterminatedComment => "comment",
            RawEditorTokenKind::UnterminatedString => "string",
            RawEditorTokenKind::Compiler(TokenKind::FunctionKw) => "keyword",
            RawEditorTokenKind::Compiler(TokenKind::Integer) => "number",
            RawEditorTokenKind::Compiler(TokenKind::StringLiteral | TokenKind::BacktickLiteral) => {
                "string"
            }
            RawEditorTokenKind::Compiler(TokenKind::Identifier) => {
                if is_keyword(text) {
                    "keyword"
                } else if type_names.contains(text) {
                    "type"
                } else if function_names.contains(text) {
                    "function"
                } else {
                    "identifier"
                }
            }
            RawEditorTokenKind::Compiler(TokenKind::Other) => {
                if text.chars().any(is_operator_character) {
                    "operator"
                } else {
                    "punctuation"
                }
            }
            RawEditorTokenKind::Compiler(TokenKind::Eof) => continue,
            RawEditorTokenKind::Compiler(_) => "punctuation",
        };
        let start = offsets.at(token.start)?;
        let end = offsets.at(token.end)?;
        if start < end {
            tokens.push(EditorToken { start, end, kind });
        }
    }
    Ok((tokens, truncated))
}

fn is_keyword(text: &str) -> bool {
    KEYWORDS.contains(&text)
}

const KEYWORDS: &[&str] = &[
    "import", "extern", "function", "struct", "enum", "global", "const", "test", "return", "let",
    "if", "else", "for", "foreach", "while", "in", "continue", "true", "false",
];

fn is_operator_character(ch: char) -> bool {
    matches!(
        ch,
        '+' | '-' | '*' | '/' | '%' | '=' | '!' | '<' | '>' | '&' | '|' | '^' | '?'
    )
}

fn is_inside_complete_literal(lexed: &DraftLex, cursor: usize) -> bool {
    lexed.tokens.iter().any(|token| {
        matches!(
            token.kind,
            RawEditorTokenKind::Compiler(TokenKind::StringLiteral | TokenKind::BacktickLiteral)
        ) && token.start < cursor
            && cursor < token.end
    })
}

fn is_inside_comment(lexed: &DraftLex, cursor: usize) -> bool {
    lexed.tokens.iter().any(|token| {
        matches!(
            token.kind,
            RawEditorTokenKind::Comment | RawEditorTokenKind::UnterminatedComment
        ) && token.start <= cursor
            && cursor <= token.end
    })
}

#[derive(Debug)]
struct Utf16Offsets {
    byte_to_utf16: Vec<u32>,
}

impl Utf16Offsets {
    fn new(source: &str) -> Self {
        let mut byte_to_utf16 = vec![0; source.len() + 1];
        let mut units = 0u32;
        for (start, ch) in source.char_indices() {
            let end = start + ch.len_utf8();
            for offset in start..end {
                byte_to_utf16[offset] = units;
            }
            units = units.saturating_add(ch.len_utf16() as u32);
            byte_to_utf16[end] = units;
        }
        Self { byte_to_utf16 }
    }

    fn at(&self, byte_offset: usize) -> Result<u32, String> {
        self.byte_to_utf16
            .get(byte_offset)
            .copied()
            .ok_or_else(|| "editor token offset is outside its source".to_string())
    }
}

fn utf16_offset_at(source: &str, byte_offset: usize) -> Result<usize, String> {
    if byte_offset > source.len() || !source.is_char_boundary(byte_offset) {
        return Err("editor byte offset is outside a UTF-8 boundary".to_string());
    }
    Ok(source[..byte_offset].encode_utf16().count())
}

fn byte_offset_from_utf16(source: &str, target: usize) -> Result<usize, String> {
    let mut units = 0usize;
    for (byte_offset, ch) in source.char_indices() {
        if target <= units {
            return Ok(byte_offset);
        }
        let next = units + ch.len_utf16();
        if target < next {
            // A DOM cursor can land between the two halves of a surrogate pair. Snap to the
            // character start so all compiler byte offsets remain valid UTF-8 boundaries.
            return Ok(byte_offset);
        }
        units = next;
    }
    if target == units {
        Ok(source.len())
    } else {
        Err("editor cursor is past the end of the active source".to_string())
    }
}

fn completion_context(
    source: &str,
    cursor: usize,
    lexed: &DraftLex,
) -> Result<(std::ops::Range<usize>, Option<Vec<String>>), String> {
    if cursor > source.len() || !source.is_char_boundary(cursor) {
        return Err("editor cursor is not on a UTF-8 boundary".to_string());
    }
    let bytes = source.as_bytes();
    let mut start = cursor;
    while start > 0 && is_identifier_continue_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = cursor;
    while end < bytes.len() && is_identifier_continue_byte(bytes[end]) {
        end += 1;
    }
    if is_inside_comment(lexed, cursor) || is_inside_complete_literal(lexed, cursor) {
        return Ok((start..end, None));
    }
    let receiver = receiver_before(source, start);
    Ok((start..end, receiver))
}

fn receiver_before(source: &str, prefix_start: usize) -> Option<Vec<String>> {
    let prefix = source.get(..prefix_start)?;
    let tokens = lex(prefix).ok()?;
    let tokens = tokens
        .into_iter()
        .filter(|token| token.kind != TokenKind::Eof)
        .collect::<Vec<_>>();
    let mut cursor = tokens.len();
    if cursor == 0 || token_text(prefix, tokens[cursor - 1]) != "." {
        return None;
    }
    cursor -= 1;
    let mut parts = Vec::new();
    loop {
        let token = tokens.get(cursor.checked_sub(1)?).copied()?;
        if token.kind != TokenKind::Identifier {
            return None;
        }
        parts.push(token_text(prefix, token).to_string());
        cursor -= 1;
        if cursor == 0 || token_text(prefix, tokens[cursor - 1]) != "." {
            break;
        }
        cursor -= 1;
    }
    parts.reverse();
    Some(parts)
}

fn is_identifier_continue_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn token_text(source: &str, token: Token) -> &str {
    source.get(token.start..token.end).unwrap_or_default()
}

fn complete(
    catalog: &EditorCatalog,
    active_path: &str,
    draft: &EditorDraft,
    source: &str,
    cursor: usize,
    replacement: &std::ops::Range<usize>,
    receiver: Option<Vec<String>>,
) -> Vec<EditorCompletion> {
    let prefix = source
        .get(replacement.start..cursor)
        .unwrap_or_default()
        .to_string();
    let mut candidates = BTreeMap::<(String, &'static str), CompletionCandidate>::new();
    let active_file = catalog.files.get(active_path);

    if let Some(parts) = receiver.as_ref() {
        let module_target = (parts.len() == 1)
            .then(|| {
                active_file
                    .and_then(|file| file.imports.iter().find(|import| import.alias == parts[0]))
                    .map(|import| import.target.as_str())
            })
            .flatten();
        if let Some(module) = module_target.and_then(|target| catalog.files.get(target)) {
            add_file_symbols(&mut candidates, module);
        } else if let Some(receiver) =
            resolve_receiver_type(catalog, active_path, draft, parts, cursor)
        {
            if let Some(definition) = find_type_identity(catalog, &receiver.identity) {
                if catalog
                    .files
                    .get(&definition.path)
                    .is_some_and(|file| file.exposure.is_public())
                {
                    for member in &definition.members {
                        if receiver.is_type && member.kind != "enum_variant" {
                            continue;
                        }
                        insert_candidate(
                            &mut candidates,
                            CompletionCandidate {
                                label: member.name.clone(),
                                kind: member.kind,
                                detail: member.type_name.as_ref().map_or_else(
                                    || format!("{} {}", definition.name, member.kind),
                                    |type_name| format!("{}: {type_name}", member.kind),
                                ),
                                signature: None,
                            },
                        );
                    }
                    if !receiver.is_type {
                        for (function_path, file) in &catalog.files {
                            if !file.exposure.is_public() {
                                continue;
                            }
                            for function in &file.functions {
                                if is_internal_function(function) {
                                    continue;
                                }
                                let Some(owner) = function.receiver_type.as_deref() else {
                                    continue;
                                };
                                if resolve_type_reference(catalog, function_path, owner).as_ref()
                                    != Some(&receiver.identity)
                                {
                                    continue;
                                }
                                insert_candidate(
                                    &mut candidates,
                                    CompletionCandidate {
                                        label: function.parsed.name.clone(),
                                        kind: "method",
                                        detail: format!("method on {}", definition.name),
                                        signature: Some(function.signature.clone()),
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
    } else {
        add_project_symbols(&mut candidates, catalog);
        if let Some(file) = active_file {
            for import in &file.imports {
                insert_candidate(
                    &mut candidates,
                    CompletionCandidate {
                        label: import.alias.clone(),
                        kind: "namespace",
                        detail: format!("module {}", import.target),
                        signature: None,
                    },
                );
            }
        }
        add_current_bindings(&mut candidates, draft, cursor);
        for keyword in KEYWORDS {
            insert_candidate(
                &mut candidates,
                CompletionCandidate {
                    label: keyword.to_string(),
                    kind: "keyword",
                    detail: "Stasis keyword".to_string(),
                    signature: None,
                },
            );
        }
    }

    candidates
        .into_values()
        .filter(|candidate| candidate.label.starts_with(&prefix))
        .take(MAX_EDITOR_COMPLETIONS)
        .map(|candidate| EditorCompletion {
            insert_text: candidate.label.clone(),
            label: truncate_display(candidate.label),
            kind: candidate.kind,
            detail: truncate_display(candidate.detail),
            signature: candidate.signature.map(truncate_display),
        })
        .collect()
}

fn add_project_symbols(
    candidates: &mut BTreeMap<(String, &'static str), CompletionCandidate>,
    catalog: &EditorCatalog,
) {
    for file in catalog.files.values() {
        add_file_symbols(candidates, file);
    }
    for type_name in builtin_type_names() {
        insert_candidate(
            candidates,
            CompletionCandidate {
                label: type_name.clone(),
                kind: "type",
                detail: "built-in type".to_string(),
                signature: None,
            },
        );
    }
}

fn add_file_symbols(
    candidates: &mut BTreeMap<(String, &'static str), CompletionCandidate>,
    file: &EditorFileCatalog,
) {
    if !file.exposure.is_public() {
        return;
    }
    for function in &file.functions {
        if function.receiver_type.is_some() || is_internal_function(function) {
            continue;
        }
        insert_candidate(
            candidates,
            CompletionCandidate {
                label: function.parsed.name.clone(),
                kind: "function",
                detail: format!("function in {} ({})", file.path, file.module_alias),
                signature: Some(function.signature.clone()),
            },
        );
    }
    for definition in &file.types {
        insert_candidate(
            candidates,
            CompletionCandidate {
                label: definition.name.clone(),
                kind: definition.kind,
                detail: definition.detail.clone(),
                signature: None,
            },
        );
    }
    for value in &file.values {
        insert_candidate(
            candidates,
            CompletionCandidate {
                label: value.name.clone(),
                kind: value.kind,
                detail: value.detail.clone(),
                signature: None,
            },
        );
    }
}

fn is_internal_function(function: &EditorFunction) -> bool {
    function
        .parsed
        .annotations
        .iter()
        .any(|annotation| annotation.name == "internal")
}

fn insert_candidate(
    candidates: &mut BTreeMap<(String, &'static str), CompletionCandidate>,
    candidate: CompletionCandidate,
) {
    let key = (candidate.label.clone(), candidate.kind);
    candidates.entry(key).or_insert(candidate);
}

fn add_current_bindings(
    candidates: &mut BTreeMap<(String, &'static str), CompletionCandidate>,
    draft: &EditorDraft,
    cursor: usize,
) {
    let Some(function) = draft.functions.iter().find(|function| {
        !function.body_range.is_empty()
            && function.body_range.start <= cursor
            && cursor <= function.body_range.end
    }) else {
        return;
    };
    for parameter in &function.params {
        insert_candidate(
            candidates,
            CompletionCandidate {
                label: parameter.name.clone(),
                kind: "parameter",
                detail: format!("parameter: {}", parameter.type_name),
                signature: None,
            },
        );
    }
    for local in draft.locals.iter().filter(|local| {
        local.function_name == function.name
            && local.visibility_range.start <= cursor
            && cursor <= local.visibility_range.end
    }) {
        let type_name = draft
            .typed_locals
            .iter()
            .find(|typed| typed.name_range == local.name_range)
            .map(|typed| typed.type_name.as_str());
        insert_candidate(
            candidates,
            CompletionCandidate {
                label: local.name.clone(),
                kind: "local",
                detail: type_name.map_or_else(
                    || "local variable".to_string(),
                    |type_name| format!("local: {type_name}"),
                ),
                signature: None,
            },
        );
    }
}

fn resolve_receiver_type(
    catalog: &EditorCatalog,
    active_path: &str,
    draft: &EditorDraft,
    parts: &[String],
    cursor: usize,
) -> Option<ResolvedTypeReceiver> {
    let (first, rest) = parts.split_first()?;
    let active_file = catalog.files.get(active_path)?;
    let local_type = draft
        .functions
        .iter()
        .find(|function| {
            !function.body_range.is_empty()
                && function.body_range.start <= cursor
                && cursor <= function.body_range.end
        })
        .and_then(|function| {
            function
                .params
                .iter()
                .find(|parameter| &parameter.name == first)
                .map(|parameter| parameter.type_name.clone())
                .or_else(|| {
                    draft
                        .typed_locals
                        .iter()
                        .find(|local| {
                            &local.name == first
                                && local.visibility_range.start <= cursor
                                && cursor <= local.visibility_range.end
                        })
                        .map(|local| local.type_name.clone())
                })
        });
    let value_type = local_type
        .or_else(|| {
            draft
                .layout
                .as_ref()?
                .globals
                .iter()
                .find(|global| &global.name == first)
                .map(|global| global.type_name.clone())
        })
        .or_else(|| {
            active_file
                .values
                .iter()
                .find(|value| &value.name == first)
                .and_then(|value| value.type_name.clone())
        });

    let (identity, remaining, is_type) = if let Some(type_name) = value_type {
        (
            resolve_type_reference(catalog, active_path, &type_name)?,
            rest,
            false,
        )
    } else if let Some(import) = active_file
        .imports
        .iter()
        .find(|import| &import.alias == first)
    {
        if rest.is_empty() {
            return None;
        }
        let module = catalog.files.get(&import.target)?;
        let member_name = &rest[0];
        if let Some(value) = module
            .values
            .iter()
            .find(|value| &value.name == member_name)
        {
            let type_name = value.type_name.as_deref()?;
            (
                resolve_type_reference(catalog, &import.target, type_name)?,
                &rest[1..],
                false,
            )
        } else {
            (
                resolve_type_reference(
                    catalog,
                    active_path,
                    &format!("{}.{}", import.alias, member_name),
                )?,
                &rest[1..],
                true,
            )
        }
    } else {
        (
            resolve_type_reference(catalog, active_path, first)?,
            rest,
            true,
        )
    };

    let mut identity = identity;
    for member_name in remaining {
        let definition = find_type_identity(catalog, &identity)?;
        let member = definition
            .members
            .iter()
            .find(|member| member.name == *member_name)?;
        identity = resolve_type_reference(catalog, &definition.path, member.type_name.as_deref()?)?;
    }
    Some(ResolvedTypeReceiver { identity, is_type })
}

fn resolve_type_reference(
    catalog: &EditorCatalog,
    context_path: &str,
    type_name: &str,
) -> Option<TypeIdentity> {
    let base = base_type_name(type_name);
    let mut parts = base.split('.');
    let first = parts.next()?;
    let Some(second) = parts.next() else {
        let active_file = catalog.files.get(context_path)?;
        if let Some(definition) = active_file
            .types
            .iter()
            .find(|definition| definition.name == first)
        {
            return Some(TypeIdentity {
                path: definition.path.clone(),
                name: definition.name.clone(),
            });
        }
        let mut matches = catalog
            .files
            .values()
            .flat_map(|file| file.types.iter())
            .filter(|definition| definition.name == first);
        let definition = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        return Some(TypeIdentity {
            path: definition.path.clone(),
            name: definition.name.clone(),
        });
    };
    if parts.next().is_some() {
        return None;
    }
    let active_file = catalog.files.get(context_path)?;
    let import = active_file
        .imports
        .iter()
        .find(|import| import.alias == first)?;
    let definition = catalog
        .files
        .get(&import.target)?
        .types
        .iter()
        .find(|definition| definition.name == second)?;
    Some(TypeIdentity {
        path: definition.path.clone(),
        name: definition.name.clone(),
    })
}

fn find_type_identity<'a>(
    catalog: &'a EditorCatalog,
    identity: &TypeIdentity,
) -> Option<&'a EditorType> {
    catalog
        .files
        .get(&identity.path)?
        .types
        .iter()
        .find(|definition| definition.name == identity.name)
}

fn truncate_display(mut value: String) -> String {
    if value.chars().count() > MAX_DISPLAY_TEXT_CHARS {
        value = value.chars().take(MAX_DISPLAY_TEXT_CHARS).collect();
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serde_json::Value;

    fn file(path: &str, source: &str) -> EditorSourceFile {
        EditorSourceFile {
            path: path.to_string(),
            source: source.to_string(),
        }
    }

    fn request(
        path: &str,
        source: &str,
        files: Vec<EditorSourceFile>,
        cursor: Option<usize>,
    ) -> Vec<u8> {
        let mut value = json!({ "path": path, "source": source, "files": files });
        if let Some(cursor) = cursor {
            value["cursor"] = json!(cursor);
        }
        serde_json::to_vec(&value).expect("serialize editor request")
    }

    fn analysis(bytes: &[u8], last_good: Option<&EditorCatalog>) -> Value {
        let output = analyze_editor_json(bytes, last_good).expect("editor analysis");
        serde_json::from_slice(&output).expect("analysis JSON")
    }

    fn canonical_playground_library_files() -> Vec<EditorSourceFile> {
        vec![
            file(
                "vendor/stasis/stdlib/graphics.stasis",
                include_str!("../../../src/stdlib/graphics.stasis"),
            ),
            file(
                "vendor/stasis/stdlib/host_frame.stasis",
                include_str!("../../../src/stdlib/host_frame.stasis"),
            ),
            file(
                "vendor/stasis/stdlib/sdl_scancodes.stasis",
                include_str!("../../../src/stdlib/sdl_scancodes.stasis"),
            ),
            file(
                "vendor/stasis/stdlib/asset_tasks.stasis",
                include_str!("../../../src/stdlib/asset_tasks.stasis"),
            ),
            file(
                "vendor/stasis/stdlib/internal/gfx_cmd.stasis",
                include_str!("../../../src/stdlib/internal/gfx_cmd.stasis"),
            ),
        ]
    }

    #[test]
    fn lexical_tokens_include_comments_keywords_literals_and_utf16_ranges() {
        let source = "// λ comment\nfunction cafe(): i32 { return 12; }\n\"hé\"";
        let result = analysis(&request("src/main.stasis", source, Vec::new(), None), None);
        let tokens = result["tokens"].as_array().expect("tokens");
        let comment = tokens
            .iter()
            .find(|token| token["kind"] == "comment")
            .expect("comment token");
        assert_eq!(
            &source[utf16_to_byte_for_test(source, comment["start"].as_u64().unwrap() as usize)
                ..utf16_to_byte_for_test(source, comment["end"].as_u64().unwrap() as usize)],
            "// λ comment"
        );
        assert!(tokens.iter().any(|token| token["kind"] == "keyword"));
        assert!(tokens.iter().any(|token| token["kind"] == "number"));
        assert!(tokens.iter().any(|token| token["kind"] == "string"));
        let name_start = utf16_offset_at(source, source.find("cafe").unwrap()).unwrap();
        let name = tokens
            .iter()
            .find(|token| token["kind"] == "function" && token["start"] == name_start)
            .expect("UTF-8 identifier span follows UTF-16 comment span");
        assert_eq!(
            name["end"].as_u64().unwrap() - name["start"].as_u64().unwrap(),
            4
        );
    }

    #[test]
    fn foreach_and_continue_are_highlighted_and_suggested_from_one_keyword_list() {
        let source = "function main(): i32 { continue; foreach (let item in items) { return 0; } }";
        let result = analysis(
            &request(
                "src/main.stasis",
                source,
                Vec::new(),
                Some(source.encode_utf16().count()),
            ),
            None,
        );
        let tokens = result["tokens"].as_array().expect("tokens");
        for keyword in ["continue", "in"] {
            let byte_start = if keyword == "in" {
                source.find(" in ").unwrap() + 1
            } else {
                source.find(keyword).unwrap()
            };
            let start = utf16_offset_at(source, byte_start).unwrap();
            assert!(
                tokens
                    .iter()
                    .any(|token| { token["kind"] == "keyword" && token["start"] == start }),
                "{keyword} should be highlighted as a keyword"
            );
            assert!(
                result["completions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["label"] == keyword),
                "{keyword} should be suggested"
            );
        }
    }

    fn utf16_to_byte_for_test(source: &str, target: usize) -> usize {
        byte_offset_from_utf16(source, target).expect("valid UTF-16 span")
    }

    #[test]
    fn incomplete_draft_keeps_lexical_prefix_and_styles_unterminated_tail() {
        let source = "function main(): i32 { return 1; /* unfinished";
        let result = analysis(&request("src/main.stasis", source, Vec::new(), None), None);
        assert!(result["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .any(|token| { token["kind"] == "keyword" && token["start"] == 0 }));
        let comment = result["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .find(|token| token["kind"] == "comment")
            .expect("unterminated comment tail");
        assert_eq!(comment["end"], source.encode_utf16().count());
    }

    #[test]
    fn incomplete_receiver_and_namespace_queries_return_fields_methods_and_signatures() {
        let library = file(
            "src/lib.stasis",
            "struct Player { hp: i32; }\nfunction helper(value: i32): i32 { return value; }\nfunction heal(self: Player, amount: i32): void { return; }\n",
        );
        let source = "import \"lib.stasis\";\nfunction main(): i32 { let player: Player; player. }";
        let cursor = source.find("player.").unwrap() + "player.".len();
        let result = analysis(
            &request(
                "src/main.stasis",
                source,
                vec![library.clone()],
                Some(source[..cursor].encode_utf16().count()),
            ),
            None,
        );
        let suggestions = result["completions"].as_array().unwrap();
        assert!(suggestions.iter().any(|item| item["label"] == "hp"));
        assert!(suggestions.iter().any(|item| item["label"] == "heal"));
        assert!(suggestions.iter().any(|item| item["signature"]
            .as_str()
            .is_some_and(|signature| signature.contains("amount: i32"))));

        let source = "import \"lib.stasis\";\nfunction main(): i32 { return lib.he }";
        let cursor = source.find("lib.he").unwrap() + "lib.he".len();
        let result = analysis(
            &request(
                "src/main.stasis",
                source,
                vec![library],
                Some(source[..cursor].encode_utf16().count()),
            ),
            None,
        );
        let helper = result["completions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["label"] == "helper")
            .expect("namespaced function suggestion");
        assert_eq!(helper["signature"], "helper(value: i32): i32");
        assert_eq!(
            result["replacementRange"]["end"],
            source[..cursor].encode_utf16().count()
        );
    }

    #[test]
    fn prototype_parameters_are_not_current_bindings_after_declaration() {
        let source = "function proto(value: i32): void;";
        let result = analysis(
            &request("src/main.stasis", source, Vec::new(), Some(source.len())),
            None,
        );
        assert!(!result["completions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["label"] == "value" && item["kind"] == "parameter" }));
    }

    #[test]
    fn canonical_playground_hints_use_stdlib_signatures_and_hide_internal_functions() {
        let mut files = canonical_playground_library_files();
        files.push(file(
            "vendor/stasis/stdlib/internal/editor_visibility_test.stasis",
            "struct EditorProbe { field: i32; }\nconst EDITOR_PROBE_VALUE: i32 = 1;\nfunction editor_probe(): i32 { return 1; }\n",
        ));

        let source = "function main(): i32 { return cle; }";
        let cursor = source.find("cle").unwrap() + "cle".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        let clear = result["completions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["label"] == "clear")
            .expect("canonical graphics clear suggestion");
        assert_eq!(
            clear["signature"],
            "clear(r: f32, g: f32, b: f32, a: f32): void"
        );

        let source = "function main(): i32 { return fill_; }";
        let cursor = source.find("fill_").unwrap() + "fill_".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        let fill_rect = result["completions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["label"] == "fill_rect")
            .expect("canonical graphics rectangle suggestion");
        assert_eq!(
            fill_rect["signature"],
            "fill_rect(x: f32, y: f32, w: f32, h: f32, r: f32, g: f32, b: f32, a: f32): void"
        );

        let source = "global sample_sprite: Sprite; function main(): i32 { sample_sprite.load_; }";
        let cursor = source.find("sample_sprite.load_").unwrap() + "sample_sprite.load_".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        let load_sprite = result["completions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["label"] == "load_sprite_from")
            .expect("canonical sprite asset method suggestion");
        assert_eq!(
            load_sprite["signature"],
            "load_sprite_from(path: string, width: i32, height: i32): bool"
        );

        let source = "global sheet: SpriteSheet; function main(): i32 { sheet. }";
        let cursor = source.find("sheet.").unwrap() + "sheet.".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        let suggestions = result["completions"].as_array().unwrap();
        let load_sheet = suggestions
            .iter()
            .find(|item| item["label"] == "load_sprite_sheet_from")
            .expect("canonical sprite sheet method suggestion");
        assert_eq!(
            load_sheet["signature"],
            "load_sprite_sheet_from(path: string, columns: i32, rows: i32, cell_width: i32, cell_height: i32): bool"
        );
        assert!(!suggestions
            .iter()
            .any(|item| item["label"] == "load_sprite_sheet_asset_from"));

        let source = "function main(): i32 { return gfx_cmd_; }";
        let cursor = source.find("gfx_cmd_").unwrap() + "gfx_cmd_".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        let suggestions = result["completions"].as_array().unwrap();
        assert!(!suggestions
            .iter()
            .any(|item| item["label"] == "gfx_cmd_clear"));
        assert!(!suggestions
            .iter()
            .any(|item| item["label"] == "gfx_cmd_rect"));
        assert!(!suggestions
            .iter()
            .any(|item| item["label"] == "gfx_cmd_submit"));
        assert!(!suggestions
            .iter()
            .any(|item| item["label"] == "gfx_cmd_i32"));

        let source = "function main(): i32 { return GFX_CMD_; }";
        let cursor = source.find("GFX_CMD_").unwrap() + "GFX_CMD_".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        assert!(!result["completions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "GFX_CMD_MAGIC"));

        let source = "function main(): i32 { return editor_; }";
        let cursor = source.find("editor_").unwrap() + "editor_".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        assert!(!result["completions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "editor_probe"));

        let source = "function main(): i32 { return EDITOR_; }";
        let cursor = source.find("EDITOR_").unwrap() + "EDITOR_".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        assert!(!result["completions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "EDITOR_PROBE_VALUE"));

        let source = "global private_state: EditorProbe; function main(): i32 { private_state. }";
        let cursor = source.find("private_state.").unwrap() + "private_state.".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        assert!(!result["completions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "field"));

        let source = "global frame: HostFrame; function main(): i32 { frame. }";
        let cursor = source.find("frame.").unwrap() + "frame.".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        let suggestions = result["completions"].as_array().unwrap();
        let refresh = suggestions
            .iter()
            .find(|item| item["label"] == "refresh")
            .expect("HostFrame refresh method suggestion");
        assert_eq!(refresh["signature"], "refresh(): void");
        let keys = suggestions
            .iter()
            .find(|item| item["label"] == "keys")
            .expect("HostFrame keyboard state field");
        assert_eq!(keys["detail"], "field: i32[512]");
        let pointers = suggestions
            .iter()
            .find(|item| item["label"] == "pointers")
            .expect("HostFrame pointer state field");
        assert_eq!(pointers["detail"], "field: HostPointerFrame[8]");
        assert!(!suggestions.iter().any(|item| item["label"] == "copy_keys"));

        let source = "global frame: HostFrame; function main(): i32 { frame.pointers. }";
        let cursor = source.find("frame.pointers.").unwrap() + "frame.pointers.".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        let suggestions = result["completions"].as_array().unwrap();
        let is_down = suggestions
            .iter()
            .find(|item| item["label"] == "is_down")
            .expect("HostPointerFrame down state field");
        assert_eq!(is_down["detail"], "field: bool");
        let x_logical = suggestions
            .iter()
            .find(|item| item["label"] == "x_logical")
            .expect("HostPointerFrame logical x field");
        assert_eq!(x_logical["detail"], "field: f32");

        let source = "function main(): i32 { Scancode. }";
        let cursor = source.find("Scancode.").unwrap() + "Scancode.".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        let suggestions = result["completions"].as_array().unwrap();
        assert!(suggestions.iter().any(|item| item["label"] == "Left"));
        assert!(suggestions.iter().any(|item| item["label"] == "Right"));

        let source = "function main(): i32 { return gfx_cmd_construction_; }";
        let cursor = source.find("gfx_cmd_construction_").unwrap() + "gfx_cmd_construction_".len();
        let result = analysis(
            &request("main.stasis", source, files.clone(), Some(cursor)),
            None,
        );
        assert!(!result["completions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "gfx_cmd_construction_reset"));
    }

    #[test]
    fn qualified_types_and_methods_keep_their_defining_module_identity() {
        let other = file(
            "src/a.stasis",
            "struct Player { score: i32; }\nfunction heal(self: Player, stamina: i32): i32 { return stamina; }",
        );
        let library = file(
            "src/lib.stasis",
            "struct Player { health: i32; }\nenum Mode { Idle = 0, Running = 1, }\nfunction heal(self: Player, amount: i32): void { return; }",
        );
        let source = "import \"lib.stasis\"; import \"a.stasis\"; function main(): i32 { let player: lib.Player; player. }";
        let cursor = source.find("player.").unwrap() + "player.".len();
        let result = analysis(
            &request(
                "src/main.stasis",
                source,
                vec![other, library],
                Some(source[..cursor].encode_utf16().count()),
            ),
            None,
        );
        let suggestions = result["completions"].as_array().unwrap();
        assert!(suggestions.iter().any(|item| item["label"] == "health"));
        assert!(!suggestions.iter().any(|item| item["label"] == "score"));
        let heal = suggestions
            .iter()
            .find(|item| item["label"] == "heal")
            .expect("method for the qualified type");
        assert_eq!(heal["signature"], "heal(amount: i32): void");

        let source = "enum Mode { Local = 0, } function main(): i32 { Mode. }";
        let cursor = source.find("Mode.").unwrap() + "Mode.".len();
        let result = analysis(
            &request(
                "src/main.stasis",
                source,
                vec![file("src/lib.stasis", library_source())],
                Some(source[..cursor].encode_utf16().count()),
            ),
            None,
        );
        assert!(result["completions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "Local"));

        let source = "import \"lib.stasis\"; function main(): i32 { lib.Mode. }";
        let cursor = source.find("lib.Mode.").unwrap() + "lib.Mode.".len();
        let result = analysis(
            &request(
                "src/main.stasis",
                source,
                vec![file("src/lib.stasis", library_source())],
                Some(source[..cursor].encode_utf16().count()),
            ),
            None,
        );
        for variant in ["Idle", "Running"] {
            assert!(
                result["completions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["label"] == variant),
                "missing {variant}"
            );
        }
    }

    fn library_source() -> &'static str {
        "struct Player { health: i32; }\nenum Mode { Idle = 0, Running = 1, }\nfunction heal(self: Player, amount: i32): void { return; }"
    }

    #[test]
    fn removed_project_file_is_not_kept_by_last_good_catalog() {
        let prior_files = vec![
            file("src/main.stasis", "function main(): i32 { return hel; }"),
            file("src/helper.stasis", "function helper(): i32 { return 1; }"),
        ];
        let last_good = EditorCatalog::from_sources(&prior_files);
        let source = "function main(): i32 { return hel; }";
        let cursor = source.find("hel").unwrap() + 3;
        let result = analysis(
            &request(
                "src/main.stasis",
                source,
                Vec::new(),
                Some(source[..cursor].encode_utf16().count()),
            ),
            Some(&last_good),
        );
        assert!(!result["completions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "helper"));
    }

    #[test]
    fn active_source_overrides_a_duplicate_file_and_utf16_cursor_is_checked() {
        let source = "function main(): i32 { return 1; }";
        let result = analysis(
            &request(
                "src/main.stasis",
                source,
                vec![file("src/main.stasis", "function old(): i32 { return 0; }")],
                None,
            ),
            None,
        );
        assert!(result["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .any(|token| { token["kind"] == "function" }));
        let invalid = request("src/main.stasis", source, Vec::new(), Some(10_000));
        assert!(analyze_editor_json(&invalid, None)
            .unwrap_err()
            .contains("past the end"));
    }

    #[test]
    fn editor_request_errors_are_bounded_and_deterministic() {
        let request = json!({
            "path": "../bad.stasis",
            "source": "",
            "files": []
        });
        let error = analyze_editor_json(&serde_json::to_vec(&request).unwrap(), None).unwrap_err();
        assert!(error.contains("must not contain"));
        assert!(
            analyze_editor_json(&vec![b'x'; MAX_EDITOR_JSON_BYTES + 1], None)
                .unwrap_err()
                .contains("bytes")
        );
    }
}
