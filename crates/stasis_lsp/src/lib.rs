#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, References, Request as _,
    SignatureHelpRequest, WorkspaceSymbolRequest,
};
use lsp_types::{
    CompletionItemKind, CompletionList, CompletionOptions, CompletionParams, CompletionResponse,
    CompletionTextEdit, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentSymbol, DocumentSymbolParams,
    DocumentSymbolResponse, Documentation, GotoDefinitionParams, GotoDefinitionResponse, Hover,
    HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InsertTextFormat, Location, MarkupContent, MarkupKind, OneOf, ParameterInformation,
    ParameterLabel, PositionEncodingKind, PublishDiagnosticsParams, ReferenceParams, SaveOptions,
    ServerCapabilities, ServerInfo, SignatureHelpOptions, SignatureHelpParams, SymbolInformation,
    SymbolKind, TextDocumentContentChangeEvent, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, TextEdit, Uri, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use stasis_language_service::{
    DiagnosticSeverity, Document, HoverInfo, LanguageCompletionItem, LanguageLocation,
    LanguageService, LanguageSymbol, LanguageSymbolKind, Position,
    SignatureHelp as SharedSignatureHelp, TextChange,
};
use url::Url;

pub fn run_stdio(project_root: &Path) -> Result<(), String> {
    let (connection, io_threads) = Connection::stdio();
    let result = run_connection(connection, project_root);
    io_threads
        .join()
        .map_err(|error| format!("LSP I/O thread failed: {error}"))?;
    result
}

pub fn run_connection(connection: Connection, project_root: &Path) -> Result<(), String> {
    let (initialize_id, initialize_params) = connection
        .initialize_start()
        .map_err(|error| format!("LSP initialization failed: {error}"))?;
    let initialize_params: InitializeParams = serde_json::from_value(initialize_params)
        .map_err(|error| format!("invalid LSP initialize parameters: {error}"))?;
    let completion_limit = initialize_params
        .initialization_options
        .as_ref()
        .and_then(|options| options.get("completionLimit"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(64)
        .clamp(1, 256);
    let initialize_result = InitializeResult {
        capabilities: ServerCapabilities {
            position_encoding: Some(PositionEncodingKind::UTF16),
            text_document_sync: Some(TextDocumentSyncCapability::Options(
                TextDocumentSyncOptions {
                    open_close: Some(true),
                    change: Some(TextDocumentSyncKind::INCREMENTAL),
                    will_save: Some(false),
                    will_save_wait_until: Some(false),
                    save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                        include_text: Some(true),
                    })),
                },
            )),
            completion_provider: Some(CompletionOptions {
                trigger_characters: Some(vec![".".to_string()]),
                resolve_provider: Some(false),
                ..CompletionOptions::default()
            }),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            signature_help_provider: Some(SignatureHelpOptions {
                trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                retrigger_characters: Some(vec![",".to_string()]),
                work_done_progress_options: Default::default(),
            }),
            definition_provider: Some(OneOf::Left(true)),
            references_provider: Some(OneOf::Left(true)),
            document_symbol_provider: Some(OneOf::Left(true)),
            workspace_symbol_provider: Some(OneOf::Left(true)),
            ..ServerCapabilities::default()
        },
        server_info: Some(ServerInfo {
            name: "stasis".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    };
    connection
        .initialize_finish(
            initialize_id,
            serde_json::to_value(initialize_result)
                .map_err(|error| format!("failed serializing LSP capabilities: {error}"))?,
        )
        .map_err(|error| format!("LSP initialization failed: {error}"))?;

    let mut server = LanguageServer::new(project_root, completion_limit)?;
    server.publish_diagnostics(&connection)?;
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection
                    .handle_shutdown(&request)
                    .map_err(|error| format!("LSP shutdown failed: {error}"))?
                {
                    break;
                }
                server.handle_request(&connection, request)?;
            }
            Message::Notification(notification) => {
                server.handle_notification(&connection, notification)?;
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

struct LanguageServer {
    service: LanguageService,
    uri_by_path: BTreeMap<String, Uri>,
    published_paths: BTreeSet<String>,
    completion_limit: usize,
}

impl LanguageServer {
    fn new(project_root: &Path, completion_limit: usize) -> Result<Self, String> {
        let project_root = absolute_path(project_root)?;
        let mut service = LanguageService::new(path_text(&project_root))?;
        let mut uri_by_path = BTreeMap::new();
        for path in stasis_files(&project_root)? {
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("failed reading {}: {error}", path.display()))?;
            let path = absolute_path(&path)?;
            let key = path_text(&path);
            uri_by_path.insert(key.clone(), path_uri(&path)?);
            service.set_disk_document(key, text);
        }
        Ok(Self {
            service,
            uri_by_path,
            published_paths: BTreeSet::new(),
            completion_limit,
        })
    }

    fn handle_request(&mut self, connection: &Connection, request: Request) -> Result<(), String> {
        let response = match request.method.as_str() {
            Completion::METHOD => {
                let (id, params): (_, CompletionParams) = request
                    .extract(Completion::METHOD)
                    .map_err(|error| format!("invalid completion request: {error}"))?;
                match self.completion(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            HoverRequest::METHOD => {
                let (id, params): (_, HoverParams) = request
                    .extract(HoverRequest::METHOD)
                    .map_err(|error| format!("invalid hover request: {error}"))?;
                match self.hover(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            SignatureHelpRequest::METHOD => {
                let (id, params): (_, SignatureHelpParams) = request
                    .extract(SignatureHelpRequest::METHOD)
                    .map_err(|error| format!("invalid signature-help request: {error}"))?;
                match self.signature_help(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            GotoDefinition::METHOD => {
                let (id, params): (_, GotoDefinitionParams) = request
                    .extract(GotoDefinition::METHOD)
                    .map_err(|error| format!("invalid definition request: {error}"))?;
                match self.definition(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            References::METHOD => {
                let (id, params): (_, ReferenceParams) = request
                    .extract(References::METHOD)
                    .map_err(|error| format!("invalid references request: {error}"))?;
                match self.references(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            DocumentSymbolRequest::METHOD => {
                let (id, params): (_, DocumentSymbolParams) = request
                    .extract(DocumentSymbolRequest::METHOD)
                    .map_err(|error| format!("invalid document-symbol request: {error}"))?;
                match self.document_symbols(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            WorkspaceSymbolRequest::METHOD => {
                let (id, params): (_, WorkspaceSymbolParams) = request
                    .extract(WorkspaceSymbolRequest::METHOD)
                    .map_err(|error| format!("invalid workspace-symbol request: {error}"))?;
                match self.workspace_symbols(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            _ => Response::new_err(
                request.id,
                lsp_server::ErrorCode::MethodNotFound as i32,
                format!("unsupported LSP request '{}'", request.method),
            ),
        };
        connection
            .sender
            .send(Message::Response(response))
            .map_err(|error| format!("failed sending LSP response: {error}"))
    }

    fn handle_notification(
        &mut self,
        connection: &Connection,
        notification: Notification,
    ) -> Result<(), String> {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let params: DidOpenTextDocumentParams = notification
                    .extract(DidOpenTextDocument::METHOD)
                    .map_err(|error| format!("invalid didOpen notification: {error}"))?;
                let path = uri_path(&params.text_document.uri)?;
                let key = path_text(&path);
                self.uri_by_path
                    .insert(key.clone(), params.text_document.uri);
                self.service.open_document(
                    key,
                    params.text_document.version as i64,
                    params.text_document.text,
                );
                self.publish_diagnostics(connection)
            }
            DidChangeTextDocument::METHOD => {
                let params: DidChangeTextDocumentParams = notification
                    .extract(DidChangeTextDocument::METHOD)
                    .map_err(|error| format!("invalid didChange notification: {error}"))?;
                let path = uri_path(&params.text_document.uri)?;
                let key = path_text(&path);
                let changes = self.text_changes(&key, params.content_changes)?;
                self.service
                    .change_document(&key, params.text_document.version as i64, &changes)
                    .map_err(|error| error.to_string())?;
                self.publish_diagnostics(connection)
            }
            DidCloseTextDocument::METHOD => {
                let params: DidCloseTextDocumentParams = notification
                    .extract(DidCloseTextDocument::METHOD)
                    .map_err(|error| format!("invalid didClose notification: {error}"))?;
                let path = uri_path(&params.text_document.uri)?;
                self.service.close_document(&path_text(&path));
                self.publish_diagnostics(connection)
            }
            DidSaveTextDocument::METHOD => {
                let params: DidSaveTextDocumentParams = notification
                    .extract(DidSaveTextDocument::METHOD)
                    .map_err(|error| format!("invalid didSave notification: {error}"))?;
                let path = uri_path(&params.text_document.uri)?;
                let key = path_text(&path);
                let text = params.text.or_else(|| {
                    self.service
                        .snapshot()
                        .document(&key)
                        .map(|document| document.text.to_string())
                });
                if let Some(text) = text {
                    self.service.set_disk_document(key, text);
                }
                self.publish_diagnostics(connection)
            }
            _ => Ok(()),
        }
    }

    fn completion(
        &mut self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>, String> {
        let (path, document, byte_offset) =
            self.document_position(params.text_document_position)?;
        let completion = self
            .service
            .completion(&path, byte_offset, self.completion_limit)?;
        let start = document
            .position(completion.replacement_start)
            .map_err(|error| error.to_string())?;
        let end = document
            .position(completion.replacement_end)
            .map_err(|error| error.to_string())?;
        let range = lsp_types::Range::new(lsp_position(start), lsp_position(end));
        let items = completion
            .items
            .iter()
            .enumerate()
            .map(|(rank, item)| lsp_completion_item(item, range, rank, &path, &document))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(CompletionResponse::List(CompletionList {
            is_incomplete: completion.truncated,
            items,
        })))
    }

    fn hover(&mut self, params: HoverParams) -> Result<Option<Hover>, String> {
        let (path, document, byte_offset) =
            self.document_position(params.text_document_position_params)?;
        let Some(hover) = self.service.hover(&path, byte_offset)? else {
            return Ok(None);
        };
        let start = document
            .position(hover.range.start)
            .map_err(|error| error.to_string())?;
        let end = document
            .position(hover.range.end)
            .map_err(|error| error.to_string())?;
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover_markdown(&hover),
            }),
            range: Some(lsp_types::Range::new(
                lsp_position(start),
                lsp_position(end),
            )),
        }))
    }

    fn signature_help(
        &mut self,
        params: SignatureHelpParams,
    ) -> Result<Option<lsp_types::SignatureHelp>, String> {
        let (path, _, byte_offset) =
            self.document_position(params.text_document_position_params)?;
        Ok(self
            .service
            .signature_help(&path, byte_offset)?
            .map(lsp_signature_help))
    }

    fn definition(
        &mut self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>, String> {
        let (path, _, byte_offset) =
            self.document_position(params.text_document_position_params)?;
        let locations = self
            .service
            .definition(&path, byte_offset)?
            .into_iter()
            .map(|location| self.lsp_location(location))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations)))
    }

    fn references(&mut self, params: ReferenceParams) -> Result<Option<Vec<Location>>, String> {
        let (path, _, byte_offset) = self.document_position(params.text_document_position)?;
        let locations = self
            .service
            .references(&path, byte_offset, params.context.include_declaration)?
            .into_iter()
            .map(|location| self.lsp_location(location))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((!locations.is_empty()).then_some(locations))
    }

    fn document_symbols(
        &mut self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>, String> {
        let path = path_text(&uri_path(&params.text_document.uri)?);
        let symbols = self
            .service
            .document_symbols(&path)?
            .into_iter()
            .map(|symbol| self.lsp_document_symbol(symbol))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((!symbols.is_empty()).then_some(DocumentSymbolResponse::Nested(symbols)))
    }

    #[allow(deprecated)]
    fn workspace_symbols(
        &mut self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>, String> {
        let symbols = self
            .service
            .workspace_symbols(&params.query, 256)?
            .into_iter()
            .map(|symbol| {
                let location = self.lsp_location(symbol.location.clone())?;
                Ok(SymbolInformation {
                    name: symbol.name,
                    kind: lsp_symbol_kind(symbol.kind),
                    tags: None,
                    deprecated: None,
                    location,
                    container_name: symbol.container_name,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok((!symbols.is_empty()).then_some(WorkspaceSymbolResponse::Flat(symbols)))
    }

    fn lsp_location(&self, location: LanguageLocation) -> Result<Location, String> {
        let document = self
            .service
            .snapshot()
            .document(&location.path)
            .cloned()
            .ok_or_else(|| format!("navigation target is not indexed: '{}'", location.path))?;
        let start = document
            .position(location.range.start)
            .map_err(|error| error.to_string())?;
        let end = document
            .position(location.range.end)
            .map_err(|error| error.to_string())?;
        let uri = self
            .uri_by_path
            .get(&location.path)
            .cloned()
            .unwrap_or(path_uri(Path::new(&location.path))?);
        Ok(Location::new(
            uri,
            lsp_types::Range::new(lsp_position(start), lsp_position(end)),
        ))
    }

    #[allow(deprecated)]
    fn lsp_document_symbol(&self, symbol: LanguageSymbol) -> Result<DocumentSymbol, String> {
        let location = self.lsp_location(symbol.location)?;
        Ok(DocumentSymbol {
            name: symbol.name,
            detail: Some(symbol.detail),
            kind: lsp_symbol_kind(symbol.kind),
            tags: None,
            deprecated: None,
            range: location.range,
            selection_range: location.range,
            children: None,
        })
    }

    fn document_position(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<(String, Document, usize), String> {
        let path = uri_path(&params.text_document.uri)?;
        let key = path_text(&path);
        let document = self
            .service
            .snapshot()
            .document(&key)
            .cloned()
            .ok_or_else(|| format!("LSP document is not indexed: '{key}'"))?;
        let byte_offset = document
            .byte_offset(Position {
                line: params.position.line,
                utf16_character: params.position.character,
            })
            .map_err(|error| error.to_string())?;
        Ok((key, document, byte_offset))
    }

    fn text_changes(
        &self,
        path: &str,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Result<Vec<TextChange>, String> {
        let mut document = self
            .service
            .snapshot()
            .document(path)
            .cloned()
            .ok_or_else(|| format!("changed LSP document '{path}' is not open"))?;
        let mut converted = Vec::with_capacity(changes.len());
        for change in changes {
            let converted_change = match change.range {
                Some(range) => {
                    let start = document
                        .byte_offset(Position {
                            line: range.start.line,
                            utf16_character: range.start.character,
                        })
                        .map_err(|error| error.to_string())?;
                    let end = document
                        .byte_offset(Position {
                            line: range.end.line,
                            utf16_character: range.end.character,
                        })
                        .map_err(|error| error.to_string())?;
                    TextChange::replace(start..end, change.text)
                }
                None => TextChange::replace_all(change.text),
            };
            document = overlay_after_change(&document, &converted_change)?;
            converted.push(converted_change);
        }
        Ok(converted)
    }

    fn publish_diagnostics(&mut self, connection: &Connection) -> Result<(), String> {
        let report = self.service.diagnostics();
        let snapshot = self.service.snapshot();
        let mut by_path = BTreeMap::<String, Vec<lsp_types::Diagnostic>>::new();
        for diagnostic in report.diagnostics {
            let Some(document) = snapshot.document(&diagnostic.path) else {
                continue;
            };
            let start = document
                .position(diagnostic.range.start)
                .map_err(|error| error.to_string())?;
            let end = document
                .position(diagnostic.range.end)
                .map_err(|error| error.to_string())?;
            by_path
                .entry(diagnostic.path)
                .or_default()
                .push(lsp_types::Diagnostic {
                    range: lsp_types::Range::new(lsp_position(start), lsp_position(end)),
                    severity: Some(match diagnostic.severity {
                        DiagnosticSeverity::Error => lsp_types::DiagnosticSeverity::ERROR,
                        DiagnosticSeverity::Warning => lsp_types::DiagnosticSeverity::WARNING,
                        DiagnosticSeverity::Information => {
                            lsp_types::DiagnosticSeverity::INFORMATION
                        }
                        DiagnosticSeverity::Hint => lsp_types::DiagnosticSeverity::HINT,
                    }),
                    code: None,
                    code_description: None,
                    source: Some(diagnostic.source.to_string()),
                    message: diagnostic.message,
                    related_information: None,
                    tags: None,
                    data: None,
                });
        }

        let current_paths = by_path.keys().cloned().collect::<BTreeSet<_>>();
        let paths = self
            .published_paths
            .union(&current_paths)
            .cloned()
            .collect::<Vec<_>>();
        for path in paths {
            let uri = self
                .uri_by_path
                .get(&path)
                .cloned()
                .or_else(|| path_uri(Path::new(&path)).ok())
                .ok_or_else(|| format!("cannot create URI for diagnostic path '{path}'"))?;
            let version = snapshot
                .document(&path)
                .and_then(|document| document.version)
                .and_then(|version| i32::try_from(version).ok());
            connection
                .sender
                .send(Message::Notification(Notification::new(
                    PublishDiagnostics::METHOD.to_string(),
                    PublishDiagnosticsParams {
                        uri,
                        diagnostics: by_path.remove(&path).unwrap_or_default(),
                        version,
                    },
                )))
                .map_err(|error| format!("failed publishing LSP diagnostics: {error}"))?;
        }
        self.published_paths = current_paths;
        Ok(())
    }
}

fn overlay_after_change(document: &Document, change: &TextChange) -> Result<Document, String> {
    let mut workspace = stasis_language_service::WorkspaceDocuments::default();
    workspace.open_document("overlay", 1, document.text.to_string());
    workspace
        .change_document("overlay", 2, std::slice::from_ref(change))
        .map_err(|error| error.to_string())?;
    workspace
        .snapshot()
        .document("overlay")
        .cloned()
        .ok_or_else(|| "incremental LSP overlay disappeared".to_string())
}

fn internal_error(id: lsp_server::RequestId, message: String) -> Response {
    Response::new_err(id, lsp_server::ErrorCode::InternalError as i32, message)
}

fn lsp_completion_item(
    item: &LanguageCompletionItem,
    range: lsp_types::Range,
    rank: usize,
    path: &str,
    document: &Document,
) -> Result<lsp_types::CompletionItem, String> {
    let additional_text_edits = item
        .additional_text_edits
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
        .collect::<Result<Vec<_>, String>>()?;
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
        ..lsp_types::CompletionItem::default()
    })
}

fn completion_kind(kind: &str) -> CompletionItemKind {
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

fn lsp_symbol_kind(kind: LanguageSymbolKind) -> SymbolKind {
    match kind {
        LanguageSymbolKind::Struct => SymbolKind::STRUCT,
        LanguageSymbolKind::Function => SymbolKind::FUNCTION,
        LanguageSymbolKind::Global => SymbolKind::VARIABLE,
        LanguageSymbolKind::Constant => SymbolKind::CONSTANT,
        LanguageSymbolKind::Test => SymbolKind::FUNCTION,
    }
}

fn hover_markdown(hover: &HoverInfo) -> String {
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

fn lsp_signature_help(help: SharedSignatureHelp) -> lsp_types::SignatureHelp {
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

fn lsp_position(position: Position) -> lsp_types::Position {
    lsp_types::Position::new(position.line, position.utf16_character)
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| format!("failed resolving {}: {error}", path.display()))?
    };
    Ok(fs::canonicalize(&absolute).unwrap_or(absolute))
}

fn path_text(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    path.strip_prefix("//?/").unwrap_or(&path).to_string()
}

fn path_uri(path: &Path) -> Result<Uri, String> {
    Url::from_file_path(path)
        .map_err(|_| format!("cannot convert {} to a file URI", path.display()))?
        .as_str()
        .parse()
        .map_err(|error| format!("invalid file URI for {}: {error}", path.display()))
}

fn uri_path(uri: &Uri) -> Result<PathBuf, String> {
    let path = Url::parse(uri.as_str())
        .map_err(|error| format!("invalid document URI '{}': {error}", uri.as_str()))?
        .to_file_path()
        .map_err(|_| format!("document URI '{}' is not a file URI", uri.as_str()))?;
    Ok(fs::canonicalize(&path).unwrap_or(path))
}

fn stasis_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("failed reading {}: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!("failed reading {} entry: {error}", directory.display())
            })?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| format!("failed reading {} type: {error}", path.display()))?;
            if file_type.is_dir() {
                let name = entry.file_name();
                if !matches!(
                    name.to_str(),
                    Some(".git" | ".worktrees" | "build" | "target")
                ) {
                    pending.push(path);
                }
            } else if file_type.is_file()
                && path.extension().and_then(|extension| extension.to_str()) == Some("stasis")
            {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_server::RequestId;
    use std::time::Duration;

    fn test_server(name: &str) -> (LanguageServer, Uri, String) {
        let root = std::env::temp_dir().join(format!("stasis-lsp-{name}"));
        let path = root.join("src/main.stasis");
        let uri = path_uri(&path).expect("file URI");
        let key = path_text(&path);
        let service = LanguageService::new(path_text(&root)).expect("language service");
        (
            LanguageServer {
                service,
                uri_by_path: BTreeMap::new(),
                published_paths: BTreeSet::new(),
                completion_limit: 64,
            },
            uri,
            key,
        )
    }

    fn receive_response(connection: &Connection, id: i32) -> Response {
        loop {
            let message = connection
                .receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("LSP response");
            if let Message::Response(response) = message {
                assert_eq!(response.id, RequestId::from(id));
                return response;
            }
        }
    }

    #[test]
    fn did_open_publishes_diagnostic_and_full_change_clears_it() {
        let (mut server, uri, _) = test_server("diagnostics");
        let (server_connection, client_connection) = Connection::memory();
        server
            .handle_notification(
                &server_connection,
                Notification::new(
                    DidOpenTextDocument::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": "stasis",
                            "version": 1,
                            "text": "function main(): i32 { return 0; }\nfunction broken(): i32 { while (true) { return 1; } }\n"
                        }
                    }),
                ),
            )
            .expect("didOpen");
        let opened = client_connection
            .receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("diagnostics");
        let Message::Notification(opened) = opened else {
            panic!("expected diagnostics notification");
        };
        assert_eq!(opened.method, PublishDiagnostics::METHOD);
        let opened: PublishDiagnosticsParams =
            serde_json::from_value(opened.params).expect("diagnostic parameters");
        assert_eq!(opened.version, Some(1));
        assert_eq!(opened.diagnostics.len(), 1);
        assert_eq!(
            opened.diagnostics[0].severity,
            Some(lsp_types::DiagnosticSeverity::ERROR)
        );
        assert!(opened.diagnostics[0].message.contains("while"));

        server
            .handle_notification(
                &server_connection,
                Notification::new(
                    DidChangeTextDocument::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri, "version": 2 },
                        "contentChanges": [{
                            "text": "function main(): i32 { return 0; }\nfunction fixed(): i32 { return 1; }\n"
                        }]
                    }),
                ),
            )
            .expect("didChange");
        let fixed = client_connection
            .receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("clear diagnostics");
        let Message::Notification(fixed) = fixed else {
            panic!("expected diagnostics notification");
        };
        let fixed: PublishDiagnosticsParams =
            serde_json::from_value(fixed.params).expect("diagnostic parameters");
        assert_eq!(fixed.version, Some(2));
        assert!(fixed.diagnostics.is_empty());
    }

    #[test]
    fn incremental_change_ranges_use_utf16_positions() {
        let (mut server, _, key) = test_server("utf16");
        server.service.open_document(&key, 1, "a\u{1f600}b");
        let changes = server
            .text_changes(
                &key,
                vec![TextDocumentContentChangeEvent {
                    range: Some(lsp_types::Range::new(
                        lsp_types::Position::new(0, 1),
                        lsp_types::Position::new(0, 3),
                    )),
                    range_length: Some(2),
                    text: "x".to_string(),
                }],
            )
            .expect("UTF-16 change");
        assert_eq!(changes, vec![TextChange::replace(1..5, "x")]);
    }

    #[test]
    fn saved_overlay_becomes_disk_state_before_close() {
        let (mut server, uri, key) = test_server("save");
        let (server_connection, _) = Connection::memory();
        let text = "function main(): i32 { return 7; }\n";
        server
            .handle_notification(
                &server_connection,
                Notification::new(
                    DidOpenTextDocument::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": "stasis",
                            "version": 1,
                            "text": text
                        }
                    }),
                ),
            )
            .expect("didOpen");
        server
            .handle_notification(
                &server_connection,
                Notification::new(
                    DidSaveTextDocument::METHOD.to_string(),
                    serde_json::json!({ "textDocument": { "uri": uri }, "text": text }),
                ),
            )
            .expect("didSave");
        server
            .handle_notification(
                &server_connection,
                Notification::new(
                    DidCloseTextDocument::METHOD.to_string(),
                    serde_json::json!({ "textDocument": { "uri": uri } }),
                ),
            )
            .expect("didClose");

        let snapshot = server.service.snapshot();
        let saved = snapshot.document(&key).expect("saved disk document");
        assert_eq!(saved.version, None);
        assert_eq!(&*saved.text, text);
    }

    #[test]
    fn standard_requests_return_completion_hover_and_signature_help() {
        let (mut server, uri, _) = test_server("intelligence");
        let (server_connection, client_connection) = Connection::memory();
        let source = "// Creates an enemy.\nfunction spawn_enemy(count: i32, health: i32): i32 { return health; }\nfunction main(): i32 { let health: i32 = 2; return spawn_enemy(1, health); }\n";
        server
            .handle_notification(
                &server_connection,
                Notification::new(
                    DidOpenTextDocument::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": "stasis",
                            "version": 1,
                            "text": source
                        }
                    }),
                ),
            )
            .expect("didOpen");

        let completion_offset = source.rfind("spawn_enemy(1").expect("function use") + 5;
        let completion_position = lsp_position(
            server
                .service
                .snapshot()
                .document(&path_text(&uri_path(&uri).expect("path")))
                .expect("document")
                .position(completion_offset)
                .expect("position"),
        );
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(10),
                    Completion::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": completion_position
                    }),
                ),
            )
            .expect("completion request");
        let response = receive_response(&client_connection, 10);
        let completion: Option<CompletionResponse> =
            serde_json::from_value(response.response_result.expect("completion result"))
                .expect("completion response");
        let CompletionResponse::List(completion) = completion.expect("completion list") else {
            panic!("expected completion list");
        };
        let function = completion
            .items
            .iter()
            .find(|item| item.label == "spawn_enemy")
            .expect("function completion");
        assert_eq!(function.insert_text_format, Some(InsertTextFormat::SNIPPET));
        let Some(CompletionTextEdit::Edit(edit)) = function.text_edit.as_ref() else {
            panic!("expected completion text edit");
        };
        assert_eq!(edit.new_text, "spawn_enemy(${1:count}, ${2:health})");
        assert!(matches!(
            function.documentation.as_ref(),
            Some(Documentation::MarkupContent(content)) if content.value.contains("Creates an enemy")
        ));

        let function_offset = source.rfind("spawn_enemy(1").expect("function use") + 2;
        let function_position = lsp_position(
            server
                .service
                .snapshot()
                .document(&path_text(&uri_path(&uri).expect("path")))
                .expect("document")
                .position(function_offset)
                .expect("position"),
        );
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(11),
                    HoverRequest::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": function_position
                    }),
                ),
            )
            .expect("hover request");
        let response = receive_response(&client_connection, 11);
        let hover: Option<Hover> =
            serde_json::from_value(response.response_result.expect("hover result"))
                .expect("hover response");
        let HoverContents::Markup(contents) = hover.expect("hover").contents else {
            panic!("expected markdown hover");
        };
        assert!(contents
            .value
            .contains("spawn_enemy(count: i32, health: i32): i32"));
        assert!(contents.value.contains("Creates an enemy."));

        let signature_offset =
            source.rfind("spawn_enemy(1, health").expect("call") + "spawn_enemy(1, ".len();
        let signature_position = lsp_position(
            server
                .service
                .snapshot()
                .document(&path_text(&uri_path(&uri).expect("path")))
                .expect("document")
                .position(signature_offset)
                .expect("position"),
        );
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(12),
                    SignatureHelpRequest::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": signature_position
                    }),
                ),
            )
            .expect("signature request");
        let response = receive_response(&client_connection, 12);
        let help: Option<lsp_types::SignatureHelp> =
            serde_json::from_value(response.response_result.expect("signature result"))
                .expect("signature response");
        let help = help.expect("signature help");
        assert_eq!(help.active_parameter, Some(1));
        assert_eq!(
            help.signatures[0].label,
            "spawn_enemy(count: i32, health: i32): i32"
        );

        for (id, method, params) in [
            (
                13,
                GotoDefinition::METHOD,
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": function_position
                }),
            ),
            (
                14,
                References::METHOD,
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": function_position,
                    "context": { "includeDeclaration": true }
                }),
            ),
            (
                15,
                DocumentSymbolRequest::METHOD,
                serde_json::json!({ "textDocument": { "uri": uri } }),
            ),
            (
                16,
                WorkspaceSymbolRequest::METHOD,
                serde_json::json!({ "query": "spawn" }),
            ),
        ] {
            server
                .handle_request(
                    &server_connection,
                    Request::new(RequestId::from(id), method.to_string(), params),
                )
                .expect("navigation request");
        }
        let definition: Option<GotoDefinitionResponse> = serde_json::from_value(
            receive_response(&client_connection, 13)
                .response_result
                .expect("definition result"),
        )
        .expect("definition response");
        let GotoDefinitionResponse::Array(definitions) = definition.expect("definition") else {
            panic!("expected definition locations");
        };
        assert_eq!(definitions.len(), 1);

        let references: Option<Vec<Location>> = serde_json::from_value(
            receive_response(&client_connection, 14)
                .response_result
                .expect("references result"),
        )
        .expect("references response");
        assert!(references.expect("references").len() >= 2);

        let document_symbols: Option<DocumentSymbolResponse> = serde_json::from_value(
            receive_response(&client_connection, 15)
                .response_result
                .expect("document symbols result"),
        )
        .expect("document-symbol response");
        let DocumentSymbolResponse::Nested(document_symbols) =
            document_symbols.expect("document symbols")
        else {
            panic!("expected nested document symbols");
        };
        assert!(document_symbols
            .iter()
            .any(|symbol| symbol.name == "spawn_enemy"));

        let workspace_symbols: Option<WorkspaceSymbolResponse> = serde_json::from_value(
            receive_response(&client_connection, 16)
                .response_result
                .expect("workspace symbols result"),
        )
        .expect("workspace-symbol response");
        let WorkspaceSymbolResponse::Flat(workspace_symbols) =
            workspace_symbols.expect("workspace symbols")
        else {
            panic!("expected flat workspace symbols");
        };
        assert_eq!(workspace_symbols.len(), 1);
        assert_eq!(workspace_symbols[0].name, "spawn_enemy");
    }

    #[cfg(windows)]
    #[test]
    fn vscode_encoded_windows_file_uri_maps_to_drive_path() {
        let uri: Uri = "file:///c%3A/Users/test/project/src/main.stasis"
            .parse()
            .expect("URI");
        assert_eq!(
            uri_path(&uri).expect("file path"),
            PathBuf::from(r"c:\Users\test\project\src\main.stasis")
        );
    }
}
