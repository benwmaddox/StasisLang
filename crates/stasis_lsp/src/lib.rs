#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use lsp_server::{Connection, Message, Notification, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, InitializeParams, InitializeResult, PositionEncodingKind,
    PublishDiagnosticsParams, SaveOptions, ServerCapabilities, ServerInfo,
    TextDocumentContentChangeEvent, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, Uri,
};
use stasis_language_service::{
    DiagnosticSeverity, Document, LanguageService, Position, TextChange,
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
    let _: InitializeParams = serde_json::from_value(initialize_params)
        .map_err(|error| format!("invalid LSP initialize parameters: {error}"))?;
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

    let mut server = LanguageServer::new(project_root)?;
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
                connection
                    .sender
                    .send(Message::Response(Response::new_err(
                        request.id,
                        lsp_server::ErrorCode::MethodNotFound as i32,
                        format!("unsupported LSP request '{}'", request.method),
                    )))
                    .map_err(|error| format!("failed sending LSP response: {error}"))?;
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
}

impl LanguageServer {
    fn new(project_root: &Path) -> Result<Self, String> {
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
        })
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
            },
            uri,
            key,
        )
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
