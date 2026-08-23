#![forbid(unsafe_code)]

mod live_process;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
    CodeActionRequest, Completion, DocumentSymbolRequest, FoldingRangeRequest, Formatting,
    GotoDefinition, HoverRequest, InlayHintRequest, LinkedEditingRange, OnTypeFormatting,
    PrepareRenameRequest, RangeFormatting, References, Rename, Request as _, ResolveCompletionItem,
    SelectionRangeRequest, SemanticTokensFullRequest, SignatureHelpRequest, TypeHierarchyPrepare,
    TypeHierarchySubtypes, TypeHierarchySupertypes, WorkspaceSymbolRequest,
};
use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
    CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    CallHierarchyServerCapability, CodeAction, CodeActionKind, CodeActionOptions,
    CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability, CompletionItemKind,
    CompletionList, CompletionOptions, CompletionParams, CompletionResponse, CompletionTextEdit,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentChanges, DocumentFormattingParams,
    DocumentOnTypeFormattingOptions, DocumentOnTypeFormattingParams, DocumentRangeFormattingParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, Documentation, FoldingRange,
    FoldingRangeParams, FoldingRangeProviderCapability, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InlayHint, InlayHintKind, InlayHintLabel, InlayHintOptions,
    InlayHintParams, InlayHintServerCapabilities, InsertTextFormat, LinkedEditingRangeParams,
    LinkedEditingRangeServerCapabilities, LinkedEditingRanges, Location, MarkupContent, MarkupKind,
    NumberOrString, OneOf, OptionalVersionedTextDocumentIdentifier, ParameterInformation,
    ParameterLabel, PositionEncodingKind, PrepareRenameResponse, PublishDiagnosticsParams,
    ReferenceParams, RenameOptions, RenameParams, SaveOptions, SelectionRange,
    SelectionRangeParams, SelectionRangeProviderCapability, SemanticToken, SemanticTokenModifier,
    SemanticTokenType, SemanticTokens, SemanticTokensFullOptions, SemanticTokensLegend,
    SemanticTokensOptions, SemanticTokensParams, SemanticTokensResult, ServerCapabilities,
    ServerInfo, SignatureHelpOptions, SignatureHelpParams, SymbolInformation, SymbolKind,
    TextDocumentContentChangeEvent, TextDocumentEdit, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, TextEdit, TypeHierarchyItem, TypeHierarchyPrepareParams,
    TypeHierarchySubtypesParams, TypeHierarchySupertypesParams, Uri, WorkspaceEdit,
    WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use stasis_language_service::{
    CompletionResolveData, DiagnosticSeverity, Document, HoverInfo, LanguageCompletionItem,
    LanguageDiagnosticOrigin, LanguageHierarchyItem, LanguageHierarchyKind, LanguageInlayHintKind,
    LanguageLocation, LanguageService, LanguageSymbol, LanguageSymbolKind, Position,
    SignatureHelp as SharedSignatureHelp, TextChange, WorkspaceRevision,
};
use url::Url;

use crate::live_process::{LiveCacheEvent, LiveCacheMailbox, LiveProcessBroker};

const LIVE_START_METHOD: &str = "stasis/live/start";
const LIVE_STOP_METHOD: &str = "stasis/live/stop";
const LIVE_REQUEST_METHOD: &str = "stasis/live/request";
const MAX_LIVE_REQUEST_WORKERS: usize = 64;

#[derive(Default, Deserialize)]
struct LiveStartParams {
    #[serde(default)]
    entry: Option<String>,
}

#[derive(Deserialize)]
struct LiveRequestParams {
    #[serde(flatten)]
    fields: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompletionResolvePayload {
    path: String,
    revision: u64,
    catalog_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HierarchyPayload {
    symbol_id: String,
}

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
                resolve_provider: Some(true),
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
            call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
            folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
            selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
            linked_editing_range_provider: Some(LinkedEditingRangeServerCapabilities::Simple(true)),
            document_formatting_provider: Some(OneOf::Left(true)),
            document_range_formatting_provider: Some(OneOf::Left(true)),
            document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
                first_trigger_character: ";".to_string(),
                more_trigger_character: Some(vec!["}".to_string()]),
            }),
            document_symbol_provider: Some(OneOf::Left(true)),
            workspace_symbol_provider: Some(OneOf::Left(true)),
            rename_provider: Some(OneOf::Right(RenameOptions {
                prepare_provider: Some(true),
                work_done_progress_options: Default::default(),
            })),
            code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
                code_action_kinds: Some(vec![
                    CodeActionKind::QUICKFIX,
                    CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                ]),
                resolve_provider: Some(false),
                work_done_progress_options: Default::default(),
            })),
            semantic_tokens_provider: Some(
                SemanticTokensOptions {
                    work_done_progress_options: Default::default(),
                    legend: semantic_tokens_legend(),
                    range: Some(false),
                    full: Some(SemanticTokensFullOptions::Bool(true)),
                }
                .into(),
            ),
            inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(
                InlayHintOptions {
                    work_done_progress_options: Default::default(),
                    resolve_provider: Some(false),
                },
            ))),
            ..ServerCapabilities::default()
        },
        server_info: Some(ServerInfo {
            name: "stasis".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    };
    let mut initialize_value = serde_json::to_value(initialize_result)
        .map_err(|error| format!("failed serializing LSP capabilities: {error}"))?;
    // LSP 3.17 defines this capability, but lsp-types 0.97 omits it from ServerCapabilities.
    initialize_value["capabilities"]["typeHierarchyProvider"] = Value::Bool(true);
    connection
        .initialize_finish(initialize_id, initialize_value)
        .map_err(|error| format!("LSP initialization failed: {error}"))?;

    let notification_sender = connection.sender.clone();
    let notify = Arc::new(move |method: &str, params: Value| {
        let _ = notification_sender.send(Message::Notification(Notification::new(
            method.to_string(),
            params,
        )));
    });
    let mut server = LanguageServer::new(project_root, completion_limit, notify)?;
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
    live_process: LiveProcessBroker,
    live_cache_event: LiveCacheMailbox,
    live_request_workers: Arc<AtomicUsize>,
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        self.live_process.shutdown();
    }
}

impl LanguageServer {
    fn new(
        project_root: &Path,
        completion_limit: usize,
        notify: Arc<dyn Fn(&str, Value) + Send + Sync>,
    ) -> Result<Self, String> {
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
        let _ = service.warm_navigation_cache();
        let (live_process, live_cache_event) = LiveProcessBroker::new(&project_root, notify)?;
        Ok(Self {
            service,
            uri_by_path,
            published_paths: BTreeSet::new(),
            completion_limit,
            live_process,
            live_cache_event,
            live_request_workers: Arc::new(AtomicUsize::new(0)),
        })
    }

    fn handle_request(&mut self, connection: &Connection, request: Request) -> Result<(), String> {
        self.drain_live_cache();
        if matches!(
            request.method.as_str(),
            LIVE_START_METHOD | LIVE_STOP_METHOD | LIVE_REQUEST_METHOD
        ) {
            return self.handle_live_request(connection, request);
        }
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
            ResolveCompletionItem::METHOD => {
                let (id, params): (_, lsp_types::CompletionItem) = request
                    .extract(ResolveCompletionItem::METHOD)
                    .map_err(|error| format!("invalid completion-resolve request: {error}"))?;
                match self.resolve_completion(params) {
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
            CallHierarchyPrepare::METHOD => {
                let (id, params): (_, CallHierarchyPrepareParams) = request
                    .extract(CallHierarchyPrepare::METHOD)
                    .map_err(|error| format!("invalid call-hierarchy prepare request: {error}"))?;
                match self.prepare_call_hierarchy(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            CallHierarchyIncomingCalls::METHOD => {
                let (id, params): (_, CallHierarchyIncomingCallsParams) = request
                    .extract(CallHierarchyIncomingCalls::METHOD)
                    .map_err(|error| format!("invalid incoming-calls request: {error}"))?;
                match self.incoming_calls(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            CallHierarchyOutgoingCalls::METHOD => {
                let (id, params): (_, CallHierarchyOutgoingCallsParams) = request
                    .extract(CallHierarchyOutgoingCalls::METHOD)
                    .map_err(|error| format!("invalid outgoing-calls request: {error}"))?;
                match self.outgoing_calls(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            TypeHierarchyPrepare::METHOD => {
                let (id, params): (_, TypeHierarchyPrepareParams) = request
                    .extract(TypeHierarchyPrepare::METHOD)
                    .map_err(|error| format!("invalid type-hierarchy prepare request: {error}"))?;
                match self.prepare_type_hierarchy(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            TypeHierarchySupertypes::METHOD => {
                let (id, params): (_, TypeHierarchySupertypesParams) = request
                    .extract(TypeHierarchySupertypes::METHOD)
                    .map_err(|error| format!("invalid type supertypes request: {error}"))?;
                match self.type_supertypes(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            TypeHierarchySubtypes::METHOD => {
                let (id, params): (_, TypeHierarchySubtypesParams) = request
                    .extract(TypeHierarchySubtypes::METHOD)
                    .map_err(|error| format!("invalid type subtypes request: {error}"))?;
                match self.type_subtypes(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            FoldingRangeRequest::METHOD => {
                let (id, params): (_, FoldingRangeParams) = request
                    .extract(FoldingRangeRequest::METHOD)
                    .map_err(|error| format!("invalid folding-range request: {error}"))?;
                match self.folding_ranges(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            SelectionRangeRequest::METHOD => {
                let (id, params): (_, SelectionRangeParams) = request
                    .extract(SelectionRangeRequest::METHOD)
                    .map_err(|error| format!("invalid selection-range request: {error}"))?;
                match self.selection_ranges(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            LinkedEditingRange::METHOD => {
                let (id, params): (_, LinkedEditingRangeParams) = request
                    .extract(LinkedEditingRange::METHOD)
                    .map_err(|error| format!("invalid linked-editing request: {error}"))?;
                match self.linked_edit_ranges(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            Formatting::METHOD => {
                let (id, params): (_, DocumentFormattingParams) = request
                    .extract(Formatting::METHOD)
                    .map_err(|error| format!("invalid document-formatting request: {error}"))?;
                match self.format_document(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            RangeFormatting::METHOD => {
                let (id, params): (_, DocumentRangeFormattingParams) = request
                    .extract(RangeFormatting::METHOD)
                    .map_err(|error| format!("invalid range-formatting request: {error}"))?;
                match self.format_range(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            OnTypeFormatting::METHOD => {
                let (id, params): (_, DocumentOnTypeFormattingParams) = request
                    .extract(OnTypeFormatting::METHOD)
                    .map_err(|error| format!("invalid on-type-formatting request: {error}"))?;
                match self.format_on_type(params) {
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
            PrepareRenameRequest::METHOD => {
                let (id, params): (_, TextDocumentPositionParams) = request
                    .extract(PrepareRenameRequest::METHOD)
                    .map_err(|error| format!("invalid prepare-rename request: {error}"))?;
                match self.prepare_rename(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            Rename::METHOD => {
                let (id, params): (_, RenameParams) = request
                    .extract(Rename::METHOD)
                    .map_err(|error| format!("invalid rename request: {error}"))?;
                match self.rename(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            CodeActionRequest::METHOD => {
                let (id, params): (_, CodeActionParams) = request
                    .extract(CodeActionRequest::METHOD)
                    .map_err(|error| format!("invalid code-action request: {error}"))?;
                match self.code_actions(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            SemanticTokensFullRequest::METHOD => {
                let (id, params): (_, SemanticTokensParams) = request
                    .extract(SemanticTokensFullRequest::METHOD)
                    .map_err(|error| format!("invalid semantic-tokens request: {error}"))?;
                match self.semantic_tokens(params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                }
            }
            InlayHintRequest::METHOD => {
                let (id, params): (_, InlayHintParams) = request
                    .extract(InlayHintRequest::METHOD)
                    .map_err(|error| format!("invalid inlay-hint request: {error}"))?;
                match self.inlay_hints(params) {
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

    fn handle_live_request(&self, connection: &Connection, request: Request) -> Result<(), String> {
        let sender = connection.sender.clone();
        let broker = self.live_process.clone();
        let (id, task): (_, Box<dyn FnOnce() -> Result<Value, String> + Send>) =
            match request.method.as_str() {
                LIVE_START_METHOD => {
                    let (id, params): (_, LiveStartParams) = request
                        .extract(LIVE_START_METHOD)
                        .map_err(|error| format!("invalid live-start request: {error}"))?;
                    let entry = params.entry;
                    (id, Box::new(move || broker.start(entry.as_deref())))
                }
                LIVE_STOP_METHOD => {
                    let (id, _): (_, Value) = request
                        .extract(LIVE_STOP_METHOD)
                        .map_err(|error| format!("invalid live-stop request: {error}"))?;
                    (id, Box::new(move || broker.stop()))
                }
                LIVE_REQUEST_METHOD => {
                    let (id, params): (_, LiveRequestParams) = request
                        .extract(LIVE_REQUEST_METHOD)
                        .map_err(|error| format!("invalid live request: {error}"))?;
                    (id, Box::new(move || broker.request(params.fields)))
                }
                _ => unreachable!("custom live request was prefiltered"),
            };
        let workers = self.live_request_workers.clone();
        if workers
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_LIVE_REQUEST_WORKERS).then_some(count + 1)
            })
            .is_err()
        {
            let response = internal_error(
                id,
                format!(
                    "live requests are limited to {MAX_LIVE_REQUEST_WORKERS} concurrent operations"
                ),
            );
            return connection
                .sender
                .send(Message::Response(response))
                .map_err(|error| format!("failed sending live backpressure response: {error}"));
        }
        let worker_result = thread::Builder::new()
            .name("stasis-lsp-live-request".to_string())
            .spawn(move || {
                let response = match task() {
                    Ok(result) => Response::new_ok(id, result),
                    Err(error) => internal_error(id, error),
                };
                let _ = sender.send(Message::Response(response));
                workers.fetch_sub(1, Ordering::AcqRel);
            })
            .map_err(|error| format!("failed starting live request worker: {error}"));
        if let Err(error) = worker_result {
            self.live_request_workers.fetch_sub(1, Ordering::AcqRel);
            return Err(error);
        }
        Ok(())
    }

    fn drain_live_cache(&mut self) {
        let event = self
            .live_cache_event
            .lock()
            .ok()
            .and_then(|mut mailbox| mailbox.take());
        if let Some(event) = event {
            match event {
                LiveCacheEvent::Publish(batch) => self.service.publish_live_observations(batch),
                LiveCacheEvent::Clear => self.service.clear_live_observations(),
            }
        }
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

    fn resolve_completion(
        &mut self,
        mut item: lsp_types::CompletionItem,
    ) -> Result<lsp_types::CompletionItem, String> {
        let Some(data) = item.data.as_ref() else {
            return Ok(item);
        };
        let payload: CompletionResolvePayload = serde_json::from_value(data.clone())
            .map_err(|error| format!("invalid Stasis completion resolve data: {error}"))?;
        let Some(resolution) = self.service.resolve_completion(
            &payload.path,
            CompletionResolveData {
                revision: WorkspaceRevision::from_raw(payload.revision),
                catalog_index: payload.catalog_index,
            },
        )?
        else {
            return Ok(item);
        };
        item.documentation = resolution.documentation.map(|value| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            })
        });
        if !resolution.additional_text_edits.is_empty() {
            let snapshot = self.service.snapshot();
            let document = snapshot.document(&payload.path).ok_or_else(|| {
                format!(
                    "completion resolve document is not indexed: '{}'",
                    payload.path
                )
            })?;
            item.additional_text_edits = Some(lsp_completion_text_edits(
                &resolution.additional_text_edits,
                &payload.path,
                document,
            )?);
        }
        Ok(item)
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

    fn prepare_call_hierarchy(
        &mut self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>, String> {
        let (path, _, byte_offset) =
            self.document_position(params.text_document_position_params)?;
        let items = self
            .service
            .prepare_call_hierarchy(&path, byte_offset)?
            .into_iter()
            .map(|item| self.lsp_call_hierarchy_item(item))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((!items.is_empty()).then_some(items))
    }

    fn incoming_calls(
        &mut self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>, String> {
        let symbol_id = hierarchy_symbol_id(params.item.data.as_ref())?;
        let calls = self
            .service
            .incoming_calls(&symbol_id)?
            .into_iter()
            .map(|relation| {
                Ok(CallHierarchyIncomingCall {
                    from: self.lsp_call_hierarchy_item(relation.item)?,
                    from_ranges: relation
                        .from_ranges
                        .into_iter()
                        .map(|location| self.lsp_location(location).map(|location| location.range))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok((!calls.is_empty()).then_some(calls))
    }

    fn outgoing_calls(
        &mut self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>, String> {
        let symbol_id = hierarchy_symbol_id(params.item.data.as_ref())?;
        let calls = self
            .service
            .outgoing_calls(&symbol_id)?
            .into_iter()
            .map(|relation| {
                Ok(CallHierarchyOutgoingCall {
                    to: self.lsp_call_hierarchy_item(relation.item)?,
                    from_ranges: relation
                        .from_ranges
                        .into_iter()
                        .map(|location| self.lsp_location(location).map(|location| location.range))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok((!calls.is_empty()).then_some(calls))
    }

    fn prepare_type_hierarchy(
        &mut self,
        params: TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>, String> {
        let (path, _, byte_offset) =
            self.document_position(params.text_document_position_params)?;
        let items = self
            .service
            .prepare_type_hierarchy(&path, byte_offset)?
            .into_iter()
            .map(|item| self.lsp_type_hierarchy_item(item))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((!items.is_empty()).then_some(items))
    }

    fn type_supertypes(
        &mut self,
        params: TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>, String> {
        let symbol_id = hierarchy_symbol_id(params.item.data.as_ref())?;
        let items = self
            .service
            .type_supertypes(&symbol_id)?
            .into_iter()
            .map(|item| self.lsp_type_hierarchy_item(item))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((!items.is_empty()).then_some(items))
    }

    fn type_subtypes(
        &mut self,
        params: TypeHierarchySubtypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>, String> {
        let symbol_id = hierarchy_symbol_id(params.item.data.as_ref())?;
        let items = self
            .service
            .type_subtypes(&symbol_id)?
            .into_iter()
            .map(|item| self.lsp_type_hierarchy_item(item))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((!items.is_empty()).then_some(items))
    }

    fn folding_ranges(
        &mut self,
        params: FoldingRangeParams,
    ) -> Result<Option<Vec<FoldingRange>>, String> {
        let path = path_text(&uri_path(&params.text_document.uri)?);
        let document = self
            .service
            .snapshot()
            .document(&path)
            .cloned()
            .ok_or_else(|| format!("folding document is not indexed: '{path}'"))?;
        let ranges = self
            .service
            .folding_ranges(&path)?
            .into_iter()
            .filter_map(|range| {
                let start = document.position(range.range.start).ok()?;
                let end = document.position(range.range.end).ok()?;
                let end_line = end.line.saturating_sub(1);
                (start.line < end_line).then_some(FoldingRange {
                    start_line: start.line,
                    start_character: Some(start.utf16_character),
                    end_line,
                    end_character: None,
                    kind: None,
                    collapsed_text: None,
                })
            })
            .collect::<Vec<_>>();
        Ok((!ranges.is_empty()).then_some(ranges))
    }

    fn selection_ranges(
        &mut self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>, String> {
        let path = path_text(&uri_path(&params.text_document.uri)?);
        let document = self
            .service
            .snapshot()
            .document(&path)
            .cloned()
            .ok_or_else(|| format!("selection document is not indexed: '{path}'"))?;
        let offsets = params
            .positions
            .into_iter()
            .map(|position| {
                document
                    .byte_offset(Position {
                        line: position.line,
                        utf16_character: position.character,
                    })
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let selections = self
            .service
            .selection_ranges(&path, &offsets)?
            .into_iter()
            .map(|ranges| lsp_selection_range(&document, ranges))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(selections))
    }

    fn linked_edit_ranges(
        &mut self,
        params: LinkedEditingRangeParams,
    ) -> Result<Option<LinkedEditingRanges>, String> {
        let (path, document, byte_offset) =
            self.document_position(params.text_document_position_params)?;
        let Some(ranges) = self.service.linked_edit_ranges(&path, byte_offset)? else {
            return Ok(None);
        };
        let ranges = ranges
            .into_iter()
            .map(|range| {
                let start = document
                    .position(range.start)
                    .map_err(|error| error.to_string())?;
                let end = document
                    .position(range.end)
                    .map_err(|error| error.to_string())?;
                Ok(lsp_types::Range::new(
                    lsp_position(start),
                    lsp_position(end),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Some(LinkedEditingRanges {
            ranges,
            word_pattern: Some("[A-Za-z_][A-Za-z0-9_]*".to_string()),
        }))
    }

    fn format_document(
        &mut self,
        params: DocumentFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>, String> {
        let path = path_text(&uri_path(&params.text_document.uri)?);
        let edits = self.service.format_document(&path)?;
        self.lsp_text_edits(&path, edits)
    }

    fn format_range(
        &mut self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>, String> {
        let path = path_text(&uri_path(&params.text_document.uri)?);
        let document = self
            .service
            .snapshot()
            .document(&path)
            .cloned()
            .ok_or_else(|| format!("range-format document is not indexed: '{path}'"))?;
        let start = document
            .byte_offset(Position {
                line: params.range.start.line,
                utf16_character: params.range.start.character,
            })
            .map_err(|error| error.to_string())?;
        let end = document
            .byte_offset(Position {
                line: params.range.end.line,
                utf16_character: params.range.end.character,
            })
            .map_err(|error| error.to_string())?;
        let edits = self.service.format_range(&path, start..end)?;
        self.lsp_text_edits(&path, edits)
    }

    fn format_on_type(
        &mut self,
        params: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>, String> {
        let (path, _, byte_offset) = self.document_position(params.text_document_position)?;
        let edits = self
            .service
            .format_on_type(&path, byte_offset, &params.ch)?;
        self.lsp_text_edits(&path, edits)
    }

    fn lsp_text_edits(
        &self,
        path: &str,
        edits: Vec<stasis_language_service::LanguageTextEdit>,
    ) -> Result<Option<Vec<TextEdit>>, String> {
        let document = self
            .service
            .snapshot()
            .document(path)
            .cloned()
            .ok_or_else(|| format!("format target is not indexed: '{path}'"))?;
        let edits = edits
            .into_iter()
            .map(|edit| {
                let start = document
                    .position(edit.range.start)
                    .map_err(|error| error.to_string())?;
                let end = document
                    .position(edit.range.end)
                    .map_err(|error| error.to_string())?;
                Ok(TextEdit {
                    range: lsp_types::Range::new(lsp_position(start), lsp_position(end)),
                    new_text: edit.new_text,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok((!edits.is_empty()).then_some(edits))
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

    fn prepare_rename(
        &mut self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>, String> {
        let (path, document, byte_offset) = self.document_position(params)?;
        let prepared = self.service.prepare_rename(&path, byte_offset)?;
        let start = document
            .position(prepared.range.start)
            .map_err(|error| error.to_string())?;
        let end = document
            .position(prepared.range.end)
            .map_err(|error| error.to_string())?;
        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: lsp_types::Range::new(lsp_position(start), lsp_position(end)),
            placeholder: prepared.placeholder,
        }))
    }

    fn rename(&mut self, params: RenameParams) -> Result<Option<WorkspaceEdit>, String> {
        let (path, _, byte_offset) = self.document_position(params.text_document_position)?;
        let plan = self.service.rename(&path, byte_offset, &params.new_name)?;
        Ok(Some(self.workspace_edit(plan.edits)?))
    }

    fn code_actions(
        &mut self,
        params: CodeActionParams,
    ) -> Result<Option<Vec<CodeActionOrCommand>>, String> {
        let path = path_text(&uri_path(&params.text_document.uri)?);
        let request_diagnostics = params.context.diagnostics;
        let requested_kinds = params
            .context
            .only
            .unwrap_or_default()
            .into_iter()
            .map(|kind| kind.as_str().to_string())
            .collect::<Vec<_>>();
        let actions = self
            .service
            .code_actions(&path, &requested_kinds)?
            .into_iter()
            .map(|action| {
                let diagnostics = if let Some(code) = action.diagnostic_code.as_ref() {
                    if request_diagnostics.is_empty() {
                        // VS Code may omit the diagnostics it is requesting actions for.
                        // Keep an action only when its originating diagnostic is relevant to
                        // the requested document/range, but do not claim an association we
                        // cannot establish from the request context.
                        let action_touches_request = {
                            let Some(origin) = action.diagnostic_origin.as_ref() else {
                                return Ok(None);
                            };
                            let snapshot = self.service.snapshot();
                            let document = snapshot.document(&path).ok_or_else(|| {
                                format!("code-action document is not indexed: '{path}'")
                            })?;
                            let requested = document
                                .byte_offset(Position {
                                    line: params.range.start.line,
                                    utf16_character: params.range.start.character,
                                })
                                .and_then(|start| {
                                    document
                                        .byte_offset(Position {
                                            line: params.range.end.line,
                                            utf16_character: params.range.end.character,
                                        })
                                        .map(|end| start..end)
                                })
                                .map_err(|error| error.to_string())?;
                            // Do not infer provenance from edit targets: a valid fix may
                            // edit multiple documents, and an unrelated file-wide edit must
                            // not make it eligible for this request.
                            diagnostic_origin_touches_request(origin, &path, &requested)
                        };
                        if !action_touches_request {
                            return Ok(None);
                        }
                        None
                    } else {
                        let matching = request_diagnostics
                            .iter()
                            .filter(|diagnostic| {
                                diagnostic.code.as_ref()
                                    == Some(&NumberOrString::String(code.clone()))
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        if matching.is_empty() {
                            return Ok(None);
                        }
                        Some(matching)
                    }
                } else {
                    None
                };
                Ok(Some(CodeActionOrCommand::CodeAction(CodeAction {
                    title: action.title,
                    kind: Some(CodeActionKind::from(action.kind)),
                    diagnostics,
                    edit: Some(self.workspace_edit(action.edits)?),
                    command: None,
                    is_preferred: Some(action.preferred),
                    disabled: None,
                    data: None,
                })))
            })
            .collect::<Result<Vec<_>, String>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        Ok((!actions.is_empty()).then_some(actions))
    }

    fn semantic_tokens(
        &mut self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>, String> {
        let path = path_text(&uri_path(&params.text_document.uri)?);
        let document = self
            .service
            .snapshot()
            .document(&path)
            .cloned()
            .ok_or_else(|| format!("semantic-token document is not indexed: '{path}'"))?;
        let mut tokens = self.service.semantic_tokens(&path)?;
        tokens.sort_by_key(|token| (token.range.start, token.range.end));
        let mut previous_line = 0u32;
        let mut previous_start = 0u32;
        let mut data = Vec::with_capacity(tokens.len());
        for token in tokens {
            let start = document
                .position(token.range.start)
                .map_err(|error| error.to_string())?;
            let end = document
                .position(token.range.end)
                .map_err(|error| error.to_string())?;
            if start.line != end.line {
                continue;
            }
            let (token_type, token_modifiers_bitset) = semantic_token_style(&token.kind);
            let delta_line = start.line - previous_line;
            let delta_start = if delta_line == 0 {
                start.utf16_character - previous_start
            } else {
                start.utf16_character
            };
            data.push(SemanticToken {
                delta_line,
                delta_start,
                length: end.utf16_character - start.utf16_character,
                token_type,
                token_modifiers_bitset,
            });
            previous_line = start.line;
            previous_start = start.utf16_character;
        }
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }

    fn inlay_hints(&mut self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>, String> {
        let path = path_text(&uri_path(&params.text_document.uri)?);
        let document = self
            .service
            .snapshot()
            .document(&path)
            .cloned()
            .ok_or_else(|| format!("inlay-hint document is not indexed: '{path}'"))?;
        let range_start = document
            .byte_offset(Position {
                line: params.range.start.line,
                utf16_character: params.range.start.character,
            })
            .map_err(|error| error.to_string())?;
        let range_end = document
            .byte_offset(Position {
                line: params.range.end.line,
                utf16_character: params.range.end.character,
            })
            .map_err(|error| error.to_string())?;
        let hints = self
            .service
            .inlay_hints(&path)?
            .into_iter()
            .filter(|hint| range_start <= hint.position && hint.position <= range_end)
            .map(|hint| {
                let kind = match hint.kind {
                    LanguageInlayHintKind::Type => InlayHintKind::TYPE,
                    LanguageInlayHintKind::Parameter => InlayHintKind::PARAMETER,
                };
                let position = document
                    .position(hint.position)
                    .map_err(|error| error.to_string())?;
                Ok(InlayHint {
                    position: lsp_position(position),
                    label: InlayHintLabel::String(hint.label),
                    kind: Some(kind),
                    text_edits: None,
                    tooltip: Some(
                        match hint.kind {
                            LanguageInlayHintKind::Type => "Inferred by the Stasis compiler",
                            LanguageInlayHintKind::Parameter => {
                                "Parameter from the compiler-resolved function signature"
                            }
                        }
                        .to_string()
                        .into(),
                    ),
                    padding_left: Some(false),
                    padding_right: Some(hint.kind == LanguageInlayHintKind::Parameter),
                    data: None,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok((!hints.is_empty()).then_some(hints))
    }

    fn workspace_edit(
        &self,
        edits: Vec<stasis_language_service::RenameEdit>,
    ) -> Result<WorkspaceEdit, String> {
        let snapshot = self.service.snapshot();
        let mut by_path = BTreeMap::<String, Vec<stasis_language_service::RenameEdit>>::new();
        for edit in edits {
            by_path.entry(edit.path.clone()).or_default().push(edit);
        }
        let mut document_edits = Vec::new();
        for (path, edits) in by_path {
            let document = snapshot
                .document(&path)
                .ok_or_else(|| format!("rename target is not indexed: '{path}'"))?;
            let uri = self
                .uri_by_path
                .get(&path)
                .cloned()
                .unwrap_or(path_uri(Path::new(&path))?);
            let edits = edits
                .into_iter()
                .map(|edit| {
                    let start = document
                        .position(edit.range.start)
                        .map_err(|error| error.to_string())?;
                    let end = document
                        .position(edit.range.end)
                        .map_err(|error| error.to_string())?;
                    Ok(OneOf::Left(TextEdit {
                        range: lsp_types::Range::new(lsp_position(start), lsp_position(end)),
                        new_text: edit.new_text,
                    }))
                })
                .collect::<Result<Vec<_>, String>>()?;
            document_edits.push(TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri,
                    version: document
                        .version
                        .map(i32::try_from)
                        .transpose()
                        .map_err(|_| format!("document version is out of range for '{path}'"))?,
                },
                edits,
            });
        }
        Ok(WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(document_edits)),
            change_annotations: None,
        })
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

    fn lsp_call_hierarchy_item(
        &self,
        item: LanguageHierarchyItem,
    ) -> Result<CallHierarchyItem, String> {
        let selection_range = self.lsp_range(&item.location.path, item.selection_range.clone())?;
        let location = self.lsp_location(item.location)?;
        Ok(CallHierarchyItem {
            name: item.name,
            kind: hierarchy_symbol_kind(item.kind),
            tags: None,
            detail: Some(item.detail),
            uri: location.uri,
            range: location.range,
            selection_range,
            data: Some(
                serde_json::to_value(HierarchyPayload {
                    symbol_id: item.symbol_id,
                })
                .map_err(|error| format!("failed serializing hierarchy identity: {error}"))?,
            ),
        })
    }

    fn lsp_type_hierarchy_item(
        &self,
        item: LanguageHierarchyItem,
    ) -> Result<TypeHierarchyItem, String> {
        let selection_range = self.lsp_range(&item.location.path, item.selection_range.clone())?;
        let location = self.lsp_location(item.location)?;
        Ok(TypeHierarchyItem {
            name: item.name,
            kind: hierarchy_symbol_kind(item.kind),
            tags: None,
            detail: Some(item.detail),
            uri: location.uri,
            range: location.range,
            selection_range,
            data: Some(
                serde_json::to_value(HierarchyPayload {
                    symbol_id: item.symbol_id,
                })
                .map_err(|error| format!("failed serializing hierarchy identity: {error}"))?,
            ),
        })
    }

    fn lsp_range(
        &self,
        path: &str,
        range: std::ops::Range<usize>,
    ) -> Result<lsp_types::Range, String> {
        let document = self
            .service
            .snapshot()
            .document(path)
            .cloned()
            .ok_or_else(|| format!("hierarchy target is not indexed: '{path}'"))?;
        let start = document
            .position(range.start)
            .map_err(|error| error.to_string())?;
        let end = document
            .position(range.end)
            .map_err(|error| error.to_string())?;
        Ok(lsp_types::Range::new(
            lsp_position(start),
            lsp_position(end),
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
                    code: Some(NumberOrString::String(diagnostic.code)),
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

fn lsp_completion_text_edits(
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

fn hierarchy_symbol_kind(kind: LanguageHierarchyKind) -> SymbolKind {
    match kind {
        LanguageHierarchyKind::Function => SymbolKind::FUNCTION,
        LanguageHierarchyKind::Struct => SymbolKind::STRUCT,
    }
}

fn hierarchy_symbol_id(data: Option<&Value>) -> Result<String, String> {
    let data = data.ok_or_else(|| "hierarchy item has no Stasis identity".to_string())?;
    serde_json::from_value::<HierarchyPayload>(data.clone())
        .map(|payload| payload.symbol_id)
        .map_err(|error| format!("invalid hierarchy item identity: {error}"))
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

fn ranges_touch(left: &std::ops::Range<usize>, right: &std::ops::Range<usize>) -> bool {
    left.start <= left.end
        && right.start <= right.end
        && left.start <= right.end
        && right.start <= left.end
}

fn diagnostic_origin_touches_request(
    origin: &LanguageDiagnosticOrigin,
    path: &str,
    requested: &std::ops::Range<usize>,
) -> bool {
    origin.path == path && ranges_touch(&origin.range, requested)
}

fn lsp_selection_range(
    document: &Document,
    ranges: Vec<std::ops::Range<usize>>,
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

fn semantic_tokens_legend() -> SemanticTokensLegend {
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

fn semantic_token_style(kind: &str) -> (u32, u32) {
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
                    Some(".git" | ".stasis_cache" | ".worktrees" | "build" | "target")
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
    use stasis_language_service::{workshop_source_hash, LiveObservation, LiveObservationBatch};
    use std::time::Duration;

    fn test_server(name: &str) -> (LanguageServer, Uri, String) {
        let root = std::env::temp_dir().join(format!("stasis-lsp-{name}"));
        let path = root.join("src/main.stasis");
        let uri = path_uri(&path).expect("file URI");
        let key = path_text(&path);
        let service = LanguageService::new(path_text(&root)).expect("language service");
        let (live_process, live_cache_event) =
            LiveProcessBroker::new(&root, Arc::new(|_, _| {})).expect("live broker");
        (
            LanguageServer {
                service,
                uri_by_path: BTreeMap::new(),
                published_paths: BTreeSet::new(),
                completion_limit: 64,
                live_process,
                live_cache_event,
                live_request_workers: Arc::new(AtomicUsize::new(0)),
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
    fn workspace_discovery_excludes_generated_toolchain_cache() {
        let root =
            std::env::temp_dir().join(format!("stasis-lsp-cache-discovery-{}", std::process::id()));
        let source = root.join("src/main.stasis");
        let cached = root.join(".stasis_cache/toolchain/src/stdlib/graphics.stasis");
        fs::create_dir_all(source.parent().unwrap()).expect("source directory");
        fs::create_dir_all(cached.parent().unwrap()).expect("cache directory");
        fs::write(&source, "function main(): i32 { return 0; }\n").expect("source");
        fs::write(&cached, "function cached(): i32 { return 1; }\n").expect("cache source");

        assert_eq!(
            stasis_files(&root).expect("workspace sources"),
            vec![source]
        );

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn invalid_disk_source_does_not_prevent_lsp_startup() {
        let root =
            std::env::temp_dir().join(format!("stasis-lsp-invalid-startup-{}", std::process::id()));
        let source = root.join("src/main.stasis");
        fs::create_dir_all(source.parent().unwrap()).expect("source directory");
        fs::write(&source, "function unfinished(): i32 {").expect("invalid source");

        let mut server = LanguageServer::new(&root, 64, Arc::new(|_, _| {}))
            .expect("LSP starts with diagnostics-capable invalid source");
        assert!(!server.service.diagnostics().diagnostics.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn live_requests_are_dispatched_asynchronously_through_the_lsp_broker() {
        let (mut server, _, _) = test_server("live-request-dispatch");
        let (server_connection, client_connection) = Connection::memory();
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(90),
                    LIVE_REQUEST_METHOD.to_string(),
                    serde_json::json!({"type": "pause"}),
                ),
            )
            .expect("dispatch live request");
        let response = receive_response(&client_connection, 90);
        assert_eq!(
            response
                .response_result
                .expect_err("missing session error")
                .message,
            "no LSP-owned live Workshop session is running"
        );
    }

    #[test]
    fn live_request_workers_apply_backpressure_before_spawning() {
        let (mut server, _, _) = test_server("live-request-backpressure");
        let (server_connection, client_connection) = Connection::memory();
        server
            .live_request_workers
            .store(MAX_LIVE_REQUEST_WORKERS, Ordering::Release);
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(91),
                    LIVE_REQUEST_METHOD.to_string(),
                    serde_json::json!({"type": "pause"}),
                ),
            )
            .expect("bounded live request");
        assert!(receive_response(&client_connection, 91)
            .response_result
            .expect_err("backpressure error")
            .message
            .contains("limited to 64 concurrent operations"));
    }

    #[test]
    fn standard_code_action_returns_versioned_organize_imports_edit() {
        let (mut server, uri, main_path) = test_server("organize-imports");
        let (server_connection, client_connection) = Connection::memory();
        let source = "import \"unused.stasis\";\nimport \"helper.stasis\";\nimport \"helper.stasis\";\nfunction main(): i32 { return helper(); }\n";
        server.service.set_disk_document(&main_path, source);
        let source_directory = uri_path(&uri)
            .expect("main path")
            .parent()
            .expect("source directory")
            .to_path_buf();
        server.service.set_disk_document(
            path_text(&source_directory.join("helper.stasis")),
            "function helper(): i32 { return 1; }\n",
        );
        server.service.set_disk_document(
            path_text(&source_directory.join("unused.stasis")),
            "function unrelated(): i32 { return 2; }\n",
        );
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(92),
                    CodeActionRequest::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": {"uri": uri},
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 0}
                        },
                        "context": {
                            "diagnostics": [],
                            "only": ["source.organizeImports"]
                        }
                    }),
                ),
            )
            .expect("organize-imports request");
        let actions: Option<Vec<CodeActionOrCommand>> = serde_json::from_value(
            receive_response(&client_connection, 92)
                .response_result
                .expect("code-action result"),
        )
        .expect("code-action response");
        let CodeActionOrCommand::CodeAction(action) = &actions.expect("actions")[0] else {
            panic!("expected code action");
        };
        assert_eq!(action.kind, Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS));
        let Some(DocumentChanges::Edits(edits)) = action
            .edit
            .as_ref()
            .and_then(|edit| edit.document_changes.as_ref())
        else {
            panic!("expected document edit");
        };
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].text_document.version, None);
        assert!(matches!(
            &edits[0].edits[0],
            OneOf::Left(edit)
                if edit.new_text == "import \"helper.stasis\";\nfunction main(): i32 { return helper(); }\n"
        ));
    }

    #[test]
    fn standard_quick_fix_is_linked_to_structured_diagnostic() {
        let (mut server, uri, main_path) = test_server("missing-import-quick-fix");
        let (server_connection, client_connection) = Connection::memory();
        let source = "import \"missing.stasis\";\nfunction main(): i32 { return 0; }\n";
        server.service.open_document(main_path.clone(), 7, source);
        server
            .publish_diagnostics(&server_connection)
            .expect("publish structured diagnostic");
        let Message::Notification(notification) = client_connection
            .receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("diagnostic notification")
        else {
            panic!("expected diagnostic notification");
        };
        let published: PublishDiagnosticsParams =
            serde_json::from_value(notification.params).expect("published diagnostics");
        assert_eq!(
            published.diagnostics[0].code,
            Some(NumberOrString::String("stasis.missingModule".to_string()))
        );
        let diagnostic = published.diagnostics[0].clone();
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(93),
                    CodeActionRequest::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": {"uri": uri},
                        "range": {
                            "start": {"line": 0, "character": 7},
                            "end": {"line": 0, "character": 23}
                        },
                        "context": {
                            "diagnostics": [diagnostic],
                            "only": ["quickfix"]
                        }
                    }),
                ),
            )
            .expect("quick-fix request");
        let actions: Option<Vec<CodeActionOrCommand>> = serde_json::from_value(
            receive_response(&client_connection, 93)
                .response_result
                .expect("code-action result"),
        )
        .expect("code-action response");
        let CodeActionOrCommand::CodeAction(action) = &actions.expect("actions")[0] else {
            panic!("expected code action");
        };
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
        assert_eq!(action.diagnostics.as_ref().map(Vec::len), Some(1));
        let Some(DocumentChanges::Edits(edits)) = action
            .edit
            .as_ref()
            .and_then(|edit| edit.document_changes.as_ref())
        else {
            panic!("expected document edit");
        };
        assert_eq!(edits[0].text_document.version, Some(7));
        assert!(matches!(
            &edits[0].edits[0],
            OneOf::Left(edit) if edit.new_text.is_empty()
        ));
    }

    #[test]
    fn standard_quick_fix_is_available_without_request_diagnostics() {
        let (mut server, uri, main_path) = test_server("missing-import-quick-fix-empty-context");
        let (server_connection, client_connection) = Connection::memory();
        let source = "import \"missing.stasis\";\nfunction main(): i32 { return 0; }\n";
        server.service.open_document(main_path, 7, source);
        server
            .publish_diagnostics(&server_connection)
            .expect("publish structured diagnostic");
        let Message::Notification(_) = client_connection
            .receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("diagnostic notification")
        else {
            panic!("expected diagnostic notification");
        };
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(94),
                    CodeActionRequest::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": {"uri": uri},
                        "range": {
                            "start": {"line": 0, "character": 7},
                            "end": {"line": 0, "character": 23}
                        },
                        "context": {
                            "diagnostics": [],
                            "only": ["quickfix"]
                        }
                    }),
                ),
            )
            .expect("quick-fix request");
        let actions: Option<Vec<CodeActionOrCommand>> = serde_json::from_value(
            receive_response(&client_connection, 94)
                .response_result
                .expect("code-action result"),
        )
        .expect("code-action response");
        let CodeActionOrCommand::CodeAction(action) = &actions.expect("actions")[0] else {
            panic!("expected code action");
        };
        assert_eq!(action.title, "Remove unresolved import 'missing'");
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
        assert!(action.diagnostics.is_none());
    }

    #[test]
    fn diagnostic_origin_filter_requires_requested_document_not_edit_target() {
        let requested = 7..23;
        let origin = LanguageDiagnosticOrigin {
            path: "project/src/main.stasis".to_string(),
            range: 7..23,
        };
        assert!(diagnostic_origin_touches_request(
            &origin,
            "project/src/main.stasis",
            &requested
        ));
        assert!(!diagnostic_origin_touches_request(
            &origin,
            "project/src/helper.stasis",
            &requested
        ));
    }

    #[test]
    fn standard_quick_fix_without_request_diagnostics_is_suppressed_for_unrelated_range() {
        let (mut server, uri, main_path) = test_server("missing-import-quick-fix-unrelated-range");
        let (server_connection, client_connection) = Connection::memory();
        let source = "import \"missing.stasis\";\nfunction main(): i32 { return 0; }\n";
        server.service.open_document(main_path, 7, source);
        server
            .publish_diagnostics(&server_connection)
            .expect("publish structured diagnostic");
        let Message::Notification(_) = client_connection
            .receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("diagnostic notification")
        else {
            panic!("expected diagnostic notification");
        };
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(96),
                    CodeActionRequest::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": {"uri": uri},
                        "range": {
                            "start": {"line": 1, "character": 1},
                            "end": {"line": 1, "character": 2}
                        },
                        "context": {
                            "diagnostics": [],
                            "only": ["quickfix"]
                        }
                    }),
                ),
            )
            .expect("quick-fix request");
        let actions: Option<Vec<CodeActionOrCommand>> = serde_json::from_value(
            receive_response(&client_connection, 96)
                .response_result
                .expect("code-action result"),
        )
        .expect("code-action response");
        assert!(actions.is_none());
    }

    #[test]
    fn standard_quick_fix_drops_nonmatching_structured_diagnostic() {
        let (mut server, uri, main_path) = test_server("missing-import-quick-fix-nonmatching");
        let (server_connection, client_connection) = Connection::memory();
        let source = "import \"missing.stasis\";\nfunction main(): i32 { return 0; }\n";
        server.service.open_document(main_path, 7, source);
        server
            .publish_diagnostics(&server_connection)
            .expect("publish structured diagnostic");
        let Message::Notification(notification) = client_connection
            .receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("diagnostic notification")
        else {
            panic!("expected diagnostic notification");
        };
        let mut diagnostic: PublishDiagnosticsParams =
            serde_json::from_value(notification.params).expect("published diagnostics");
        diagnostic.diagnostics[0].code =
            Some(NumberOrString::String("stasis.otherDiagnostic".to_string()));
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(95),
                    CodeActionRequest::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": {"uri": uri},
                        "range": {
                            "start": {"line": 0, "character": 7},
                            "end": {"line": 0, "character": 23}
                        },
                        "context": {
                            "diagnostics": diagnostic.diagnostics,
                            "only": ["quickfix"]
                        }
                    }),
                ),
            )
            .expect("quick-fix request");
        let actions: Option<Vec<CodeActionOrCommand>> = serde_json::from_value(
            receive_response(&client_connection, 95)
                .response_result
                .expect("code-action result"),
        )
        .expect("code-action response");
        assert!(actions.is_none());
    }

    #[test]
    fn standard_semantic_tokens_distinguish_compiler_bound_symbols() {
        let (mut server, uri, main_path) = test_server("semantic-tokens");
        let (server_connection, client_connection) = Connection::memory();
        server.service.set_disk_document(
            &main_path,
            "struct State { x: i32; }\nglobal state: State;\nfunction read(value: State): i32 { let local: State = value; state.x = local.x; return state.x; }\nfunction main(): i32 { return read(state); }\n",
        );
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(93),
                    SemanticTokensFullRequest::METHOD.to_string(),
                    serde_json::json!({"textDocument": {"uri": uri}}),
                ),
            )
            .expect("semantic-token request");
        let response: Option<SemanticTokensResult> = serde_json::from_value(
            receive_response(&client_connection, 93)
                .response_result
                .expect("semantic-token result"),
        )
        .expect("semantic-token response");
        let SemanticTokensResult::Tokens(tokens) = response.expect("semantic tokens") else {
            panic!("expected full semantic tokens");
        };
        assert!(tokens.data.iter().any(|token| token.token_type == 0));
        assert!(tokens.data.iter().any(|token| token.token_type == 3));
        assert!(tokens
            .data
            .iter()
            .any(|token| token.token_type == 5 && token.token_modifiers_bitset == 0));
        assert!(tokens
            .data
            .iter()
            .any(|token| token.token_type == 5 && token.token_modifiers_bitset == 1 << 1));
        assert!(tokens.data.iter().any(|token| token.token_type == 6));
        assert!(tokens.data.iter().any(|token| token.token_type == 7));
    }

    #[test]
    fn standard_inlay_hints_return_inferred_types_and_parameter_names() {
        let (mut server, uri, main_path) = test_server("inlay-hints");
        let (server_connection, client_connection) = Connection::memory();
        let source = "struct Player { score: i32; }\nglobal player: Player;\nfunction boost(self: Player, amount: i32, bonus: i32): void { self.score += amount + bonus; }\nfunction main(): i32 { let total = 0; player.boost(1, 2); return total; }\n";
        server.service.set_disk_document(&main_path, source);
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(94),
                    InlayHintRequest::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": {"uri": uri},
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 4, "character": 0}
                        }
                    }),
                ),
            )
            .expect("inlay-hint request");
        let hints: Option<Vec<InlayHint>> = serde_json::from_value(
            receive_response(&client_connection, 94)
                .response_result
                .expect("inlay-hint result"),
        )
        .expect("inlay-hint response");
        let hints = hints.expect("inlay hints");
        assert!(hints.iter().any(|hint| {
            hint.kind == Some(InlayHintKind::TYPE)
                && matches!(&hint.label, InlayHintLabel::String(label) if label == ": i32")
        }));
        assert!(hints.iter().any(|hint| {
            hint.kind == Some(InlayHintKind::PARAMETER)
                && matches!(&hint.label, InlayHintLabel::String(label) if label == "amount:")
        }));
        assert!(hints.iter().any(|hint| {
            hint.kind == Some(InlayHintKind::PARAMETER)
                && matches!(&hint.label, InlayHintLabel::String(label) if label == "bonus:")
        }));
        let call_line = source.lines().nth(3).expect("receiver call line");
        let call_start = call_line.find("player.boost(").expect("receiver call");
        for (relative, label) in [
            ("player.boost(".len(), "amount:"),
            ("player.boost(1, ".len(), "bonus:"),
        ] {
            let character = (call_start + relative) as u32;
            assert!(hints.iter().any(|hint| {
                hint.position == lsp_types::Position::new(3, character)
                    && matches!(&hint.label, InlayHintLabel::String(actual) if actual == label)
            }));
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
        let source = "global score: i32;\n// Creates an enemy.\nfunction spawn_enemy(count: i32, health: i32): i32 { return health; }\nfunction main(): i32 { let health: i32 = 2; score = health; return spawn_enemy(1, health); }\n";
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
        server
            .service
            .publish_live_observations(LiveObservationBatch {
                session_id: "test-live-session".to_string(),
                generation: 3,
                complete: true,
                source_hashes: BTreeMap::from([(
                    "src/main.stasis".to_string(),
                    workshop_source_hash(source),
                )]),
                indexed_collections: Vec::new(),
                observations: vec![LiveObservation {
                    path: "score".to_string(),
                    type_name: Some("i32".to_string()),
                    value: "2".to_string(),
                    tick: 9,
                }],
            });

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
            .expect("function completion")
            .clone();
        assert_eq!(function.insert_text_format, None);
        let Some(CompletionTextEdit::Edit(edit)) = function.text_edit.as_ref() else {
            panic!("expected completion text edit");
        };
        assert_eq!(edit.new_text, "spawn_enemy");
        assert!(function.documentation.is_none());
        assert!(function.data.is_some());

        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(13),
                    ResolveCompletionItem::METHOD.to_string(),
                    function,
                ),
            )
            .expect("completion resolve request");
        let response = receive_response(&client_connection, 13);
        let function: lsp_types::CompletionItem =
            serde_json::from_value(response.response_result.expect("completion resolve result"))
                .expect("resolved completion item");
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

        let score_offset = source.find("score =").expect("global use") + 2;
        let score_position = lsp_position(
            server
                .service
                .snapshot()
                .document(&path_text(&uri_path(&uri).expect("path")))
                .expect("document")
                .position(score_offset)
                .expect("position"),
        );
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(19),
                    HoverRequest::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": score_position
                    }),
                ),
            )
            .expect("live hover request");
        let live_hover: Option<Hover> = serde_json::from_value(
            receive_response(&client_connection, 19)
                .response_result
                .expect("live hover result"),
        )
        .expect("live hover response");
        let HoverContents::Markup(live_contents) = live_hover.expect("live hover").contents else {
            panic!("expected live markdown hover");
        };
        assert!(live_contents.value.contains("Live value:** `2 (tick 9)`"));
        server.service.clear_live_observations();
        assert_eq!(
            server
                .service
                .hover(&path_text(&uri_path(&uri).expect("path")), score_offset)
                .expect("static hover after live clear")
                .and_then(|hover| hover.live_value),
            None
        );

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

        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(17),
                    PrepareRenameRequest::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": function_position
                    }),
                ),
            )
            .expect("prepare rename request");
        let prepared: Option<PrepareRenameResponse> = serde_json::from_value(
            receive_response(&client_connection, 17)
                .response_result
                .expect("prepare rename result"),
        )
        .expect("prepare rename response");
        assert!(matches!(
            prepared,
            Some(PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. })
                if placeholder == "spawn_enemy"
        ));

        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(18),
                    Rename::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": function_position,
                        "newName": "create_enemy"
                    }),
                ),
            )
            .expect("rename request");
        let edit: Option<WorkspaceEdit> = serde_json::from_value(
            receive_response(&client_connection, 18)
                .response_result
                .expect("rename result"),
        )
        .expect("rename response");
        let Some(DocumentChanges::Edits(document_edits)) =
            edit.expect("workspace edit").document_changes
        else {
            panic!("expected versioned document edits");
        };
        assert_eq!(document_edits.len(), 1);
        assert_eq!(document_edits[0].text_document.version, Some(1));
        assert_eq!(document_edits[0].edits.len(), 2);
        assert!(document_edits[0]
            .edits
            .iter()
            .all(|edit| { matches!(edit, OneOf::Left(edit) if edit.new_text == "create_enemy") }));

        let partial = source.replace("spawn_enemy(1, health);", "spawn_e");
        server
            .handle_notification(
                &server_connection,
                Notification::new(
                    DidChangeTextDocument::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri, "version": 2 },
                        "contentChanges": [{ "text": partial }]
                    }),
                ),
            )
            .expect("didChange to partial call");
        let partial_offset = partial.rfind("spawn_e").expect("partial call") + "spawn_e".len();
        let partial_position = lsp_position(
            server
                .service
                .snapshot()
                .document(&path_text(&uri_path(&uri).expect("path")))
                .expect("partial document")
                .position(partial_offset)
                .expect("partial position"),
        );
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(20),
                    Completion::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": partial_position
                    }),
                ),
            )
            .expect("partial completion request");
        let completion: Option<CompletionResponse> = serde_json::from_value(
            receive_response(&client_connection, 20)
                .response_result
                .expect("partial completion result"),
        )
        .expect("partial completion response");
        let CompletionResponse::List(completion) = completion.expect("partial completion list")
        else {
            panic!("expected partial completion list");
        };
        let function = completion
            .items
            .iter()
            .find(|item| item.label == "spawn_enemy")
            .expect("partial function completion");
        assert_eq!(function.insert_text_format, Some(InsertTextFormat::SNIPPET));
        let Some(CompletionTextEdit::Edit(edit)) = function.text_edit.as_ref() else {
            panic!("expected partial completion text edit");
        };
        assert_eq!(edit.new_text, "spawn_enemy(${1:count}, ${2:health})");
    }

    #[test]
    fn standard_call_and_composition_type_hierarchy_requests_work() {
        let (mut server, uri, _) = test_server("hierarchy");
        let (server_connection, client_connection) = Connection::memory();
        let source = "struct Position { x: i32; }\nstruct Enemy { position: Position; }\nfunction a(): i32 { return 1; }\nfunction b(): i32 { return a(); }\n";
        server
            .handle_notification(
                &server_connection,
                Notification::new(
                    DidOpenTextDocument::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri, "languageId": "stasis", "version": 1, "text": source }
                    }),
                ),
            )
            .expect("didOpen");
        let document = server
            .service
            .snapshot()
            .document(&path_text(&uri_path(&uri).expect("path")))
            .cloned()
            .expect("document");
        let position = |offset| lsp_position(document.position(offset).expect("position"));

        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(30),
                    CallHierarchyPrepare::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": position(source.find("a():").expect("a"))
                    }),
                ),
            )
            .expect("prepare calls");
        let prepared: Option<Vec<CallHierarchyItem>> = serde_json::from_value(
            receive_response(&client_connection, 30)
                .response_result
                .expect("prepare calls result"),
        )
        .expect("prepare calls response");
        let item = prepared.expect("call item").remove(0);
        assert_eq!(item.name, "a");
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(31),
                    CallHierarchyIncomingCalls::METHOD.to_string(),
                    serde_json::json!({ "item": item }),
                ),
            )
            .expect("incoming calls");
        let incoming: Option<Vec<CallHierarchyIncomingCall>> = serde_json::from_value(
            receive_response(&client_connection, 31)
                .response_result
                .expect("incoming result"),
        )
        .expect("incoming response");
        assert_eq!(incoming.expect("incoming")[0].from.name, "b");

        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(32),
                    TypeHierarchyPrepare::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": position(source.find("Enemy").expect("Enemy"))
                    }),
                ),
            )
            .expect("prepare type");
        let prepared: Option<Vec<TypeHierarchyItem>> = serde_json::from_value(
            receive_response(&client_connection, 32)
                .response_result
                .expect("prepare type result"),
        )
        .expect("prepare type response");
        let item = prepared.expect("type item").remove(0);
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(33),
                    TypeHierarchySubtypes::METHOD.to_string(),
                    serde_json::json!({ "item": item }),
                ),
            )
            .expect("component types");
        let components: Option<Vec<TypeHierarchyItem>> = serde_json::from_value(
            receive_response(&client_connection, 33)
                .response_result
                .expect("components result"),
        )
        .expect("components response");
        assert_eq!(components.expect("components")[0].name, "Position");
    }

    #[test]
    fn standard_folding_and_selection_ranges_tolerate_incomplete_source() {
        let (mut server, uri, _) = test_server("folding-selection");
        let (server_connection, client_connection) = Connection::memory();
        let source = "function a(): i32 {\n    if (true) {\n        return 1;\n    }\n}\nfunction b(): i32 {\n    a(state.";
        server
            .handle_notification(
                &server_connection,
                Notification::new(
                    DidOpenTextDocument::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri, "languageId": "stasis", "version": 1, "text": source }
                    }),
                ),
            )
            .expect("didOpen");
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(40),
                    FoldingRangeRequest::METHOD.to_string(),
                    serde_json::json!({ "textDocument": { "uri": uri } }),
                ),
            )
            .expect("folding request");
        let folds: Option<Vec<FoldingRange>> = serde_json::from_value(
            receive_response(&client_connection, 40)
                .response_result
                .expect("folding result"),
        )
        .expect("folding response");
        let folds = folds.expect("folds");
        assert_eq!(folds.len(), 2);
        assert_eq!((folds[0].start_line, folds[0].end_line), (0, 3));
        assert_eq!((folds[1].start_line, folds[1].end_line), (1, 2));

        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(41),
                    SelectionRangeRequest::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "positions": [{ "line": 2, "character": 10 }]
                    }),
                ),
            )
            .expect("selection request");
        let selections: Option<Vec<SelectionRange>> = serde_json::from_value(
            receive_response(&client_connection, 41)
                .response_result
                .expect("selection result"),
        )
        .expect("selection response");
        let selection = &selections.expect("selections")[0];
        assert_eq!(selection.range.start.line, 2);
        assert!(selection.parent.is_some());
    }

    #[test]
    fn standard_linked_editing_is_scoped_and_pauses_on_broken_source() {
        let (mut server, uri, _) = test_server("linked-edit");
        let (server_connection, client_connection) = Connection::memory();
        let source = "function main(): i32 { let copy: i32 = 1; copy += 1; return copy; }\n";
        server
            .handle_notification(
                &server_connection,
                Notification::new(
                    DidOpenTextDocument::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri, "languageId": "stasis", "version": 1, "text": source }
                    }),
                ),
            )
            .expect("didOpen");
        let copy_character =
            u32::try_from(source.find("copy: i32").expect("copy") + 1).expect("copy character");
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(42),
                    LinkedEditingRange::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": 0, "character": copy_character }
                    }),
                ),
            )
            .expect("linked-edit request");
        let linked: Option<LinkedEditingRanges> = serde_json::from_value(
            receive_response(&client_connection, 42)
                .response_result
                .expect("linked-edit result"),
        )
        .expect("linked-edit response");
        assert_eq!(linked.expect("linked edits").ranges.len(), 3);
    }

    #[test]
    fn standard_document_range_and_on_type_formatting_use_shared_formatter() {
        let (mut server, uri, _) = test_server("formatting");
        let (server_connection, client_connection) = Connection::memory();
        let source = "function main(): i32 {\r\nreturn 0;\r\n}\r\n";
        server
            .handle_notification(
                &server_connection,
                Notification::new(
                    DidOpenTextDocument::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri, "languageId": "stasis", "version": 1, "text": source }
                    }),
                ),
            )
            .expect("didOpen");

        let options = serde_json::json!({ "tabSize": 4, "insertSpaces": true });
        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(43),
                    Formatting::METHOD.to_string(),
                    serde_json::json!({ "textDocument": { "uri": uri }, "options": options }),
                ),
            )
            .expect("document formatting request");
        let document: Option<Vec<TextEdit>> = serde_json::from_value(
            receive_response(&client_connection, 43)
                .response_result
                .expect("document formatting result"),
        )
        .expect("document formatting response");
        let document = document.expect("document edits");
        assert_eq!(document.len(), 1);
        assert_eq!(document[0].new_text, "    ");

        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(44),
                    RangeFormatting::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "range": {
                            "start": { "line": 0, "character": 0 },
                            "end": { "line": 3, "character": 0 }
                        },
                        "options": options
                    }),
                ),
            )
            .expect("range formatting request");
        let range: Option<Vec<TextEdit>> = serde_json::from_value(
            receive_response(&client_connection, 44)
                .response_result
                .expect("range formatting result"),
        )
        .expect("range formatting response");
        assert_eq!(range.expect("range edits"), document);

        server
            .handle_request(
                &server_connection,
                Request::new(
                    RequestId::from(45),
                    OnTypeFormatting::METHOD.to_string(),
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": 2, "character": 1 },
                        "ch": "}",
                        "options": options
                    }),
                ),
            )
            .expect("on-type formatting request");
        let on_type: Option<Vec<TextEdit>> = serde_json::from_value(
            receive_response(&client_connection, 45)
                .response_result
                .expect("on-type formatting result"),
        )
        .expect("on-type formatting response");
        assert_eq!(on_type.expect("on-type edits"), document);
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
