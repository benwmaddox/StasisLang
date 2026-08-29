use std::ops::Range;

use lsp_types::{
    CompletionItemKind, CompletionTextEdit, Documentation, InsertTextFormat, MarkupContent,
    MarkupKind, ParameterInformation, ParameterLabel, SelectionRange, SemanticTokenModifier,
    SemanticTokenType, SemanticTokensLegend, SymbolKind, TextEdit,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stasis_language_service::{
    Document, HoverInfo, LanguageCompletionItem, LanguageDiagnosticOrigin, LanguageHierarchyKind,
    LanguageSymbolKind, Position, SignatureHelp as SharedSignatureHelp,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CompletionResolvePayload {
    pub(super) path: String,
    pub(super) revision: u64,
    pub(super) catalog_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HierarchyPayload {
    pub(super) symbol_id: String,
}

pub(super) fn lsp_completion_item(
    item: &LanguageCompletionItem,
    range: lsp_types::Range,
    rank: usize,
    path: &str,
    document: &Document,
) -> Result<lsp_types::CompletionItem, String> {
    let additional_text_edits =
        lsp_completion_text_edits(&item.additional_text_edits, path, document)?;
    let data = item
        .resolve_data
        .map(|data| {
            serde_json::to_value(CompletionResolvePayload {
                path: path.to_string(),
                revision: data.revision.get(),
                catalog_index: data.catalog_index,
            })
            .map_err(|error| format!("failed serializing completion resolve data: {error}"))
        })
        .transpose()?;
    Ok(lsp_types::CompletionItem {
        label: item.text.clone(),
        kind: Some(completion_kind(&item.kind)),
        detail: Some(item.detail.clone()),
        documentation: item.documentation.clone().map(|value| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            })
        }),
        sort_text: Some(format!("{rank:06}")),
        filter_text: Some(item.text.clone()),
        insert_text_format: item.snippet.then_some(InsertTextFormat::SNIPPET),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: item.insert_text.clone(),
        })),
        additional_text_edits: (!additional_text_edits.is_empty()).then_some(additional_text_edits),
        data,
        ..lsp_types::CompletionItem::default()
    })
}

pub(super) fn lsp_completion_text_edits(
    edits: &[stasis_language_service::CompletionTextChange],
    path: &str,
    document: &Document,
) -> Result<Vec<TextEdit>, String> {
    edits
        .iter()
        .map(|edit| {
            if edit.path != path {
                return Err(format!(
                    "completion edit targets '{}' instead of request document '{path}'",
                    edit.path
                ));
            }
            let start = document
                .position(edit.range.start)
                .map_err(|error| error.to_string())?;
            let end = document
                .position(edit.range.end)
                .map_err(|error| error.to_string())?;
            Ok(TextEdit {
                range: lsp_types::Range::new(lsp_position(start), lsp_position(end)),
                new_text: edit.text.clone(),
            })
        })
        .collect()
}

pub(super) fn completion_kind(kind: &str) -> CompletionItemKind {
    match kind {
        "function" | "test" => CompletionItemKind::FUNCTION,
        "method" => CompletionItemKind::METHOD,
        "struct" | "type" => CompletionItemKind::STRUCT,
        "enum" => CompletionItemKind::ENUM,
        "enum_variant" => CompletionItemKind::ENUM_MEMBER,
        "field" | "state_path" => CompletionItemKind::FIELD,
        "parameter" => CompletionItemKind::VARIABLE,
        "local" | "global" => CompletionItemKind::VARIABLE,
        "constant" => CompletionItemKind::CONSTANT,
        "keyword" | "command" => CompletionItemKind::KEYWORD,
        _ => CompletionItemKind::TEXT,
    }
}

pub(super) fn lsp_symbol_kind(kind: LanguageSymbolKind) -> SymbolKind {
    match kind {
        LanguageSymbolKind::Struct => SymbolKind::STRUCT,
        LanguageSymbolKind::Function => SymbolKind::FUNCTION,
        LanguageSymbolKind::Global => SymbolKind::VARIABLE,
        LanguageSymbolKind::Constant => SymbolKind::CONSTANT,
        LanguageSymbolKind::Test => SymbolKind::FUNCTION,
    }
}

pub(super) fn hierarchy_symbol_kind(kind: LanguageHierarchyKind) -> SymbolKind {
    match kind {
        LanguageHierarchyKind::Function => SymbolKind::FUNCTION,
        LanguageHierarchyKind::Struct => SymbolKind::STRUCT,
    }
}

pub(super) fn hierarchy_symbol_id(data: Option<&Value>) -> Result<String, String> {
    let data = data.ok_or_else(|| "hierarchy item has no Stasis identity".to_string())?;
    serde_json::from_value::<HierarchyPayload>(data.clone())
        .map(|payload| payload.symbol_id)
        .map_err(|error| format!("invalid hierarchy item identity: {error}"))
}

pub(super) fn hover_markdown(hover: &HoverInfo) -> String {
    let mut sections = Vec::new();
    if !hover.signatures.is_empty() {
        sections.push(format!("```stasis\n{}\n```", hover.signatures.join("\n")));
    } else if let Some(type_name) = &hover.type_name {
        sections.push(format!("```stasis\n{}: {type_name}\n```", hover.symbol));
    } else {
        sections.push(format!("```stasis\n{}\n```", hover.symbol));
    }
    let mut facts = vec![format!("**Kind:** {}", hover.kind)];
    if let Some(owner) = &hover.owner {
        facts.push(format!("**Owner:** `{owner}`"));
    }
    if let Some(type_name) = &hover.type_name {
        facts.push(format!("**Type:** `{type_name}`"));
    }
    if let Some(value) = &hover.live_value {
        facts.push(format!("**Live value:** `{value}`"));
    }
    sections.push(facts.join("  \n"));
    if let Some(documentation) = &hover.documentation {
        sections.push(documentation.clone());
    }
    sections.join("\n\n")
}

pub(super) fn lsp_signature_help(help: SharedSignatureHelp) -> lsp_types::SignatureHelp {
    lsp_types::SignatureHelp {
        signatures: help
            .signatures
            .into_iter()
            .map(|signature| lsp_types::SignatureInformation {
                label: signature.label,
                documentation: signature.documentation.map(Documentation::String),
                parameters: Some(
                    signature
                        .parameters
                        .into_iter()
                        .map(|parameter| ParameterInformation {
                            label: ParameterLabel::Simple(parameter.label),
                            documentation: parameter.documentation.map(Documentation::String),
                        })
                        .collect(),
                ),
                active_parameter: None,
            })
            .collect(),
        active_signature: u32::try_from(help.active_signature).ok(),
        active_parameter: u32::try_from(help.active_parameter).ok(),
    }
}

pub(super) fn lsp_position(position: Position) -> lsp_types::Position {
    lsp_types::Position::new(position.line, position.utf16_character)
}

pub(super) fn ranges_touch(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start <= left.end
        && right.start <= right.end
        && left.start <= right.end
        && right.start <= left.end
}

pub(super) fn diagnostic_origin_touches_request(
    origin: &LanguageDiagnosticOrigin,
    path: &str,
    requested: &Range<usize>,
) -> bool {
    origin.path == path && ranges_touch(&origin.range, requested)
}

pub(super) fn lsp_selection_range(
    document: &Document,
    ranges: Vec<Range<usize>>,
) -> Result<SelectionRange, String> {
    let mut parent = None;
    for range in ranges.into_iter().rev() {
        let start = document
            .position(range.start)
            .map_err(|error| error.to_string())?;
        let end = document
            .position(range.end)
            .map_err(|error| error.to_string())?;
        parent = Some(Box::new(SelectionRange {
            range: lsp_types::Range::new(lsp_position(start), lsp_position(end)),
            parent,
        }));
    }
    parent
        .map(|selection| *selection)
        .ok_or_else(|| "selection range chain is empty".to_string())
}

pub(super) fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::STRUCT,
            SemanticTokenType::ENUM,
            SemanticTokenType::ENUM_MEMBER,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::METHOD,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::PARAMETER,
            SemanticTokenType::TYPE,
        ],
        token_modifiers: vec![
            SemanticTokenModifier::READONLY,
            SemanticTokenModifier::STATIC,
        ],
    }
}

pub(super) fn semantic_token_style(kind: &str) -> (u32, u32) {
    match kind {
        "struct" => (0, 0),
        "enum" => (1, 0),
        "enum_variant" => (2, 1),
        "function" | "test" => (3, 0),
        "method" => (4, 0),
        "global" => (5, 1 << 1),
        "constant" => (5, (1 << 0) | (1 << 1)),
        "local" => (5, 0),
        "field" | "state_path" => (6, 0),
        "parameter" => (7, 0),
        _ => (8, 0),
    }
}
