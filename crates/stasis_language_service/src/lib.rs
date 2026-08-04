#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use stasis_compiler::compiler::Compiler;
use stasis_compiler::frontend::formatter::format_source;
use stasis_compiler::frontend::lexer::{lex, Token, TokenKind};
use stasis_compiler::frontend::parser::completion_expected_type;
use stasis_compiler::frontend::workshop::{
    find_workshop_references, organize_workshop_imports, plan_workshop_rename,
    prepare_workshop_rename, workshop_call_hierarchy, workshop_completion_items,
    workshop_folding_ranges, workshop_inlay_hints, workshop_inlay_hints_from_local_types,
    workshop_linked_edit_ranges, workshop_reachable_files, workshop_selection_ranges,
    workshop_semantic_tokens, workshop_source_items, workshop_symbols, workshop_type_hierarchy,
    WorkshopCallHierarchyEdge, WorkshopCompletionItem, WorkshopCompletionScope,
    WorkshopHierarchyItem, WorkshopInlayHint, WorkshopInlayHintKind, WorkshopSourceFile,
    WorkshopSourceItem, WorkshopSourceItemKind, WorkshopSymbol, WorkshopSymbolKind,
    WorkshopTypeHierarchyEdge,
};
pub use stasis_compiler::frontend::workshop::{
    workshop_source_hash, WorkshopReference, WorkshopReferenceKind,
};
use stasis_compiler::identity::canonical_source_path;
use stasis_compiler::SourceDiagnosticEdit;
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

    pub fn from_raw(value: u64) -> Self {
        Self(value)
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
    pub code: String,
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
pub struct LiveObservation {
    pub path: String,
    pub type_name: Option<String>,
    pub value: String,
    pub tick: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveObservationBatch {
    pub session_id: String,
    pub generation: u64,
    pub source_hashes: BTreeMap<String, String>,
    pub observations: Vec<LiveObservation>,
    pub indexed_collections: Vec<LiveIndexedCollection>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveIndexedCollection {
    pub path: String,
    pub fields: BTreeMap<String, String>,
}

#[derive(Default)]
pub struct LiveSessionBroker {
    latest: Option<LiveObservationBatch>,
}

impl LiveSessionBroker {
    pub fn publish(&mut self, mut batch: LiveObservationBatch) {
        batch.observations.truncate(512);
        self.latest = Some(batch);
    }

    pub fn clear(&mut self) {
        self.latest = None;
    }

    fn compatible_value(
        &self,
        path: &str,
        type_name: Option<&str>,
        owner_file: &str,
        owner_source: &str,
    ) -> Option<String> {
        let batch = self.latest.as_ref()?;
        if !batch.complete {
            return None;
        }
        let accepted_hash = batch.source_hashes.get(owner_file)?;
        if accepted_hash != &workshop_source_hash(owner_source) {
            return None;
        }
        batch
            .observations
            .iter()
            .find(|observation| {
                observation.path == path
                    && observation
                        .type_name
                        .as_deref()
                        .zip(type_name)
                        .is_none_or(|(observed, indexed)| observed == indexed)
            })
            .map(|observation| format!("{} (tick {})", observation.value, observation.tick))
    }

    fn indexed_completion_items(
        &self,
        files: &[WorkshopSourceFile],
        source: &str,
        cursor: usize,
    ) -> Vec<CompletionItem> {
        let Some(batch) = self.latest.as_ref() else {
            return Vec::new();
        };
        if !batch.complete {
            return Vec::new();
        }
        if batch.source_hashes.is_empty()
            || batch.source_hashes.iter().any(|(path, accepted_hash)| {
                if path.starts_with(".stasis_cache/toolchain/") {
                    return false;
                }
                files
                    .iter()
                    .find(|file| file.path == *path)
                    .is_none_or(|file| &workshop_source_hash(&file.source) != accepted_hash)
            })
        {
            return Vec::new();
        }
        let Some((collection_path, receiver)) = indexed_completion_receiver(source, cursor) else {
            return Vec::new();
        };
        let Some(collection) = batch
            .indexed_collections
            .iter()
            .find(|collection| collection.path == collection_path)
        else {
            return Vec::new();
        };
        collection
            .fields
            .iter()
            .map(|(field, type_name)| CompletionItem {
                text: format!("{receiver}{field}"),
                kind: "field".to_string(),
                detail: format!("{type_name} via indexed state collection {collection_path}"),
                type_name: Some(type_name.clone()),
                source: Some("runtime state layout".to_string()),
                selector: None,
                scope: None,
            })
            .collect()
    }
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
    pub resolve_data: Option<CompletionResolveData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionResolveData {
    pub revision: WorkspaceRevision,
    pub catalog_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageCompletionResolution {
    pub documentation: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageLocation {
    pub path: String,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageSymbolKind {
    Struct,
    Function,
    Global,
    Constant,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageSymbol {
    pub name: String,
    pub kind: LanguageSymbolKind,
    pub detail: String,
    pub container_name: Option<String>,
    pub location: LanguageLocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamePreparation {
    pub range: Range<usize>,
    pub placeholder: String,
    pub kind: String,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameEdit {
    pub path: String,
    pub range: Range<usize>,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamePlan {
    pub revision: WorkspaceRevision,
    pub old_name: String,
    pub new_name: String,
    pub kind: String,
    pub owner: Option<String>,
    pub edits: Vec<RenameEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageCodeAction {
    pub title: String,
    pub kind: String,
    pub preferred: bool,
    pub diagnostic_code: Option<String>,
    pub edits: Vec<RenameEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageSemanticToken {
    pub range: Range<usize>,
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageInlayHintKind {
    Type,
    Parameter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageInlayHint {
    pub position: usize,
    pub anchor: Range<usize>,
    pub kind: LanguageInlayHintKind,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageFoldingRange {
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageTextEdit {
    pub range: Range<usize>,
    pub new_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageHierarchyKind {
    Function,
    Struct,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageHierarchyItem {
    pub symbol_id: String,
    pub name: String,
    pub detail: String,
    pub kind: LanguageHierarchyKind,
    pub location: LanguageLocation,
    pub selection_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageCallHierarchyRelation {
    pub item: LanguageHierarchyItem,
    pub from_ranges: Vec<LanguageLocation>,
}

struct LanguageIndex {
    revision: WorkspaceRevision,
    files: Vec<WorkshopSourceFile>,
    source_items: Vec<WorkshopSourceItem>,
    completion: CompletionIndex,
    workshop_items: Vec<WorkshopCompletionItem>,
    symbols: Vec<WorkshopSymbol>,
    definitions: WarmDefinitionIndex,
}

#[derive(Default)]
struct WarmDefinitionIndex {
    simple: BTreeMap<String, Vec<WorkshopReference>>,
    root_types: BTreeMap<String, String>,
    root_dependencies: BTreeMap<String, WarmDefinitionDependency>,
    fields: BTreeMap<(String, String), (String, WorkshopReference)>,
    field_dependencies: BTreeMap<(String, String), WarmDefinitionDependency>,
    struct_names: BTreeSet<String>,
}

struct WarmDefinitionResolution {
    references: Vec<WorkshopReference>,
    dependencies: Vec<WarmDefinitionDependency>,
}

#[derive(Clone)]
struct WarmDefinitionDependency {
    file: String,
    source_spans: Vec<stasis_compiler::frontend::workshop::WorkshopSourceSpan>,
}

impl WarmDefinitionIndex {
    fn build(
        files: &[WorkshopSourceFile],
        source_items: &[WorkshopSourceItem],
        completion_items: &[WorkshopCompletionItem],
    ) -> Result<Self, String> {
        let mut index = Self::default();
        let mut field_types = BTreeMap::<(String, String), String>::new();
        for item in completion_items {
            match item.kind.as_str() {
                "global" | "constant" => {
                    if let Some(type_name) = item.type_name.as_ref() {
                        index
                            .root_types
                            .entry(item.text.clone())
                            .or_insert_with(|| type_name.clone());
                    }
                }
                "field" if !item.text.contains('.') => {
                    if let (Some(owner), Some(type_name)) =
                        (item.owner.as_ref(), item.type_name.as_ref())
                    {
                        field_types
                            .entry((owner.clone(), item.text.clone()))
                            .or_insert_with(|| type_name.clone());
                    }
                }
                "struct" => {
                    index.struct_names.insert(item.text.clone());
                }
                _ => {}
            }
        }

        for file in files {
            let tokens = lex(&file.source)?;
            for (token_index, token) in tokens.iter().copied().enumerate() {
                if token.kind != TokenKind::Identifier {
                    continue;
                }
                let name = &file.source[token.start..token.end];
                let containing = source_items
                    .iter()
                    .filter(|item| {
                        item.file == file.path
                            && item.source_spans.iter().any(|span| {
                                span.start as usize <= token.start && token.end <= span.end as usize
                            })
                    })
                    .min_by_key(|item| {
                        item.source_spans
                            .iter()
                            .map(|span| span.end.saturating_sub(span.start))
                            .min()
                            .unwrap_or(u32::MAX)
                    });
                let previous = token_index
                    .checked_sub(1)
                    .and_then(|index| tokens.get(index).copied())
                    .map(|token| &file.source[token.start..token.end]);
                let simple_kind = match previous {
                    Some("function") => Some(WorkshopSourceItemKind::Function),
                    Some("test") => Some(WorkshopSourceItemKind::Test),
                    Some("struct") => Some(WorkshopSourceItemKind::Struct),
                    Some("global" | "const") => Some(WorkshopSourceItemKind::Globals),
                    _ => None,
                };
                if let Some(kind) = simple_kind {
                    if let Some(container) = containing.filter(|item| item.kind == kind) {
                        index
                            .simple
                            .entry(name.to_string())
                            .or_default()
                            .push(definition_reference(file, token, container, name)?);
                        if kind == WorkshopSourceItemKind::Globals {
                            index
                                .root_dependencies
                                .entry(name.to_string())
                                .or_insert_with(|| WarmDefinitionDependency {
                                    file: file.path.clone(),
                                    source_spans: container.source_spans.clone(),
                                });
                        }
                    }
                }

                if tokens
                    .get(token_index + 1)
                    .is_some_and(|next| next.kind == TokenKind::Colon)
                {
                    if let Some(container) =
                        containing.filter(|item| item.kind == WorkshopSourceItemKind::Struct)
                    {
                        let key = (container.name.clone(), name.to_string());
                        if let Some(type_name) = field_types.get(&key) {
                            if !index.fields.contains_key(&key) {
                                index.fields.insert(
                                    key.clone(),
                                    (
                                        type_name.clone(),
                                        definition_reference(file, token, container, name)?,
                                    ),
                                );
                                index.field_dependencies.insert(
                                    key,
                                    WarmDefinitionDependency {
                                        file: file.path.clone(),
                                        source_spans: container.source_spans.clone(),
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
        Ok(index)
    }

    fn definition(&self, symbol: &str) -> Option<WarmDefinitionResolution> {
        let segments = symbol.split('.').collect::<Vec<_>>();
        if let [name] = segments.as_slice() {
            return Some(WarmDefinitionResolution {
                references: self.simple.get(*name)?.clone(),
                dependencies: Vec::new(),
            });
        }
        let root = *segments.first()?;
        let mut dependencies = Vec::new();
        let mut type_name = if let Some(type_name) = self.root_types.get(root) {
            if let Some(dependency) = self.root_dependencies.get(root) {
                dependencies.push(dependency.clone());
            }
            type_name.clone()
        } else if self.struct_names.contains(root) {
            root.to_string()
        } else {
            return None;
        };
        let mut definition = None;
        for field_name in &segments[1..] {
            let owner = type_name
                .split('[')
                .next()
                .unwrap_or(&type_name)
                .trim()
                .to_string();
            let (field_type, reference) = self
                .fields
                .get(&(owner.clone(), (*field_name).to_string()))?;
            type_name = field_type.clone();
            if let Some(dependency) = self
                .field_dependencies
                .get(&(owner, (*field_name).to_string()))
            {
                dependencies.push(dependency.clone());
            }
            definition = Some(reference.clone());
        }
        Some(WarmDefinitionResolution {
            references: vec![definition?],
            dependencies,
        })
    }
}

fn definition_reference(
    file: &WorkshopSourceFile,
    token: Token,
    container: &WorkshopSourceItem,
    symbol: &str,
) -> Result<WorkshopReference, String> {
    Ok(WorkshopReference {
        symbol: symbol.to_string(),
        kind: WorkshopReferenceKind::Definition,
        file: file.path.clone(),
        source_span: stasis_compiler::frontend::workshop::WorkshopSourceSpan {
            start: u32::try_from(token.start)
                .map_err(|_| "definition start exceeds u32".to_string())?,
            end: u32::try_from(token.end).map_err(|_| "definition end exceeds u32".to_string())?,
        },
        containing_kind: container.kind,
        containing_name: container.name.clone(),
        containing_signature: container.signature.clone(),
        containing_source_hash: container.source_hash.clone(),
    })
}

struct LanguageHierarchyIndex {
    revision: WorkspaceRevision,
    files: Vec<WorkshopSourceFile>,
    call_hierarchy_items: Vec<WorkshopHierarchyItem>,
    call_hierarchy_edges: Vec<WorkshopCallHierarchyEdge>,
    type_hierarchy_items: Vec<WorkshopHierarchyItem>,
    type_hierarchy_edges: Vec<WorkshopTypeHierarchyEdge>,
}

struct LanguageInlayIndex {
    revision: WorkspaceRevision,
    files: Vec<WorkshopSourceFile>,
    hints: Vec<WorkshopInlayHint>,
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

#[derive(Clone, Default)]
pub struct LanguageNavigationSnapshot {
    files: Vec<WorkshopSourceFile>,
}

impl LanguageNavigationSnapshot {
    pub fn new(files: Vec<WorkshopSourceFile>) -> Self {
        Self { files }
    }

    pub fn references(&self, symbol: &str, limit: usize) -> Result<Vec<WorkshopReference>, String> {
        find_workshop_references(&self.files, symbol, limit)
    }
}

pub struct LanguageService {
    documents: WorkspaceDocuments,
    compiler: Compiler,
    project_root: String,
    language_index: Option<LanguageIndex>,
    language_index_error: Option<(WorkspaceRevision, String)>,
    hierarchy_index: Option<LanguageHierarchyIndex>,
    hierarchy_index_error: Option<(WorkspaceRevision, String)>,
    inlay_index: Option<LanguageInlayIndex>,
    inlay_index_error: Option<(WorkspaceRevision, String)>,
    live: LiveSessionBroker,
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
            language_index_error: None,
            hierarchy_index: None,
            hierarchy_index_error: None,
            inlay_index: None,
            inlay_index_error: None,
            live: LiveSessionBroker::default(),
        })
    }

    pub fn snapshot(&self) -> WorkspaceSnapshot {
        self.documents.snapshot()
    }

    pub fn publish_live_observations(&mut self, batch: LiveObservationBatch) {
        self.live.publish(batch);
    }

    pub fn clear_live_observations(&mut self) {
        self.live.clear();
    }

    pub fn warm_navigation_cache(&mut self) -> Result<(), String> {
        self.language_index().map(|_| ())
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
            Ok(_) => {
                let files = snapshot
                    .documents()
                    .filter_map(|(path, document)| {
                        canonical_source_path(Some(&self.project_root), path)
                            .ok()
                            .map(|path| WorkshopSourceFile {
                                path,
                                source: document.text.to_string(),
                            })
                    })
                    .collect::<Vec<_>>();
                if files.len() == snapshot.documents().len() {
                    if let Ok(locals) = self.compiler.indexed_local_types() {
                        if let Ok(hints) = workshop_inlay_hints_from_local_types(&files, &locals) {
                            self.inlay_index = Some(LanguageInlayIndex {
                                revision: snapshot.revision(),
                                files,
                                hints,
                            });
                            self.inlay_index_error = None;
                        }
                    }
                }
                Vec::new()
            }
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
                    code: diagnostic
                        .map(|diagnostic| diagnostic.code.as_str().to_string())
                        .unwrap_or_else(|| "stasis.generic".to_string()),
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
        let files = self.language_index()?.files.clone();
        let live_items = self
            .live
            .indexed_completion_items(&files, &document.text, byte_offset);
        let source = document.text.clone();
        let index = self.language_index()?;
        let mut completion_index = index.completion.clone();
        completion_index.extend(live_items);
        let query = completion_index.query_with_context(&source, byte_offset, limit, &context);
        let items = query
            .items
            .iter()
            .filter_map(|ranked| {
                let Some((catalog_index, catalog)) =
                    matching_workshop_completion(&index.workshop_items, ranked)
                else {
                    return Some(LanguageCompletionItem {
                        text: ranked.text.clone(),
                        kind: ranked.kind.clone(),
                        detail: ranked.detail.clone(),
                        type_name: ranked.type_name.clone(),
                        signature: None,
                        documentation: None,
                        insert_text: ranked.text.clone(),
                        snippet: false,
                        additional_text_edits: Vec::new(),
                        resolve_data: None,
                    });
                };
                let (insert_text, snippet) =
                    completion_insert_text(catalog, &source, query.replacement_end);
                Some(LanguageCompletionItem {
                    text: ranked.text.clone(),
                    kind: ranked.kind.clone(),
                    detail: ranked.detail.clone(),
                    type_name: ranked.type_name.clone(),
                    signature: catalog.signature.clone(),
                    documentation: None,
                    insert_text,
                    snippet,
                    additional_text_edits: Vec::new(),
                    resolve_data: Some(CompletionResolveData {
                        revision: index.revision,
                        catalog_index,
                    }),
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

    pub fn resolve_completion(
        &mut self,
        path: &str,
        data: CompletionResolveData,
    ) -> Result<Option<LanguageCompletionResolution>, String> {
        let relative = canonical_source_path(Some(&self.project_root), path)?;
        let current_revision = self.documents.snapshot().revision();
        let index = self.language_index()?;
        if index.revision != data.revision {
            return Ok(None);
        }
        let Some(item) = index.workshop_items.get(data.catalog_index) else {
            return Ok(None);
        };
        let documentation = documentation_for_completion(index, item);
        let additional_text_edits = if current_revision == index.revision {
            let reachable = workshop_reachable_files(&index.files, Path::new(&relative))?
                .into_iter()
                .map(|file| file.path)
                .collect::<BTreeSet<_>>();
            completion_import_edit(path, &relative, &reachable, item)
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };
        Ok(Some(LanguageCompletionResolution {
            documentation,
            additional_text_edits,
        }))
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
        let Some(primary) = matches.first().map(|item| (*item).clone()) else {
            return Ok(None);
        };
        let mut signatures = matches
            .iter()
            .filter_map(|item| item_signature(item))
            .collect::<Vec<_>>();
        signatures.sort();
        signatures.dedup();
        let documentation = documentation_for_completion(index, &primary);
        let live_owner = if matches!(primary.kind.as_str(), "global" | "field" | "state_path") {
            index
                .files
                .iter()
                .find(|file| file.path == primary.file)
                .map(|file| (primary.file.clone(), file.source.clone()))
        } else {
            None
        };
        let live_value = live_owner.and_then(|(owner_file, owner_source)| {
            self.live.compatible_value(
                &symbol,
                primary.type_name.as_deref(),
                &owner_file,
                &owner_source,
            )
        });
        Ok(Some(HoverInfo {
            range,
            symbol,
            kind: primary.kind.clone(),
            type_name: primary.type_name.clone(),
            owner: primary.owner.clone(),
            signatures,
            documentation,
            live_value,
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

    pub fn definition(
        &mut self,
        path: &str,
        byte_offset: usize,
    ) -> Result<Vec<LanguageLocation>, String> {
        let snapshot = self.documents.snapshot();
        let document = snapshot
            .document(path)
            .ok_or_else(|| format!("navigation document is not indexed: '{path}'"))?;
        let symbol = reference_symbol_at(&document.text, byte_offset)
            .ok_or_else(|| "no Stasis symbol at navigation position".to_string())?;
        let project_root = self.project_root.clone();
        if let Some(locations) = self.language_index.as_ref().and_then(|index| {
            index
                .definitions
                .definition(&symbol)
                .and_then(|resolution| {
                    remap_definition_locations(&project_root, &snapshot, index, &resolution)
                })
        }) {
            return Ok(locations);
        }
        let index = self.language_index()?;
        if let Some(resolution) = index.definitions.definition(&symbol) {
            if let Some(locations) =
                remap_definition_locations(&project_root, &snapshot, index, &resolution)
            {
                return Ok(locations);
            }
        }
        Ok(
            find_workshop_references(&self.language_index()?.files, &symbol, 256)?
                .into_iter()
                .filter_map(|reference| {
                    (reference.kind == WorkshopReferenceKind::Definition).then(|| {
                        LanguageLocation {
                            path: absolute_source_path(&project_root, &reference.file),
                            range: reference.source_span.start as usize
                                ..reference.source_span.end as usize,
                        }
                    })
                })
                .collect(),
        )
    }

    pub fn references(
        &mut self,
        path: &str,
        byte_offset: usize,
        include_declaration: bool,
    ) -> Result<Vec<LanguageLocation>, String> {
        Ok(self
            .symbol_references(path, byte_offset)?
            .into_iter()
            .filter_map(|(kind, location)| {
                (include_declaration || kind != WorkshopReferenceKind::Definition)
                    .then_some(location)
            })
            .collect())
    }

    pub fn document_symbols(&mut self, path: &str) -> Result<Vec<LanguageSymbol>, String> {
        let relative = canonical_source_path(Some(&self.project_root), path)?;
        let project_root = self.project_root.clone();
        Ok(self
            .language_index()?
            .symbols
            .iter()
            .filter(|symbol| symbol.file == relative)
            .map(|symbol| language_symbol(&project_root, symbol))
            .collect())
    }

    pub fn workspace_symbols(
        &mut self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<LanguageSymbol>, String> {
        let query = query.to_ascii_lowercase();
        let project_root = self.project_root.clone();
        Ok(self
            .language_index()?
            .symbols
            .iter()
            .filter(|symbol| {
                query.is_empty()
                    || symbol.name.to_ascii_lowercase().contains(&query)
                    || symbol.signature.to_ascii_lowercase().contains(&query)
            })
            .take(limit.clamp(1, 256))
            .map(|symbol| language_symbol(&project_root, symbol))
            .collect())
    }

    pub fn prepare_rename(
        &mut self,
        path: &str,
        byte_offset: usize,
    ) -> Result<RenamePreparation, String> {
        let relative = canonical_source_path(Some(&self.project_root), path)?;
        let files = self.current_language_index()?.files.clone();
        let prepared = prepare_workshop_rename(&files, &relative, byte_offset)?;
        Ok(RenamePreparation {
            range: prepared.request_span.start as usize..prepared.request_span.end as usize,
            placeholder: prepared.name,
            kind: prepared.kind,
            owner: prepared.owner,
        })
    }

    pub fn rename(
        &mut self,
        path: &str,
        byte_offset: usize,
        new_name: &str,
    ) -> Result<RenamePlan, String> {
        let snapshot = self.documents.snapshot();
        let relative = canonical_source_path(Some(&self.project_root), path)?;
        let files = self.current_language_index()?.files.clone();
        let (after, plan) = plan_workshop_rename(&files, &relative, byte_offset, new_name)?;
        let mut validator = Compiler::new();
        validator.set_project_root(self.project_root.clone())?;
        for file in &after {
            validator.upsert_file(file.path.clone(), file.source.clone());
        }
        if let Err(error) = validator.check() {
            let detail = validator
                .last_source_diagnostic()
                .map(|diagnostic| {
                    format!(
                        "{}:{}..{}: {}",
                        diagnostic.path, diagnostic.start, diagnostic.end, diagnostic.message
                    )
                })
                .unwrap_or_else(|| format!("{error:?}"));
            return Err(format!("rename would not compile: {detail}"));
        }
        let mut edits = plan
            .edits
            .into_iter()
            .map(|edit| RenameEdit {
                path: absolute_source_path(&self.project_root, &edit.file),
                range: edit.source_span.start as usize..edit.source_span.end as usize,
                new_text: edit.new_text,
            })
            .collect::<Vec<_>>();
        edits.sort_by_key(|edit| (edit.path.clone(), edit.range.start, edit.range.end));
        Ok(RenamePlan {
            revision: snapshot.revision(),
            old_name: plan.old_name,
            new_name: plan.new_name,
            kind: plan.kind,
            owner: plan.owner,
            edits,
        })
    }

    pub fn code_actions(
        &mut self,
        path: &str,
        requested_kinds: &[String],
    ) -> Result<Vec<LanguageCodeAction>, String> {
        let quick_fix_requested = requested_kinds.is_empty()
            || requested_kinds
                .iter()
                .any(|kind| "quickfix".starts_with(kind));
        let organize_requested = requested_kinds.is_empty()
            || requested_kinds
                .iter()
                .any(|kind| "source.organizeImports".starts_with(kind));
        let mut actions = Vec::new();
        if quick_fix_requested {
            actions.extend(self.diagnostic_quick_fixes(path)?);
        }
        if !organize_requested {
            return Ok(actions);
        }
        let relative = canonical_source_path(Some(&self.project_root), path)?;
        let project_root = self.project_root.clone();
        let files = match self.current_language_index() {
            Ok(index) => index.files.clone(),
            Err(_) => return Ok(actions),
        };
        let Some(change) = organize_workshop_imports(&files, &relative)? else {
            return Ok(actions);
        };
        let mut candidate = files;
        let candidate_file = candidate
            .iter_mut()
            .find(|file| file.path == change.file)
            .expect("organized file remains in language index");
        candidate_file.source = change.after_source.clone();
        let mut validator = Compiler::new();
        validator.set_project_root(project_root.clone())?;
        for file in candidate {
            validator.upsert_file(file.path, file.source);
        }
        if validator.check().is_err() {
            return Ok(actions);
        }
        actions.push(LanguageCodeAction {
            title: "Organize Stasis imports".to_string(),
            kind: "source.organizeImports".to_string(),
            preferred: true,
            diagnostic_code: None,
            edits: vec![RenameEdit {
                path: absolute_source_path(&project_root, &change.file),
                range: 0..change.before_source.len(),
                new_text: change.after_source,
            }],
        });
        Ok(actions)
    }

    pub fn prepare_call_hierarchy(
        &mut self,
        path: &str,
        byte_offset: usize,
    ) -> Result<Vec<LanguageHierarchyItem>, String> {
        self.prepare_hierarchy(path, byte_offset, LanguageHierarchyKind::Function)
    }

    pub fn incoming_calls(
        &mut self,
        symbol_id: &str,
    ) -> Result<Vec<LanguageCallHierarchyRelation>, String> {
        let snapshot = self.documents.snapshot();
        let project_root = self.project_root.clone();
        let index = self.hierarchy_index()?;
        let mut grouped = BTreeMap::<String, Vec<LanguageLocation>>::new();
        for edge in index
            .call_hierarchy_edges
            .iter()
            .filter(|edge| edge.callee_symbol_id == symbol_id)
        {
            if let Some(location) = remap_hierarchy_span(
                &project_root,
                &snapshot,
                &index.files,
                &index.call_hierarchy_items,
                &edge.caller_symbol_id,
                edge.call_span.start as usize..edge.call_span.end as usize,
            ) {
                grouped
                    .entry(edge.caller_symbol_id.clone())
                    .or_default()
                    .push(location);
            }
        }
        Ok(grouped
            .into_iter()
            .filter_map(|(caller, from_ranges)| {
                let item = index
                    .call_hierarchy_items
                    .iter()
                    .find(|item| item.symbol_id == caller)?;
                Some(LanguageCallHierarchyRelation {
                    item: remap_hierarchy_item(
                        &project_root,
                        &snapshot,
                        &index.files,
                        item,
                        LanguageHierarchyKind::Function,
                    )?,
                    from_ranges,
                })
            })
            .collect())
    }

    pub fn outgoing_calls(
        &mut self,
        symbol_id: &str,
    ) -> Result<Vec<LanguageCallHierarchyRelation>, String> {
        let snapshot = self.documents.snapshot();
        let project_root = self.project_root.clone();
        let index = self.hierarchy_index()?;
        let mut grouped = BTreeMap::<String, Vec<LanguageLocation>>::new();
        for edge in index
            .call_hierarchy_edges
            .iter()
            .filter(|edge| edge.caller_symbol_id == symbol_id)
        {
            if let Some(location) = remap_hierarchy_span(
                &project_root,
                &snapshot,
                &index.files,
                &index.call_hierarchy_items,
                symbol_id,
                edge.call_span.start as usize..edge.call_span.end as usize,
            ) {
                grouped
                    .entry(edge.callee_symbol_id.clone())
                    .or_default()
                    .push(location);
            }
        }
        Ok(grouped
            .into_iter()
            .filter_map(|(callee, from_ranges)| {
                let item = index
                    .call_hierarchy_items
                    .iter()
                    .find(|item| item.symbol_id == callee)?;
                Some(LanguageCallHierarchyRelation {
                    item: remap_hierarchy_item(
                        &project_root,
                        &snapshot,
                        &index.files,
                        item,
                        LanguageHierarchyKind::Function,
                    )?,
                    from_ranges,
                })
            })
            .collect())
    }

    pub fn prepare_type_hierarchy(
        &mut self,
        path: &str,
        byte_offset: usize,
    ) -> Result<Vec<LanguageHierarchyItem>, String> {
        self.prepare_hierarchy(path, byte_offset, LanguageHierarchyKind::Struct)
    }

    /// Stasis has composition, not inheritance. Supertypes are the structs that contain this type.
    pub fn type_supertypes(
        &mut self,
        symbol_id: &str,
    ) -> Result<Vec<LanguageHierarchyItem>, String> {
        self.related_types(symbol_id, true)
    }

    /// Stasis has composition, not inheritance. Subtypes are the component structs this type contains.
    pub fn type_subtypes(&mut self, symbol_id: &str) -> Result<Vec<LanguageHierarchyItem>, String> {
        self.related_types(symbol_id, false)
    }

    fn prepare_hierarchy(
        &mut self,
        path: &str,
        byte_offset: usize,
        kind: LanguageHierarchyKind,
    ) -> Result<Vec<LanguageHierarchyItem>, String> {
        let snapshot = self.documents.snapshot();
        let document = snapshot
            .document(path)
            .ok_or_else(|| format!("hierarchy document is not indexed: '{path}'"))?;
        let Some(symbol) = reference_symbol_at(&document.text, byte_offset) else {
            return Ok(Vec::new());
        };
        let name = symbol.rsplit('.').next().unwrap_or(&symbol);
        let project_root = self.project_root.clone();
        let index = self.hierarchy_index()?;
        let items = match kind {
            LanguageHierarchyKind::Function => &index.call_hierarchy_items,
            LanguageHierarchyKind::Struct => &index.type_hierarchy_items,
        };
        Ok(items
            .iter()
            .filter(|item| item.name == name)
            .filter_map(|item| {
                remap_hierarchy_item(&project_root, &snapshot, &index.files, item, kind)
            })
            .collect())
    }

    fn related_types(
        &mut self,
        symbol_id: &str,
        containers: bool,
    ) -> Result<Vec<LanguageHierarchyItem>, String> {
        let snapshot = self.documents.snapshot();
        let project_root = self.project_root.clone();
        let index = self.hierarchy_index()?;
        let related = index
            .type_hierarchy_edges
            .iter()
            .filter_map(|edge| {
                if containers && edge.component_symbol_id == symbol_id {
                    Some(edge.container_symbol_id.as_str())
                } else if !containers && edge.container_symbol_id == symbol_id {
                    Some(edge.component_symbol_id.as_str())
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();
        Ok(index
            .type_hierarchy_items
            .iter()
            .filter(|item| related.contains(item.symbol_id.as_str()))
            .filter_map(|item| {
                remap_hierarchy_item(
                    &project_root,
                    &snapshot,
                    &index.files,
                    item,
                    LanguageHierarchyKind::Struct,
                )
            })
            .collect())
    }

    fn diagnostic_quick_fixes(&mut self, path: &str) -> Result<Vec<LanguageCodeAction>, String> {
        let report = self.diagnostics();
        let Some(published) = report
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.path == path)
        else {
            return Ok(Vec::new());
        };
        let Some(diagnostic) = self.compiler.last_source_diagnostic().cloned() else {
            return Ok(Vec::new());
        };
        if published.code != diagnostic.code.as_str() || diagnostic.fixes.is_empty() {
            return Ok(Vec::new());
        }

        let snapshot = self.documents.snapshot();
        let project_root = self.project_root.clone();
        let files = snapshot
            .documents()
            .map(|(path, document)| {
                Ok(WorkshopSourceFile {
                    path: canonical_source_path(Some(&project_root), path)?,
                    source: document.text.to_string(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut actions = Vec::new();
        for fix in diagnostic.fixes {
            let Some(candidate) = apply_diagnostic_fix(&files, &fix.edits) else {
                continue;
            };
            let mut validator = Compiler::new();
            validator.set_project_root(project_root.clone())?;
            for file in &candidate {
                validator.upsert_file(file.path.clone(), file.source.clone());
            }
            if validator.check().is_err() {
                continue;
            }
            actions.push(LanguageCodeAction {
                title: fix.title,
                kind: "quickfix".to_string(),
                preferred: true,
                diagnostic_code: Some(diagnostic.code.as_str().to_string()),
                edits: fix
                    .edits
                    .into_iter()
                    .map(|edit| RenameEdit {
                        path: absolute_source_path(&project_root, &edit.path),
                        range: edit.start..edit.end,
                        new_text: edit.new_text,
                    })
                    .collect(),
            });
        }
        Ok(actions)
    }

    pub fn semantic_tokens(&mut self, path: &str) -> Result<Vec<LanguageSemanticToken>, String> {
        let snapshot = self.documents.snapshot();
        let current = snapshot
            .document(path)
            .ok_or_else(|| format!("semantic-token document is not indexed: '{path}'"))?
            .text
            .clone();
        let relative = canonical_source_path(Some(&self.project_root), path)?;
        let index = self.language_index()?;
        let indexed = index
            .files
            .iter()
            .find(|file| file.path == relative)
            .ok_or_else(|| format!("semantic-token file is not indexed: '{relative}'"))?;
        let tokens = workshop_semantic_tokens(&index.files, &relative)?;
        Ok(remap_semantic_tokens(
            &indexed.source,
            &current,
            tokens.into_iter().map(|token| LanguageSemanticToken {
                range: token.source_span.start as usize..token.source_span.end as usize,
                kind: token.kind,
            }),
        ))
    }

    pub fn inlay_hints(&mut self, path: &str) -> Result<Vec<LanguageInlayHint>, String> {
        let snapshot = self.documents.snapshot();
        let revision = snapshot.revision();
        let current = snapshot
            .document(path)
            .ok_or_else(|| format!("inlay-hint document is not indexed: '{path}'"))?
            .text
            .clone();
        let relative = canonical_source_path(Some(&self.project_root), path)?;
        let needs_rebuild = self
            .inlay_index
            .as_ref()
            .is_none_or(|index| index.revision != revision)
            && self
                .inlay_index_error
                .as_ref()
                .is_none_or(|(failed_revision, _)| *failed_revision != revision);
        if needs_rebuild {
            let files = snapshot
                .documents()
                .map(|(path, document)| {
                    Ok(WorkshopSourceFile {
                        path: canonical_source_path(Some(&self.project_root), path)?,
                        source: document.text.to_string(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            match workshop_inlay_hints(&files) {
                Ok(hints) => {
                    self.inlay_index = Some(LanguageInlayIndex {
                        revision,
                        files,
                        hints,
                    });
                    self.inlay_index_error = None;
                }
                Err(error) => self.inlay_index_error = Some((revision, error)),
            }
        }
        let Some(index) = self.inlay_index.as_ref() else {
            return Ok(Vec::new());
        };
        let Some(indexed) = index.files.iter().find(|file| file.path == relative) else {
            return Ok(Vec::new());
        };
        let regions = UnchangedRegions::new(&indexed.source, &current);
        Ok(index
            .hints
            .iter()
            .filter(|hint| hint.file == relative)
            .filter_map(|hint| {
                let anchor = regions.remap(
                    &indexed.source,
                    &current,
                    hint.source_span.start as usize..hint.source_span.end as usize,
                )?;
                let kind = match hint.kind {
                    WorkshopInlayHintKind::Type => LanguageInlayHintKind::Type,
                    WorkshopInlayHintKind::Parameter => LanguageInlayHintKind::Parameter,
                };
                Some(LanguageInlayHint {
                    position: match kind {
                        LanguageInlayHintKind::Type => anchor.end,
                        LanguageInlayHintKind::Parameter => anchor.start,
                    },
                    anchor,
                    kind,
                    label: hint.label.clone(),
                })
            })
            .collect())
    }

    pub fn folding_ranges(&self, path: &str) -> Result<Vec<LanguageFoldingRange>, String> {
        let snapshot = self.documents.snapshot();
        let document = snapshot
            .document(path)
            .ok_or_else(|| format!("folding document is not indexed: '{path}'"))?;
        Ok(workshop_folding_ranges(&document.text)?
            .into_iter()
            .map(|range| LanguageFoldingRange {
                range: range.source_span.start as usize..range.source_span.end as usize,
            })
            .collect())
    }

    pub fn selection_ranges(
        &self,
        path: &str,
        byte_offsets: &[usize],
    ) -> Result<Vec<Vec<Range<usize>>>, String> {
        let snapshot = self.documents.snapshot();
        let document = snapshot
            .document(path)
            .ok_or_else(|| format!("selection document is not indexed: '{path}'"))?;
        byte_offsets
            .iter()
            .map(|offset| {
                workshop_selection_ranges(&document.text, *offset).map(|ranges| {
                    ranges
                        .into_iter()
                        .map(|range| range.start as usize..range.end as usize)
                        .collect()
                })
            })
            .collect()
    }

    pub fn linked_edit_ranges(
        &mut self,
        path: &str,
        byte_offset: usize,
    ) -> Result<Option<Vec<Range<usize>>>, String> {
        let relative = canonical_source_path(Some(&self.project_root), path)?;
        let files = match self.current_language_index() {
            Ok(index) => index.files.clone(),
            Err(_) => return Ok(None),
        };
        Ok(
            workshop_linked_edit_ranges(&files, &relative, byte_offset)?.map(|ranges| {
                ranges
                    .into_iter()
                    .map(|range| range.start as usize..range.end as usize)
                    .collect()
            }),
        )
    }

    fn symbol_references(
        &mut self,
        path: &str,
        byte_offset: usize,
    ) -> Result<Vec<(WorkshopReferenceKind, LanguageLocation)>, String> {
        let snapshot = self.documents.snapshot();
        let document = snapshot
            .document(path)
            .ok_or_else(|| format!("navigation document is not indexed: '{path}'"))?;
        let symbol = reference_symbol_at(&document.text, byte_offset)
            .ok_or_else(|| "no Stasis symbol at navigation position".to_string())?;
        let project_root = self.project_root.clone();
        let index = self.language_index()?;
        Ok(find_workshop_references(&index.files, &symbol, 256)?
            .into_iter()
            .map(|reference| {
                (
                    reference.kind,
                    LanguageLocation {
                        path: absolute_source_path(&project_root, &reference.file),
                        range: reference.source_span.start as usize
                            ..reference.source_span.end as usize,
                    },
                )
            })
            .collect())
    }

    fn language_index(&mut self) -> Result<&LanguageIndex, String> {
        let snapshot = self.documents.snapshot();
        let revision = snapshot.revision();
        let needs_rebuild = self
            .language_index
            .as_ref()
            .is_none_or(|index| index.revision != revision)
            && self
                .language_index_error
                .as_ref()
                .is_none_or(|(failed_revision, _)| *failed_revision != revision);
        if needs_rebuild {
            let files = snapshot
                .documents()
                .map(|(path, document)| {
                    Ok(WorkshopSourceFile {
                        path: canonical_source_path(Some(&self.project_root), path)?,
                        source: document.text.to_string(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let rebuilt = (|| {
                let source_items = workshop_source_items(&files)?;
                let symbols = workshop_symbols(&files)?;
                let workshop_items = workshop_completion_items(&files)?;
                let definitions =
                    WarmDefinitionIndex::build(&files, &source_items, &workshop_items)?;
                let completion_items = workshop_items
                    .iter()
                    .map(shared_completion_item)
                    .collect::<Vec<_>>();
                let mut completion = CompletionIndex::default();
                completion.replace(completion_items);
                Ok::<_, String>(LanguageIndex {
                    revision,
                    files,
                    source_items,
                    completion,
                    workshop_items,
                    symbols,
                    definitions,
                })
            })();
            match rebuilt {
                Ok(index) => {
                    self.language_index = Some(index);
                    self.language_index_error = None;
                }
                Err(error) => self.language_index_error = Some((revision, error)),
            }
        }
        if let Some(index) = self.language_index.as_ref() {
            return Ok(index);
        }
        Err(self
            .language_index_error
            .as_ref()
            .map(|(_, error)| format!("language index is unavailable: {error}"))
            .unwrap_or_else(|| "language index was not published".to_string()))
    }

    fn hierarchy_index(&mut self) -> Result<&LanguageHierarchyIndex, String> {
        let snapshot = self.documents.snapshot();
        let revision = snapshot.revision();
        let needs_rebuild = self
            .hierarchy_index
            .as_ref()
            .is_none_or(|index| index.revision != revision)
            && self
                .hierarchy_index_error
                .as_ref()
                .is_none_or(|(failed_revision, _)| *failed_revision != revision);
        if needs_rebuild {
            let files = snapshot
                .documents()
                .map(|(path, document)| {
                    Ok(WorkshopSourceFile {
                        path: canonical_source_path(Some(&self.project_root), path)?,
                        source: document.text.to_string(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let rebuilt = (|| {
                let (call_hierarchy_items, call_hierarchy_edges) = workshop_call_hierarchy(&files)?;
                let (type_hierarchy_items, type_hierarchy_edges) = workshop_type_hierarchy(&files)?;
                Ok::<_, String>(LanguageHierarchyIndex {
                    revision,
                    files,
                    call_hierarchy_items,
                    call_hierarchy_edges,
                    type_hierarchy_items,
                    type_hierarchy_edges,
                })
            })();
            match rebuilt {
                Ok(index) => {
                    self.hierarchy_index = Some(index);
                    self.hierarchy_index_error = None;
                }
                Err(error) => self.hierarchy_index_error = Some((revision, error)),
            }
        }
        if let Some(index) = self.hierarchy_index.as_ref() {
            return Ok(index);
        }
        Err(self
            .hierarchy_index_error
            .as_ref()
            .map(|(_, error)| format!("hierarchy index is unavailable: {error}"))
            .unwrap_or_else(|| "hierarchy index was not published".to_string()))
    }

    fn current_language_index(&mut self) -> Result<&LanguageIndex, String> {
        let revision = self.documents.snapshot().revision();
        let index = self.language_index()?;
        if index.revision != revision {
            return Err(
                "rename is unavailable until the current source can be indexed safely".to_string(),
            );
        }
        Ok(index)
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

    pub fn format_document(&self, path: &str) -> Result<Vec<LanguageTextEdit>, String> {
        let snapshot = self.documents.snapshot();
        let document = snapshot
            .document(path)
            .ok_or_else(|| format!("format document is not indexed: '{path}'"))?;
        let formatted = format_source(&document.text)?;
        Ok(minimal_text_edit(&document.text, &formatted)
            .into_iter()
            .collect())
    }

    pub fn format_range(
        &self,
        path: &str,
        requested: Range<usize>,
    ) -> Result<Vec<LanguageTextEdit>, String> {
        let edits = self.format_document(path)?;
        Ok(edits
            .into_iter()
            .filter(|edit| requested.start <= edit.range.start && edit.range.end <= requested.end)
            .collect())
    }

    pub fn format_on_type(
        &self,
        path: &str,
        byte_offset: usize,
        typed: &str,
    ) -> Result<Vec<LanguageTextEdit>, String> {
        if !matches!(typed, ";" | "}") {
            return Ok(Vec::new());
        }
        let snapshot = self.documents.snapshot();
        let document = snapshot
            .document(path)
            .ok_or_else(|| format!("on-type format document is not indexed: '{path}'"))?;
        let selections = workshop_selection_ranges(&document.text, byte_offset)?;
        let Some(block) = selections.into_iter().find(|range| {
            document
                .text
                .get(range.start as usize..range.end as usize)
                .is_some_and(|text| text.starts_with('{') && text.ends_with('}'))
        }) else {
            return Ok(Vec::new());
        };
        self.format_range(path, block.start as usize..block.end as usize)
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

fn remap_semantic_tokens(
    indexed: &str,
    current: &str,
    tokens: impl IntoIterator<Item = LanguageSemanticToken>,
) -> Vec<LanguageSemanticToken> {
    let regions = UnchangedRegions::new(indexed, current);
    tokens
        .into_iter()
        .filter_map(|mut token| {
            token.range = regions.remap(indexed, current, token.range)?;
            Some(token)
        })
        .collect()
}

fn minimal_text_edit(current: &str, formatted: &str) -> Option<LanguageTextEdit> {
    if current == formatted {
        return None;
    }
    let mut prefix = current
        .bytes()
        .zip(formatted.bytes())
        .take_while(|(left, right)| left == right)
        .count();
    while !current.is_char_boundary(prefix) || !formatted.is_char_boundary(prefix) {
        prefix = prefix.saturating_sub(1);
    }
    let maximum_suffix = current.len().min(formatted.len()).saturating_sub(prefix);
    let mut suffix = current
        .as_bytes()
        .iter()
        .rev()
        .zip(formatted.as_bytes().iter().rev())
        .take(maximum_suffix)
        .take_while(|(left, right)| left == right)
        .count();
    while !current.is_char_boundary(current.len() - suffix)
        || !formatted.is_char_boundary(formatted.len() - suffix)
    {
        suffix = suffix.saturating_sub(1);
    }
    Some(LanguageTextEdit {
        range: prefix..current.len() - suffix,
        new_text: formatted[prefix..formatted.len() - suffix].to_string(),
    })
}

struct UnchangedRegions {
    prefix: usize,
    indexed_suffix_start: usize,
    shift: isize,
}

impl UnchangedRegions {
    fn new(indexed: &str, current: &str) -> Self {
        let mut prefix = indexed
            .bytes()
            .zip(current.bytes())
            .take_while(|(left, right)| left == right)
            .count();
        while !indexed.is_char_boundary(prefix) || !current.is_char_boundary(prefix) {
            prefix = prefix.saturating_sub(1);
        }
        let maximum_suffix = indexed.len().min(current.len()).saturating_sub(prefix);
        let mut suffix = indexed
            .as_bytes()
            .iter()
            .rev()
            .zip(current.as_bytes().iter().rev())
            .take(maximum_suffix)
            .take_while(|(left, right)| left == right)
            .count();
        while !indexed.is_char_boundary(indexed.len() - suffix)
            || !current.is_char_boundary(current.len() - suffix)
        {
            suffix = suffix.saturating_sub(1);
        }
        Self {
            prefix,
            indexed_suffix_start: indexed.len() - suffix,
            shift: current.len() as isize - indexed.len() as isize,
        }
    }

    fn remap(
        &self,
        indexed: &str,
        current: &str,
        indexed_range: Range<usize>,
    ) -> Option<Range<usize>> {
        let current_range = if indexed_range.end <= self.prefix {
            indexed_range.clone()
        } else if indexed_range.start >= self.indexed_suffix_start {
            indexed_range.start.checked_add_signed(self.shift)?
                ..indexed_range.end.checked_add_signed(self.shift)?
        } else {
            return None;
        };
        (indexed.get(indexed_range) == current.get(current_range.clone())).then_some(current_range)
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
) -> Option<(usize, &'a WorkshopCompletionItem)> {
    items.iter().enumerate().find(|(_, item)| {
        item.text == ranked.text
            && item.kind == ranked.kind
            && item.detail == ranked.detail
            && ranked.source.as_deref() == Some(item.file.as_str())
            && item.type_name == ranked.type_name
            && item.scope.as_ref().map(shared_completion_scope) == ranked.scope
    })
}

fn apply_diagnostic_fix(
    files: &[WorkshopSourceFile],
    edits: &[SourceDiagnosticEdit],
) -> Option<Vec<WorkshopSourceFile>> {
    let mut candidate = files.to_vec();
    let mut by_path = BTreeMap::<&str, Vec<&SourceDiagnosticEdit>>::new();
    for edit in edits {
        by_path.entry(&edit.path).or_default().push(edit);
    }
    for (path, mut edits) in by_path {
        let file = candidate.iter_mut().find(|file| file.path == path)?;
        edits.sort_by_key(|edit| (edit.start, edit.end));
        for pair in edits.windows(2) {
            if pair[0].end > pair[1].start {
                return None;
            }
        }
        for edit in edits.into_iter().rev() {
            if edit.start > edit.end
                || edit.end > file.source.len()
                || !file.source.is_char_boundary(edit.start)
                || !file.source.is_char_boundary(edit.end)
            {
                return None;
            }
            file.source
                .replace_range(edit.start..edit.end, &edit.new_text);
        }
    }
    Some(candidate)
}

fn completion_insert_text(
    item: &WorkshopCompletionItem,
    source: &str,
    replacement_end: usize,
) -> (String, bool) {
    if !matches!(item.kind.as_str(), "function" | "method" | "test") {
        return (item.text.clone(), false);
    }
    let follows_existing_call = source.get(replacement_end..).is_some_and(|suffix| {
        let suffix = suffix
            .trim_start_matches(|character: char| character == '_' || character.is_alphanumeric());
        suffix.trim_start().starts_with('(')
    });
    if follows_existing_call {
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

fn indexed_completion_receiver(source: &str, cursor: usize) -> Option<(&str, &str)> {
    let prefix = source.get(..cursor.min(source.len()))?;
    let field_separator = prefix.rfind("].")?;
    let indexed_path = prefix.get(..field_separator + 1)?;
    let open = indexed_path.rfind('[')?;
    let index_text = indexed_path.get(open + 1..indexed_path.len() - 1)?;
    if index_text.is_empty() || !index_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let path_prefix = indexed_path.get(..open)?;
    let path_start = path_prefix
        .char_indices()
        .rev()
        .find(|(_, character)| {
            !(character.is_ascii_alphanumeric() || *character == '_' || *character == '.')
        })
        .map_or(0, |(index, character)| index + character.len_utf8());
    let collection_path = path_prefix.get(path_start..)?;
    let receiver = prefix.get(path_start..field_separator + 2)?;
    collection_path
        .split('.')
        .all(|segment| {
            let mut bytes = segment.bytes();
            bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
        .then_some((collection_path, receiver))
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

fn reference_symbol_at(source: &str, byte_offset: usize) -> Option<String> {
    if byte_offset > source.len() || !source.is_char_boundary(byte_offset) {
        return None;
    }
    let tokens = lex(source).ok()?;
    let token = tokens.iter().find(|token| {
        token.kind == TokenKind::Identifier
            && token.start <= byte_offset
            && byte_offset <= token.end
    })?;
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
    let mut normalized = String::new();
    bracket_depth = 0;
    for character in prefix[start..].chars() {
        match character {
            '[' => bracket_depth = bracket_depth.saturating_add(1),
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ if bracket_depth > 0 || character.is_ascii_whitespace() => {}
            _ => normalized.push(character),
        }
    }
    normalized
        .split('.')
        .all(|segment| {
            let mut bytes = segment.bytes();
            bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
        .then_some(normalized)
}

fn absolute_source_path(project_root: &str, relative_path: &str) -> String {
    Path::new(project_root)
        .join(relative_path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn remap_hierarchy_item(
    project_root: &str,
    snapshot: &WorkspaceSnapshot,
    files: &[WorkshopSourceFile],
    item: &WorkshopHierarchyItem,
    kind: LanguageHierarchyKind,
) -> Option<LanguageHierarchyItem> {
    let path = absolute_source_path(project_root, &item.file);
    let indexed = files.iter().find(|file| file.path == item.file)?;
    let current = snapshot.document(&path)?;
    let regions = UnchangedRegions::new(&indexed.source, &current.text);
    let selection_range = regions.remap(
        &indexed.source,
        &current.text,
        item.selection_span.start as usize..item.selection_span.end as usize,
    )?;
    let range = regions
        .remap(
            &indexed.source,
            &current.text,
            item.source_span.start as usize..item.source_span.end as usize,
        )
        .unwrap_or_else(|| selection_range.clone());
    Some(LanguageHierarchyItem {
        symbol_id: item.symbol_id.clone(),
        name: item.name.clone(),
        detail: item.detail.clone(),
        kind,
        location: LanguageLocation { path, range },
        selection_range,
    })
}

fn remap_definition_locations(
    project_root: &str,
    snapshot: &WorkspaceSnapshot,
    index: &LanguageIndex,
    resolution: &WarmDefinitionResolution,
) -> Option<Vec<LanguageLocation>> {
    for dependency in &resolution.dependencies {
        let indexed = index
            .files
            .iter()
            .find(|candidate| candidate.path == dependency.file)?;
        let path = absolute_source_path(project_root, &dependency.file);
        let current = snapshot.document(&path)?;
        let regions = UnchangedRegions::new(&indexed.source, &current.text);
        for span in &dependency.source_spans {
            regions.remap(
                &indexed.source,
                &current.text,
                span.start as usize..span.end as usize,
            )?;
        }
    }
    resolution
        .references
        .iter()
        .map(|reference| {
            let path = absolute_source_path(project_root, &reference.file);
            let indexed = index
                .files
                .iter()
                .find(|file| file.path == reference.file)?;
            let current = snapshot.document(&path)?;
            let range = UnchangedRegions::new(&indexed.source, &current.text).remap(
                &indexed.source,
                &current.text,
                reference.source_span.start as usize..reference.source_span.end as usize,
            )?;
            Some(LanguageLocation { path, range })
        })
        .collect()
}

fn remap_hierarchy_span(
    project_root: &str,
    snapshot: &WorkspaceSnapshot,
    files: &[WorkshopSourceFile],
    items: &[WorkshopHierarchyItem],
    owner_symbol_id: &str,
    span: Range<usize>,
) -> Option<LanguageLocation> {
    let owner = items
        .iter()
        .find(|item| item.symbol_id == owner_symbol_id)?;
    let path = absolute_source_path(project_root, &owner.file);
    let indexed = files.iter().find(|file| file.path == owner.file)?;
    let current = snapshot.document(&path)?;
    let range = UnchangedRegions::new(&indexed.source, &current.text).remap(
        &indexed.source,
        &current.text,
        span,
    )?;
    Some(LanguageLocation { path, range })
}

fn language_symbol(project_root: &str, symbol: &WorkshopSymbol) -> LanguageSymbol {
    LanguageSymbol {
        name: symbol.name.clone(),
        kind: match symbol.kind {
            WorkshopSymbolKind::Struct => LanguageSymbolKind::Struct,
            WorkshopSymbolKind::Function => LanguageSymbolKind::Function,
            WorkshopSymbolKind::Global => LanguageSymbolKind::Global,
            WorkshopSymbolKind::Constant => LanguageSymbolKind::Constant,
            WorkshopSymbolKind::Test => LanguageSymbolKind::Test,
        },
        detail: symbol.signature.clone(),
        container_name: symbol.owner.clone(),
        location: LanguageLocation {
            path: absolute_source_path(project_root, &symbol.file),
            range: symbol.source_span.start as usize..symbol.source_span.end as usize,
        },
    }
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
    use std::fs;
    use std::path::PathBuf;
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
        let root = std::env::temp_dir().join(format!(
            "stasis-language-service-vscode-fixture-{}",
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(root.join("src")).expect("fixture source directory");
        let path = root.join("src/main.stasis");
        let source = include_str!("../../../vscode-stasis/test/fixture/src/main.stasis");
        fs::write(&path, source).expect("fixture source");
        let toolchain_source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src");
        for directory in ["runtime", "stdlib"] {
            let cached_directory = root.join(".stasis_cache/toolchain/src").join(directory);
            fs::create_dir_all(&cached_directory).expect("toolchain cache directory");
            for entry in std::fs::read_dir(toolchain_source.join(directory))
                .expect("toolchain source directory")
            {
                let entry = entry.expect("toolchain source entry");
                if entry.path().extension().and_then(|value| value.to_str()) != Some("stasis") {
                    continue;
                }
                fs::copy(entry.path(), cached_directory.join(entry.file_name()))
                    .expect("cached toolchain source");
            }
        }
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(path.to_string_lossy(), source);

        let report = service.diagnostics();
        assert!(
            report.diagnostics.is_empty(),
            "fixture diagnostics: {:?}",
            report.diagnostics
        );

        service.open_document(
            path.to_string_lossy(),
            1,
            format!(
                "{source}\nfunction lsp_diagnostic_probe(): i32 {{ while (true) {{ return 1; }} }}\n"
            ),
        );
        let dirty = service.diagnostics();
        assert!(
            dirty
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("while")),
            "dirty fixture diagnostics: {:?}",
            dirty.diagnostics
        );
        fs::remove_dir_all(root).ok();
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
        assert_eq!(function.insert_text, "spawn_enemy");
        assert!(!function.snippet);
        assert!(function.documentation.is_none());
        let resolution = service
            .resolve_completion(&path, function.resolve_data.expect("resolve data"))
            .expect("completion resolve")
            .expect("current completion resolution");
        assert_eq!(
            resolution.documentation.as_deref(),
            Some("Creates an enemy with explicit health.")
        );

        let snippet_source = source.replace("spawn_enemy(1, foe.hp);", "spawn_e");
        let snippet_cursor = snippet_source.rfind("spawn_e").expect("partial call") + 7;
        service.open_document(path.clone(), 1, snippet_source);
        let snippet = service
            .completion(&path, snippet_cursor, 64)
            .expect("snippet completion");
        let function = snippet
            .items
            .iter()
            .find(|item| item.text == "spawn_enemy")
            .expect("snippet function item");
        assert_eq!(function.insert_text, "spawn_enemy(${1:count}, ${2:health})");
        assert!(function.snippet);
    }

    #[test]
    fn broken_dirty_source_uses_last_good_index_for_read_only_queries() {
        let (mut service, path, source) = intelligence_service();
        let call = source.find("spawn_enemy(1").expect("call") + 2;
        assert_eq!(
            service
                .definition(&path, call)
                .expect("warm definition")
                .len(),
            1
        );

        let broken = format!("{source}\nfunction unfinished(): i32 {{");
        service.open_document(path.clone(), 1, broken);
        let definitions = service
            .definition(&path, call)
            .expect("last-good definition during syntax error");
        assert_eq!(definitions.len(), 1);
        assert_eq!(&source[definitions[0].range.clone()], "spawn_enemy");
        assert!(service
            .completion(&path, call, 64)
            .expect("last-good completion during syntax error")
            .items
            .iter()
            .any(|item| item.text == "spawn_enemy"));
        assert!(service.prepare_rename(&path, call).is_err());
        assert_eq!(service.diagnostics().diagnostics.len(), 1);
    }

    #[test]
    fn semantic_tokens_recover_only_unchanged_regions_of_broken_source() {
        let (mut service, path, source) = intelligence_service();
        let warm = service
            .semantic_tokens(&path)
            .expect("warm semantic tokens");
        assert!(warm.iter().any(|token| token.kind == "function"));
        assert!(warm.iter().any(|token| token.kind == "field"));

        let broken = source.replace("foe.hp = 3;", "foe.");
        service.open_document(path.clone(), 1, broken.clone());
        let recovered = service
            .semantic_tokens(&path)
            .expect("recovered semantic tokens");
        assert!(recovered
            .iter()
            .all(|token| broken.get(token.range.clone()).is_some()));
        assert!(recovered.iter().any(|token| {
            token.kind == "function" && broken.get(token.range.clone()) == Some("spawn_enemy")
        }));
        assert!(recovered.iter().any(|token| {
            token.kind == "field" && broken.get(token.range.clone()) == Some("hp")
        }));
    }

    #[test]
    fn inlay_hints_publish_inferred_types_parameters_and_last_good_recovery() {
        let root = std::env::temp_dir().join("stasis-language-service-inlay-hints");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let source = "function add(amount: i32, bonus: i32): i32 { return amount + bonus; }\nfunction main(): i32 { let total = add(1, 2); return total; }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(path_text.clone(), source);
        let warm = service.inlay_hints(&path_text).expect("inlay hints");
        assert!(warm.iter().any(|hint| {
            hint.kind == LanguageInlayHintKind::Type
                && hint.label == ": i32"
                && &source[hint.anchor.clone()] == "total"
        }));
        assert!(warm.iter().any(|hint| {
            hint.kind == LanguageInlayHintKind::Parameter
                && hint.label == "amount:"
                && &source[hint.anchor.clone()] == "1"
        }));

        let broken = format!("function unfinished(): i32 {{\n{source}");
        service.open_document(path_text.clone(), 1, broken.clone());
        let recovered = service
            .inlay_hints(&path_text)
            .expect("last-good inlay hints");
        assert!(recovered.iter().any(|hint| {
            hint.kind == LanguageInlayHintKind::Type
                && broken.get(hint.anchor.clone()) == Some("total")
        }));
        assert!(recovered.iter().any(|hint| {
            hint.kind == LanguageInlayHintKind::Parameter
                && broken.get(hint.anchor.clone()) == Some("1")
        }));
    }

    #[test]
    fn incomplete_call_keeps_global_receiver_field_completion() {
        let root = std::env::temp_dir().join("stasis-language-service-recovery-completion");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let good = "struct State { x: i32; }\nglobal state: State;\nfunction a(value: i32): i32 { return value; }\nfunction b(): i32 { return a(state.x); }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(path_text.clone(), good);
        let warm_cursor = good.find("state.x").expect("warm receiver") + "state.".len();
        assert!(service
            .completion(&path_text, warm_cursor, 64)
            .expect("warm completion")
            .items
            .iter()
            .any(|item| item.text == "state.x"));

        for broken in [
            "struct State { x: i32; }\nglobal state: State;\nfunction a(value: i32): i32 { return value; }\nfunction b(): i32 { a(state.",
            "struct State { x: i32; }\nglobal state: State;\nfunction a(value: i32): i32 { return value; }\nfunction b(): i32 { a(state.\n}\n",
        ] {
            service.open_document(path_text.clone(), 1, broken);
            let completion = service
                .completion(&path_text, broken.find("state.").unwrap() + "state.".len(), 64)
                .expect("recovered field completion");
            assert!(completion.items.iter().any(|item| item.text == "state.x"));
        }
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
        assert!(helper.additional_text_edits.is_empty());
        assert!(helper.documentation.is_none());
        let resolution = service
            .resolve_completion(&main_text, helper.resolve_data.expect("resolve data"))
            .expect("completion resolve")
            .expect("current completion resolution");
        assert_eq!(resolution.additional_text_edits.len(), 1);
        assert_eq!(resolution.additional_text_edits[0].range, 0..0);
        assert_eq!(
            resolution.additional_text_edits[0].text,
            "import \"helper.stasis\";\n"
        );

        let stale_data = helper.resolve_data.expect("resolve data");
        service.open_document(
            main_text.clone(),
            1,
            format!("{source}function unfinished() {{"),
        );
        let stale = service
            .resolve_completion(&main_text, stale_data)
            .expect("stale completion resolve")
            .expect("last-good documentation resolution");
        assert!(stale.additional_text_edits.is_empty());
    }

    #[test]
    fn organize_imports_code_action_is_current_compiler_validated_edit() {
        let root = std::env::temp_dir().join("stasis-language-service-organize-imports");
        let main_path = root.join("src/main.stasis");
        let main_text = main_path.to_string_lossy().replace('\\', "/");
        let source = "import \"unused.stasis\";\nimport \"helper.stasis\";\nimport \"helper.stasis\";\nfunction main(): i32 { return helper(); }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(main_text.clone(), source);
        service.set_disk_document(
            root.join("src/helper.stasis").to_string_lossy(),
            "function helper(): i32 { return 1; }\n",
        );
        service.set_disk_document(
            root.join("src/unused.stasis").to_string_lossy(),
            "function unrelated(): i32 { return 2; }\n",
        );

        let actions = service
            .code_actions(&main_text, &["source.organizeImports".to_string()])
            .expect("code actions");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, "source.organizeImports");
        assert!(actions[0].preferred);
        assert_eq!(actions[0].edits[0].range, 0..source.len());
        assert_eq!(
            actions[0].edits[0].new_text,
            "import \"helper.stasis\";\nfunction main(): i32 { return helper(); }\n"
        );

        service.open_document(
            main_text.clone(),
            1,
            format!("{source}function unfinished(): i32 {{"),
        );
        assert!(service
            .code_actions(&main_text, &["source.organizeImports".to_string()])
            .expect("safe broken-source actions")
            .is_empty());
    }

    #[test]
    fn structured_import_quick_fixes_are_compiler_validated() {
        let root = std::env::temp_dir().join("stasis-language-service-quick-fixes");
        let main_path = root.join("src/main.stasis");
        let main_text = main_path.to_string_lossy().replace('\\', "/");
        let source = "import \"missing.stasis\";\nfunction main(): i32 { return 0; }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(main_text.clone(), source);

        let report = service.diagnostics();
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].code, "stasis.missingModule");
        let actions = service
            .code_actions(&main_text, &["quickfix".to_string()])
            .expect("quick fixes");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, "quickfix");
        assert_eq!(
            actions[0].diagnostic_code.as_deref(),
            Some("stasis.missingModule")
        );
        assert_eq!(
            &source[actions[0].edits[0].range.clone()],
            "import \"missing.stasis\";\n"
        );
        assert!(actions[0].edits[0].new_text.is_empty());
    }

    #[test]
    fn duplicate_import_quick_fix_preserves_the_compiling_import() {
        let root = std::env::temp_dir().join("stasis-language-service-duplicate-import-fix");
        let main_path = root.join("src/main.stasis");
        let main_text = main_path.to_string_lossy().replace('\\', "/");
        let source = "import \"helper.stasis\";\nimport \"helper.stasis\";\nfunction main(): i32 { return helper(); }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(main_text.clone(), source);
        service.set_disk_document(
            root.join("src/helper.stasis").to_string_lossy(),
            "function helper(): i32 { return 1; }\n",
        );

        let actions = service
            .code_actions(&main_text, &["quickfix".to_string()])
            .expect("duplicate import quick fix");
        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0].diagnostic_code.as_deref(),
            Some("stasis.duplicateImportAlias")
        );
        assert_eq!(
            &source[actions[0].edits[0].range.clone()],
            "import \"helper.stasis\";\n"
        );
        assert_eq!(
            actions[0].edits[0].range.start,
            source.rfind("import").unwrap()
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
    fn hover_uses_only_hash_and_type_compatible_live_observations() {
        let root = std::env::temp_dir().join("stasis-language-service-live-hover");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let source = "global score: i32;\nfunction main(): i32 { return score; }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(path_text.clone(), source);
        service.publish_live_observations(LiveObservationBatch {
            session_id: "test-session".into(),
            generation: 4,
            source_hashes: BTreeMap::from([
                ("src/main.stasis".into(), workshop_source_hash(source)),
                (
                    ".stasis_cache/toolchain/src/stdlib/graphics.stasis".into(),
                    "accepted-toolchain-hash".into(),
                ),
            ]),
            observations: vec![LiveObservation {
                path: "score".into(),
                type_name: Some("i32".into()),
                value: "7".into(),
                tick: 12,
            }],
            indexed_collections: Vec::new(),
            complete: true,
        });
        let offset = source.rfind("score").expect("score use") + 2;
        assert_eq!(
            service
                .hover(&path_text, offset)
                .expect("compatible hover")
                .and_then(|hover| hover.live_value),
            Some("7 (tick 12)".into())
        );

        let changed = source.replace("return score", "return score + 0");
        service.open_document(path_text.clone(), 1, changed.clone());
        let changed_offset = changed.rfind("score").expect("changed score") + 2;
        assert_eq!(
            service
                .hover(&path_text, changed_offset)
                .expect("stale hover")
                .and_then(|hover| hover.live_value),
            None
        );
    }

    #[test]
    fn compatible_runtime_layout_completes_indexed_collection_fields() {
        let root = std::env::temp_dir().join("stasis-language-service-indexed-live-completion");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let source = "struct Enemy { hp: i32; speed: i32; }\nstruct State { enemies: Enemy[2]; }\nglobal state: State;\nfunction main(): i32 { return state.enemies[0].speed; }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(path_text.clone(), source);
        service.publish_live_observations(LiveObservationBatch {
            session_id: "indexed-session".into(),
            generation: 1,
            source_hashes: BTreeMap::from([
                ("src/main.stasis".into(), workshop_source_hash(source)),
                (
                    ".stasis_cache/toolchain/src/stdlib/graphics.stasis".into(),
                    "accepted-toolchain-hash".into(),
                ),
            ]),
            observations: Vec::new(),
            indexed_collections: vec![LiveIndexedCollection {
                path: "state.enemies".into(),
                fields: BTreeMap::from([
                    ("hp".into(), "i32".into()),
                    ("speed".into(), "i32".into()),
                ]),
            }],
            complete: true,
        });
        let cursor =
            source.find("state.enemies[0].").expect("indexed receiver") + "state.enemies[0].".len();
        let completion = service
            .completion(&path_text, cursor, 64)
            .expect("indexed completion");
        assert!(completion
            .items
            .iter()
            .any(|item| item.text == "state.enemies[0].hp"));
        assert!(completion
            .items
            .iter()
            .any(|item| item.text == "state.enemies[0].speed"));
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
    fn navigation_and_symbols_share_compiler_owned_spans() {
        let (mut service, path, source) = intelligence_service();
        let call = source.find("spawn_enemy(1").expect("function call") + 2;
        let definitions = service.definition(&path, call).expect("definition");
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].path, path);
        assert_eq!(&source[definitions[0].range.clone()], "spawn_enemy");

        let references = service.references(&path, call, true).expect("references");
        assert!(references.len() >= 2);
        assert!(references
            .iter()
            .all(|location| &source[location.range.clone()] == "spawn_enemy"));

        let document = service.document_symbols(&path).expect("document symbols");
        assert!(document
            .iter()
            .any(|symbol| { symbol.name == "Enemy" && symbol.kind == LanguageSymbolKind::Struct }));
        assert!(document.iter().any(|symbol| {
            symbol.name == "spawn_enemy" && symbol.kind == LanguageSymbolKind::Function
        }));
        let workspace = service
            .workspace_symbols("spawn", 64)
            .expect("workspace symbols");
        assert_eq!(workspace.len(), 1);
        assert_eq!(workspace[0].name, "spawn_enemy");
    }

    #[test]
    fn indexed_reference_symbol_uses_semantic_field_path() {
        let source = "state.enemies[0].speed = 4;";
        let offset = source.find("speed").expect("speed") + 2;
        assert_eq!(
            reference_symbol_at(source, offset).as_deref(),
            Some("state.enemies.speed")
        );
    }

    #[test]
    fn global_receiver_and_owned_field_navigate_to_distinct_definitions() {
        let root = std::env::temp_dir().join("stasis-language-service-global-field-navigation");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let source = "struct State { x: i32; }\nglobal state: State;\nfunction main(): i32 { state.x = 7; return state.x; }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(path_text.clone(), source);
        let use_start = source.find("state.x =").expect("global field use");

        let global = service
            .definition(&path_text, use_start + 2)
            .expect("global definition");
        assert_eq!(global.len(), 1);
        assert_eq!(&source[global[0].range.clone()], "state");
        assert_eq!(source[..global[0].range.start].matches('\n').count(), 1);

        let field = service
            .definition(&path_text, use_start + "state.".len())
            .expect("field definition");
        assert_eq!(field.len(), 1);
        assert_eq!(&source[field[0].range.clone()], "x");
        assert_eq!(source[..field[0].range.start].matches('\n').count(), 0);

        let warm_revision = service
            .language_index
            .as_ref()
            .expect("warm definition index")
            .revision;
        let shifted = format!("\n{source}");
        service.set_disk_document(path_text.clone(), shifted.clone());
        let shifted_use = shifted.find("state.x =").expect("shifted field use");
        let shifted_field = service
            .definition(&path_text, shifted_use + "state.".len())
            .expect("definition from warm cache after unrelated edit");
        assert_eq!(&shifted[shifted_field[0].range.clone()], "x");
        assert_eq!(
            service
                .language_index
                .as_ref()
                .expect("preserved warm index")
                .revision,
            warm_revision,
            "an unrelated edit remaps the warm declaration without rebuilding the workspace",
        );

        let renamed = shifted.replace(" x:", " y:").replace("state.x", "state.y");
        service.set_disk_document(path_text.clone(), renamed.clone());
        let renamed_use = renamed.find("state.y =").expect("renamed field use");
        let renamed_field = service
            .definition(&path_text, renamed_use + "state.".len())
            .expect("definition after declaration edit");
        assert_eq!(&renamed[renamed_field[0].range.clone()], "y");
        assert_eq!(
            service
                .language_index
                .as_ref()
                .expect("refreshed definition index")
                .revision,
            service.snapshot().revision(),
            "changing a declaration rebuilds the warm cache for the new revision",
        );
    }

    #[test]
    fn warm_field_definition_rebuilds_when_the_global_owner_type_changes() {
        let root = std::env::temp_dir().join("stasis-language-service-global-type-edge");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let source = "struct Alpha { value: i32; }\nstruct Bravo { value: i32; }\nglobal state: Alpha;\nfunction main(): i32 { return state.value; }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(path_text.clone(), source);
        let use_offset = source.rfind("state.value").expect("field use") + "state.".len();
        let alpha = service
            .definition(&path_text, use_offset)
            .expect("alpha field definition");
        assert_eq!(source[..alpha[0].range.start].matches('\n').count(), 0);

        let changed = source.replace("state: Alpha", "state: Bravo");
        service.set_disk_document(path_text.clone(), changed.clone());
        let changed_use = changed.rfind("state.value").expect("changed field use") + "state.".len();
        let bravo = service
            .definition(&path_text, changed_use)
            .expect("bravo field definition");
        assert_eq!(changed[..bravo[0].range.start].matches('\n').count(), 1);
        assert_eq!(
            service
                .language_index
                .as_ref()
                .expect("rebuilt field index")
                .revision,
            service.snapshot().revision(),
        );
    }

    #[test]
    fn warm_definition_returns_every_overload() {
        let root = std::env::temp_dir().join("stasis-language-service-overload-definition");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let source = "function select(value: i32): i32 { return value; }\nfunction select(value: f32): f32 { return value; }\nfunction main(): i32 { return select(1); }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("language service");
        service.set_disk_document(path_text.clone(), source);
        let use_offset = source.rfind("select(1)").expect("overload use") + 2;

        let definitions = service
            .definition(&path_text, use_offset)
            .expect("overload definitions");
        assert_eq!(definitions.len(), 2);
        assert!(definitions
            .iter()
            .all(|location| &source[location.range.clone()] == "select"));
    }

    #[test]
    fn rename_is_revisioned_and_compiler_validated() {
        let (mut service, path, source) = intelligence_service();
        let call = source.find("spawn_enemy(1").expect("call") + 2;
        let prepared = service.prepare_rename(&path, call).expect("prepare rename");
        assert_eq!(prepared.placeholder, "spawn_enemy");
        assert_eq!(&source[prepared.range], "spawn_enemy");

        let revision = service.snapshot().revision();
        let plan = service
            .rename(&path, call, "create_enemy")
            .expect("validated rename");
        assert_eq!(plan.revision, revision);
        assert_eq!(plan.kind, "function");
        assert_eq!(plan.edits.len(), 2);
        assert!(plan.edits.iter().all(|edit| {
            edit.path == path
                && &source[edit.range.clone()] == "spawn_enemy"
                && edit.new_text == "create_enemy"
        }));

        let collision = service
            .rename(&path, call, "main")
            .expect_err("rename collision");
        assert!(collision.contains("collide"), "{collision}");
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
        service
            .semantic_tokens(&path)
            .expect("warm semantic tokens");
        service.inlay_hints(&path).expect("warm inlay hints");
        service
            .definition(&path, hover_offset)
            .expect("warm definition");

        let mut completion_micros = Vec::new();
        let mut hover_micros = Vec::new();
        let mut signature_micros = Vec::new();
        let mut semantic_token_micros = Vec::new();
        let mut inlay_hint_micros = Vec::new();
        let mut definition_micros = Vec::new();
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

            let started = Instant::now();
            service.semantic_tokens(&path).expect("semantic tokens");
            semantic_token_micros.push(started.elapsed().as_micros());

            let started = Instant::now();
            service.inlay_hints(&path).expect("inlay hints");
            inlay_hint_micros.push(started.elapsed().as_micros());

            let started = Instant::now();
            service.definition(&path, hover_offset).expect("definition");
            definition_micros.push(started.elapsed().as_micros());
        }
        completion_micros.sort_unstable();
        hover_micros.sort_unstable();
        signature_micros.sort_unstable();
        semantic_token_micros.sort_unstable();
        inlay_hint_micros.sort_unstable();
        definition_micros.sort_unstable();
        let p95 = |samples: &[u128]| samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
        let completion_p95 = p95(&completion_micros);
        let hover_p95 = p95(&hover_micros);
        let signature_p95 = p95(&signature_micros);
        let semantic_token_p95 = p95(&semantic_token_micros);
        let inlay_hint_p95 = p95(&inlay_hint_micros);
        let definition_p95 = p95(&definition_micros);
        eprintln!(
            "warm p95: completion={completion_p95}us hover={hover_p95}us signature={signature_p95}us semantic_tokens={semantic_token_p95}us inlay_hints={inlay_hint_p95}us definition={definition_p95}us"
        );
        assert!(completion_p95 < 20_000, "completion p95 {completion_p95}us");
        assert!(hover_p95 < 30_000, "hover p95 {hover_p95}us");
        assert!(signature_p95 < 30_000, "signature p95 {signature_p95}us");
        assert!(
            semantic_token_p95 < 30_000,
            "semantic tokens p95 {semantic_token_p95}us"
        );
        assert!(
            inlay_hint_p95 < 30_000,
            "inlay hints p95 {inlay_hint_p95}us"
        );
        assert!(
            definition_p95 < 100_000,
            "definition p95 {definition_p95}us"
        );
    }

    #[test]
    #[ignore = "requires STASIS_CHESSTD_ROOT"]
    fn chess_td_warm_definition_reports_service_component_latency() {
        fn collect_stasis_files(root: &Path, files: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(root).expect("read ChessTD source directory") {
                let path = entry.expect("ChessTD directory entry").path();
                if path.is_dir() {
                    collect_stasis_files(&path, files);
                } else if path
                    .extension()
                    .is_some_and(|extension| extension == "stasis")
                {
                    files.push(path);
                }
            }
        }

        let root = PathBuf::from(
            std::env::var("STASIS_CHESSTD_ROOT").expect("STASIS_CHESSTD_ROOT must name ChessTD"),
        );
        let mut paths = Vec::new();
        collect_stasis_files(&root.join("src"), &mut paths);
        collect_stasis_files(&root.join("tests"), &mut paths);
        paths.sort();
        let root_text = root.to_string_lossy().replace('\\', "/");
        let mut service = LanguageService::new(&root_text).expect("ChessTD service");
        let mut files = Vec::new();
        for path in paths {
            let source = std::fs::read_to_string(&path).expect("read ChessTD source");
            let path_text = path.to_string_lossy().replace('\\', "/");
            service.set_disk_document(path_text.clone(), source.clone());
            files.push(WorkshopSourceFile {
                path: canonical_source_path(Some(&root_text), &path_text)
                    .expect("canonical ChessTD path"),
                source,
            });
        }
        let main_path = root.join("src/main.stasis");
        let main_path = main_path.to_string_lossy().replace('\\', "/");
        let main_source = service
            .snapshot()
            .document(&main_path)
            .expect("ChessTD main source")
            .text
            .clone();
        let symbol = "game.progression_dirty";
        let offset = main_source.find(symbol).expect("ChessTD field use") + "game.".len() + 2;

        let diagnostics_started = Instant::now();
        let diagnostics = service.diagnostics();
        let diagnostics_millis = diagnostics_started.elapsed().as_millis();
        let diagnostic_count = diagnostics.diagnostics.len();
        let warm_started = Instant::now();
        service
            .workspace_symbols("", 256)
            .expect("warm ChessTD index");
        let warm_millis = warm_started.elapsed().as_millis();
        let legacy_started = Instant::now();
        let legacy = find_workshop_references(&files, symbol, 256).expect("legacy definition scan");
        let legacy_millis = legacy_started.elapsed().as_millis();
        assert!(legacy
            .iter()
            .any(|reference| reference.kind == WorkshopReferenceKind::Definition));

        let mut micros = Vec::new();
        for _ in 0..50 {
            let started = Instant::now();
            let definitions = service
                .definition(&main_path, offset)
                .expect("cached ChessTD definition");
            micros.push(started.elapsed().as_micros());
            assert_eq!(definitions.len(), 1);
            assert!(definitions[0].path.ends_with("src/game/model.stasis"));
        }
        micros.sort_unstable();
        let p95 = micros[47];
        eprintln!(
            "ChessTD definition: diagnostics={diagnostics_millis}ms/{diagnostic_count} warm_index={warm_millis}ms legacy_scan={legacy_millis}ms cached_p95={p95}us"
        );
        assert!(p95 < 100_000, "ChessTD cached definition p95 {p95}us");
    }

    #[test]
    fn hierarchy_recovers_from_incomplete_function_without_stale_call_ranges() {
        let root = std::env::temp_dir().join("stasis-language-service-hierarchy");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let valid = "function a(): i32 { return 1; }\nfunction b(): i32 { return a(); }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("service");
        service.set_disk_document(path_text.clone(), valid);
        let b_offset = valid.find("b():").expect("b") + 1;
        let b = service
            .prepare_call_hierarchy(&path_text, b_offset)
            .expect("prepare b")
            .pop()
            .expect("b item");
        let outgoing = service.outgoing_calls(&b.symbol_id).expect("outgoing");
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].item.name, "a");
        assert_eq!(&valid[outgoing[0].from_ranges[0].range.clone()], "a");

        let broken = "function a(): i32 { return 1; }\nfunction b(): i32 { a(state.";
        service.open_document(path_text.clone(), 1, broken);
        let a_call = broken.rfind("a(state").expect("incomplete call") + 1;
        let prepared = service
            .prepare_call_hierarchy(&path_text, a_call)
            .expect("last-good prepare");
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].name, "a");
        assert!(service
            .incoming_calls(&prepared[0].symbol_id)
            .expect("incoming")
            .is_empty());
        let broken_b_offset = broken.find("b():").expect("broken b") + 1;
        let broken_b = service
            .prepare_call_hierarchy(&path_text, broken_b_offset)
            .expect("last-good b prepare")
            .pop()
            .expect("b item");
        assert_eq!(broken_b.name, "b");
        assert!(service
            .outgoing_calls(&broken_b.symbol_id)
            .expect("broken outgoing")
            .is_empty());
    }

    #[test]
    fn type_hierarchy_exposes_struct_composition() {
        let root = std::env::temp_dir().join("stasis-language-service-type-hierarchy");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let source = "struct Position { x: i32; }\nstruct Enemy { position: Position; }\nfunction main(): i32 { return 0; }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("service");
        service.set_disk_document(path_text.clone(), source);
        let offset = source.find("Enemy").expect("Enemy") + 1;
        let enemy = service
            .prepare_type_hierarchy(&path_text, offset)
            .expect("prepare Enemy")
            .pop()
            .expect("Enemy item");
        let components = service.type_subtypes(&enemy.symbol_id).expect("components");
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].name, "Position");
        let containers = service
            .type_supertypes(&components[0].symbol_id)
            .expect("containers");
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].name, "Enemy");
    }

    #[test]
    fn folding_and_selection_use_current_incomplete_overlay() {
        let root = std::env::temp_dir().join("stasis-language-service-selection");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let source = "function a(): i32 { if (true) { return 1; } }\nfunction b(): i32 { a(state.";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("service");
        service.set_disk_document(path_text.clone(), source);
        let folds = service.folding_ranges(&path_text).expect("folds");
        assert_eq!(folds.len(), 2);
        let offset = source.find("return 1").expect("return") + 2;
        let selections = service
            .selection_ranges(&path_text, &[offset])
            .expect("selections");
        assert_eq!(&source[selections[0][0].clone()], "return");
        assert_eq!(
            selections[0].last().expect("file range"),
            &(0..source.len())
        );
    }

    #[test]
    fn linked_edits_use_current_compiler_scopes() {
        let root = std::env::temp_dir().join("stasis-language-service-linked-edit");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let source = "function add(value: i32): i32 { let copy: i32 = value; copy += value; return copy; }\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("service");
        service.set_disk_document(path_text.clone(), source);
        let offset = source.find("copy: i32").expect("copy") + 1;
        let ranges = service
            .linked_edit_ranges(&path_text, offset)
            .expect("linked edits")
            .expect("ranges");
        assert_eq!(ranges.len(), 3);

        service.open_document(
            path_text.clone(),
            1,
            format!("{source}function broken() {{"),
        );
        assert!(service
            .linked_edit_ranges(&path_text, offset)
            .expect("broken linked edit")
            .is_none());
    }

    #[test]
    fn document_range_and_on_type_formatting_share_canonical_formatter() {
        let root = std::env::temp_dir().join("stasis-language-service-formatting");
        let path = root.join("src/main.stasis");
        let path_text = path.to_string_lossy().replace('\\', "/");
        let source = "function main(): i32 {\r\nreturn 0;\r\n}\r\n";
        let mut service = LanguageService::new(root.to_string_lossy()).expect("service");
        service.set_disk_document(path_text.clone(), source);
        let document = service
            .format_document(&path_text)
            .expect("document format");
        assert_eq!(document.len(), 1);
        let edit = &document[0];
        let mut applied = source.to_string();
        applied.replace_range(edit.range.clone(), &edit.new_text);
        assert_eq!(applied, format_source(source).expect("canonical format"));
        assert_eq!(
            service
                .format_range(&path_text, 0..source.len())
                .expect("range format"),
            document
        );
        let close = source.rfind('}').expect("close") + 1;
        assert_eq!(
            service
                .format_on_type(&path_text, close, "}")
                .expect("on-type format"),
            document
        );
        assert!(service
            .format_on_type(&path_text, close, ",")
            .expect("unregistered type")
            .is_empty());
    }
}
