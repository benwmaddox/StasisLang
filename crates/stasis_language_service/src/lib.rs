#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use stasis_compiler::compiler::Compiler;
use stasis_compiler::frontend::lexer::{lex, Token, TokenKind};
use stasis_compiler::frontend::parser::completion_expected_type;
use stasis_compiler::frontend::workshop::{
    workshop_completion_items, workshop_reachable_files, workshop_source_items,
    WorkshopCompletionItem, WorkshopCompletionScope, WorkshopSourceFile, WorkshopSourceItem,
    WorkshopSourceItemKind,
};
use stasis_compiler::identity::canonical_source_path;
pub use stasis_runner::live::{
    CompletionContext, CompletionIndex, CompletionItem, CompletionQuery, CompletionScope,
    LiveSymbolTarget,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    pub range: Range<usize>,
    pub symbol: String,
    pub kind: String,
    pub type_name: Option<String>,
    pub owner: Option<String>,
    pub signatures: Vec<String>,
    pub documentation: Option<String>,
    pub live_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionTextChange {
    pub path: String,
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageCompletionItem {
    pub text: String,
    pub kind: String,
    pub detail: String,
    pub type_name: Option<String>,
    pub signature: Option<String>,
    pub documentation: Option<String>,
    pub insert_text: String,
    pub snippet: bool,
    pub additional_text_edits: Vec<CompletionTextChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageCompletion {
    pub replacement_start: usize,
    pub replacement_end: usize,
    pub truncated: bool,
    pub items: Vec<LanguageCompletionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureParameter {
    pub label: String,
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInformation {
    pub label: String,
    pub documentation: Option<String>,
    pub parameters: Vec<SignatureParameter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInformation>,
    pub active_signature: usize,
    pub active_parameter: usize,
}

struct LanguageIndex {
    revision: WorkspaceRevision,
    files: Vec<WorkshopSourceFile>,
    source_items: Vec<WorkshopSourceItem>,
    completion: CompletionIndex,
    workshop_items: Vec<WorkshopCompletionItem>,
}

#[derive(Clone, Default)]
pub struct LanguageCompletionSnapshot {
    source_items: Vec<WorkshopSourceItem>,
    source_files: Vec<WorkshopSourceFile>,
}

impl LanguageCompletionSnapshot {
    pub fn new(
        source_items: Vec<WorkshopSourceItem>,
        source_files: Vec<WorkshopSourceFile>,
    ) -> Self {
        Self {
            source_items,
            source_files,
        }
    }

    pub fn query_with_index(
        &self,
        mut index: CompletionIndex,
        buffer: &str,
        cursor: usize,
        limit: usize,
        context: &CompletionContext,
    ) -> CompletionQuery {
        let mut effective_context = context.clone();
        if effective_context.expected_type.is_none() {
            effective_context.expected_type =
                completion_expected_type(buffer, cursor).unwrap_or_default();
        }
        let overlay = if effective_context.owner.is_none() {
            overlay_document_completion_items(
                &self.source_items,
                &self.source_files,
                buffer,
                cursor,
                &mut effective_context,
            )
        } else {
            overlay_completion_items(
                &self.source_items,
                &self.source_files,
                buffer,
                &effective_context,
            )
        };
        if let Some(items) = overlay {
            index.retain(|item| !completion_item_belongs_to_context(item, &effective_context));
            index.extend(items.iter().map(shared_completion_item));
        }
        index.query_with_context(buffer, cursor, limit, &effective_context)
    }
}

pub struct LanguageService {
    documents: WorkspaceDocuments,
    compiler: Compiler,
    project_root: String,
    language_index: Option<LanguageIndex>,
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
            language_index: None,
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

    pub fn completion(
        &mut self,
        path: &str,
        byte_offset: usize,
        limit: usize,
    ) -> Result<LanguageCompletion, String> {
        let snapshot = self.documents.snapshot();
        let document = snapshot
            .document(path)
            .ok_or_else(|| format!("completion document is not indexed: '{path}'"))?;
        if byte_offset > document.text.len() || !document.text.is_char_boundary(byte_offset) {
            return Err(format!(
                "completion offset {byte_offset} is invalid for '{path}'"
            ));
        }
        let relative = canonical_source_path(Some(&self.project_root), path)?;
        let context = self.query_context(&relative, byte_offset, &document.text)?;
        let source = document.text.clone();
        let index = self.language_index()?;
        let query = index
            .completion
            .query_with_context(&source, byte_offset, limit, &context);
        let reachable = workshop_reachable_files(&index.files, Path::new(&relative))?
            .into_iter()
            .map(|file| file.path)
            .collect::<BTreeSet<_>>();
        let items = query
            .items
            .iter()
            .filter_map(|ranked| {
                let catalog = matching_workshop_completion(&index.workshop_items, ranked)?;
                let (insert_text, snippet) = completion_insert_text(catalog);
                Some(LanguageCompletionItem {
                    text: ranked.text.clone(),
                    kind: ranked.kind.clone(),
                    detail: ranked.detail.clone(),
                    type_name: ranked.type_name.clone(),
                    signature: catalog.signature.clone(),
                    documentation: documentation_for_completion(index, catalog),
                    insert_text,
                    snippet,
                    additional_text_edits: completion_import_edit(
                        path, &relative, &reachable, catalog,
                    )
                    .into_iter()
                    .collect(),
                })
            })
            .collect();
        Ok(LanguageCompletion {
            replacement_start: query.replacement_start,
            replacement_end: query.replacement_end,
            truncated: query.truncated,
            items,
        })
    }

    pub fn hover(&mut self, path: &str, byte_offset: usize) -> Result<Option<HoverInfo>, String> {
        let snapshot = self.documents.snapshot();
        let document = snapshot
            .document(path)
            .ok_or_else(|| format!("hover document is not indexed: '{path}'"))?;
        let Some(range) = identifier_path_range(&document.text, byte_offset) else {
            return Ok(None);
        };
        let symbol = document.text[range.clone()].to_string();
        let relative = canonical_source_path(Some(&self.project_root), path)?;
        let context = self.query_context(&relative, byte_offset, &document.text)?;
        let index = self.language_index()?;
        let mut matches = index
            .workshop_items
            .iter()
            .filter(|item| item.text == symbol && workshop_completion_visible(item, &context))
            .collect::<Vec<_>>();
        matches.sort_by_key(|item| workshop_completion_specificity(item, &context));
        let Some(primary) = matches.first().copied() else {
            return Ok(None);
        };
        let mut signatures = matches
            .iter()
            .filter_map(|item| item_signature(item))
            .collect::<Vec<_>>();
        signatures.sort();
        signatures.dedup();
        let documentation = documentation_for_completion(index, primary);
        Ok(Some(HoverInfo {
            range,
            symbol,
            kind: primary.kind.clone(),
            type_name: primary.type_name.clone(),
            owner: primary.owner.clone(),
            signatures,
            documentation,
            live_value: None,
        }))
    }

    pub fn signature_help(
        &mut self,
        path: &str,
        byte_offset: usize,
    ) -> Result<Option<SignatureHelp>, String> {
        let snapshot = self.documents.snapshot();
        let document = snapshot
            .document(path)
            .ok_or_else(|| format!("signature document is not indexed: '{path}'"))?;
        let Some(call) = call_context(&document.text, byte_offset)? else {
            return Ok(None);
        };
        let relative = canonical_source_path(Some(&self.project_root), path)?;
        let context = self.query_context(&relative, byte_offset, &document.text)?;
        let index = self.language_index()?;
        let mut signatures = index
            .workshop_items
            .iter()
            .filter(|item| item.text == call.target && workshop_completion_visible(item, &context))
            .filter_map(|item| {
                let label = item_signature(item)?;
                Some(SignatureInformation {
                    parameters: signature_parameters(&label),
                    documentation: documentation_for_completion(index, item),
                    label,
                })
            })
            .collect::<Vec<_>>();
        signatures.sort_by(|left, right| left.label.cmp(&right.label));
        signatures.dedup_by(|left, right| left.label == right.label);
        if signatures.is_empty() {
            return Ok(None);
        }
        let active_signature = signatures
            .iter()
            .position(|signature| call.active_parameter < signature.parameters.len())
            .unwrap_or(0);
        Ok(Some(SignatureHelp {
            signatures,
            active_signature,
            active_parameter: call.active_parameter,
        }))
    }

    fn language_index(&mut self) -> Result<&LanguageIndex, String> {
        let snapshot = self.documents.snapshot();
        if self
            .language_index
            .as_ref()
            .is_none_or(|index| index.revision != snapshot.revision())
        {
            let files = snapshot
                .documents()
                .map(|(path, document)| {
                    Ok(WorkshopSourceFile {
                        path: canonical_source_path(Some(&self.project_root), path)?,
                        source: document.text.to_string(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let source_items = workshop_source_items(&files)?;
            let workshop_items = workshop_completion_items(&files)?;
            let completion_items = workshop_items
                .iter()
                .map(shared_completion_item)
                .collect::<Vec<_>>();
            let mut completion = CompletionIndex::default();
            completion.replace(completion_items.clone());
            self.language_index = Some(LanguageIndex {
                revision: snapshot.revision(),
                files,
                source_items,
                completion,
                workshop_items,
            });
        }
        self.language_index
            .as_ref()
            .ok_or_else(|| "language index was not published".to_string())
    }

    fn query_context(
        &mut self,
        relative_path: &str,
        byte_offset: usize,
        source: &str,
    ) -> Result<CompletionContext, String> {
        let index = self.language_index()?;
        let owner = index
            .source_items
            .iter()
            .filter(|item| {
                item.file == relative_path && item.kind == WorkshopSourceItemKind::Function
            })
            .filter_map(|item| {
                let span = item.source_spans.first()?;
                ((span.start as usize) <= byte_offset && byte_offset <= span.end as usize)
                    .then_some((span.end.saturating_sub(span.start), item))
            })
            .min_by_key(|(width, _)| *width)
            .map(|(_, item)| item);
        Ok(CompletionContext {
            owner: owner.map(|item| item.name.clone()),
            file: Some(relative_path.to_string()),
            owner_signature: owner.map(|item| item.signature.clone()),
            source_offset: Some(byte_offset),
            expected_type: completion_expected_type(source, byte_offset).unwrap_or_default(),
        })
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

fn shared_completion_item(item: &WorkshopCompletionItem) -> CompletionItem {
    CompletionItem {
        text: item.text.clone(),
        kind: item.kind.clone(),
        detail: item.detail.clone(),
        type_name: item.type_name.clone(),
        source: Some(item.file.clone()),
        selector: item.signature.as_ref().map(|signature| LiveSymbolTarget {
            name: item.text.clone(),
            kind: Some(item.kind.clone()),
            file: Some(item.file.clone()),
            owner: item.owner.clone(),
            signature: Some(signature.clone()),
        }),
        scope: item.scope.as_ref().map(shared_completion_scope),
    }
}

fn matching_workshop_completion<'a>(
    items: &'a [WorkshopCompletionItem],
    ranked: &CompletionItem,
) -> Option<&'a WorkshopCompletionItem> {
    items.iter().find(|item| {
        item.text == ranked.text
            && item.kind == ranked.kind
            && item.detail == ranked.detail
            && ranked.source.as_deref() == Some(item.file.as_str())
            && item.type_name == ranked.type_name
            && item.scope.as_ref().map(shared_completion_scope) == ranked.scope
    })
}

fn completion_insert_text(item: &WorkshopCompletionItem) -> (String, bool) {
    if !matches!(item.kind.as_str(), "function" | "method" | "test") {
        return (item.text.clone(), false);
    }
    let Some(signature) = item.signature.as_deref() else {
        return (item.text.clone(), false);
    };
    let parameters = signature_parameters(signature);
    let parameters = if item.kind == "method"
        && item.text.contains('.')
        && parameters
            .first()
            .is_some_and(|parameter| parameter.label.starts_with("self:"))
    {
        &parameters[1..]
    } else {
        &parameters
    };
    let arguments = parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let name = parameter
                .label
                .split(':')
                .next()
                .unwrap_or(&parameter.label)
                .trim();
            format!("${{{}:{}}}", index + 1, name)
        })
        .collect::<Vec<_>>()
        .join(", ");
    (format!("{}({arguments})", item.text), true)
}

fn completion_import_edit(
    absolute_path: &str,
    relative_path: &str,
    reachable: &BTreeSet<String>,
    item: &WorkshopCompletionItem,
) -> Option<CompletionTextChange> {
    if item.file == relative_path
        || reachable.contains(&item.file)
        || item.scope.is_some()
        || !matches!(
            item.kind.as_str(),
            "function" | "struct" | "enum" | "global" | "constant"
        )
    {
        return None;
    }
    let import = relative_import_path(relative_path, &item.file)?;
    Some(CompletionTextChange {
        path: absolute_path.to_string(),
        range: 0..0,
        text: format!("import \"{import}\";\n"),
    })
}

fn relative_import_path(from_file: &str, target_file: &str) -> Option<String> {
    let from = Path::new(from_file)
        .parent()?
        .components()
        .collect::<Vec<_>>();
    let target = Path::new(target_file).components().collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let mut components = vec!["..".to_string(); from.len().saturating_sub(common)];
    components.extend(
        target[common..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy().to_string()),
    );
    (!components.is_empty()).then(|| components.join("/"))
}

fn shared_completion_scope(scope: &WorkshopCompletionScope) -> CompletionScope {
    CompletionScope {
        owner: scope.owner.clone(),
        file: scope.file.clone(),
        owner_signature: scope.owner_signature.clone(),
        owner_end: scope.owner_end,
        visible_from: scope.visible_from,
        visible_to: scope.visible_to,
    }
}

fn overlay_document_completion_items(
    source_items: &[WorkshopSourceItem],
    source_files: &[WorkshopSourceFile],
    buffer: &str,
    cursor: usize,
    context: &mut CompletionContext,
) -> Option<Vec<WorkshopCompletionItem>> {
    let file = context.file.as_deref()?;
    let mut files = source_files.to_vec();
    let mut cursor = cursor.min(buffer.len());
    while !buffer.is_char_boundary(cursor) {
        cursor = cursor.saturating_sub(1);
    }
    let source_index = files.iter().position(|source| source.path == file)?;
    files[source_index].source = buffer.to_string();
    let owner = workshop_source_items(&files)
        .ok()?
        .into_iter()
        .filter(|item| item.kind == WorkshopSourceItemKind::Function && item.file == file)
        .filter_map(|item| {
            let span = item.source_spans.first()?;
            let start = span.start as usize;
            let end = span.end as usize;
            (start <= cursor && cursor <= end).then_some((end.saturating_sub(start), start, item))
        })
        .min_by_key(|(width, _, _)| *width);
    let (_, dirty_start, dirty_owner) = owner?;
    let accepted_owner = source_items
        .iter()
        .find(|item| {
            item.kind == WorkshopSourceItemKind::Function
                && item.file == file
                && item.name == dirty_owner.name
                && item.signature == dirty_owner.signature
        })
        .or_else(|| {
            let mut matches = source_items.iter().filter(|item| {
                item.kind == WorkshopSourceItemKind::Function
                    && item.file == file
                    && item.name == dirty_owner.name
            });
            let first = matches.next()?;
            matches.next().is_none().then_some(first)
        })?;
    let accepted_start = accepted_owner.source_spans.first()?.start as usize;
    let definition = buffer.get(dirty_start..cursor)?;
    context.owner = Some(accepted_owner.name.clone());
    context.owner_signature = Some(accepted_owner.signature.clone());
    context.source_offset = Some(accepted_start.saturating_add(definition.len()));
    overlay_completion_items(source_items, source_files, definition, context)
}

fn overlay_completion_items(
    source_items: &[WorkshopSourceItem],
    source_files: &[WorkshopSourceFile],
    buffer: &str,
    context: &CompletionContext,
) -> Option<Vec<WorkshopCompletionItem>> {
    let file = context.file.as_deref()?;
    let owner = context.owner.as_deref()?;
    let item = source_items.iter().find(|item| {
        item.file == file
            && item.name == owner
            && context
                .owner_signature
                .as_deref()
                .is_none_or(|signature| item.signature == signature)
    })?;
    let span = item.source_spans.first()?;
    let mut files = source_files.to_vec();
    let source_file = files.iter_mut().find(|source| source.path == file)?;
    let start = span.start as usize;
    let end = span.end as usize;
    if start > end || end > source_file.source.len() {
        return None;
    }
    let overlay = balanced_definition(buffer)?;
    source_file.source.replace_range(start..end, &overlay);
    let mut items = workshop_completion_items(&files).ok()?;
    for completion in &mut items {
        if let Some(scope) = completion
            .scope
            .as_mut()
            .filter(|scope| scope.owner == owner && scope.file == file)
        {
            scope.owner_signature = context.owner_signature.clone();
        }
    }
    Some(items)
}

fn completion_item_belongs_to_context(item: &CompletionItem, context: &CompletionContext) -> bool {
    let Some(scope) = item.scope.as_ref() else {
        return false;
    };
    scope.owner == context.owner.as_deref().unwrap_or_default()
        && scope.file == context.file.as_deref().unwrap_or_default()
        && context
            .owner_signature
            .as_deref()
            .is_none_or(|signature| scope.owner_signature.as_deref() == Some(signature))
}

fn balanced_definition(source: &str) -> Option<String> {
    let tokens = lex(source).ok()?;
    let mut depth = 0usize;
    for token in tokens {
        match token.kind {
            TokenKind::LBrace => depth = depth.saturating_add(1),
            TokenKind::RBrace => depth = depth.checked_sub(1)?,
            _ => {}
        }
    }
    let mut balanced = source.to_string();
    for _ in 0..depth {
        balanced.push('}');
    }
    Some(balanced)
}

fn workshop_completion_visible(item: &WorkshopCompletionItem, context: &CompletionContext) -> bool {
    let Some(scope) = item.scope.as_ref() else {
        return true;
    };
    context.owner.as_deref() == Some(scope.owner.as_str())
        && context.file.as_deref() == Some(scope.file.as_str())
        && context
            .owner_signature
            .as_deref()
            .is_none_or(|signature| scope.owner_signature.as_deref() == Some(signature))
        && context
            .source_offset
            .is_some_and(|offset| scope.visible_from <= offset && offset <= scope.visible_to)
}

fn workshop_completion_specificity<'a>(
    item: &'a WorkshopCompletionItem,
    context: &CompletionContext,
) -> (u8, usize, &'a str) {
    match item.scope.as_ref() {
        Some(scope) => (
            0,
            context
                .source_offset
                .unwrap_or(scope.visible_from)
                .saturating_sub(scope.visible_from),
            item.detail.as_str(),
        ),
        None => (1, usize::MAX, item.detail.as_str()),
    }
}

fn item_signature(item: &WorkshopCompletionItem) -> Option<String> {
    item.signature
        .clone()
        .filter(|signature| signature.contains('('))
}

fn documentation_for_completion(
    index: &LanguageIndex,
    item: &WorkshopCompletionItem,
) -> Option<String> {
    let declaration_name = item.text.rsplit('.').next().unwrap_or(&item.text);
    index
        .source_items
        .iter()
        .filter(|source| source.file == item.file && source.name == declaration_name)
        .filter(|source| {
            item.owner
                .as_deref()
                .is_none_or(|owner| source.owner.as_deref() == Some(owner))
        })
        .filter(|source| {
            item.signature
                .as_deref()
                .is_none_or(|signature| source.signature == signature)
        })
        .find_map(|source| leading_documentation(&source.source))
}

fn leading_documentation(source: &str) -> Option<String> {
    let lines = source.lines().collect::<Vec<_>>();
    let first_declaration = lines
        .iter()
        .position(|line| !line.trim().is_empty() && !line.trim_start().starts_with("//"))?;
    let documentation = lines[..first_declaration]
        .iter()
        .filter_map(|line| line.trim_start().strip_prefix("//"))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    (!documentation.is_empty()).then_some(documentation)
}

fn identifier_path_range(source: &str, byte_offset: usize) -> Option<Range<usize>> {
    if byte_offset > source.len() || !source.is_char_boundary(byte_offset) {
        return None;
    }
    let tokens = lex(source).ok()?;
    let mut index = tokens.iter().position(|token| {
        token.kind == TokenKind::Identifier
            && (token.start <= byte_offset && byte_offset <= token.end)
    })?;
    let mut right_index = index;
    let mut start = tokens[index].start;
    let mut end = tokens[index].end;
    while index >= 2
        && tokens[index - 1].kind == TokenKind::Other
        && token_text(source, tokens[index - 1]) == "."
        && tokens[index - 2].kind == TokenKind::Identifier
    {
        index -= 2;
        start = tokens[index].start;
    }
    while right_index + 2 < tokens.len()
        && tokens[right_index + 1].kind == TokenKind::Other
        && token_text(source, tokens[right_index + 1]) == "."
        && tokens[right_index + 2].kind == TokenKind::Identifier
    {
        right_index += 2;
        end = tokens[right_index].end;
    }
    Some(start..end)
}

struct CallContext {
    target: String,
    active_parameter: usize,
}

fn call_context(source: &str, byte_offset: usize) -> Result<Option<CallContext>, String> {
    if byte_offset > source.len() || !source.is_char_boundary(byte_offset) {
        return Err(format!("signature offset {byte_offset} is invalid"));
    }
    let prefix = &source[..byte_offset];
    let tokens = lex(prefix)?;
    let mut open_calls = Vec::<usize>::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LParen => open_calls.push(index),
            TokenKind::RParen => {
                open_calls.pop();
            }
            _ => {}
        }
    }
    let Some(open_index) = open_calls.last().copied() else {
        return Ok(None);
    };
    let Some(target_end_index) = open_index.checked_sub(1) else {
        return Ok(None);
    };
    if tokens[target_end_index].kind != TokenKind::Identifier {
        return Ok(None);
    }
    let mut target_start_index = target_end_index;
    while target_start_index >= 2
        && tokens[target_start_index - 1].kind == TokenKind::Other
        && token_text(prefix, tokens[target_start_index - 1]) == "."
        && tokens[target_start_index - 2].kind == TokenKind::Identifier
    {
        target_start_index -= 2;
    }
    let mut nested_parentheses = 0usize;
    let mut active_parameter = 0usize;
    for token in &tokens[open_index + 1..] {
        match token.kind {
            TokenKind::LParen => nested_parentheses = nested_parentheses.saturating_add(1),
            TokenKind::RParen => nested_parentheses = nested_parentheses.saturating_sub(1),
            TokenKind::Comma if nested_parentheses == 0 => {
                active_parameter = active_parameter.saturating_add(1);
            }
            _ => {}
        }
    }
    Ok(Some(CallContext {
        target: prefix[tokens[target_start_index].start..tokens[target_end_index].end].to_string(),
        active_parameter,
    }))
}

fn signature_parameters(signature: &str) -> Vec<SignatureParameter> {
    let Some(start) = signature.find('(') else {
        return Vec::new();
    };
    let Some(end) = signature[start + 1..].find(')').map(|end| start + 1 + end) else {
        return Vec::new();
    };
    signature[start + 1..end]
        .split(',')
        .map(str::trim)
        .filter(|parameter| !parameter.is_empty())
        .map(|parameter| SignatureParameter {
            label: parameter.to_string(),
            documentation: None,
        })
        .collect()
}

fn token_text(source: &str, token: Token) -> &str {
    &source[token.start..token.end]
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
    use std::time::Instant;

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

    fn intelligence_service() -> (LanguageService, String, String) {
        let root = std::env::temp_dir().join("stasis-language-service-intelligence");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let source = r#"struct Enemy { hp: i32; }

// Creates an enemy with explicit health.
function spawn_enemy(count: i32, health: i32): Enemy {
    let enemy: Enemy;
    enemy.hp = health;
    return enemy;
}

function main(): i32 {
    let foe: Enemy;
    foe.hp = 3;
    spawn_enemy(1, foe.hp);
    return foe.hp;
}
"#
        .to_string();
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(path_text.clone(), source.clone());
        (service, path_text, source)
    }

    #[test]
    fn completion_uses_typed_dirty_snapshot_scope_and_replacement_range() {
        let (mut service, path, source) = intelligence_service();
        let cursor = source.find("foe.hp = 3").expect("field expression") + "foe.h".len();
        let completion = service.completion(&path, cursor, 64).expect("completion");
        let field = completion
            .items
            .iter()
            .find(|item| item.text == "foe.hp")
            .expect("typed field completion");
        assert_eq!(field.kind, "field");
        assert_eq!(field.type_name.as_deref(), Some("i32"));
        assert_eq!(&source[completion.replacement_start..cursor], "foe.h");

        let function_cursor =
            source.find("spawn_enemy(1").expect("function call") + "spawn_e".len();
        let functions = service
            .completion(&path, function_cursor, 64)
            .expect("function completion");
        let function = functions
            .items
            .iter()
            .find(|item| item.text == "spawn_enemy")
            .expect("function completion item");
        assert_eq!(function.insert_text, "spawn_enemy(${1:count}, ${2:health})");
        assert!(function.snippet);
        assert_eq!(
            function.documentation.as_deref(),
            Some("Creates an enemy with explicit health.")
        );
    }

    #[test]
    fn completion_adds_import_for_unreachable_workspace_symbol() {
        let root = std::env::temp_dir().join("stasis-language-service-auto-import");
        let main_path = root.join("src/main.stasis");
        let helper_path = root.join("src/helper.stasis");
        let main_text = main_path.to_string_lossy().replace('\\', "/");
        let source = "function main(): i32 { return hel; }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(main_text.clone(), source);
        service.set_disk_document(
            helper_path.to_string_lossy(),
            "function helper(): i32 { return 1; }\n",
        );

        let cursor = source.find("hel").expect("query") + 3;
        let completion = service
            .completion(&main_text, cursor, 64)
            .expect("completion");
        let helper = completion
            .items
            .iter()
            .find(|item| item.text == "helper")
            .expect("helper completion");
        assert_eq!(helper.additional_text_edits.len(), 1);
        assert_eq!(helper.additional_text_edits[0].range, 0..0);
        assert_eq!(
            helper.additional_text_edits[0].text,
            "import \"helper.stasis\";\n"
        );
    }

    #[test]
    fn hover_reports_inferred_type_owner_signature_and_documentation() {
        let (mut service, path, source) = intelligence_service();
        let field_start = source.rfind("foe.hp").expect("field hover");
        let field = service
            .hover(&path, field_start + 5)
            .expect("hover")
            .expect("field information");
        assert_eq!(field.symbol, "foe.hp");
        assert_eq!(field.kind, "field");
        assert_eq!(field.type_name.as_deref(), Some("i32"));
        assert_eq!(field.owner.as_deref(), Some("Enemy"));

        let function_start = source.find("spawn_enemy(1").expect("function call");
        let function = service
            .hover(&path, function_start + 2)
            .expect("hover")
            .expect("function information");
        assert_eq!(
            function.signatures,
            vec!["spawn_enemy(count: i32, health: i32): Enemy"]
        );
        assert_eq!(
            function.documentation.as_deref(),
            Some("Creates an enemy with explicit health.")
        );
    }

    #[test]
    fn signature_help_tracks_active_parameter() {
        let (mut service, path, source) = intelligence_service();
        let cursor = source.find("spawn_enemy(1, foe").expect("call") + "spawn_enemy(1, ".len();
        let help = service
            .signature_help(&path, cursor)
            .expect("signature help")
            .expect("call signature");
        assert_eq!(help.active_parameter, 1);
        assert_eq!(help.active_signature, 0);
        assert_eq!(
            help.signatures[0].label,
            "spawn_enemy(count: i32, health: i32): Enemy"
        );
        assert_eq!(help.signatures[0].parameters[1].label, "health: i32");
        assert_eq!(
            help.signatures[0].documentation.as_deref(),
            Some("Creates an enemy with explicit health.")
        );
    }

    #[test]
    fn warm_intelligence_queries_meet_local_latency_contract() {
        let (mut service, path, source) = intelligence_service();
        let completion_offset = source.find("spawn_enemy(1").expect("completion") + 7;
        let hover_offset = source.rfind("foe.hp").expect("hover") + 5;
        let signature_offset =
            source.find("spawn_enemy(1, foe").expect("signature") + "spawn_enemy(1, ".len();
        service
            .completion(&path, completion_offset, 64)
            .expect("warm completion");
        service.hover(&path, hover_offset).expect("warm hover");
        service
            .signature_help(&path, signature_offset)
            .expect("warm signature");

        let mut completion_micros = Vec::new();
        let mut hover_micros = Vec::new();
        let mut signature_micros = Vec::new();
        for _ in 0..50 {
            let started = Instant::now();
            service
                .completion(&path, completion_offset, 64)
                .expect("completion");
            completion_micros.push(started.elapsed().as_micros());

            let started = Instant::now();
            service.hover(&path, hover_offset).expect("hover");
            hover_micros.push(started.elapsed().as_micros());

            let started = Instant::now();
            service
                .signature_help(&path, signature_offset)
                .expect("signature");
            signature_micros.push(started.elapsed().as_micros());
        }
        completion_micros.sort_unstable();
        hover_micros.sort_unstable();
        signature_micros.sort_unstable();
        let p95 = |samples: &[u128]| samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
        let completion_p95 = p95(&completion_micros);
        let hover_p95 = p95(&hover_micros);
        let signature_p95 = p95(&signature_micros);
        eprintln!(
            "warm p95: completion={completion_p95}us hover={hover_p95}us signature={signature_p95}us"
        );
        assert!(completion_p95 < 20_000, "completion p95 {completion_p95}us");
        assert!(hover_p95 < 30_000, "hover p95 {hover_p95}us");
        assert!(signature_p95 < 30_000, "signature p95 {signature_p95}us");
    }
}
