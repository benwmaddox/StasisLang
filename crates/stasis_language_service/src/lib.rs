#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use stasis_compiler::compiler::Compiler;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkspaceRevision(u64);

impl WorkspaceRevision {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub utf16_character: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChange {
    pub range: Option<Range<usize>>,
    pub text: String,
}

impl TextChange {
    pub fn replace(range: Range<usize>, text: impl Into<String>) -> Self {
        Self {
            range: Some(range),
            text: text.into(),
        }
    }

    pub fn replace_all(text: impl Into<String>) -> Self {
        Self {
            range: None,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub version: Option<i64>,
    pub text: Arc<str>,
    line_starts: Arc<[usize]>,
}

impl Document {
    fn disk(text: String) -> Self {
        Self::new(None, text)
    }

    fn overlay(version: i64, text: String) -> Self {
        Self::new(Some(version), text)
    }

    fn new(version: Option<i64>, text: String) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(offset, _)| offset + 1),
        );
        Self {
            version,
            text: text.into(),
            line_starts: line_starts.into(),
        }
    }

    pub fn byte_offset(&self, position: Position) -> Result<usize, PositionError> {
        let line_index = position.line as usize;
        let line_start = self
            .line_starts
            .get(line_index)
            .copied()
            .ok_or(PositionError::LineOutOfBounds(position.line))?;
        let line_end = self
            .line_starts
            .get(line_index + 1)
            .map_or(self.text.len(), |next_start| next_start - 1);
        let line = &self.text[line_start..line_end];
        let mut utf16 = 0u32;
        for (relative, character) in line.char_indices() {
            if utf16 == position.utf16_character {
                return Ok(line_start + relative);
            }
            utf16 = utf16.saturating_add(character.len_utf16() as u32);
            if utf16 > position.utf16_character {
                return Err(PositionError::InsideUtf16Character {
                    line: position.line,
                    utf16_character: position.utf16_character,
                });
            }
        }
        if utf16 == position.utf16_character {
            Ok(line_end)
        } else {
            Err(PositionError::CharacterOutOfBounds {
                line: position.line,
                utf16_character: position.utf16_character,
            })
        }
    }

    pub fn position(&self, byte_offset: usize) -> Result<Position, PositionError> {
        if byte_offset > self.text.len() || !self.text.is_char_boundary(byte_offset) {
            return Err(PositionError::InvalidByteOffset(byte_offset));
        }
        let line = self
            .line_starts
            .partition_point(|start| *start <= byte_offset)
            - 1;
        let line_start = self.line_starts[line];
        let utf16_character = self.text[line_start..byte_offset].encode_utf16().count() as u32;
        Ok(Position {
            line: line as u32,
            utf16_character,
        })
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceSnapshot {
    revision: WorkspaceRevision,
    documents: Arc<BTreeMap<String, Document>>,
}

impl WorkspaceSnapshot {
    pub fn revision(&self) -> WorkspaceRevision {
        self.revision
    }

    pub fn document(&self, path: &str) -> Option<&Document> {
        self.documents.get(path)
    }

    pub fn documents(&self) -> impl ExactSizeIterator<Item = (&str, &Document)> {
        self.documents
            .iter()
            .map(|(path, document)| (path.as_str(), document))
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceDocuments {
    revision: WorkspaceRevision,
    disk: BTreeMap<String, Arc<str>>,
    overlays: BTreeMap<String, Document>,
    snapshot: WorkspaceSnapshot,
}

impl Default for WorkspaceDocuments {
    fn default() -> Self {
        let revision = WorkspaceRevision(0);
        Self {
            revision,
            disk: BTreeMap::new(),
            overlays: BTreeMap::new(),
            snapshot: WorkspaceSnapshot {
                revision,
                documents: Arc::new(BTreeMap::new()),
            },
        }
    }
}

impl WorkspaceDocuments {
    pub fn snapshot(&self) -> WorkspaceSnapshot {
        self.snapshot.clone()
    }

    pub fn set_disk_document(&mut self, path: impl Into<String>, text: impl Into<String>) {
        let path = path.into();
        let text = text.into();
        if self
            .disk
            .get(&path)
            .is_some_and(|current| current.as_ref() == text)
        {
            return;
        }
        self.disk.insert(path, text.into());
        self.publish();
    }

    pub fn remove_disk_document(&mut self, path: &str) {
        if self.disk.remove(path).is_some() {
            self.publish();
        }
    }

    pub fn open_document(
        &mut self,
        path: impl Into<String>,
        version: i64,
        text: impl Into<String>,
    ) {
        self.overlays
            .insert(path.into(), Document::overlay(version, text.into()));
        self.publish();
    }

    pub fn change_document(
        &mut self,
        path: &str,
        version: i64,
        changes: &[TextChange],
    ) -> Result<(), DocumentChangeError> {
        let document = self
            .overlays
            .get(path)
            .ok_or_else(|| DocumentChangeError::NotOpen(path.to_string()))?;
        let current_version = document
            .version
            .expect("open document always has a version");
        if version <= current_version {
            return Err(DocumentChangeError::StaleVersion {
                path: path.to_string(),
                current: current_version,
                requested: version,
            });
        }
        let mut next = document.text.to_string();
        for change in changes {
            match &change.range {
                Some(range)
                    if range.start <= range.end
                        && range.end <= next.len()
                        && next.is_char_boundary(range.start)
                        && next.is_char_boundary(range.end) =>
                {
                    next.replace_range(range.clone(), &change.text);
                }
                Some(range) => {
                    return Err(DocumentChangeError::InvalidRange {
                        path: path.to_string(),
                        start: range.start,
                        end: range.end,
                        length: next.len(),
                    });
                }
                None => next = change.text.clone(),
            }
        }
        self.overlays
            .insert(path.to_string(), Document::overlay(version, next));
        self.publish();
        Ok(())
    }

    pub fn close_document(&mut self, path: &str) {
        if self.overlays.remove(path).is_some() {
            self.publish();
        }
    }

    fn publish(&mut self) {
        let mut documents = self
            .disk
            .iter()
            .map(|(path, text)| (path.clone(), Document::disk(text.to_string())))
            .collect::<BTreeMap<_, _>>();
        documents.extend(self.overlays.clone());
        self.revision = WorkspaceRevision(
            self.revision
                .get()
                .checked_add(1)
                .expect("workspace revision overflow"),
        );
        self.snapshot = WorkspaceSnapshot {
            revision: self.revision,
            documents: Arc::new(documents),
        };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub path: String,
    pub range: Range<usize>,
    pub severity: DiagnosticSeverity,
    pub source: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticReport {
    pub revision: WorkspaceRevision,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct LanguageService {
    documents: WorkspaceDocuments,
    compiler: Compiler,
    project_root: String,
}

impl LanguageService {
    pub fn new(project_root: impl Into<String>) -> Result<Self, String> {
        let project_root = project_root.into().replace('\\', "/");
        let mut compiler = Compiler::new();
        compiler.set_project_root(project_root.clone())?;
        Ok(Self {
            documents: WorkspaceDocuments::default(),
            compiler,
            project_root,
        })
    }

    pub fn snapshot(&self) -> WorkspaceSnapshot {
        self.documents.snapshot()
    }

    pub fn set_disk_document(&mut self, path: impl Into<String>, text: impl Into<String>) {
        self.documents.set_disk_document(path, text);
    }

    pub fn remove_disk_document(&mut self, path: &str) {
        self.documents.remove_disk_document(path);
    }

    pub fn open_document(
        &mut self,
        path: impl Into<String>,
        version: i64,
        text: impl Into<String>,
    ) {
        self.documents.open_document(path, version, text);
    }

    pub fn change_document(
        &mut self,
        path: &str,
        version: i64,
        changes: &[TextChange],
    ) -> Result<(), DocumentChangeError> {
        self.documents.change_document(path, version, changes)
    }

    pub fn close_document(&mut self, path: &str) {
        self.documents.close_document(path);
    }

    pub fn diagnostics(&mut self) -> DiagnosticReport {
        let snapshot = self.documents.snapshot();
        let paths = snapshot
            .documents()
            .map(|(path, _)| path.to_string())
            .collect::<BTreeSet<_>>();
        self.compiler.retain_files(&paths);
        for (path, document) in snapshot.documents() {
            self.compiler.upsert_file(path, document.text.to_string());
        }

        let diagnostics = match self.compiler.check() {
            Ok(_) => Vec::new(),
            Err(error) => {
                let diagnostic = self.compiler.last_source_diagnostic();
                let path = diagnostic
                    .map(|diagnostic| self.snapshot_path(&paths, &diagnostic.path))
                    .or_else(|| paths.first().cloned())
                    .unwrap_or_default();
                vec![Diagnostic {
                    path,
                    range: diagnostic
                        .map(|diagnostic| diagnostic.start..diagnostic.end)
                        .unwrap_or(0..0),
                    severity: DiagnosticSeverity::Error,
                    source: "stasis",
                    message: diagnostic
                        .map(|diagnostic| diagnostic.message.clone())
                        .unwrap_or_else(|| format!("{error:?}")),
                }]
            }
        };
        DiagnosticReport {
            revision: snapshot.revision(),
            diagnostics,
        }
    }

    fn snapshot_path(&self, paths: &BTreeSet<String>, compiler_path: &str) -> String {
        if paths.contains(compiler_path) {
            return compiler_path.to_string();
        }
        let absolute = Path::new(&self.project_root)
            .join(compiler_path)
            .to_string_lossy()
            .replace('\\', "/");
        paths
            .get(&absolute)
            .cloned()
            .unwrap_or_else(|| compiler_path.to_string())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DocumentChangeError {
    NotOpen(String),
    StaleVersion {
        path: String,
        current: i64,
        requested: i64,
    },
    InvalidRange {
        path: String,
        start: usize,
        end: usize,
        length: usize,
    },
}

impl fmt::Display for DocumentChangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOpen(path) => write!(formatter, "document '{path}' is not open"),
            Self::StaleVersion {
                path,
                current,
                requested,
            } => write!(
                formatter,
                "document '{path}' change version {requested} is not newer than {current}"
            ),
            Self::InvalidRange {
                path,
                start,
                end,
                length,
            } => write!(
                formatter,
                "document '{path}' byte range {start}..{end} is invalid for {length} bytes"
            ),
        }
    }
}

impl std::error::Error for DocumentChangeError {}

#[derive(Debug, PartialEq, Eq)]
pub enum PositionError {
    LineOutOfBounds(u32),
    InsideUtf16Character { line: u32, utf16_character: u32 },
    CharacterOutOfBounds { line: u32, utf16_character: u32 },
    InvalidByteOffset(usize),
}

impl fmt::Display for PositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LineOutOfBounds(line) => write!(formatter, "line {line} is outside the document"),
            Self::InsideUtf16Character {
                line,
                utf16_character,
            } => write!(
                formatter,
                "UTF-16 position {line}:{utf16_character} splits a Unicode character"
            ),
            Self::CharacterOutOfBounds {
                line,
                utf16_character,
            } => write!(
                formatter,
                "UTF-16 position {line}:{utf16_character} is outside the line"
            ),
            Self::InvalidByteOffset(offset) => write!(
                formatter,
                "byte offset {offset} is outside the document or splits a Unicode character"
            ),
        }
    }
}

impl std::error::Error for PositionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_overlay_shadows_disk_and_close_restores_it() {
        let mut workspace = WorkspaceDocuments::default();
        workspace.set_disk_document("src/main.stasis", "function main(): i32 { return 1; }");
        let disk = workspace.snapshot();
        workspace.open_document("src/main.stasis", 3, "function main(): i32 { return 2; }");
        let dirty = workspace.snapshot();

        assert_eq!(disk.revision().get(), 1);
        assert_eq!(dirty.revision().get(), 2);
        assert_eq!(
            dirty.document("src/main.stasis").unwrap().text.as_ref(),
            "function main(): i32 { return 2; }"
        );
        assert_eq!(
            disk.document("src/main.stasis").unwrap().text.as_ref(),
            "function main(): i32 { return 1; }"
        );

        workspace.close_document("src/main.stasis");
        let closed = workspace.snapshot();
        assert_eq!(closed.revision().get(), 3);
        assert_eq!(
            closed.document("src/main.stasis").unwrap().text.as_ref(),
            "function main(): i32 { return 1; }"
        );
    }

    #[test]
    fn incremental_changes_are_atomic_and_reject_stale_versions() {
        let mut workspace = WorkspaceDocuments::default();
        workspace.open_document("src/main.stasis", 1, "score = 1;");
        workspace
            .change_document(
                "src/main.stasis",
                2,
                &[
                    TextChange::replace(8..9, "20"),
                    TextChange::replace(0..5, "total"),
                ],
            )
            .unwrap();
        let accepted = workspace.snapshot();
        assert_eq!(
            accepted.document("src/main.stasis").unwrap().text.as_ref(),
            "total = 20;"
        );

        let revision = accepted.revision();
        let error = workspace
            .change_document("src/main.stasis", 2, &[TextChange::replace_all("stale")])
            .unwrap_err();
        assert!(matches!(error, DocumentChangeError::StaleVersion { .. }));
        assert_eq!(workspace.snapshot().revision(), revision);

        let error = workspace
            .change_document(
                "src/main.stasis",
                3,
                &[TextChange::replace(0..100, "invalid")],
            )
            .unwrap_err();
        assert!(matches!(error, DocumentChangeError::InvalidRange { .. }));
        assert_eq!(workspace.snapshot().revision(), revision);
        assert_eq!(
            workspace
                .snapshot()
                .document("src/main.stasis")
                .unwrap()
                .text
                .as_ref(),
            "total = 20;"
        );
    }

    #[test]
    fn positions_convert_between_utf8_bytes_and_lsp_utf16() {
        let document = Document::overlay(1, "a\u{1f600}b\n\u{03b2}".to_string());

        assert_eq!(
            document
                .byte_offset(Position {
                    line: 0,
                    utf16_character: 3,
                })
                .unwrap(),
            5
        );
        assert_eq!(
            document.position(5).unwrap(),
            Position {
                line: 0,
                utf16_character: 3,
            }
        );
        assert_eq!(
            document.position(9).unwrap(),
            Position {
                line: 1,
                utf16_character: 1,
            }
        );
        assert_eq!(
            document.byte_offset(Position {
                line: 1,
                utf16_character: 1,
            }),
            Ok(9)
        );
        assert!(matches!(
            document.byte_offset(Position {
                line: 0,
                utf16_character: 2,
            }),
            Err(PositionError::InsideUtf16Character { .. })
        ));
        assert_eq!(
            document.position(2),
            Err(PositionError::InvalidByteOffset(2))
        );
    }

    #[test]
    fn invalid_multibyte_edit_does_not_publish_a_revision() {
        let mut workspace = WorkspaceDocuments::default();
        workspace.open_document("src/main.stasis", 1, "\u{1f600}");
        let revision = workspace.snapshot().revision();

        assert!(matches!(
            workspace.change_document("src/main.stasis", 2, &[TextChange::replace(1..4, "x")],),
            Err(DocumentChangeError::InvalidRange { .. })
        ));
        assert_eq!(workspace.snapshot().revision(), revision);
    }

    #[test]
    fn diagnostics_follow_dirty_overlay_revisions_and_clear_on_close() {
        let root = std::env::temp_dir().join("stasis-language-service-diagnostics");
        let root_text = root.to_string_lossy().replace('\\', "/");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let mut service = LanguageService::new(root_text).expect("language service");
        service.set_disk_document(path_text.clone(), "function main(): i32 { return 0; }\n");
        assert!(service.diagnostics().diagnostics.is_empty());

        service.open_document(
            path_text.clone(),
            1,
            "function main(): i32 { return 0; }\nfunction broken(): i32 { while (true) { return 1; } }\n",
        );
        let dirty_revision = service.snapshot().revision();
        let dirty = service.diagnostics();
        assert_eq!(dirty.revision, dirty_revision);
        assert_eq!(dirty.diagnostics.len(), 1);
        assert_eq!(dirty.diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(dirty.diagnostics[0].source, "stasis");
        assert_eq!(dirty.diagnostics[0].path, path_text);
        assert!(dirty.diagnostics[0].message.contains("while"));
        assert_eq!(
            &service
                .snapshot()
                .document(&path_text)
                .expect("dirty document")
                .text[dirty.diagnostics[0].range.clone()],
            "{ while (true) { return 1; } }"
        );

        service.close_document(&path_text);
        let restored = service.diagnostics();
        assert!(restored.revision > dirty.revision);
        assert!(restored.diagnostics.is_empty());
    }

    #[test]
    fn vscode_fixture_is_clean_under_full_workspace_check() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vscode-stasis/test/fixture")
            .canonicalize()
            .expect("VS Code fixture root");
        let path = root.join("src/main.stasis");
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(
            path.to_string_lossy(),
            include_str!("../../../vscode-stasis/test/fixture/src/main.stasis"),
        );

        let report = service.diagnostics();
        assert!(
            report.diagnostics.is_empty(),
            "fixture diagnostics: {:?}",
            report.diagnostics
        );
    }
}
