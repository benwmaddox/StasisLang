use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::Range;
use std::sync::Arc;

use crate::backend::emit::{
    parse_simple_statements_from_block, SimpleCondition, SimpleExpr, SimpleStmt,
};
use crate::data_flow::{build_function_data_flow_summaries, FunctionDataFlowSummary};
use crate::frontend::indexer::{hash_text, index_file, IndexedCallDependency};
use crate::frontend::module_graph::ModuleGraph;
use crate::frontend::types::{TypeId, TypeTable};
use crate::identity::{overload_discriminator, FnId, SymbolId};
use crate::ir::hir::{Block, FunctionHIR};

pub type FunctionId = FnId;
pub type FunctionStorageIndex = u32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    pub path: String,
    pub content: String,
    pub hash: u64,
    pub functions: Vec<FunctionId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionMeta {
    pub id: FunctionId,
    pub symbol_id: SymbolId,
    /// Dense compiler-owned storage position. Never serialize or use as identity.
    pub storage_index: FunctionStorageIndex,
    pub name: String,
    pub module_alias: String,
    pub name_hash: u64,
    pub file_id: u32,
    pub source_range: Range<u32>,
    pub signature_range: Range<u32>,
    pub signature_hash: u64,
    pub body_hash: u64,
    pub param_names: Vec<String>,
    pub params: Vec<TypeId>,
    pub return_type: TypeId,
    pub dependencies: Vec<FunctionId>,
    pub dependents: Vec<FunctionId>,
    pub dirty: bool,
}

/// Parser-owned source item used by hosts that need function ranges/names before
/// choosing a target contract. Hosts must not independently reparse Stasis files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFunctionItem {
    pub path: String,
    pub name: String,
    pub source_range: Range<u32>,
    pub signature_range: Range<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStructItem {
    pub path: String,
    pub name: String,
    pub definition_range: Range<u32>,
}

/// Parser-owned declaration bundle for editor tooling.  It is intentionally a
/// frontend record, not a semantic compilation result: callers use it for an
/// already-open workspace without inventing a second parser pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceWorkshopItems {
    pub layout: crate::frontend::parser::ParsedTypeLayout,
    pub functions: Vec<crate::frontend::parser::ParsedFunctionSignature>,
    pub typed_local_bindings: Vec<crate::frontend::parser::ParsedLocalBinding>,
    pub structs: Vec<crate::frontend::parser::ParsedStructDefinitionRange>,
}

pub fn source_workshop_items(source: &str) -> Result<SourceWorkshopItems, String> {
    Ok(SourceWorkshopItems {
        layout: crate::frontend::parser::parse_top_level_type_layout(source)?,
        functions: crate::frontend::parser::parse_top_level_functions(source)?,
        typed_local_bindings: crate::frontend::parser::parse_typed_local_bindings(source)?,
        structs: crate::frontend::parser::parse_top_level_struct_definitions(source)?,
    })
}

pub fn source_function_items(
    files: impl IntoIterator<Item = (String, String)>,
) -> Result<Vec<SourceFunctionItem>, String> {
    let mut items = Vec::new();
    for (path, content) in files {
        for item in crate::frontend::indexer::source_function_items(&content)? {
            items.push(SourceFunctionItem {
                path: path.clone(),
                name: item.name,
                source_range: item.source_range,
                signature_range: item.signature_range,
            });
        }
    }
    Ok(items)
}

pub fn source_struct_items(
    source: &str,
    path: impl Into<String>,
) -> Result<Vec<SourceStructItem>, String> {
    let path = path.into();
    crate::frontend::parser::parse_top_level_struct_definitions(source).map(|items| {
        items
            .into_iter()
            .map(|item| SourceStructItem {
                path: path.clone(),
                name: item.name,
                definition_range: item.definition_range.start as u32
                    ..item.definition_range.end as u32,
            })
            .collect()
    })
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SymbolEntry {
    name_hash: u64,
    function_id: FunctionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StatementCacheKey {
    path: String,
    module_context_hash: u64,
    name_hash: u64,
    signature_hash: u64,
    body_hash: u64,
}

#[derive(Debug, Clone, Default)]
struct ModuleResolutionIndex {
    function_indices_by_name: HashMap<String, Vec<usize>>,
    context_hash_by_path: BTreeMap<String, u64>,
}

impl ModuleResolutionIndex {
    fn build(graph: &ModuleGraph, files: &[SourceFile], functions: &[FunctionMeta]) -> Self {
        let mut function_indices_by_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut signatures_by_path: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for (index, function) in functions.iter().enumerate() {
            function_indices_by_name
                .entry(function.name.clone())
                .or_default()
                .push(index);
            let path = files[function.file_id as usize].path.as_str();
            signatures_by_path.entry(path).or_default().push(format!(
                "{path}|{}|{}|{}",
                function.module_alias, function.name, function.signature_hash
            ));
        }

        let context_hash_by_path = files
            .iter()
            .map(|file| {
                let visible_paths = graph.dependency_closure(&file.path);
                let mut context = visible_paths.iter().cloned().collect::<Vec<_>>();
                for path in &visible_paths {
                    if let Some(signatures) = signatures_by_path.get(path.as_str()) {
                        context.extend(signatures.iter().cloned());
                    }
                }
                (file.path.clone(), hash_text(&context.join("\n")))
            })
            .collect();

        Self {
            function_indices_by_name,
            context_hash_by_path,
        }
    }

    fn function_indices(&self, name: &str) -> &[usize] {
        self.function_indices_by_name
            .get(name)
            .map_or(&[], Vec::as_slice)
    }

    fn context_hash(&self, path: &str) -> Option<u64> {
        self.context_hash_by_path.get(path).copied()
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    slots: Vec<Option<SymbolEntry>>,
    len: usize,
}

#[cfg(test)]
impl SymbolTable {
    const MIN_CAPACITY: usize = 8;
    const LOAD_FACTOR_NUMERATOR: usize = 7;
    const LOAD_FACTOR_DENOMINATOR: usize = 10;

    fn insert(&mut self, name_hash: u64, function_id: FunctionId) {
        self.ensure_capacity_for_insert();
        self.insert_no_resize(name_hash, function_id);
    }

    fn get(&self, name_hash: u64) -> Option<FunctionId> {
        if self.len == 0 || self.slots.is_empty() {
            return None;
        }

        let mut index = self.bucket_index(name_hash);
        loop {
            match self.slots[index] {
                Some(entry) if entry.name_hash == name_hash => return Some(entry.function_id),
                Some(_) => {
                    index = (index + 1) & (self.slots.len() - 1);
                }
                None => return None,
            }
        }
    }

    fn ensure_capacity_for_insert(&mut self) {
        if self.slots.is_empty() {
            self.slots = vec![None; Self::MIN_CAPACITY];
            self.len = 0;
            return;
        }

        let threshold =
            (self.slots.len() * Self::LOAD_FACTOR_NUMERATOR) / Self::LOAD_FACTOR_DENOMINATOR;
        if self.len + 1 <= threshold {
            return;
        }

        self.resize(self.slots.len() * 2);
    }

    fn resize(&mut self, requested_capacity: usize) {
        let mut capacity = requested_capacity.max(Self::MIN_CAPACITY);
        if !capacity.is_power_of_two() {
            capacity = capacity.next_power_of_two();
        }

        let previous_slots = std::mem::replace(&mut self.slots, vec![None; capacity]);
        self.len = 0;
        for entry in previous_slots.into_iter().flatten() {
            self.insert_no_resize(entry.name_hash, entry.function_id);
        }
    }

    fn insert_no_resize(&mut self, name_hash: u64, function_id: FunctionId) {
        let mut index = self.bucket_index(name_hash);
        loop {
            match &mut self.slots[index] {
                Some(entry) if entry.name_hash == name_hash => {
                    entry.function_id = function_id;
                    return;
                }
                Some(_) => {
                    index = (index + 1) & (self.slots.len() - 1);
                }
                slot @ None => {
                    *slot = Some(SymbolEntry {
                        name_hash,
                        function_id,
                    });
                    self.len += 1;
                    return;
                }
            }
        }
    }

    fn bucket_index(&self, name_hash: u64) -> usize {
        (name_hash as usize) & (self.slots.len() - 1)
    }

    #[cfg(test)]
    fn with_test_capacity(capacity: usize) -> Self {
        let mut table = Self::default();
        table.resize(capacity);
        table
    }
}

#[derive(Debug, Clone, Default)]
pub struct DependencyGraph;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    Frontend(String),
    Backend(String),
    Invariant(String),
}

pub type CompileResult<T> = Result<T, CompileError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexPassResult {
    pub parsed_functions: usize,
    pub dirty_functions: usize,
    pub signature_changed_functions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitPassResult {
    pub emitted_functions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileReport {
    pub index: IndexPassResult,
    pub emit: EmitPassResult,
}

#[derive(Debug, Clone, Default)]
pub struct Compiler {
    files: Vec<SourceFile>,
    functions: Vec<FunctionMeta>,
    function_index_by_id: HashMap<FunctionId, usize>,
    deps: DependencyGraph,
    types: TypeTable,
    parsed_statements: Vec<Vec<SimpleStmt>>,
    parsed_statement_ids: BTreeSet<FunctionId>,
    statement_cache: HashMap<StatementCacheKey, Vec<SimpleStmt>>,
    analysis_required_roots: Vec<String>,
    data_flow_summaries: Arc<[FunctionDataFlowSummary]>,
    data_flow_context_fingerprint: u64,
    #[cfg(test)]
    statement_parse_count: usize,
    last_source_diagnostic: Option<crate::SourceDiagnostic>,
    project_root: Option<String>,
    pending_path_error: Option<String>,
    module_graph: ModuleGraph,
    module_resolution: ModuleResolutionIndex,
    entry_roots: BTreeSet<String>,
    indexed_file_hashes: BTreeMap<String, u64>,
}

impl Compiler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        let path = path.into();
        let normalized_path =
            match crate::identity::canonical_source_path(self.project_root.as_deref(), &path) {
                Ok(path) => path,
                Err(error) => {
                    self.pending_path_error = Some(error);
                    return;
                }
            };
        self.entry_roots.insert(normalized_path.clone());
        let content = content.into();
        self.upsert_loaded_file(normalized_path, content);
    }

    fn upsert_loaded_file(&mut self, normalized_path: String, content: String) {
        let hash = hash_text(&content);
        if let Some(existing) = self
            .files
            .iter_mut()
            .find(|file| file.path == normalized_path)
        {
            existing.content = content;
            existing.hash = hash;
            return;
        }
        self.files.push(SourceFile {
            path: normalized_path,
            content,
            hash,
            functions: Vec::new(),
        });
    }

    pub fn retain_files(&mut self, paths: &BTreeSet<String>) {
        let normalized: BTreeSet<_> = paths
            .iter()
            .filter_map(|path| {
                crate::identity::canonical_source_path(self.project_root.as_deref(), path).ok()
            })
            .collect();
        self.files.retain(|file| normalized.contains(&file.path));
        self.entry_roots.retain(|path| normalized.contains(path));
    }

    pub fn set_project_root(&mut self, root: impl Into<String>) -> Result<(), String> {
        let root = root.into().replace('\\', "/");
        if root.to_ascii_uppercase().starts_with("//?/UNC/") {
            return Err(format!("UNC project roots are not supported: '{root}'"));
        }
        let root = root.strip_prefix("//?/").unwrap_or(&root).to_string();
        if !root.starts_with('/') && root.as_bytes().get(1) != Some(&b':') {
            return Err(format!("project root must be absolute: '{root}'"));
        }
        if root.starts_with("//") {
            return Err(format!("UNC project roots are not supported: '{root}'"));
        }
        self.project_root = Some(root);
        Ok(())
    }

    pub(crate) fn project_root(&self) -> Option<&str> {
        self.project_root.as_deref()
    }

    pub fn module_graph(&self) -> &ModuleGraph {
        &self.module_graph
    }

    pub fn refresh_module_graph(&mut self) -> CompileResult<()> {
        if self.entry_roots.is_empty() {
            self.module_graph = ModuleGraph::default();
            return Ok(());
        }
        let available: BTreeMap<String, String> = self
            .files
            .iter()
            .map(|file| (file.path.clone(), file.content.clone()))
            .collect();
        let project_root = self.project_root.clone();
        let mut confined_root = None;
        let result = ModuleGraph::load(self.entry_roots.iter().cloned(), |path| {
            if let Some(source) = available.get(path) {
                return Ok(source.clone());
            }
            let root = project_root.as_deref().ok_or_else(|| {
                format!("missing imported module '{path}': compiler project root is not set")
            })?;
            if confined_root.is_none() {
                confined_root = Some(
                    crate::frontend::module_graph::ConfinedProjectRoot::new(std::path::Path::new(
                        root,
                    ))
                    .map_err(|message| format!("missing imported module '{path}': {message}"))?,
                );
            }
            confined_root.as_ref().unwrap().read_source(path)
        });
        let (mut graph, loaded_sources) = match result {
            Ok(result) => result,
            Err(diagnostic) => {
                let message = diagnostic.message.clone();
                self.last_source_diagnostic = Some(diagnostic);
                return Err(CompileError::Frontend(message));
            }
        };
        let imported: BTreeSet<String> = graph
            .modules()
            .values()
            .flat_map(|module| module.imports.iter().map(|import| import.target.clone()))
            .collect();
        let inferred_roots: BTreeSet<String> =
            self.entry_roots.difference(&imported).cloned().collect();
        if !inferred_roots.is_empty() {
            self.entry_roots = inferred_roots;
            graph.set_roots(self.entry_roots.clone());
        }
        let closure: BTreeSet<String> = graph.modules().keys().cloned().collect();
        self.files.retain(|file| closure.contains(&file.path));
        for (path, source) in loaded_sources {
            self.upsert_loaded_file(path, source);
        }
        self.files.sort_by(|left, right| left.path.cmp(&right.path));
        self.module_graph = graph;
        Ok(())
    }

    pub fn compile_with<F>(&mut self, mut emit_function: F) -> CompileResult<CompileReport>
    where
        F: FnMut(&FunctionMeta, &FunctionHIR, &TypeTable) -> Result<(), String>,
    {
        let index = self.index_pass()?;
        let emit = self.emit_pass_with(&mut emit_function)?;
        Ok(CompileReport { index, emit })
    }

    pub fn index_pass(&mut self) -> CompileResult<IndexPassResult> {
        self.index_pass_with_scope(false)
    }

    pub fn check(&mut self) -> CompileResult<IndexPassResult> {
        let index = self.index_pass_with_scope(true)?;
        let functions = self.functions.clone();
        for function in functions {
            if let Err(error) = self.lower_function_to_hir(&function) {
                self.record_function_diagnostic(&function, compile_error_message(&error));
                return Err(error);
            }
        }
        Ok(index)
    }

    fn index_pass_with_scope(&mut self, analyze_all: bool) -> CompileResult<IndexPassResult> {
        self.last_source_diagnostic = None;
        if let Some(error) = self.pending_path_error.take() {
            return Err(CompileError::Frontend(error));
        }
        self.refresh_module_graph()?;
        let changed_paths: Vec<String> = self
            .files
            .iter()
            .filter(|file| self.indexed_file_hashes.get(&file.path) != Some(&file.hash))
            .map(|file| file.path.clone())
            .collect();
        let reverse_invalidated: BTreeSet<String> = changed_paths
            .iter()
            .flat_map(|path| self.module_graph.invalidation_closure(path))
            .filter(|path| !changed_paths.contains(path))
            .collect();
        let has_tick_budget_annotation = self
            .files
            .iter()
            .any(|file| file.content.contains("@tick_budget_us"));
        let tick_budget_result = if has_tick_budget_annotation {
            crate::performance::tick_budget_us(&self.files)
        } else {
            Ok(None)
        };
        if let Err(message) = tick_budget_result {
            let file = self
                .files
                .iter()
                .find(|file| {
                    crate::performance::tick_budget_us(std::slice::from_ref(file)).is_err()
                })
                .or_else(|| {
                    self.files
                        .iter()
                        .filter(|file| file.content.contains("@tick_budget_us"))
                        .nth(1)
                })
                .or_else(|| self.files.first());
            if let Some(file) = file {
                let is_tick_budget_error = message.contains("tick_budget_us");
                let start = is_tick_budget_error
                    .then(|| file.content.find("@tick_budget_us"))
                    .flatten()
                    .unwrap_or(0);
                let end = if is_tick_budget_error {
                    start
                        .saturating_add("@tick_budget_us".len())
                        .min(file.content.len())
                } else {
                    file.content.len()
                };
                let symbol = is_tick_budget_error
                    .then(|| {
                        crate::frontend::parser::parse_top_level_functions(&file.content)
                            .ok()
                            .and_then(|functions| {
                                functions.into_iter().find(|function| {
                                    function
                                        .annotations
                                        .iter()
                                        .any(|annotation| annotation.name == "tick_budget_us")
                                })
                            })
                            .map(|function| function.name)
                    })
                    .flatten()
                    .unwrap_or_default();
                self.last_source_diagnostic = Some(crate::SourceDiagnostic {
                    path: file.path.clone(),
                    start,
                    end,
                    symbol,
                    message: message.clone(),
                });
            }
            return Err(CompileError::Frontend(message));
        }
        let previous_hashes = self.capture_previous_hashes();
        self.functions.clear();
        self.function_index_by_id.clear();
        self.parsed_statements.clear();
        self.parsed_statement_ids.clear();
        self.deps = DependencyGraph;

        let mut dependencies_by_function: Vec<Vec<IndexedCallDependency>> = Vec::new();
        let mut signature_changed_ids: Vec<FunctionId> = Vec::new();

        for file_id in 0..self.files.len() {
            let indexed = match index_file(&self.files[file_id].content, &mut self.types) {
                Ok(indexed) => indexed,
                Err(message) => {
                    let file = &self.files[file_id];
                    self.last_source_diagnostic = Some(crate::SourceDiagnostic {
                        path: file.path.clone(),
                        start: 0,
                        end: file.content.len(),
                        symbol: String::new(),
                        message: message.clone(),
                    });
                    return Err(CompileError::Frontend(message));
                }
            };
            self.files[file_id].functions.clear();
            for indexed_function in indexed {
                let storage_index = self.functions.len() as FunctionStorageIndex;
                let source_path = crate::identity::CanonicalSourcePath::project_relative(
                    &self.files[file_id].path,
                )
                .map_err(CompileError::Invariant)?;
                let symbol_id = SymbolId::function(
                    &source_path,
                    &indexed_function.name,
                    &overload_discriminator(&indexed_function.param_type_names),
                );
                let function_id = symbol_id.fn_id();
                if let Some(existing_index) = self.function_index_by_id.get(&function_id).copied() {
                    let existing = &self.functions[existing_index];
                    if existing.symbol_id != symbol_id {
                        return Err(CompileError::Invariant(format!(
                            "FnId collision {function_id:08x}: '{}' and '{}'",
                            existing.symbol_id, symbol_id
                        )));
                    }
                    return Err(CompileError::Frontend(format!(
                        "duplicate declaration identity '{symbol_id}'"
                    )));
                }
                self.function_index_by_id
                    .insert(function_id, storage_index as usize);
                self.files[file_id].functions.push(function_id);
                let previous = previous_hashes.get(&symbol_id).copied();
                let signature_changed = previous
                    .is_none_or(|old| old.signature_hash != indexed_function.signature_hash);
                let body_changed =
                    previous.is_none_or(|old| old.body_hash != indexed_function.body_hash);
                if signature_changed {
                    signature_changed_ids.push(function_id);
                }

                dependencies_by_function.push(indexed_function.dependencies);
                self.functions.push(FunctionMeta {
                    id: function_id,
                    symbol_id,
                    storage_index,
                    name: indexed_function.name,
                    module_alias: self
                        .module_graph
                        .module(&self.files[file_id].path)
                        .map_or_else(String::new, |module| module.alias.clone()),
                    name_hash: indexed_function.name_hash,
                    file_id: file_id as u32,
                    source_range: indexed_function.source_range,
                    signature_range: indexed_function.signature_range,
                    signature_hash: indexed_function.signature_hash,
                    body_hash: indexed_function.body_hash,
                    param_names: indexed_function.param_names,
                    params: indexed_function.params,
                    return_type: indexed_function.return_type,
                    dependencies: Vec::new(),
                    dependents: Vec::new(),
                    dirty: signature_changed
                        || body_changed
                        || reverse_invalidated.contains(&self.files[file_id].path),
                });
            }
        }
        self.module_resolution =
            ModuleResolutionIndex::build(&self.module_graph, &self.files, &self.functions);

        let mut unique_edges = BTreeSet::new();
        for (caller_index, dependencies) in dependencies_by_function.into_iter().enumerate() {
            let caller = self.functions[caller_index].id;
            let caller_path = &self.files[self.functions[caller_index].file_id as usize].path;
            for dependency in dependencies {
                let resolution = match resolve_module_call(
                    dependency.qualifier.as_deref(),
                    &dependency.name,
                    caller_path,
                    &self.module_graph,
                    &self.files,
                    &self.functions,
                    &self.module_resolution,
                ) {
                    Ok(resolution) => resolution,
                    Err(error) => {
                        let relative_span = dependency.name_span.clone();
                        let base = self.functions[caller_index].source_range.start as usize;
                        let message =
                            module_call_resolution_message(error, &dependency.name, caller_path);
                        self.last_source_diagnostic = Some(crate::SourceDiagnostic {
                            path: caller_path.clone(),
                            start: base + relative_span.start as usize,
                            end: base + relative_span.end as usize,
                            symbol: dependency.name.clone(),
                            message: message.clone(),
                        });
                        return Err(CompileError::Frontend(message));
                    }
                };
                let Some(module_alias) = resolution.module_alias else {
                    continue;
                };
                for callee in self
                    .module_resolution
                    .function_indices(&dependency.name)
                    .iter()
                    .map(|index| &self.functions[*index])
                    .filter(|function| function.module_alias == module_alias)
                {
                    if caller != callee.id {
                        unique_edges.insert((caller, callee.id));
                    }
                }
            }
        }
        for (caller, callee) in unique_edges {
            let caller_index = self.function_index(caller)?;
            let callee_index = self.function_index(callee)?;
            self.functions[caller_index].dependencies.push(callee);
            self.functions[callee_index].dependents.push(caller);
        }

        self.parsed_statements = vec![Vec::new(); self.functions.len()];
        let reachable = if analyze_all {
            self.functions.iter().map(|function| function.id).collect()
        } else {
            crate::backend::reachability::compute_reachable_function_ids(
                &self.functions,
                &self.analysis_required_roots,
            )
        };
        self.prepare_statement_artifacts(&reachable.iter().copied().collect::<Vec<_>>())?;

        self.propagate_dirty_from_signature_changes(&signature_changed_ids);
        let dirty_functions = self
            .functions
            .iter()
            .filter(|function| function.dirty)
            .count();
        self.indexed_file_hashes = self
            .files
            .iter()
            .map(|file| (file.path.clone(), file.hash))
            .collect();
        Ok(IndexPassResult {
            parsed_functions: self.functions.len(),
            dirty_functions,
            signature_changed_functions: signature_changed_ids.len(),
        })
    }

    fn prepare_statement_artifacts(&mut self, function_ids: &[FunctionId]) -> CompileResult<()> {
        if function_ids
            .iter()
            .all(|function_id| self.parsed_statement_ids.contains(function_id))
        {
            return Ok(());
        }
        let mut next_statement_cache = HashMap::with_capacity(function_ids.len());
        let mut changed_function_ids = BTreeSet::new();
        for function_id in function_ids {
            if self.parsed_statement_ids.contains(function_id) {
                continue;
            }
            let function_index = self.function_index(*function_id)?;
            let function = &self.functions[function_index];
            let file = &self.files[function.file_id as usize];
            let module_context_hash =
                self.module_resolution
                    .context_hash(&file.path)
                    .ok_or_else(|| {
                        CompileError::Invariant(format!(
                            "missing module resolution context for '{}'",
                            file.path
                        ))
                    })?;
            let key = StatementCacheKey {
                path: file.path.clone(),
                module_context_hash,
                name_hash: function.name_hash,
                signature_hash: function.signature_hash,
                body_hash: function.body_hash,
            };
            let statements = if let Some(cached) = self.statement_cache.get(&key) {
                cached.clone()
            } else {
                changed_function_ids.insert(*function_id);
                #[cfg(test)]
                {
                    self.statement_parse_count += 1;
                }
                let body = file
                    .content
                    .get(function.source_range.start as usize..function.source_range.end as usize)
                    .ok_or_else(|| {
                        CompileError::Invariant(format!(
                            "function '{}' body range is invalid",
                            function.name
                        ))
                    })?;
                let statements = match parse_simple_statements_from_block(body, &mut self.types) {
                    Ok(statements) => statements,
                    Err(message) => {
                        self.last_source_diagnostic = Some(crate::SourceDiagnostic {
                            path: file.path.clone(),
                            start: function.source_range.start as usize,
                            end: function.source_range.end as usize,
                            symbol: function.name.clone(),
                            message: message.clone(),
                        });
                        return Err(CompileError::Backend(message));
                    }
                };
                let mut validated = statements.clone();
                if let Err(message) = qualify_module_calls(
                    &mut validated,
                    &file.path,
                    &self.module_graph,
                    &self.files,
                    &self.functions,
                    &self.module_resolution,
                ) {
                    return Err(CompileError::Frontend(message));
                }
                statements
            };
            next_statement_cache.insert(key, statements.clone());
            self.parsed_statements[function.storage_index as usize] = statements;
            self.parsed_statement_ids.insert(*function_id);
        }
        self.statement_cache = next_statement_cache;
        let (summaries, context_fingerprint) = build_function_data_flow_summaries(
            &self.files,
            &self.functions,
            &self.parsed_statements,
            &self.parsed_statement_ids,
            &changed_function_ids,
            &self.types,
            &self.data_flow_summaries,
            self.data_flow_context_fingerprint,
        )
        .map_err(CompileError::Backend)?;
        if let Some(summaries) = summaries {
            self.data_flow_summaries = summaries.into();
        }
        self.data_flow_context_fingerprint = context_fingerprint;
        Ok(())
    }

    pub fn emit_pass_with<F>(&mut self, emit_function: &mut F) -> CompileResult<EmitPassResult>
    where
        F: FnMut(&FunctionMeta, &FunctionHIR, &TypeTable) -> Result<(), String>,
    {
        let dirty_ids: Vec<FunctionId> = self
            .functions
            .iter()
            .filter(|function| function.dirty)
            .map(|function| function.id)
            .collect();
        self.emit_pass_for_ids_with(&dirty_ids, emit_function)
    }

    pub fn emit_pass_for_ids_with<F>(
        &mut self,
        function_ids: &[FunctionId],
        emit_function: &mut F,
    ) -> CompileResult<EmitPassResult>
    where
        F: FnMut(&FunctionMeta, &FunctionHIR, &TypeTable) -> Result<(), String>,
    {
        self.prepare_statement_artifacts(function_ids)?;
        let mut emitted_functions = 0usize;
        let mut emitted_ids: Vec<FunctionId> = Vec::with_capacity(function_ids.len());
        for function_id in function_ids {
            let snapshot = self
                .function_index_by_id
                .get(function_id)
                .and_then(|index| self.functions.get(*index))
                .ok_or_else(|| {
                    CompileError::Invariant(format!("invalid function id {}", function_id))
                })?
                .clone();
            let hir = match self.lower_function_to_hir(&snapshot) {
                Ok(hir) => hir,
                Err(error) => {
                    self.record_function_diagnostic(&snapshot, compile_error_message(&error));
                    return Err(error);
                }
            };
            if let Err(message) = emit_function(&snapshot, &hir, &self.types) {
                self.record_function_diagnostic(&snapshot, &message);
                return Err(CompileError::Backend(message));
            }
            emitted_ids.push(*function_id);
            emitted_functions += 1;
        }
        for function_id in emitted_ids {
            let index = self.function_index(function_id)?;
            self.functions[index].dirty = false;
        }
        Ok(EmitPassResult { emitted_functions })
    }

    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }

    pub fn functions(&self) -> &[FunctionMeta] {
        &self.functions
    }

    pub fn function_data_flow_summaries(&self) -> &[FunctionDataFlowSummary] {
        &self.data_flow_summaries
    }

    pub(crate) fn data_flow_summaries_shared(&self) -> Arc<[FunctionDataFlowSummary]> {
        Arc::clone(&self.data_flow_summaries)
    }

    pub fn set_analysis_required_roots(&mut self, roots: &[String]) {
        self.analysis_required_roots.clear();
        self.analysis_required_roots.extend_from_slice(roots);
    }

    pub fn types(&self) -> &TypeTable {
        &self.types
    }

    pub fn types_mut(&mut self) -> &mut TypeTable {
        &mut self.types
    }

    pub fn last_source_diagnostic(&self) -> Option<&crate::SourceDiagnostic> {
        self.last_source_diagnostic.as_ref()
    }

    fn record_function_diagnostic(&mut self, function: &FunctionMeta, message: &str) {
        let Some(file) = self.files.get(function.file_id as usize) else {
            return;
        };
        self.last_source_diagnostic = Some(crate::SourceDiagnostic {
            path: file.path.clone(),
            start: function.source_range.start as usize,
            end: function.source_range.end as usize,
            symbol: function.name.clone(),
            message: message.to_string(),
        });
    }

    fn capture_previous_hashes(&self) -> HashMap<SymbolId, PreviousFunctionHashes> {
        let mut out = HashMap::new();
        for function in &self.functions {
            out.insert(
                function.symbol_id.clone(),
                PreviousFunctionHashes {
                    signature_hash: function.signature_hash,
                    body_hash: function.body_hash,
                },
            );
        }
        out
    }

    fn propagate_dirty_from_signature_changes(&mut self, roots: &[FunctionId]) {
        let mut queue = VecDeque::new();
        for root in roots {
            queue.push_back(*root);
        }
        while let Some(function_id) = queue.pop_front() {
            let Some(index) = self.function_index_by_id.get(&function_id).copied() else {
                continue;
            };
            let dependents = self.functions[index].dependents.clone();
            for dependent_id in dependents {
                let Some(dependent_index) = self.function_index_by_id.get(&dependent_id).copied()
                else {
                    continue;
                };
                if !self.functions[dependent_index].dirty {
                    self.functions[dependent_index].dirty = true;
                    queue.push_back(dependent_id);
                }
            }
        }
    }

    fn lower_function_to_hir(&mut self, function: &FunctionMeta) -> CompileResult<FunctionHIR> {
        let file = self.files.get(function.file_id as usize).ok_or_else(|| {
            CompileError::Invariant("function references missing file".to_string())
        })?;
        let body = file
            .content
            .get(function.source_range.start as usize..function.source_range.end as usize)
            .ok_or_else(|| {
                CompileError::Invariant("function body range out of bounds".to_string())
            })?
            .to_string();
        let mut statements = self
            .parsed_statements
            .get(function.storage_index as usize)
            .cloned()
            .ok_or_else(|| {
                CompileError::Invariant(format!(
                    "function '{}' has no parsed statement artifact",
                    function.name
                ))
            })?;
        qualify_module_calls(
            &mut statements,
            &file.path,
            &self.module_graph,
            &self.files,
            &self.functions,
            &self.module_resolution,
        )
        .map_err(CompileError::Frontend)?;
        Ok(FunctionHIR {
            blocks: vec![Block { source: body }],
            statements,
        })
    }

    fn function_index(&self, id: FunctionId) -> CompileResult<usize> {
        self.function_index_by_id
            .get(&id)
            .copied()
            .ok_or_else(|| CompileError::Invariant(format!("unknown stable function id {id:08x}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleCallResolutionError {
    Ambiguous,
    Inaccessible,
}

struct ModuleCallResolution {
    module_alias: Option<String>,
    consume_qualifier: bool,
}

fn resolve_module_call(
    qualifier: Option<&str>,
    name: &str,
    caller_path: &str,
    graph: &ModuleGraph,
    files: &[SourceFile],
    functions: &[FunctionMeta],
    resolution: &ModuleResolutionIndex,
) -> Result<ModuleCallResolution, ModuleCallResolutionError> {
    if let Some(alias) = qualifier {
        if graph.imported_alias_target(caller_path, alias).is_some() {
            return Ok(ModuleCallResolution {
                module_alias: Some(alias.to_string()),
                consume_qualifier: true,
            });
        }
    }

    let candidates = resolution.function_indices(name);
    let local_modules: BTreeSet<String> = candidates
        .iter()
        .filter_map(|index| {
            let function = &functions[*index];
            (files[function.file_id as usize].path == caller_path)
                .then(|| function.module_alias.clone())
        })
        .collect();
    let modules = if local_modules.is_empty() {
        let mut imported_paths = graph.dependency_closure(caller_path);
        imported_paths.remove(caller_path);
        candidates
            .iter()
            .filter_map(|index| {
                let function = &functions[*index];
                imported_paths
                    .contains(&files[function.file_id as usize].path)
                    .then(|| function.module_alias.clone())
            })
            .collect::<BTreeSet<_>>()
    } else {
        local_modules
    };
    match modules.len() {
        0 if qualifier.is_none() && !candidates.is_empty() => {
            Err(ModuleCallResolutionError::Inaccessible)
        }
        0 => Ok(ModuleCallResolution {
            module_alias: None,
            consume_qualifier: false,
        }),
        1 => Ok(ModuleCallResolution {
            module_alias: modules.into_iter().next(),
            consume_qualifier: false,
        }),
        _ => Err(ModuleCallResolutionError::Ambiguous),
    }
}

fn module_call_resolution_message(
    error: ModuleCallResolutionError,
    name: &str,
    caller_path: &str,
) -> String {
    match error {
        ModuleCallResolutionError::Ambiguous => format!(
            "ambiguous unqualified call '{}'; qualify it as module.{}",
            name, name
        ),
        ModuleCallResolutionError::Inaccessible => format!(
            "unqualified call '{}' is not visible from '{}'; import its module",
            name, caller_path
        ),
    }
}

fn qualify_module_calls(
    statements: &mut [SimpleStmt],
    caller_path: &str,
    graph: &ModuleGraph,
    files: &[SourceFile],
    functions: &[FunctionMeta],
    resolution: &ModuleResolutionIndex,
) -> Result<(), String> {
    fn expression(
        value: &mut SimpleExpr,
        caller_path: &str,
        graph: &ModuleGraph,
        files: &[SourceFile],
        functions: &[FunctionMeta],
        resolution: &ModuleResolutionIndex,
    ) -> Result<(), String> {
        match value {
            SimpleExpr::Condition(condition) => {
                condition_value(condition, caller_path, graph, files, functions, resolution)
            }
            SimpleExpr::IndexedPath { index, .. } => {
                expression(index, caller_path, graph, files, functions, resolution)
            }
            SimpleExpr::Call { target, args } => {
                for argument in args.iter_mut() {
                    expression(argument, caller_path, graph, files, functions, resolution)?;
                }
                let qualifier = args.first().and_then(|argument| match argument {
                    SimpleExpr::Identifier(alias) => Some(alias.as_str()),
                    _ => None,
                });
                match resolve_module_call(
                    qualifier,
                    target,
                    caller_path,
                    graph,
                    files,
                    functions,
                    resolution,
                ) {
                    Ok(resolution) => {
                        if let Some(alias) = resolution.module_alias {
                            *target = format!("{alias}.{target}");
                        }
                        if resolution.consume_qualifier {
                            args.remove(0);
                        }
                        Ok(())
                    }
                    Err(error) => Err(module_call_resolution_message(error, target, caller_path)),
                }
            }
            SimpleExpr::Binary { lhs, rhs, .. } => {
                expression(lhs, caller_path, graph, files, functions, resolution)?;
                expression(rhs, caller_path, graph, files, functions, resolution)
            }
            SimpleExpr::Int(_)
            | SimpleExpr::Float(_)
            | SimpleExpr::Bool(_)
            | SimpleExpr::StringLiteral(_)
            | SimpleExpr::Identifier(_) => Ok(()),
        }
    }

    fn condition_value(
        condition: &mut SimpleCondition,
        caller_path: &str,
        graph: &ModuleGraph,
        files: &[SourceFile],
        functions: &[FunctionMeta],
        resolution: &ModuleResolutionIndex,
    ) -> Result<(), String> {
        match condition {
            SimpleCondition::Comparison { lhs, rhs, .. } => {
                expression(lhs, caller_path, graph, files, functions, resolution)?;
                expression(rhs, caller_path, graph, files, functions, resolution)
            }
            SimpleCondition::Expr(value) => {
                expression(value, caller_path, graph, files, functions, resolution)
            }
            SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
                condition_value(lhs, caller_path, graph, files, functions, resolution)?;
                condition_value(rhs, caller_path, graph, files, functions, resolution)
            }
            SimpleCondition::Not(inner) => {
                condition_value(inner, caller_path, graph, files, functions, resolution)
            }
        }
    }

    fn statement(
        value: &mut SimpleStmt,
        caller_path: &str,
        graph: &ModuleGraph,
        files: &[SourceFile],
        functions: &[FunctionMeta],
        resolution: &ModuleResolutionIndex,
    ) -> Result<(), String> {
        match value {
            SimpleStmt::Let {
                expression: value, ..
            }
            | SimpleStmt::Assign {
                expression: value, ..
            }
            | SimpleStmt::Expr(value)
            | SimpleStmt::Return(value) => {
                expression(value, caller_path, graph, files, functions, resolution)
            }
            SimpleStmt::Convert { source, .. } => {
                expression(source, caller_path, graph, files, functions, resolution)
            }
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                condition_value(condition, caller_path, graph, files, functions, resolution)?;
                for nested in then_statements {
                    statement(nested, caller_path, graph, files, functions, resolution)?;
                }
                if let Some(nested) = else_statements {
                    for statement_value in nested {
                        statement(
                            statement_value,
                            caller_path,
                            graph,
                            files,
                            functions,
                            resolution,
                        )?;
                    }
                }
                Ok(())
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                statement(init, caller_path, graph, files, functions, resolution)?;
                condition_value(condition, caller_path, graph, files, functions, resolution)?;
                statement(step, caller_path, graph, files, functions, resolution)?;
                for nested in body_statements {
                    statement(nested, caller_path, graph, files, functions, resolution)?;
                }
                Ok(())
            }
            SimpleStmt::Foreach {
                body_statements, ..
            } => {
                for nested in body_statements {
                    statement(nested, caller_path, graph, files, functions, resolution)?;
                }
                Ok(())
            }
            SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => Ok(()),
        }
    }

    for statement_value in statements {
        statement(
            statement_value,
            caller_path,
            graph,
            files,
            functions,
            resolution,
        )?;
    }
    Ok(())
}

fn compile_error_message(error: &CompileError) -> &str {
    match error {
        CompileError::Frontend(message)
        | CompileError::Backend(message)
        | CompileError::Invariant(message) => message,
    }
}

#[derive(Debug, Clone, Copy)]
struct PreviousFunctionHashes {
    signature_hash: u64,
    body_hash: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn function_by_name<'a>(compiler: &'a Compiler, name: &str) -> &'a FunctionMeta {
        compiler
            .functions()
            .iter()
            .find(|function| function.name == name)
            .expect("missing function by name")
    }

    #[test]
    fn symbol_table_resolves_collision_heavy_keys() {
        let mut table = SymbolTable::with_test_capacity(8);
        let colliding_hashes = [1_u64, 9_u64, 17_u64, 25_u64];
        for (offset, hash) in colliding_hashes.iter().enumerate() {
            table.insert(*hash, (offset + 10) as FunctionId);
        }

        for (offset, hash) in colliding_hashes.iter().enumerate() {
            assert_eq!(table.get(*hash), Some((offset + 10) as FunctionId));
        }
    }

    #[test]
    fn symbol_table_last_insert_wins_for_duplicate_hash() {
        let mut table = SymbolTable::with_test_capacity(8);
        table.insert(42, 1);
        table.insert(42, 7);
        assert_eq!(table.get(42), Some(7));
    }

    #[test]
    fn first_index_marks_all_functions_dirty() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper(); }\n",
        );

        let index = compiler.index_pass().expect("index pass");
        assert_eq!(index.parsed_functions, 2);
        assert_eq!(index.dirty_functions, 2);
        assert_eq!(index.signature_changed_functions, 2);
    }

    #[test]
    fn tick_budget_diagnostic_identifies_the_second_annotated_file() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "first.stasis",
            "function @tick_budget_us(100) tick(): i32 { return 1; }\n",
        );
        compiler.upsert_file(
            "second.stasis",
            "function @tick_budget_us(200) tick(): i32 { return 2; }\n",
        );

        compiler
            .index_pass()
            .expect_err("duplicate budget must fail");
        let diagnostic = compiler
            .last_source_diagnostic()
            .expect("tick budget diagnostic");
        assert_eq!(diagnostic.path, "second.stasis");
        assert_eq!(diagnostic.symbol, "tick");
        assert_eq!(diagnostic.start, "function ".len());
        assert_eq!(
            &compiler.files()[1].content[diagnostic.start..diagnostic.end],
            "@tick_budget_us"
        );
    }

    #[test]
    fn unchanged_source_emits_nothing_after_initial_emit() {
        let mut compiler = Compiler::new();
        compiler.upsert_file("sample.stasis", "function main(): i32 { return 7; }\n");
        let first = compiler
            .compile_with(|_, _, _| Ok(()))
            .expect("first compile");
        assert_eq!(first.emit.emitted_functions, 1);

        let second = compiler
            .compile_with(|_, _, _| Ok(()))
            .expect("second compile");
        assert_eq!(second.index.dirty_functions, 0);
        assert_eq!(second.emit.emitted_functions, 0);
    }

    #[test]
    fn structured_statements_are_cached_by_function_body() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper(); }\n",
        );
        compiler.index_pass().expect("first index");
        assert_eq!(compiler.statement_parse_count, 2);

        compiler.index_pass().expect("unchanged index");
        assert_eq!(compiler.statement_parse_count, 2);

        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 2; }\nfunction main(): i32 { return helper(); }\n",
        );
        compiler.index_pass().expect("changed index");
        assert_eq!(compiler.statement_parse_count, 3);
    }

    #[test]
    fn body_only_change_marks_only_changed_function_dirty() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper(); }\n",
        );
        compiler
            .compile_with(|_, _, _| Ok(()))
            .expect("initial compile");

        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 2; }\nfunction main(): i32 { return helper(); }\n",
        );
        let index = compiler.index_pass().expect("index pass");
        assert_eq!(index.dirty_functions, 1);
        assert!(function_by_name(&compiler, "helper").dirty);
        assert!(!function_by_name(&compiler, "main").dirty);

        let emit = compiler
            .emit_pass_with(&mut |_, _, _| Ok(()))
            .expect("emit pass");
        assert_eq!(emit.emitted_functions, 1);
    }

    #[test]
    fn mixed_file_body_edit_only_emits_changed_file_function() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "core.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper(); }\n",
        );
        compiler.upsert_file("extra.stasis", "function utility(): i32 { return 3; }\n");
        compiler
            .compile_with(|_, _, _| Ok(()))
            .expect("initial compile");

        compiler.upsert_file("extra.stasis", "function utility(): i32 { return 4; }\n");
        let index = compiler.index_pass().expect("index pass");
        assert_eq!(index.signature_changed_functions, 0);
        assert_eq!(index.dirty_functions, 1);
        assert!(!function_by_name(&compiler, "helper").dirty);
        assert!(!function_by_name(&compiler, "main").dirty);
        assert!(function_by_name(&compiler, "utility").dirty);

        let mut emitted_names = Vec::new();
        let emit = compiler
            .emit_pass_with(&mut |meta, _, _| {
                emitted_names.push(meta.name.clone());
                Ok(())
            })
            .expect("emit pass");
        assert_eq!(emit.emitted_functions, 1);
        assert_eq!(emitted_names, vec!["utility".to_string()]);
    }

    #[test]
    fn signature_change_propagates_dirty_to_dependents() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper(); }\n",
        );
        compiler
            .compile_with(|_, _, _| Ok(()))
            .expect("initial compile");

        compiler.upsert_file(
            "sample.stasis",
            "function helper(x: i32): i32 { return x; }\nfunction main(): i32 { return helper(); }\n",
        );
        let index = compiler.index_pass().expect("index pass");
        assert_eq!(index.signature_changed_functions, 1);
        assert_eq!(index.dirty_functions, 2);
        assert!(function_by_name(&compiler, "helper").dirty);
        assert!(function_by_name(&compiler, "main").dirty);
    }

    #[test]
    fn signature_equivalent_formatting_edit_does_not_dirty_or_emit() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function helper(value: i32): i32 { return value; }\nfunction main(): i32 { return helper(7); }\n",
        );
        compiler
            .compile_with(|_, _, _| Ok(()))
            .expect("initial compile");

        compiler.upsert_file(
            "sample.stasis",
            "function helper( value : i32 ) : i32 { return value; }\nfunction main(): i32 { return helper(7); }\n",
        );
        let index = compiler.index_pass().expect("index pass");
        assert_eq!(index.signature_changed_functions, 0);
        assert_eq!(index.dirty_functions, 0);
        assert!(!function_by_name(&compiler, "helper").dirty);
        assert!(!function_by_name(&compiler, "main").dirty);

        let emit = compiler
            .emit_pass_with(&mut |_, _, _| Ok(()))
            .expect("emit pass");
        assert_eq!(emit.emitted_functions, 0);
    }

    #[test]
    fn emit_pass_runs_only_dirty_functions() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper(); }\n",
        );
        compiler
            .compile_with(|_, _, _| Ok(()))
            .expect("initial compile");

        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 2; }\nfunction main(): i32 { return helper(); }\n",
        );
        let _ = compiler.index_pass().expect("index pass");

        let mut emitted_names = Vec::new();
        let _ = compiler
            .emit_pass_with(&mut |meta, _, _| {
                emitted_names.push(meta.name.clone());
                Ok(())
            })
            .expect("emit pass");
        assert_eq!(emitted_names, vec!["helper".to_string()]);
    }

    #[test]
    fn emit_pass_failure_keeps_dirty_flags_for_retry() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function main(): i32 { return helper(); }\nfunction helper(): i32 { return 1; }\n",
        );
        let _ = compiler.index_pass().expect("index pass");

        let error = compiler.emit_pass_with(&mut |meta, _, _| {
            if meta.name == "helper" {
                return Err("forced emit failure".to_string());
            }
            Ok(())
        });
        assert!(error.is_err(), "expected emit failure");
        assert!(function_by_name(&compiler, "main").dirty);
        assert!(function_by_name(&compiler, "helper").dirty);
    }

    #[test]
    fn duplicate_name_across_files_resolves_to_last_indexed_function() {
        let mut compiler = Compiler::new();
        compiler.upsert_file("a.stasis", "function shared(): i32 { return 1; }\n");
        compiler.upsert_file(
            "b.stasis",
            "function shared(): i32 { return 2; }\nfunction main(): i32 { return shared(); }\n",
        );

        let index = compiler.index_pass().expect("index pass");
        assert_eq!(index.parsed_functions, 3);

        let main = function_by_name(&compiler, "main");
        assert_eq!(main.dependencies.len(), 1);
        let callee = compiler
            .functions()
            .iter()
            .find(|function| function.id == main.dependencies[0])
            .expect("resolved canonical identity");
        assert_eq!(callee.name, "shared");
        assert_eq!(callee.file_id, 1);
    }

    #[test]
    fn duplicate_name_lookup_is_deterministic_across_repeated_index_passes() {
        let mut compiler = Compiler::new();
        compiler.upsert_file("a.stasis", "function shared(): i32 { return 1; }\n");
        compiler.upsert_file(
            "b.stasis",
            "function shared(): i32 { return 2; }\nfunction main(): i32 { return shared(); }\n",
        );

        let mut resolved_file_ids = Vec::new();
        for _ in 0..2 {
            let _ = compiler.index_pass().expect("index pass");
            let main = function_by_name(&compiler, "main");
            let callee = compiler
                .functions()
                .iter()
                .find(|function| function.id == main.dependencies[0])
                .expect("resolved canonical identity");
            resolved_file_ids.push(callee.file_id);
        }

        assert_eq!(resolved_file_ids, vec![1_u32, 1_u32]);
    }

    #[test]
    fn signature_change_propagates_dirty_to_fan_out_dependents() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction left(): i32 { return helper(); }\nfunction right(): i32 { return helper(); }\n",
        );
        compiler
            .compile_with(|_, _, _| Ok(()))
            .expect("initial compile");

        compiler.upsert_file(
            "sample.stasis",
            "function helper(seed: i32): i32 { return seed; }\nfunction left(): i32 { return helper(); }\nfunction right(): i32 { return helper(); }\n",
        );
        let index = compiler.index_pass().expect("index pass");
        assert_eq!(index.signature_changed_functions, 1);
        assert_eq!(index.dirty_functions, 3);
        assert!(function_by_name(&compiler, "helper").dirty);
        assert!(function_by_name(&compiler, "left").dirty);
        assert!(function_by_name(&compiler, "right").dirty);
    }

    #[test]
    fn body_change_keeps_fan_out_dependents_clean() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction left(): i32 { return helper(); }\nfunction right(): i32 { return helper(); }\n",
        );
        compiler
            .compile_with(|_, _, _| Ok(()))
            .expect("initial compile");

        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 2; }\nfunction left(): i32 { return helper(); }\nfunction right(): i32 { return helper(); }\n",
        );
        let index = compiler.index_pass().expect("index pass");
        assert_eq!(index.signature_changed_functions, 0);
        assert_eq!(index.dirty_functions, 1);
        assert!(function_by_name(&compiler, "helper").dirty);
        assert!(!function_by_name(&compiler, "left").dirty);
        assert!(!function_by_name(&compiler, "right").dirty);
    }

    #[test]
    fn signature_change_propagates_dirty_through_multi_level_chain() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function leaf(): i32 { return 1; }\nfunction mid(): i32 { return leaf(); }\nfunction top(): i32 { return mid(); }\n",
        );
        compiler
            .compile_with(|_, _, _| Ok(()))
            .expect("initial compile");

        compiler.upsert_file(
            "sample.stasis",
            "function leaf(seed: i32): i32 { return seed; }\nfunction mid(): i32 { return leaf(); }\nfunction top(): i32 { return mid(); }\n",
        );
        let index = compiler.index_pass().expect("index pass");
        assert_eq!(index.signature_changed_functions, 1);
        assert_eq!(index.dirty_functions, 3);
        assert!(function_by_name(&compiler, "leaf").dirty);
        assert!(function_by_name(&compiler, "mid").dirty);
        assert!(function_by_name(&compiler, "top").dirty);
    }

    #[test]
    fn emitted_hir_contains_structured_statements() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function add_one(value: i32): i32 { let next = value + 1; return next; }\n",
        );

        let mut statement_counts = Vec::new();
        compiler
            .compile_with(|meta, hir, _| {
                if meta.name == "add_one" {
                    statement_counts.push(hir.statements.len());
                }
                Ok(())
            })
            .expect("compile should succeed");

        assert_eq!(statement_counts, vec![2]);
    }

    #[test]
    fn data_flow_summaries_include_field_subsets_nested_calls_and_loop_bounds() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            r#"
struct Enemy { hp: i32; active: bool; }
struct State { score: i32; enemies: Enemy[4]; }
global state: State;
extern function host_emit(value: i32): void;

function damage_all(): void {
    foreach (let enemy in state.enemies) {
        enemy.damage(1);
    }
}

function damage(enemy: Enemy, amount: i32): void {
    enemy.hp -= amount;
}

function damage_view(enemies: Enemy[4]): void {
    foreach (let enemy in enemies) {
        enemy.damage(1);
    }
}

function damage_all_function_form(): void {
    foreach (let enemy in state.enemies) {
        damage(enemy, 1);
    }
}

function tick(): i32 {
    damage_all();
    damage_all_function_form();
    damage_view(state.enemies);
    for (let i: i32 = 0; i < 3; i += 1) {
        state.score += state.enemies[i].hp;
    }
    let score_f32: f32 = i32_to_f32(state.score);
    host_emit(state.score);
    return state.score;
}
"#,
        );

        compiler.index_pass().expect("index pass");
        let summaries = compiler.function_data_flow_summaries();
        let tick = summaries
            .iter()
            .find(|summary| summary.function == "tick")
            .expect("tick summary");

        assert_eq!(
            tick.schema_version,
            crate::data_flow::FUNCTION_DATA_FLOW_SCHEMA_VERSION
        );
        assert_eq!(
            tick.direct.calls,
            vec!["damage_all", "damage_all_function_form", "damage_view"]
        );
        assert_eq!(tick.direct.host_calls, vec!["host_emit"]);
        assert_eq!(
            tick.direct.reads,
            vec!["state.enemies[*].hp", "state.score"]
        );
        assert_eq!(tick.direct.writes, vec!["state.score"]);
        assert_eq!(tick.direct.bounded_iterations[0].max_iterations, Some(3));
        assert_eq!(
            tick.aggregate.writes,
            vec!["state.enemies[*].hp", "state.score"]
        );
        assert!(tick.aggregate.calls.contains(&"damage_all".to_string()));
        let damage = summaries
            .iter()
            .find(|summary| summary.function == "damage")
            .expect("damage summary");
        assert_eq!(damage.direct.parameter_reads, vec!["enemy.hp"]);
        assert_eq!(damage.direct.parameter_writes, vec!["enemy.hp"]);
        let damage_view = summaries
            .iter()
            .find(|summary| summary.function == "damage_view")
            .expect("parameter foreach summary");
        assert_eq!(damage_view.direct.bounded_iterations[0].bound, "enemies");
        assert_eq!(
            damage_view.direct.bounded_iterations[0].reads,
            vec!["enemies.length"]
        );
        assert_eq!(
            damage_view.direct.bounded_iterations[0].max_iterations,
            Some(4)
        );
        assert_eq!(
            damage_view.aggregate.parameter_writes,
            vec!["enemies[*].hp"]
        );
        assert!(tick.aggregate.bounded_iterations.iter().any(|iteration| {
            iteration.bound == "state.enemies" && iteration.max_iterations == Some(4)
        }));
        for name in ["damage_all", "damage_all_function_form"] {
            let summary = summaries
                .iter()
                .find(|summary| summary.function == name)
                .expect("nested caller summary");
            assert_eq!(summary.aggregate.writes, vec!["state.enemies[*].hp"]);
            assert!(!summary
                .direct
                .reads
                .contains(&"state.enemies[*]".to_string()));
        }
    }

    #[test]
    fn data_flow_does_not_claim_a_bound_when_the_loop_body_writes_its_index() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "loop.stasis",
            r#"
function tick(): i32 {
    for (let i: i32 = 0; i < 3; i += 1) {
        i -= 1;
    }
    return 0;
}
"#,
        );

        compiler.index_pass().expect("index pass");
        let tick = compiler
            .function_data_flow_summaries()
            .iter()
            .find(|summary| summary.function == "tick")
            .expect("tick summary");
        assert_eq!(tick.direct.bounded_iterations[0].max_iterations, None);
    }

    #[test]
    fn data_flow_resolves_receiver_overloads_before_aggregating_effects() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "overloads.stasis",
            r#"
struct Enemy { hp: i32; }
struct Player { score: i32; }
struct State { enemy: Enemy; player: Player; }
global state: State;

function touch(enemy: Enemy): void { enemy.hp += 1; }
function touch(player: Player): void { player.score += 1; }

function tick(): i32 {
    touch(state.enemy);
    touch(state.player);
    return 0;
}
"#,
        );

        compiler.index_pass().expect("index pass");
        let tick = compiler
            .function_data_flow_summaries()
            .iter()
            .find(|summary| summary.function == "tick")
            .expect("tick summary");
        assert_eq!(
            tick.aggregate.writes,
            vec!["state.enemy.hp", "state.player.score"]
        );
    }

    #[test]
    fn data_flow_skips_unreachable_unsupported_bodies() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "dead.stasis",
            r#"
function main(): i32 { return 0; }
function unreachable(): i32 { while (true) { return 1; } }
"#,
        );

        compiler
            .index_pass()
            .expect("unreachable body stays deferred");
        assert_eq!(
            compiler
                .function_data_flow_summaries()
                .iter()
                .map(|summary| summary.function.as_str())
                .collect::<Vec<_>>(),
            vec!["main"]
        );
    }

    #[test]
    fn check_reports_errors_in_functions_unreachable_from_runtime_roots() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "dead.stasis",
            r#"
function main(): i32 { return 0; }
function unreachable(): i32 { while (true) { return 1; } }
"#,
        );

        compiler
            .check()
            .expect_err("full check must analyze dead code");
        let diagnostic = compiler
            .last_source_diagnostic()
            .expect("structured unreachable-function diagnostic");
        assert_eq!(diagnostic.path, "dead.stasis");
        assert_eq!(diagnostic.symbol, "unreachable");
        assert!(diagnostic.message.contains("while"));
    }

    #[test]
    fn data_flow_rebuilds_when_fixed_capacity_metadata_changes() {
        let mut compiler = Compiler::new();
        for capacity in [4, 8] {
            compiler.upsert_file(
                "capacity.stasis",
                format!(
                    "struct Enemy {{ hp: i32; }}\nstruct State {{ enemies: Enemy[{capacity}]; }}\nglobal state: State;\nfunction tick(): i32 {{ foreach (let enemy in state.enemies) {{ enemy.hp += 1; }} return 0; }}\n"
                ),
            );
            compiler.index_pass().expect("index pass");
            let tick = compiler
                .function_data_flow_summaries()
                .iter()
                .find(|summary| summary.function == "tick")
                .expect("tick summary");
            assert_eq!(
                tick.direct.bounded_iterations[0].max_iterations,
                Some(capacity)
            );
        }
    }

    #[test]
    fn data_flow_aggregates_every_member_of_a_mutual_call_cycle() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "cycle.stasis",
            r#"
struct State { left: i32; right: i32; }
global state: State;
function left(view: State): void { view.left += 1; right(view); }
function right(view: State): void { view.right += 1; left(view); }
function tick(): i32 { left(state); return 0; }
"#,
        );

        compiler.index_pass().expect("index pass");
        for name in ["left", "right"] {
            let summary = compiler
                .function_data_flow_summaries()
                .iter()
                .find(|summary| summary.function == name)
                .expect("cycle summary");
            assert_eq!(
                summary.aggregate.parameter_writes,
                vec!["view.left", "view.right"]
            );
        }
        let tick = compiler
            .function_data_flow_summaries()
            .iter()
            .find(|summary| summary.function == "tick")
            .expect("tick summary");
        assert_eq!(tick.aggregate.writes, vec!["state.left", "state.right"]);
    }

    #[test]
    fn data_flow_resolves_overloads_with_nested_intrinsic_arguments() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "nested_overload.stasis",
            r#"
struct State { integer: i32; float: i32; }
global state: State;
function choose(value: i32): void { state.integer += value; }
function choose(value: f32): void { state.float += 1; }
function tick(): i32 { choose(i32_to_f32(state.integer)); return 0; }
"#,
        );

        compiler.index_pass().expect("index pass");
        let tick = compiler
            .function_data_flow_summaries()
            .iter()
            .find(|summary| summary.function == "tick")
            .expect("tick summary");
        assert!(tick.aggregate.writes.contains(&"state.float".to_string()));
        assert!(!tick.aggregate.writes.contains(&"state.integer".to_string()));
    }

    #[test]
    fn data_flow_rebuilds_when_only_a_resolved_call_site_changes() {
        let mut compiler = Compiler::new();
        for (argument, expected) in [
            ("state.enemy", "state.enemy.hp"),
            ("state.pilot", "state.pilot.score"),
        ] {
            compiler.upsert_file(
                "call_site.stasis",
                format!(
                    "struct Enemy {{ hp: i32; }}\nstruct Player {{ score: i32; }}\nstruct State {{ enemy: Enemy; pilot: Player; }}\nglobal state: State;\nfunction touch(value: Enemy): void {{ value.hp += 1; }}\nfunction touch(value: Player): void {{ value.score += 1; }}\nfunction tick(): i32 {{ touch({argument}); return 0; }}\n"
                ),
            );
            compiler.index_pass().expect("index pass");
            let tick = compiler
                .function_data_flow_summaries()
                .iter()
                .find(|summary| summary.function == "tick")
                .expect("tick summary");
            assert_eq!(tick.aggregate.writes, vec![expected]);
        }
    }

    #[test]
    fn data_flow_resolves_overloads_with_nested_fixed32_intrinsics() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "fixed32_overload.stasis",
            r#"
struct State { integer: i32; float: i32; }
global state: State;
function choose(value: i32): void { state.integer += value; }
function choose(value: f32): void { state.float += 1; }
function tick(): i32 { choose(fixed32_mul(1, 2)); return 0; }
"#,
        );

        compiler.index_pass().expect("index pass");
        let tick = compiler
            .function_data_flow_summaries()
            .iter()
            .find(|summary| summary.function == "tick")
            .expect("tick summary");
        assert!(tick.aggregate.writes.contains(&"state.integer".to_string()));
        assert!(!tick.aggregate.writes.contains(&"state.float".to_string()));
    }

    #[test]
    fn data_flow_fast_reuse_distinguishes_nested_call_grouping() {
        let mut compiler = Compiler::new();
        for (expression, expected, rejected) in [
            ("outer(inner(1), 2)", "left", "right"),
            ("outer(inner(1, 2))", "right", "left"),
        ] {
            compiler.upsert_file(
                "grouping.stasis",
                format!(
                    "function left(): void {{ return; }}\nfunction right(): void {{ return; }}\nfunction inner(a: i32): i32 {{ return a; }}\nfunction inner(a: i32, b: i32): i32 {{ return b; }}\nfunction outer(a: i32, b: i32): void {{ left(); }}\nfunction outer(a: i32): void {{ right(); }}\nfunction tick(): i32 {{ {expression}; return 0; }}\n"
                ),
            );
            compiler.index_pass().expect("index pass");
            let tick = compiler
                .function_data_flow_summaries()
                .iter()
                .find(|summary| summary.function == "tick")
                .expect("tick summary");
            assert!(tick.aggregate.calls.contains(&expected.to_string()));
            assert!(!tick.aggregate.calls.contains(&rejected.to_string()));
        }
    }

    #[test]
    fn source_items_preserve_annotation_and_same_line_definition_ranges() {
        let source = "@tick_budget_us(100) function tick(): i32 { return 0; } function helper(): i32 { return 1; } struct State { score: i32; }";
        let functions = source_function_items([("sample.stasis".to_string(), source.to_string())])
            .expect("source functions");
        assert_eq!(
            functions
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["tick", "helper"]
        );
        let tick = functions
            .iter()
            .find(|item| item.name == "tick")
            .expect("tick");
        assert_eq!(
            &source[tick.signature_range.start as usize..tick.signature_range.end as usize],
            "function tick(): i32 "
        );
        let structs = source_struct_items(source, "sample.stasis").expect("source structs");
        assert_eq!(structs.len(), 1);
        assert_eq!(
            &source[structs[0].definition_range.start as usize
                ..structs[0].definition_range.end as usize],
            "struct State { score: i32; }"
        );
    }

    #[test]
    fn canonical_function_identity_survives_body_edits_and_declaration_reordering() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "src/main.stasis",
            "function helper(): i32 { return 1; } function main(): i32 { return helper(); }",
        );
        compiler.index_pass().expect("initial index");
        let initial: HashMap<_, _> = compiler
            .functions()
            .iter()
            .map(|function| {
                (
                    function.name.clone(),
                    (function.symbol_id.clone(), function.id),
                )
            })
            .collect();

        compiler.upsert_file(
            "src/main.stasis",
            "function main(): i32 { return helper(); } function inserted(): i32 { return 9; } function helper(): i32 { return 2; }",
        );
        compiler.index_pass().expect("edited index");
        for name in ["helper", "main"] {
            let current = compiler
                .functions()
                .iter()
                .find(|function| function.name == name)
                .expect("retained declaration");
            assert_eq!(&(current.symbol_id.clone(), current.id), &initial[name]);
        }
    }

    #[test]
    fn identity_and_signature_compatibility_are_separate_contracts() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "api.stasis",
            "function value(input: i32): i32 { return input; }",
        );
        compiler.index_pass().expect("initial index");
        let initial = compiler.functions()[0].clone();

        compiler.upsert_file(
            "api.stasis",
            "function value(input: i32): f32 { return to_f32(input); }",
        );
        compiler.index_pass().expect("return edit index");
        let return_edit = compiler.functions()[0].clone();
        assert_eq!(
            return_edit.id, initial.id,
            "return edits retain declaration identity"
        );
        assert_ne!(return_edit.signature_hash, initial.signature_hash);

        compiler.upsert_file(
            "api.stasis",
            "function value(input: f32): f32 { return input; }",
        );
        compiler.index_pass().expect("parameter edit index");
        assert_ne!(
            compiler.functions()[0].id,
            initial.id,
            "overload-selection edits replace identity"
        );
    }

    #[test]
    fn canonical_ids_distinguish_overloads_receivers_files_and_file_order() {
        let files = [
            (
                "src/a.stasis",
                "function act(self: Player): i32 { return 1; } function pick(value: i32): i32 { return value; }",
            ),
            (
                "src/b.stasis",
                "function act(self: Enemy): i32 { return 2; } function pick(value: f32): f32 { return value; }",
            ),
        ];
        let mut left = Compiler::new();
        for (path, source) in files {
            left.upsert_file(path, source);
        }
        left.index_pass().expect("left index");
        let mut right = Compiler::new();
        for (path, source) in files.into_iter().rev() {
            right.upsert_file(path, source);
        }
        right.index_pass().expect("right index");
        let left_ids: BTreeSet<_> = left
            .functions()
            .iter()
            .map(|function| function.id)
            .collect();
        let right_ids: BTreeSet<_> = right
            .functions()
            .iter()
            .map(|function| function.id)
            .collect();
        assert_eq!(left_ids, right_ids);
        assert_eq!(
            left_ids.len(),
            4,
            "no overload, receiver, or file collision"
        );
    }

    #[test]
    fn module_context_reports_ambiguous_bare_calls_with_source_span() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "import \"one.stasis\"; import \"two.stasis\"; function main(): i32 { return one.value() + value(); }",
        );
        compiler.upsert_file("one.stasis", "function value(): i32 { return 1; }");
        compiler.upsert_file("two.stasis", "function value(): i32 { return 2; }");
        let error = compiler.index_pass().unwrap_err();
        assert!(matches!(error, CompileError::Frontend(_)));
        let diagnostic = compiler.last_source_diagnostic().unwrap();
        assert_eq!(diagnostic.path, "main.stasis");
        assert_eq!(diagnostic.symbol, "value");
        let main = compiler
            .files()
            .iter()
            .find(|file| file.path == "main.stasis")
            .unwrap();
        assert_eq!(&main.content[diagnostic.start..diagnostic.end], "value");
        assert_eq!(diagnostic.start, main.content.rfind("value").unwrap());
        assert!(diagnostic
            .message
            .contains("ambiguous unqualified call 'value'"));
    }

    #[test]
    fn loaded_module_alias_does_not_steal_an_ordinary_receiver_call() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "function value(self: i32): i32 { return self + 1; } function main(): i32 { let two: i32 = 1; return two.value(); }",
        );
        compiler.upsert_file("two.stasis", "function value(): i32 { return 99; }");
        compiler.index_pass().expect("ordinary receiver resolution");
        let main = compiler
            .functions()
            .iter()
            .find(|function| function.name == "main")
            .expect("main function");
        let local_value = compiler
            .functions()
            .iter()
            .find(|function| {
                function.name == "value"
                    && compiler.files()[function.file_id as usize].path == "main.stasis"
            })
            .expect("local receiver function");
        let unrelated_value = compiler
            .files()
            .iter()
            .find(|file| file.path == "two.stasis")
            .and_then(|file| file.functions.first())
            .copied()
            .expect("unrelated module value");
        assert_eq!(main.dependencies, vec![local_value.id]);
        assert_ne!(main.dependencies, vec![unrelated_value]);
    }

    #[test]
    fn importer_can_call_directly_imported_child() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "import \"child.stasis\"; function main(): i32 { return child(); }",
        );
        compiler.upsert_file("child.stasis", "function child(): i32 { return 1; }");
        compiler.index_pass().expect("directional graph index");

        let main = compiler
            .functions()
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        let child = compiler
            .functions()
            .iter()
            .find(|function| function.name == "child")
            .unwrap();
        assert_eq!(main.dependencies, vec![child.id]);
        assert_eq!(
            compiler.module_graph().dependency_closure("child.stasis"),
            BTreeSet::from(["child.stasis".to_string()])
        );
        assert_eq!(
            compiler.module_graph().invalidation_closure("child.stasis"),
            BTreeSet::from(["child.stasis".to_string(), "main.stasis".to_string()])
        );
    }

    #[test]
    fn imported_child_cannot_call_its_importer() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "import \"child.stasis\"; function main(): i32 { return child(); }",
        );
        let child_source = "function child(): i32 { return main(); }";
        compiler.upsert_file("child.stasis", child_source);
        let error = compiler.index_pass().expect_err("importer is not visible");
        assert!(format!("{error:?}").contains("not visible"));
        let diagnostic = compiler.last_source_diagnostic().unwrap();
        assert_eq!(diagnostic.path, "child.stasis");
        assert_eq!(diagnostic.symbol, "main");
        assert_eq!(&child_source[diagnostic.start..diagnostic.end], "main");
    }

    #[test]
    fn imported_child_cannot_call_a_sibling_from_the_importer() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "import \"child.stasis\"; import \"sibling.stasis\"; function main(): i32 { return child(); }",
        );
        let child_source = "function child(): i32 { return sibling(); }";
        compiler.upsert_file("child.stasis", child_source);
        compiler.upsert_file("sibling.stasis", "function sibling(): i32 { return 2; }");
        let error = compiler.index_pass().expect_err("sibling is not visible");
        assert!(format!("{error:?}").contains("not visible"));
        let diagnostic = compiler.last_source_diagnostic().unwrap();
        assert_eq!(diagnostic.path, "child.stasis");
        assert_eq!(diagnostic.symbol, "sibling");
        assert_eq!(&child_source[diagnostic.start..diagnostic.end], "sibling");
    }

    #[test]
    fn transitive_imports_are_visible_for_unqualified_calls() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "import \"child.stasis\"; function main(): i32 { return leaf(); }",
        );
        compiler.upsert_file(
            "child.stasis",
            "import \"leaf.stasis\"; function child(): i32 { return leaf(); }",
        );
        compiler.upsert_file("leaf.stasis", "function leaf(): i32 { return 7; }");
        compiler.index_pass().expect("transitive graph index");

        let main = compiler
            .functions()
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        let leaf = compiler
            .functions()
            .iter()
            .find(|function| function.name == "leaf")
            .unwrap();
        assert_eq!(main.dependencies, vec![leaf.id]);
    }

    #[test]
    fn imported_upserts_collapse_to_entry_roots_while_unrelated_files_remain_roots() {
        let mut entry = Compiler::new();
        entry.upsert_file("helper.stasis", "function helper(): i32 { return 1; }");
        entry.upsert_file(
            "main.stasis",
            "import \"helper.stasis\"; function main(): i32 { return helper(); }",
        );
        entry.index_pass().expect("entry closure index");
        assert_eq!(
            entry.module_graph().roots(),
            &BTreeSet::from(["main.stasis".to_string()])
        );

        entry.upsert_file("orphan.stasis", "function orphan(): i32 { return 0; }");
        entry.index_pass().expect("directory roots index");
        assert_eq!(
            entry.module_graph().roots(),
            &BTreeSet::from(["main.stasis".to_string(), "orphan.stasis".to_string()])
        );
    }

    #[test]
    fn qualified_dependency_reaches_only_the_selected_module_declaration() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "import \"one.stasis\"; import \"two.stasis\"; function main(): i32 { return one.value(); }",
        );
        compiler.upsert_file("one.stasis", "function value(): i32 { return 1; }");
        compiler.upsert_file("two.stasis", "function value(): i32 { return 2; }");
        compiler.index_pass().unwrap();
        let main = compiler
            .functions()
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        let selected = compiler
            .functions()
            .iter()
            .find(|function| function.module_alias == "one")
            .unwrap();
        let unselected = compiler
            .functions()
            .iter()
            .find(|function| function.module_alias == "two")
            .unwrap();
        assert_eq!(main.dependencies, vec![selected.id]);
        assert!(unselected.dependents.is_empty());
    }

    #[test]
    fn graph_refresh_removes_modules_that_leave_the_entry_closure() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "import \"helper.stasis\"; function main(): i32 { return helper(); }",
        );
        compiler.upsert_file("helper.stasis", "function helper(): i32 { return 1; }");
        compiler.index_pass().unwrap();
        assert_eq!(compiler.files().len(), 2);

        compiler.upsert_file("main.stasis", "function main(): i32 { return 1; }");
        compiler.index_pass().unwrap();
        assert_eq!(
            compiler
                .files()
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec!["main.stasis"]
        );
    }

    #[test]
    fn imported_file_change_invalidates_reverse_module_dependents() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "import \"helper.stasis\"; function main(): i32 { return helper(); }",
        );
        compiler.upsert_file("helper.stasis", "function helper(): i32 { return 1; }");
        compiler.index_pass().unwrap();
        compiler
            .emit_pass_with(&mut |_, _, _| Ok(()))
            .expect("accept initial functions");

        compiler.upsert_file("helper.stasis", "function helper(): i32 { return 2; }");
        compiler.index_pass().unwrap();
        let dirty: BTreeSet<_> = compiler
            .functions()
            .iter()
            .filter(|function| function.dirty)
            .map(|function| function.name.as_str())
            .collect();
        assert_eq!(dirty, BTreeSet::from(["helper", "main"]));
        assert_eq!(
            compiler
                .module_graph()
                .invalidation_closure("helper.stasis"),
            BTreeSet::from(["helper.stasis".to_string(), "main.stasis".to_string()])
        );
    }

    #[test]
    fn imported_body_edit_preserves_unchanged_dependent_statement_cache() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "import \"helper.stasis\"; function main(): i32 { return helper(); }",
        );
        compiler.upsert_file("helper.stasis", "function helper(): i32 { return 1; }");
        compiler.index_pass().expect("initial index");
        assert_eq!(compiler.statement_parse_count, 2);

        compiler.upsert_file("helper.stasis", "function helper(): i32 { return 2; }");
        compiler.index_pass().expect("body edit index");
        assert_eq!(compiler.statement_parse_count, 3);
        assert!(function_by_name(&compiler, "main").dirty);
    }

    #[test]
    fn imported_signature_edit_invalidates_file_context_statement_cache_once() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "import \"helper.stasis\"; function main(): i32 { return helper(); }",
        );
        compiler.upsert_file("helper.stasis", "function helper(): i32 { return 1; }");
        compiler.index_pass().expect("initial index");
        assert_eq!(compiler.statement_parse_count, 2);

        compiler.upsert_file("helper.stasis", "function helper(): f32 { return 1.0; }");
        compiler.index_pass().expect("signature edit index");
        assert_eq!(compiler.statement_parse_count, 4);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn compiler_disk_import_rejects_directory_link_escape_at_literal_span() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("stasis_compiler_link_escape_{stamp}"));
        let root = base.join("project");
        let outside = base.join("outside");
        let escape = root.join("escape");
        std::fs::create_dir_all(&root).expect("project directory");
        std::fs::create_dir_all(&outside).expect("outside directory");
        std::fs::write(
            outside.join("helper.stasis"),
            "function helper(): i32 { return 7; }",
        )
        .expect("outside helper");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &escape).expect("directory symlink");
        #[cfg(windows)]
        {
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&escape)
                .arg(&outside)
                .status()
                .expect("create directory junction");
            assert!(status.success(), "create directory junction");
        }

        let source = "import \"escape/helper.stasis\"; function main(): i32 { return helper(); }";
        let mut compiler = Compiler::new();
        compiler
            .set_project_root(root.to_string_lossy())
            .expect("set project root");
        compiler.upsert_file(root.join("main.stasis").to_string_lossy(), source);
        let error = compiler
            .index_pass()
            .expect_err("linked import must remain confined");
        assert!(format!("{error:?}").contains("escapes project root"));
        let diagnostic = compiler
            .last_source_diagnostic()
            .expect("source diagnostic");
        assert_eq!(diagnostic.path, "main.stasis");
        assert_eq!(
            &source[diagnostic.start..diagnostic.end],
            "\"escape/helper.stasis\""
        );

        #[cfg(windows)]
        std::fs::remove_dir(&escape).expect("junction cleanup");
        std::fs::remove_dir_all(&base).expect("fixture cleanup");
    }
}
