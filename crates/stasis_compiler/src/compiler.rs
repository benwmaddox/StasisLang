use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ops::Range;

use crate::backend::emit::{parse_simple_statements_from_block, SimpleStmt};
use crate::data_flow::{build_function_data_flow_summaries, FunctionDataFlowSummary};
use crate::frontend::indexer::{hash_text, index_file};
use crate::frontend::types::{TypeId, TypeTable};
use crate::ir::hir::{Block, FunctionHIR};

pub type FunctionId = u32;

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
    pub name: String,
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
    name_hash: u64,
    signature_hash: u64,
    body_hash: u64,
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
    deps: DependencyGraph,
    types: TypeTable,
    parsed_statements: Vec<Vec<SimpleStmt>>,
    parsed_statement_ids: BTreeSet<FunctionId>,
    statement_cache: HashMap<StatementCacheKey, Vec<SimpleStmt>>,
    analysis_required_roots: Vec<String>,
    data_flow_summaries: Vec<FunctionDataFlowSummary>,
    data_flow_context_fingerprint: u64,
    #[cfg(test)]
    statement_parse_count: usize,
    last_source_diagnostic: Option<crate::SourceDiagnostic>,
}

impl Compiler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        let path = path.into();
        let content = content.into();
        let hash = hash_text(&content);
        if let Some(existing) = self.files.iter_mut().find(|file| file.path == path) {
            existing.content = content;
            existing.hash = hash;
            return;
        }
        self.files.push(SourceFile {
            path,
            content,
            hash,
            functions: Vec::new(),
        });
    }

    pub fn retain_files(&mut self, paths: &BTreeSet<String>) {
        self.files.retain(|file| paths.contains(&file.path));
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
        self.last_source_diagnostic = None;
        if let Err(message) = crate::performance::tick_budget_us(&self.files) {
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
        self.parsed_statements.clear();
        self.parsed_statement_ids.clear();
        self.deps = DependencyGraph;

        let mut dependency_hashes_by_function: Vec<Vec<u64>> = Vec::new();
        let mut overload_ids_by_name_hash: HashMap<u64, Vec<(u64, FunctionId)>> = HashMap::new();
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
                let function_id = self.functions.len() as FunctionId;
                self.files[file_id].functions.push(function_id);
                let overloads = overload_ids_by_name_hash
                    .entry(indexed_function.name_hash)
                    .or_default();
                if let Some((_, existing_id)) = overloads
                    .iter_mut()
                    .find(|(signature_hash, _)| *signature_hash == indexed_function.signature_hash)
                {
                    *existing_id = function_id;
                } else {
                    overloads.push((indexed_function.signature_hash, function_id));
                }

                let previous = previous_hashes
                    .get(&(file_id as u32, indexed_function.name_hash))
                    .copied();
                let signature_changed = previous
                    .is_none_or(|old| old.signature_hash != indexed_function.signature_hash);
                let body_changed =
                    previous.is_none_or(|old| old.body_hash != indexed_function.body_hash);
                if signature_changed {
                    signature_changed_ids.push(function_id);
                }

                dependency_hashes_by_function.push(indexed_function.dependency_name_hashes);
                self.functions.push(FunctionMeta {
                    id: function_id,
                    name: indexed_function.name,
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
                    dirty: signature_changed || body_changed,
                });
            }
        }

        let mut unique_edges = BTreeSet::new();
        for (caller_index, dependency_hashes) in
            dependency_hashes_by_function.into_iter().enumerate()
        {
            let caller = caller_index as FunctionId;
            for dependency_hash in dependency_hashes {
                if let Some(callees) = overload_ids_by_name_hash.get(&dependency_hash) {
                    for (_, callee) in callees {
                        if caller != *callee {
                            unique_edges.insert((caller, *callee));
                        }
                    }
                }
            }
        }
        for (caller, callee) in unique_edges {
            self.functions[caller as usize].dependencies.push(callee);
            self.functions[callee as usize].dependents.push(caller);
        }

        self.parsed_statements = vec![Vec::new(); self.functions.len()];
        let reachable = crate::backend::reachability::compute_reachable_function_ids(
            &self.functions,
            &self.analysis_required_roots,
        );
        self.prepare_statement_artifacts(&reachable.iter().copied().collect::<Vec<_>>())?;

        self.propagate_dirty_from_signature_changes(&signature_changed_ids);
        let dirty_functions = self
            .functions
            .iter()
            .filter(|function| function.dirty)
            .count();
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
            let function = &self.functions[*function_id as usize];
            let file = &self.files[function.file_id as usize];
            let key = StatementCacheKey {
                path: file.path.clone(),
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
                parse_simple_statements_from_block(body, &mut self.types)
                    .map_err(CompileError::Backend)?
            };
            next_statement_cache.insert(key, statements.clone());
            self.parsed_statements[*function_id as usize] = statements;
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
            self.data_flow_summaries = summaries;
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
                .functions
                .get(*function_id as usize)
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
            self.functions[function_id as usize].dirty = false;
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

    fn capture_previous_hashes(&self) -> HashMap<(u32, u64), PreviousFunctionHashes> {
        let mut out = HashMap::new();
        for function in &self.functions {
            out.insert(
                (function.file_id, function.name_hash),
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
            let dependents = self.functions[function_id as usize].dependents.clone();
            for dependent_id in dependents {
                if !self.functions[dependent_id as usize].dirty {
                    self.functions[dependent_id as usize].dirty = true;
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
        let statements = self
            .parsed_statements
            .get(function.id as usize)
            .cloned()
            .ok_or_else(|| {
                CompileError::Invariant(format!(
                    "function '{}' has no parsed statement artifact",
                    function.name
                ))
            })?;
        Ok(FunctionHIR {
            blocks: vec![Block { source: body }],
            statements,
        })
    }
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
        let callee = &compiler.functions()[main.dependencies[0] as usize];
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
            let callee = &compiler.functions()[main.dependencies[0] as usize];
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
}
