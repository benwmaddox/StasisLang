use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ops::Range;

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
    pub signature_hash: u64,
    pub body_hash: u64,
    pub param_names: Vec<String>,
    pub params: Vec<TypeId>,
    pub return_type: TypeId,
    pub dependencies: Vec<FunctionId>,
    pub dependents: Vec<FunctionId>,
    pub dirty: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    pub map: HashMap<u64, FunctionId>,
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

#[derive(Debug, Default)]
pub struct Compiler {
    files: Vec<SourceFile>,
    functions: Vec<FunctionMeta>,
    symbols: SymbolTable,
    deps: DependencyGraph,
    types: TypeTable,
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

    pub fn compile_with<F>(&mut self, mut emit_function: F) -> CompileResult<CompileReport>
    where
        F: FnMut(&FunctionMeta, &FunctionHIR) -> Result<(), String>,
    {
        let index = self.index_pass()?;
        let emit = self.emit_pass_with(&mut emit_function)?;
        Ok(CompileReport { index, emit })
    }

    pub fn index_pass(&mut self) -> CompileResult<IndexPassResult> {
        let previous_hashes = self.capture_previous_hashes();
        self.functions.clear();
        self.symbols.map.clear();
        self.deps = DependencyGraph;

        let mut dependency_hashes_by_function: Vec<Vec<u64>> = Vec::new();
        let mut signature_changed_ids: Vec<FunctionId> = Vec::new();

        for file_id in 0..self.files.len() {
            let indexed = index_file(&self.files[file_id].content, &mut self.types)
                .map_err(CompileError::Frontend)?;
            self.files[file_id].functions.clear();
            for indexed_function in indexed {
                let function_id = self.functions.len() as FunctionId;
                self.files[file_id].functions.push(function_id);
                self.symbols
                    .map
                    .insert(indexed_function.name_hash, function_id);

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
                if let Some(&callee) = self.symbols.map.get(&dependency_hash) {
                    if caller != callee {
                        unique_edges.insert((caller, callee));
                    }
                }
            }
        }
        for (caller, callee) in unique_edges {
            self.functions[caller as usize].dependencies.push(callee);
            self.functions[callee as usize].dependents.push(caller);
        }

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

    pub fn emit_pass_with<F>(&mut self, emit_function: &mut F) -> CompileResult<EmitPassResult>
    where
        F: FnMut(&FunctionMeta, &FunctionHIR) -> Result<(), String>,
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
        F: FnMut(&FunctionMeta, &FunctionHIR) -> Result<(), String>,
    {
        let mut emitted_functions = 0usize;
        for function_id in function_ids {
            let snapshot = self
                .functions
                .get(*function_id as usize)
                .ok_or_else(|| {
                    CompileError::Invariant(format!("invalid function id {}", function_id))
                })?
                .clone();
            let hir = self.lower_function_to_hir(&snapshot)?;
            emit_function(&snapshot, &hir).map_err(CompileError::Backend)?;
            self.functions[*function_id as usize].dirty = false;
            emitted_functions += 1;
        }
        Ok(EmitPassResult { emitted_functions })
    }

    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }

    pub fn functions(&self) -> &[FunctionMeta] {
        &self.functions
    }

    pub fn types(&self) -> &TypeTable {
        &self.types
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

    fn lower_function_to_hir(&self, function: &FunctionMeta) -> CompileResult<FunctionHIR> {
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
        Ok(FunctionHIR {
            blocks: vec![Block { source: body }],
        })
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
    fn unchanged_source_emits_nothing_after_initial_emit() {
        let mut compiler = Compiler::new();
        compiler.upsert_file("sample.stasis", "function main(): i32 { return 7; }\n");
        let first = compiler.compile_with(|_, _| Ok(())).expect("first compile");
        assert_eq!(first.emit.emitted_functions, 1);

        let second = compiler
            .compile_with(|_, _| Ok(()))
            .expect("second compile");
        assert_eq!(second.index.dirty_functions, 0);
        assert_eq!(second.emit.emitted_functions, 0);
    }

    #[test]
    fn body_only_change_marks_only_changed_function_dirty() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper(); }\n",
        );
        compiler
            .compile_with(|_, _| Ok(()))
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
            .emit_pass_with(&mut |_, _| Ok(()))
            .expect("emit pass");
        assert_eq!(emit.emitted_functions, 1);
    }

    #[test]
    fn signature_change_propagates_dirty_to_dependents() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper(); }\n",
        );
        compiler
            .compile_with(|_, _| Ok(()))
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
    fn emit_pass_runs_only_dirty_functions() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper(); }\n",
        );
        compiler
            .compile_with(|_, _| Ok(()))
            .expect("initial compile");

        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 2; }\nfunction main(): i32 { return helper(); }\n",
        );
        let _ = compiler.index_pass().expect("index pass");

        let mut emitted_names = Vec::new();
        let _ = compiler
            .emit_pass_with(&mut |meta, _| {
                emitted_names.push(meta.name.clone());
                Ok(())
            })
            .expect("emit pass");
        assert_eq!(emitted_names, vec!["helper".to_string()]);
    }

    #[test]
    fn signature_change_propagates_dirty_to_fan_out_dependents() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction left(): i32 { return helper(); }\nfunction right(): i32 { return helper(); }\n",
        );
        compiler
            .compile_with(|_, _| Ok(()))
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
    fn signature_change_propagates_dirty_through_multi_level_chain() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "sample.stasis",
            "function leaf(): i32 { return 1; }\nfunction mid(): i32 { return leaf(); }\nfunction top(): i32 { return mid(); }\n",
        );
        compiler
            .compile_with(|_, _| Ok(()))
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
}
