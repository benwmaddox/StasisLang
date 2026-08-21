use crate::backend::emit::{
    debug_variable_slot, AssignTarget, CompileAnalysisCache, DirectStorageBinding,
    DirectStorageBindings, RuntimeHelperLinkage, SimpleCondition, SimpleExpr, SimpleStmt,
};
use crate::backend::patch_plan::{
    capture_accepted_program, plan_patch, AcceptedProgram, FunctionKey, PatchReason,
    PatchReasonChain,
};
use crate::backend::program_snapshot::{ProgramArtifactMapping, ProgramFunction, ProgramSnapshot};
use crate::backend::state_layout::{build_state_memory_report, is_named_scalar_state_path};
use crate::backend::state_query::{
    parse_state_query, BinaryOperator, ScalarExpression, StateQuery, StateValueReference,
};
use crate::backend::EngineEntrypoints;
use crate::compiler::{CompileReport, CompileResult, Compiler, FunctionId, FunctionMeta};
use crate::frontend::indexer::hash_text;
use crate::frontend::types::{
    TypeCategory, TypeTable, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_F64, TYPE_ID_I32, TYPE_ID_U16,
    TYPE_ID_U32, TYPE_ID_U8, TYPE_ID_VOID,
};
use crate::ir::hir::{DebugStatement, FunctionHIR};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, Module};
use serde_json::{json, Value as JsonValue};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::sync::{MutexGuard, OnceLock};
use std::time::{Instant, SystemTime};

pub use crate::backend::state_layout::{
    StateCapacityChangeReport as JitStateCapacityChangeReport,
    StateCollectionFieldLayout as JitStateCollectionFieldLayout,
    StateCollectionLayout as JitStateCollectionLayout, StateLayout as JitStateLayout,
    StateMemoryEntry as JitStateMemoryEntry, StateMemoryPoolReport as JitStateMemoryPoolReport,
    StateMemoryReport as JitStateMemoryReport,
    StateMemoryStructFieldReport as JitStateMemoryStructFieldReport,
    StateMemoryStructReport as JitStateMemoryStructReport,
    StateOpaqueLayout as JitStateOpaqueLayout, StateScalarLayout as JitStateScalarLayout,
    StateStructFieldLayout as JitStateStructFieldLayout, StateStructLayout as JitStateStructLayout,
};

const MAX_STATE_QUERY_SCAN: usize = 4096;
const MAX_STATE_QUERY_MATCHES: usize = 64;

fn elapsed_micros(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

#[derive(Clone, Default)]
struct FunctionLoweringReferences {
    paths: BTreeSet<String>,
    calls: BTreeSet<String>,
}

fn path_is_local(path: &str, scopes: &[BTreeSet<String>]) -> bool {
    let root = path.split('.').next().unwrap_or(path);
    scopes.iter().rev().any(|scope| scope.contains(root))
}

fn collect_condition_references(
    condition: &SimpleCondition,
    references: &mut FunctionLoweringReferences,
    scopes: &[BTreeSet<String>],
) {
    match condition {
        SimpleCondition::Comparison { lhs, rhs, .. } => {
            collect_expression_references(lhs, references, scopes);
            collect_expression_references(rhs, references, scopes);
        }
        SimpleCondition::Expr(expression) => {
            collect_expression_references(expression, references, scopes)
        }
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            collect_condition_references(lhs, references, scopes);
            collect_condition_references(rhs, references, scopes);
        }
        SimpleCondition::Not(inner) => collect_condition_references(inner, references, scopes),
    }
}

fn collect_expression_references(
    expression: &SimpleExpr,
    references: &mut FunctionLoweringReferences,
    scopes: &[BTreeSet<String>],
) {
    match expression {
        SimpleExpr::Identifier(path) => {
            if !path_is_local(path, scopes) {
                references.paths.insert(path.clone());
            }
        }
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            if !path_is_local(collection_path, scopes) {
                references.paths.insert(collection_path.clone());
                if !suffix.is_empty() {
                    references
                        .paths
                        .insert(format!("{collection_path}.{suffix}"));
                }
            }
            collect_expression_references(index, references, scopes);
        }
        SimpleExpr::Call { target, args } => {
            references.calls.insert(target.clone());
            for argument in args {
                collect_expression_references(argument, references, scopes);
            }
        }
        SimpleExpr::Binary { lhs, rhs, .. } => {
            collect_expression_references(lhs, references, scopes);
            collect_expression_references(rhs, references, scopes);
        }
        SimpleExpr::Condition(condition) => {
            collect_condition_references(condition, references, scopes)
        }
        SimpleExpr::Int(_)
        | SimpleExpr::Float(_)
        | SimpleExpr::Bool(_)
        | SimpleExpr::StringLiteral(_) => {}
    }
}

fn collect_target_references(
    target: &AssignTarget,
    references: &mut FunctionLoweringReferences,
    scopes: &[BTreeSet<String>],
) {
    match target {
        AssignTarget::Local(_) | AssignTarget::GlobalPath(_) => {}
        AssignTarget::IndexedPath { index, .. } => {
            collect_expression_references(index, references, scopes);
        }
    }
}

fn collect_statement_references(
    statements: &[SimpleStmt],
    references: &mut FunctionLoweringReferences,
    scopes: &mut Vec<BTreeSet<String>>,
) {
    for statement in statements {
        match statement {
            SimpleStmt::Let {
                name, expression, ..
            } => {
                collect_expression_references(expression, references, scopes);
                scopes
                    .last_mut()
                    .expect("function dependency scope")
                    .insert(name.clone());
            }
            SimpleStmt::Expr(expression) | SimpleStmt::Return(expression) => {
                collect_expression_references(expression, references, scopes)
            }
            SimpleStmt::Assign {
                target, expression, ..
            } => {
                collect_target_references(target, references, scopes);
                collect_expression_references(expression, references, scopes);
            }
            SimpleStmt::Convert { target, source, .. } => {
                collect_target_references(target, references, scopes);
                collect_expression_references(source, references, scopes);
            }
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                collect_condition_references(condition, references, scopes);
                scopes.push(BTreeSet::new());
                collect_statement_references(then_statements, references, scopes);
                scopes.pop();
                if let Some(statements) = else_statements {
                    scopes.push(BTreeSet::new());
                    collect_statement_references(statements, references, scopes);
                    scopes.pop();
                }
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                scopes.push(BTreeSet::new());
                collect_statement_references(std::slice::from_ref(init), references, scopes);
                collect_condition_references(condition, references, scopes);
                scopes.push(BTreeSet::new());
                collect_statement_references(body_statements, references, scopes);
                scopes.pop();
                collect_statement_references(std::slice::from_ref(step), references, scopes);
                scopes.pop();
            }
            SimpleStmt::Foreach {
                item_name,
                index_name,
                body_statements,
                ..
            } => {
                scopes.push(BTreeSet::from_iter(
                    std::iter::once(item_name.clone()).chain(index_name.clone()),
                ));
                collect_statement_references(body_statements, references, scopes);
                scopes.pop();
            }
            SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
        }
    }
}

fn function_lowering_contract_hash(
    references: &FunctionLoweringReferences,
    analysis: &CompileAnalysisCache,
    parameter_storage_kinds: Option<&[crate::data_flow::ParameterStorageKind]>,
) -> u64 {
    let mut facts = vec![format!("parameter_storage:{parameter_storage_kinds:?}")];
    for path in &references.paths {
        if let Some(value) = analysis.constant_values.get(path) {
            facts.push(format!("constant:{path}:{value:?}"));
        }
    }
    for signature in &analysis.resolved_extern_signatures {
        if references.calls.contains(&signature.name) {
            facts.push(format!("extern:{signature:?}"));
            if let Some(address) = analysis.extern_symbol_addresses.get(&signature.symbol) {
                facts.push(format!("extern_address:{}:{address}", signature.symbol));
            }
        }
    }

    facts.sort();
    hash_text(&facts.join("\n"))
}

fn collect_function_lowering_references(
    function: &FunctionMeta,
    hir: &FunctionHIR,
) -> FunctionLoweringReferences {
    let mut references = FunctionLoweringReferences::default();
    let mut scopes = vec![function.param_names.iter().cloned().collect()];
    collect_statement_references(&hir.statements, &mut references, &mut scopes);
    references
}

fn debug_function_metadata(
    function: &FunctionMeta,
    hir: &FunctionHIR,
    file: &str,
) -> Result<JitDebugFunctionMetadata, String> {
    fn collect_sites(
        function_start: u32,
        statements: &[DebugStatement],
        out: &mut Vec<JitDebugSite>,
    ) -> Result<(), String> {
        for statement in statements {
            let source_offset = function_start
                .checked_add(statement.source_offset)
                .ok_or_else(|| "debug site source offset overflow".to_string())?;
            out.push(JitDebugSite {
                site_id: statement.source_offset,
                source_offset,
            });
            collect_sites(function_start, &statement.children, out)?;
        }
        Ok(())
    }

    fn collect_names(statements: &[SimpleStmt], out: &mut BTreeSet<String>) {
        for statement in statements {
            match statement {
                SimpleStmt::Let { name, .. } => {
                    out.insert(name.clone());
                }
                SimpleStmt::If {
                    then_statements,
                    else_statements,
                    ..
                } => {
                    collect_names(then_statements, out);
                    if let Some(else_statements) = else_statements {
                        collect_names(else_statements, out);
                    }
                }
                SimpleStmt::For {
                    init,
                    step,
                    body_statements,
                    ..
                } => {
                    collect_names(std::slice::from_ref(init.as_ref()), out);
                    collect_names(std::slice::from_ref(step.as_ref()), out);
                    collect_names(body_statements, out);
                }
                SimpleStmt::Foreach {
                    item_name,
                    index_name,
                    body_statements,
                    ..
                } => {
                    out.insert(item_name.clone());
                    if let Some(index_name) = index_name {
                        out.insert(index_name.clone());
                    }
                    collect_names(body_statements, out);
                }
                _ => {}
            }
        }
    }

    let mut sites = Vec::new();
    collect_sites(
        function.source_range.start,
        &hir.debug_statements,
        &mut sites,
    )?;
    sites.sort_by_key(|site| site.source_offset);
    let mut names = function
        .param_names
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    collect_names(&hir.statements, &mut names);
    let mut variables = BTreeMap::new();
    for name in names {
        let slot = debug_variable_slot(&name);
        if let Some(existing) = variables.insert(slot, name.clone()) {
            return Err(format!(
                "debug variable slot collision between '{existing}' and '{name}'"
            ));
        }
    }
    Ok(JitDebugFunctionMetadata {
        function_id: function.id,
        name: function.name.clone(),
        file: file.to_string(),
        source_range: function.source_range.clone(),
        sites,
        variables,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum JitScalarValue {
    I32(i32),
    F32(f32),
    F64(f64),
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
}

impl JitScalarValue {
    pub fn type_name(self) -> &'static str {
        match self {
            Self::I32(_) => "i32",
            Self::F32(_) => "f32",
            Self::F64(_) => "f64",
            Self::Bool(_) => "bool",
            Self::U8(_) => "u8",
            Self::U16(_) => "u16",
            Self::U32(_) => "u32",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitArtifact {
    pub function_id: FunctionId,
    pub function_key: FunctionKey,
    pub slot: u32,
    pub body_hash: u64,
    pub(crate) code_ptr: u64,
    pub executable_bytes: usize,
    pub clif: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitDebugSite {
    pub site_id: u32,
    pub source_offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitDebugFunctionMetadata {
    pub function_id: FunctionId,
    pub name: String,
    pub file: String,
    pub source_range: std::ops::Range<u32>,
    pub sites: Vec<JitDebugSite>,
    pub variables: BTreeMap<u32, String>,
}

pub struct JitProcess {
    compiler: Compiler,
    active_compiler: Option<Compiler>,
    artifacts: Vec<JitArtifact>,
    artifact_index: HashMap<FunctionId, usize>,
    modules: Vec<Arc<JitArena>>,
    runtime_libraries: Vec<stasis_dynload::Library>,
    runtime_symbol_cache: BTreeMap<String, usize>,
    source_disk_probe_cache: BTreeMap<String, SourceDiskProbe>,
    program_snapshot: Option<Arc<ProgramSnapshot>>,
    active_program_snapshot: Option<Arc<ProgramSnapshot>>,
    generation_metadata: Option<JitGenerationMetadata>,
    accepted_program: Option<AcceptedProgram>,
    accepted_lowering_references: BTreeMap<FunctionKey, FunctionLoweringReferences>,
    accepted_lowering_contracts: BTreeMap<FunctionKey, u64>,
    staged_string_literals: HashMap<i32, String>,
    active_string_literals: HashMap<i32, String>,
    last_failed_source_diagnostic: Option<crate::SourceDiagnostic>,
    required_emit_roots: Vec<String>,
    local_runtime_helper_trampolines: bool,
    debug_instrumentation: bool,
    profile_function_names: BTreeSet<String>,
    debug_metadata: BTreeMap<FunctionId, JitDebugFunctionMetadata>,
    #[cfg(test)]
    _test_guard: Option<MutexGuard<'static, ()>>,
}

struct JitArena {
    _module: Mutex<JITModule>,
    executable_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitEnginePackage {
    pub tick_code_ptr: u64,
    pub render_code_ptr: u64,
    pub on_code_swap_code_ptr: Option<u64>,
    pub symbol_code_ptrs: BTreeMap<String, u64>,
    pub function_code_ptrs: BTreeMap<FunctionId, u64>,
}

impl JitEnginePackage {
    pub fn host_entry_targets(
        &self,
        revision: u64,
    ) -> Result<stasis_dynload::JitHostEntryTargets, String> {
        let main =
            self.symbol_code_ptrs.get("main").copied().ok_or_else(|| {
                "JIT host-entry package is missing required main target".to_string()
            })?;
        Ok(stasis_dynload::JitHostEntryTargets {
            revision,
            main: main as usize,
            tick: self.tick_code_ptr as usize,
            render: self.render_code_ptr as usize,
            on_code_swap: self.on_code_swap_code_ptr.map(|address| address as usize),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitGenerationMetadata {
    pub source_revision: u64,
    pub layout_hash: u64,
    pub emitted_function_ids: Vec<FunctionId>,
    pub reused_function_ids: Vec<FunctionId>,
    pub patch_reasons: Vec<PatchReasonChain>,
    pub affected_host_entries: Vec<FunctionKey>,
    pub retained_dependencies: Vec<FunctionKey>,
    pub host_export_signatures: BTreeMap<String, String>,
    pub host_export_code_ptrs: BTreeMap<String, u64>,
    pub diagnostics: Vec<String>,
    pub code_owner_revision: u64,
    pub data_owner_layout_hash: u64,
    pub codegen_micros: u64,
    pub plan_micros: u64,
    pub finalize_micros: u64,
    pub module_count: usize,
    pub retained_arena_count: usize,
    pub emitted_clif_bytes: usize,
    pub executable_bytes: usize,
    pub retained_jit_bytes: usize,
    pub total_jit_bytes: usize,
}

impl JitProcess {
    pub fn new() -> Self {
        #[cfg(test)]
        let _test_guard = acquire_jit_process_test_guard();
        #[cfg(test)]
        clear_jit_process_test_runtime();

        Self::new_inner(
            #[cfg(test)]
            Some(_test_guard),
        )
    }

    fn new_inner(#[cfg(test)] _test_guard: Option<MutexGuard<'static, ()>>) -> Self {
        Self {
            compiler: Compiler::new(),
            active_compiler: None,
            artifacts: Vec::new(),
            artifact_index: HashMap::new(),
            modules: Vec::new(),
            runtime_libraries: Vec::new(),
            runtime_symbol_cache: BTreeMap::new(),
            source_disk_probe_cache: BTreeMap::new(),
            program_snapshot: None,
            active_program_snapshot: None,
            generation_metadata: None,
            accepted_program: None,
            accepted_lowering_references: BTreeMap::new(),
            accepted_lowering_contracts: BTreeMap::new(),
            staged_string_literals: HashMap::new(),
            active_string_literals: HashMap::new(),
            last_failed_source_diagnostic: None,
            required_emit_roots: Vec::new(),
            local_runtime_helper_trampolines: false,
            debug_instrumentation: false,
            profile_function_names: BTreeSet::new(),
            debug_metadata: BTreeMap::new(),
            #[cfg(test)]
            _test_guard,
        }
    }

    pub fn set_debug_instrumentation(&mut self, enabled: bool) -> Result<(), String> {
        if self.program_snapshot.is_some() || self.active_program_snapshot.is_some() {
            return Err(
                "JIT debug instrumentation must be configured before the first compilation"
                    .to_string(),
            );
        }
        self.debug_instrumentation = enabled;
        Ok(())
    }

    pub fn set_profile_functions(
        &mut self,
        names: impl IntoIterator<Item = String>,
    ) -> Result<(), String> {
        if self.program_snapshot.is_some() || self.active_program_snapshot.is_some() {
            return Err(
                "JIT function profiling must be configured before the first compilation"
                    .to_string(),
            );
        }
        self.profile_function_names = names
            .into_iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();
        Ok(())
    }

    pub fn staged_candidate(&self) -> Self {
        let mut candidate = Self::new_inner(
            #[cfg(test)]
            None,
        );
        if let Some(project_root) = self.compiler.project_root() {
            candidate
                .compiler
                .set_project_root(project_root.to_string())
                .expect("accepted compiler project root remains valid");
        }
        for file in self.compiler.files() {
            candidate.upsert_file(file.path.clone(), file.content.clone());
        }
        candidate.active_compiler = self.active_compiler.clone();
        candidate.artifacts = self.artifacts.clone();
        candidate.modules = self.modules.clone();
        candidate.program_snapshot = self.program_snapshot.clone();
        candidate.active_program_snapshot = self.active_program_snapshot.clone();
        candidate.generation_metadata = self.generation_metadata.clone();
        candidate.accepted_program = self.accepted_program.clone();
        candidate.accepted_lowering_references = self.accepted_lowering_references.clone();
        candidate.accepted_lowering_contracts = self.accepted_lowering_contracts.clone();
        candidate.staged_string_literals = self.staged_string_literals.clone();
        candidate.active_string_literals = self.active_string_literals.clone();
        candidate.required_emit_roots = self.required_emit_roots.clone();
        candidate
            .compiler
            .set_analysis_required_roots(&candidate.required_emit_roots);
        candidate.local_runtime_helper_trampolines = self.local_runtime_helper_trampolines;
        candidate.debug_instrumentation = self.debug_instrumentation;
        candidate.profile_function_names = self.profile_function_names.clone();
        candidate.debug_metadata = self.debug_metadata.clone();
        candidate
    }

    pub fn accept_staged_candidate(&mut self, candidate: Self) {
        #[cfg(test)]
        let candidate = {
            let mut candidate = candidate;
            candidate._test_guard = self._test_guard.take();
            candidate
        };
        *self = candidate;
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.compiler.upsert_file(path, content);
    }

    pub fn retain_files(&mut self, paths: &BTreeSet<String>) {
        self.compiler.retain_files(paths);
    }

    pub fn function_data_flow_summaries(&self) -> &[crate::data_flow::FunctionDataFlowSummary] {
        self.compiler.function_data_flow_summaries()
    }

    pub fn debug_metadata(&self) -> &BTreeMap<FunctionId, JitDebugFunctionMetadata> {
        &self.debug_metadata
    }

    pub fn tick_budget_us(&self) -> Result<Option<u64>, String> {
        crate::performance::tick_budget_us(self.compiler.files())
    }

    pub fn set_required_emit_roots(&mut self, roots: &[String]) {
        self.required_emit_roots.clear();
        self.required_emit_roots.extend_from_slice(roots);
        self.compiler.set_analysis_required_roots(roots);
    }

    pub fn set_local_runtime_helper_trampolines(&mut self, enabled: bool) {
        self.local_runtime_helper_trampolines = enabled;
    }

    pub fn refresh_imported_sources_from_disk(&mut self, root_source_path: &str) -> bool {
        let root_source_path =
            crate::identity::canonical_source_path(self.compiler.project_root(), root_source_path)
                .unwrap_or_else(|_| root_source_path.to_string());
        let tracked: Vec<(String, u64)> = self
            .compiler
            .files()
            .iter()
            .filter(|file| file.path != root_source_path)
            .map(|file| (file.path.clone(), file.hash))
            .collect();
        let tracked_paths: BTreeSet<String> =
            tracked.iter().map(|(path, _)| path.clone()).collect();
        self.source_disk_probe_cache
            .retain(|path, _| tracked_paths.contains(path));

        let mut changed = false;
        for (path, known_hash) in tracked {
            let relative_path = Path::new(&path);
            let disk_path = self
                .compiler
                .project_root()
                .map(Path::new)
                .filter(|_| relative_path.is_relative())
                .map_or_else(
                    || relative_path.to_path_buf(),
                    |root| root.join(relative_path),
                );
            let Some(probe) = probe_disk_source(&disk_path) else {
                continue;
            };
            if self.source_disk_probe_cache.get(&path) == Some(&probe) {
                continue;
            }

            let Ok(content) = std::fs::read_to_string(&disk_path) else {
                continue;
            };
            let disk_hash = hash_text(&content);
            self.source_disk_probe_cache.insert(path.clone(), probe);
            if disk_hash != known_hash {
                self.compiler.upsert_file(path, content);
                changed = true;
            }
        }
        changed
    }

    pub fn compile(&mut self) -> CompileResult<CompileReport> {
        let previous_compiler = self.compiler.clone();
        let previous_snapshot = self.program_snapshot.clone();
        let previous_artifacts = self.artifacts.clone();
        let previous_metadata = self.generation_metadata.clone();
        let previous_accepted_program = self.accepted_program.clone();
        let previous_lowering_references = self.accepted_lowering_references.clone();
        let previous_lowering_contracts = self.accepted_lowering_contracts.clone();
        let previous_literals = self.staged_string_literals.clone();
        let previous_active_compiler = self.active_compiler.clone();
        let previous_active_snapshot = self.active_program_snapshot.clone();
        let previous_active_literals = self.active_string_literals.clone();
        let previous_module_count = self.modules.len();
        let report = self.compile_staged()?;
        match self.activate_staged_runtime() {
            Ok(()) => Ok(report),
            Err(error) => {
                self.modules.truncate(previous_module_count);
                self.compiler = previous_compiler;
                self.program_snapshot = previous_snapshot;
                self.artifacts = previous_artifacts;
                self.generation_metadata = previous_metadata;
                self.accepted_program = previous_accepted_program;
                self.accepted_lowering_references = previous_lowering_references;
                self.accepted_lowering_contracts = previous_lowering_contracts;
                self.staged_string_literals = previous_literals;
                self.active_compiler = previous_active_compiler;
                self.active_program_snapshot = previous_active_snapshot;
                self.active_string_literals = previous_active_literals;
                self.rebuild_artifact_index();
                Err(crate::compiler::CompileError::Backend(error))
            }
        }
    }

    pub fn compile_staged(&mut self) -> CompileResult<CompileReport> {
        let result = self.compile_candidate_staged();
        match result {
            Ok(report) => {
                self.last_failed_source_diagnostic = None;
                self.active_compiler = Some(self.compiler.clone());
                self.active_program_snapshot = self.program_snapshot.clone();
                self.active_string_literals = self.staged_string_literals.clone();
                Ok(report)
            }
            Err(error) => {
                self.last_failed_source_diagnostic =
                    self.compiler.last_source_diagnostic().cloned().or_else(|| {
                        self.compiler.files().first().map(|file| {
                            crate::SourceDiagnostic::new(
                                file.path.clone(),
                                0,
                                file.content.len(),
                                "",
                                match &error {
                                    crate::compiler::CompileError::Frontend(message)
                                    | crate::compiler::CompileError::Backend(message)
                                    | crate::compiler::CompileError::Invariant(message) => {
                                        message.clone()
                                    }
                                },
                            )
                        })
                    });
                self.restore_active_compiler_with_pending_sources();
                Err(error)
            }
        }
    }

    fn compile_candidate_staged(&mut self) -> CompileResult<CompileReport> {
        stasis_dynload::begin_jit_string_literal_staging()
            .map_err(crate::compiler::CompileError::Backend)?;
        let result = self.compile_internal();
        if result.is_err() {
            let _ = stasis_dynload::finish_jit_string_literal_staging();
        }
        result
    }

    fn restore_active_compiler_with_pending_sources(&mut self) {
        let pending_files = self.compiler.files().to_vec();
        self.compiler = self.active_compiler.clone().unwrap_or_default();
        self.compiler
            .set_analysis_required_roots(&self.required_emit_roots);
        for file in pending_files {
            self.compiler.upsert_file(file.path, file.content);
        }
        self.program_snapshot = self.active_program_snapshot.clone();
        self.staged_string_literals = self.active_string_literals.clone();
    }

    fn compile_internal(&mut self) -> CompileResult<CompileReport> {
        let index = self.compiler.index_pass()?;
        self.validate_host_aliases()
            .map_err(crate::compiler::CompileError::Backend)?;
        self.compiler
            .types_mut()
            .ensure_utf8_view_id()
            .map_err(crate::compiler::CompileError::Backend)?;
        self.compiler
            .types_mut()
            .ensure_ascii_view_id()
            .map_err(crate::compiler::CompileError::Backend)?;
        let mut analysis_type_table = self.compiler.types().clone();
        let files_fingerprint = compute_files_fingerprint(self.compiler.files());
        let snapshot_revision =
            crate::backend::program_snapshot::semantic_revision_with_required_roots(
                files_fingerprint,
                &self.required_emit_roots,
            );
        let snapshot_miss = self
            .program_snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.source_revision() != snapshot_revision);
        if snapshot_miss {
            let extern_signatures = collect_supported_extern_call_signatures(
                self.compiler.files(),
                &mut analysis_type_table,
            )
            .map_err(crate::compiler::CompileError::Backend)?;
            let (resolved_extern_signatures, extern_symbol_addresses) = self
                .resolve_extern_call_signatures(&extern_signatures)
                .map_err(crate::compiler::CompileError::Backend)?;
            let next_cache = build_compile_analysis_cache_from_resolved_externs(
                self.compiler.files(),
                self.compiler.functions(),
                &mut analysis_type_table,
                snapshot_revision,
                resolved_extern_signatures,
                extern_symbol_addresses,
            )
            .map_err(crate::compiler::CompileError::Backend)?;
            self.program_snapshot = Some(Arc::new(
                ProgramSnapshot::build(
                    snapshot_revision,
                    self.compiler.files(),
                    self.compiler.module_graph(),
                    self.compiler.functions(),
                    &analysis_type_table,
                    self.compiler.data_flow_summaries_shared(),
                    &self.required_emit_roots,
                    next_cache,
                )
                .map_err(crate::compiler::CompileError::Backend)?,
            ));
        }
        *self.compiler.types_mut() = analysis_type_table.clone();
        let snapshot = self.program_snapshot.clone().ok_or_else(|| {
            crate::compiler::CompileError::Invariant(
                "jit program snapshot missing after refresh".to_string(),
            )
        })?;
        let analysis = snapshot.analysis.clone();
        let reachable_ids: Vec<_> = snapshot.reachable_function_ids().iter().copied().collect();
        let function_keys_by_id: BTreeMap<FunctionId, FunctionKey> = self
            .compiler
            .functions()
            .iter()
            .map(|function| (function.id, FunctionKey::from_function(function)))
            .collect();
        let mut lowering_references = BTreeMap::new();
        let mut lowering_contracts = BTreeMap::new();
        let mut lowered_contract_changes = BTreeSet::new();
        let compiler_layout_changed =
            self.active_program_snapshot
                .as_ref()
                .is_some_and(|accepted| {
                    accepted.compiler_layout_digest() != snapshot.compiler_layout_digest()
                });
        for function_id in &reachable_ids {
            let function = self
                .compiler
                .functions()
                .iter()
                .find(|function| function.id == *function_id)
                .ok_or_else(|| {
                    crate::compiler::CompileError::Invariant(format!(
                        "reachable function id {function_id} is missing"
                    ))
                })?;
            let key = function_keys_by_id.get(function_id).ok_or_else(|| {
                crate::compiler::CompileError::Invariant(format!(
                    "function '{}' has no stable key",
                    function.name
                ))
            })?;
            if compiler_layout_changed {
                lowered_contract_changes.insert(key.clone());
            }
            if let Some(references) = self.accepted_lowering_references.get(key) {
                let hash = function_lowering_contract_hash(
                    references,
                    &analysis,
                    self.compiler
                        .function_data_flow_summaries()
                        .iter()
                        .find(|summary| summary.internal_function_id == function.storage_index)
                        .map(|summary| summary.parameter_storage_kinds.as_slice()),
                );
                if self.accepted_lowering_contracts.get(key) != Some(&hash) {
                    lowered_contract_changes.insert(key.clone());
                }
                lowering_references.insert(key.clone(), references.clone());
                lowering_contracts.insert(key.clone(), hash);
            }
        }
        let direct_storage = build_direct_storage_bindings(
            &analysis.global_path_types,
            &analysis.collection_infos,
            self.compiler.types(),
            false,
        )
        .map_err(crate::compiler::CompileError::Backend)?;
        let plan_started = Instant::now();
        let patch_plan = plan_patch(
            self.compiler.functions(),
            self.compiler.files(),
            &self.required_emit_roots,
            self.accepted_program.as_ref(),
            &lowered_contract_changes,
        )
        .map_err(crate::compiler::CompileError::Backend)?;
        let mut patch_plan = patch_plan;
        if compiler_layout_changed {
            for reason in &mut patch_plan.reasons {
                reason.reason = PatchReason::CompilerLayoutChanged;
                reason.path_from_change.clear();
            }
        }
        let plan_micros = elapsed_micros(plan_started);
        let emit_function_ids = patch_plan.re_jit_ids.clone();
        let source_paths = self
            .compiler
            .files()
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        let data_flow_summaries = self.compiler.data_flow_summaries_shared();
        let mut staged_debug_metadata = self.debug_metadata.clone();
        for function_id in &emit_function_ids {
            staged_debug_metadata.remove(function_id);
        }
        let previous_artifacts_by_key: BTreeMap<FunctionKey, JitArtifact> = self
            .artifacts
            .iter()
            .map(|artifact| (artifact.function_key.clone(), artifact.clone()))
            .collect();
        let mut retained_symbols = BTreeMap::new();
        for key in &patch_plan.reused {
            let current_id = function_keys_by_id
                .iter()
                .find_map(|(id, candidate)| (candidate == key).then_some(*id))
                .ok_or_else(|| {
                    crate::compiler::CompileError::Invariant(format!(
                        "reused function '{}' has no current id",
                        key.display_name()
                    ))
                })?;
            let artifact = previous_artifacts_by_key.get(key).ok_or_else(|| {
                crate::compiler::CompileError::Backend(format!(
                    "retained JIT address missing for '{}'",
                    key.display_name()
                ))
            })?;
            retained_symbols.insert(format!("jit_fn_{current_id}"), artifact.code_ptr as usize);
        }
        let local_runtime_helper_trampolines = self.local_runtime_helper_trampolines;
        let debug_instrumentation = self.debug_instrumentation;
        let profile_function_names = self.profile_function_names.clone();
        let mut staged_module = if emit_function_ids.is_empty() {
            None
        } else {
            Some(
                new_generation_jit_module(&analysis.extern_symbol_addresses, &retained_symbols)
                    .map_err(crate::compiler::CompileError::Backend)?,
            )
        };
        let mut staged_functions = Vec::with_capacity(emit_function_ids.len());
        let mut defined_runtime_helper_trampolines = BTreeSet::new();
        let codegen_started = Instant::now();
        let emit = self.compiler.emit_pass_for_ids_with(
            &emit_function_ids,
            &mut |meta, hir, lowered_types| {
                if debug_instrumentation {
                    let file = source_paths.get(meta.file_id as usize).ok_or_else(|| {
                        format!("debug metadata file {} is missing", meta.file_id)
                    })?;
                    staged_debug_metadata
                        .insert(meta.id, debug_function_metadata(meta, hir, file)?);
                }
                let symbol = format!("jit_fn_{}", meta.id);
                let profile_instrumentation = profile_function_names.contains(&meta.name);
                let data_flow_summary = data_flow_summaries
                    .iter()
                    .find(|summary| summary.internal_function_id == meta.storage_index);
                let mut type_table = lowered_types.clone();
                type_table.ensure_utf8_view_id()?;
                type_table.ensure_ascii_view_id()?;
                let module = staged_module.take().ok_or_else(|| {
                    "JIT generation module missing during function emission".to_string()
                })?;
                let compiled = catch_unwind(AssertUnwindSafe(|| {
                    compile_function_into_jit_module(
                        module,
                        meta,
                        hir,
                        &symbol,
                        &analysis.call_signatures,
                        &mut type_table,
                        &analysis.global_path_types,
                        &analysis.constant_values,
                        &analysis.collection_infos,
                        &analysis.named_struct_field_types,
                        data_flow_summary,
                        &analysis.extern_symbol_addresses,
                        &direct_storage,
                        local_runtime_helper_trampolines,
                        debug_instrumentation,
                        profile_instrumentation,
                        &mut defined_runtime_helper_trampolines,
                    )
                }));
                let (module, function_id, clif, executable_bytes) = match compiled {
                    Ok(result) => result?,
                    Err(payload) => {
                        let message = payload
                            .downcast_ref::<&str>()
                            .copied()
                            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                            .unwrap_or("unknown panic");
                        return Err(format!(
                            "JIT backend panicked while compiling '{}': {message}",
                            meta.name
                        ));
                    }
                };
                staged_module = Some(module);
                let function_key = function_keys_by_id
                    .get(&meta.id)
                    .cloned()
                    .ok_or_else(|| format!("emitted function '{}' has no stable key", meta.name))?;
                let references = collect_function_lowering_references(meta, hir);
                let contract_hash = function_lowering_contract_hash(
                    &references,
                    &analysis,
                    data_flow_summary.map(|summary| summary.parameter_storage_kinds.as_slice()),
                );
                lowering_references.insert(function_key.clone(), references);
                lowering_contracts.insert(function_key.clone(), contract_hash);
                staged_functions.push((
                    meta.id,
                    function_key,
                    meta.body_hash,
                    function_id,
                    clif,
                    executable_bytes,
                ));
                Ok(())
            },
        )?;
        let codegen_micros = elapsed_micros(codegen_started);
        let finalize_started = Instant::now();
        if let Some(module) = staged_module.as_mut() {
            module.finalize_definitions().map_err(|error| {
                crate::compiler::CompileError::Backend(format!(
                    "failed to finalize selective JIT patch: {error}"
                ))
            })?;
        }
        let finalize_micros = elapsed_micros(finalize_started);
        let executable_bytes = staged_functions
            .iter()
            .map(|(_, _, _, _, _, bytes)| *bytes)
            .sum();
        let patch_artifacts: Vec<JitArtifact> = staged_functions
            .into_iter()
            .enumerate()
            .map(
                |(
                    slot,
                    (
                        function_id,
                        function_key,
                        body_hash,
                        clif_function_id,
                        clif,
                        executable_bytes,
                    ),
                )| {
                    let module = staged_module.as_ref().ok_or_else(|| {
                        crate::compiler::CompileError::Invariant(
                            "emitted JIT patch has no finalized module".to_string(),
                        )
                    })?;
                    Ok(JitArtifact {
                        function_id,
                        function_key,
                        slot: u32::try_from(slot).unwrap_or(u32::MAX),
                        body_hash,
                        code_ptr: module.get_finalized_function(clif_function_id) as usize as u64,
                        executable_bytes,
                        clif,
                    })
                },
            )
            .collect::<CompileResult<Vec<_>>>()?;
        let patch_artifacts_by_key: BTreeMap<FunctionKey, JitArtifact> = patch_artifacts
            .iter()
            .map(|artifact| (artifact.function_key.clone(), artifact.clone()))
            .collect();
        let mut current_keys = patch_plan.re_jit.clone();
        current_keys.extend(patch_plan.reused.iter().cloned());
        current_keys.sort();
        let staged_artifacts: Vec<JitArtifact> = current_keys
            .into_iter()
            .enumerate()
            .map(|(slot, key)| {
                let current_id = function_keys_by_id
                    .iter()
                    .find_map(|(id, candidate)| (candidate == &key).then_some(*id))
                    .ok_or_else(|| {
                        crate::compiler::CompileError::Invariant(format!(
                            "planned function '{}' has no current id",
                            key.display_name()
                        ))
                    })?;
                let mut artifact = patch_artifacts_by_key
                    .get(&key)
                    .or_else(|| previous_artifacts_by_key.get(&key))
                    .cloned()
                    .ok_or_else(|| {
                        crate::compiler::CompileError::Backend(format!(
                            "planned JIT artifact missing for '{}'",
                            key.display_name()
                        ))
                    })?;
                artifact.function_id = current_id;
                artifact.slot = u32::try_from(slot).unwrap_or(u32::MAX);
                Ok(artifact)
            })
            .collect::<CompileResult<Vec<_>>>()?;
        let staged_function_ids = staged_artifacts
            .iter()
            .map(|artifact| artifact.function_id)
            .collect::<BTreeSet<_>>();
        staged_debug_metadata.retain(|function_id, _| staged_function_ids.contains(function_id));
        let layout_hash = u64::from_le_bytes(
            snapshot.layout_digest()[..8]
                .try_into()
                .expect("layout digest prefix has eight bytes"),
        );
        let host_export_signatures = self
            .compiler
            .functions()
            .iter()
            .filter(|function| self.is_host_export_name(&function.name))
            .map(|function| {
                (
                    function.name.clone(),
                    format!("{:?}->{:?}", function.params, function.return_type),
                )
            })
            .collect();
        let host_export_code_ptrs = self
            .compiler
            .functions()
            .iter()
            .filter(|function| self.is_host_export_name(&function.name))
            .filter_map(|function| {
                staged_artifacts
                    .iter()
                    .find(|artifact| artifact.function_id == function.id)
                    .map(|artifact| (function.name.clone(), artifact.code_ptr))
            })
            .collect();
        let emitted_clif_bytes = patch_artifacts
            .iter()
            .map(|artifact| artifact.clif.len())
            .sum();
        let reused_function_ids = staged_artifacts
            .iter()
            .filter(|artifact| patch_plan.reused.contains(&artifact.function_key))
            .map(|artifact| artifact.function_id)
            .collect();
        let retained_jit_bytes = self
            .modules
            .iter()
            .map(|arena| arena.executable_bytes)
            .sum();
        let total_jit_bytes = retained_jit_bytes + executable_bytes;
        let staged_metadata = JitGenerationMetadata {
            source_revision: files_fingerprint,
            layout_hash,
            emitted_function_ids: patch_artifacts
                .iter()
                .map(|artifact| artifact.function_id)
                .collect(),
            reused_function_ids,
            patch_reasons: patch_plan.reasons.clone(),
            affected_host_entries: patch_plan.affected_host_entries.clone(),
            retained_dependencies: patch_plan.retained_dependencies.clone(),
            host_export_signatures,
            host_export_code_ptrs,
            diagnostics: Vec::new(),
            code_owner_revision: files_fingerprint,
            data_owner_layout_hash: layout_hash,
            codegen_micros,
            plan_micros,
            finalize_micros,
            module_count: self.modules.len() + usize::from(staged_module.is_some()),
            retained_arena_count: self.modules.len(),
            emitted_clif_bytes,
            executable_bytes,
            retained_jit_bytes,
            total_jit_bytes,
        };
        stasis_dynload::finish_jit_string_literal_staging()
            .map_err(crate::compiler::CompileError::Backend)?;
        let staged_string_literals = snapshot
            .literal_table()
            .iter()
            .map(|(id, value)| (*id, value.clone()))
            .collect();
        drop(snapshot);
        let accepted_program = capture_accepted_program(
            self.compiler.functions(),
            self.compiler.files(),
            &self.required_emit_roots,
        )
        .map_err(crate::compiler::CompileError::Backend)?;
        self.artifacts = staged_artifacts;
        if let Some(snapshot) = self.program_snapshot.as_mut() {
            let snapshot = Arc::make_mut(snapshot);
            snapshot
                .set_artifact_mappings(self.artifacts.iter().map(|artifact| {
                    ProgramArtifactMapping {
                        function_id: artifact.function_id,
                        symbol_id: artifact.function_key.symbol_id.clone(),
                        symbol: artifact.function_key.display_name(),
                        target_path: None,
                        code_pointer: Some(artifact.code_ptr),
                    }
                }))
                .map_err(crate::compiler::CompileError::Invariant)?;
        }
        if let Some(module) = staged_module {
            self.modules.push(Arc::new(JitArena {
                _module: Mutex::new(module),
                executable_bytes,
            }));
        }
        self.generation_metadata = Some(staged_metadata);
        self.accepted_program = Some(accepted_program);
        self.accepted_lowering_references = lowering_references;
        self.accepted_lowering_contracts = lowering_contracts;
        self.debug_metadata = staged_debug_metadata;
        self.staged_string_literals = staged_string_literals;
        let report = CompileReport { index, emit };
        self.rebuild_artifact_index();
        Ok(report)
    }

    pub fn artifacts(&self) -> &[JitArtifact] {
        &self.artifacts
    }

    pub fn program_snapshot(&self) -> Option<&ProgramSnapshot> {
        self.program_snapshot.as_deref()
    }

    pub fn generation_metadata(&self) -> Option<&JitGenerationMetadata> {
        self.generation_metadata.as_ref()
    }

    fn is_host_export_name(&self, name: &str) -> bool {
        matches!(name, "main" | "tick" | "render" | "on_code_swap")
            || self.required_emit_roots.iter().any(|root| root == name)
    }

    pub fn clif_for_function_name(&self, name: &str) -> Option<&str> {
        let function = self
            .compiler
            .functions()
            .iter()
            .find(|function| function.name == name)?;
        self.artifact_for_function_id(function.id)
            .map(|artifact| artifact.clif.as_str())
    }

    pub fn last_source_diagnostic(&self) -> Option<&crate::SourceDiagnostic> {
        self.last_failed_source_diagnostic
            .as_ref()
            .or_else(|| self.compiler.last_source_diagnostic())
    }

    pub fn activate_runtime_state(&self) {
        stasis_dynload::replace_jit_string_literal_table(&self.staged_string_literals);
    }

    pub fn activate_staged_runtime(&self) -> Result<(), String> {
        let analysis = &self
            .program_snapshot
            .as_ref()
            .ok_or_else(|| "cannot activate an uncompiled JIT process".to_string())?
            .analysis
            .clone();
        build_direct_storage_bindings(
            &analysis.global_path_types,
            &analysis.collection_infos,
            self.compiler.types(),
            true,
        )?;
        seed_fixed_collection_max_length_headers(
            &analysis.global_path_types,
            self.compiler.types(),
        )?;
        stasis_dynload::replace_jit_string_literal_table(&self.staged_string_literals);
        Ok(())
    }

    pub fn artifact_slot_for_function_name(&self, name: &str) -> Option<u32> {
        let function = self.unique_function_by_name(name).ok()?;
        self.artifact_for_function_id(function.id)
            .map(|artifact| artifact.slot)
    }

    pub fn execute_i32_noarg_by_name(&self, name: &str) -> Result<i32, String> {
        let function = self.unique_function_by_name(name)?;
        if function.return_type != TYPE_ID_I32 {
            return Err(format!(
                "function '{name}' is not i32-returning (type id {})",
                function.return_type
            ));
        }
        if !function.params.is_empty() {
            return Err(format!(
                "function '{name}' is not a no-argument function (param count {})",
                function.params.len()
            ));
        }
        let artifact = self
            .artifact_for_function_id(function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;
        let raw = stasis_dynload::invoke_noarg_u64(artifact.code_ptr as usize)?;
        Ok((raw as u32) as i32)
    }

    /// Validate the narrow host ABI used by deterministic recording hooks.
    ///
    /// Name lookup goes through the accepted program snapshot so duplicate
    /// aliases are rejected instead of selecting an arbitrary declaration.
    pub fn validate_i32_onearg_by_name(&self, name: &str) -> Result<(), String> {
        let function = self.unique_function_by_name(name)?;
        if function.return_type != TYPE_ID_I32 || function.params.as_slice() != [TYPE_ID_I32] {
            return Err(format!(
                "before-tick hook '{name}' signature mismatch; expected `function {name}(frame: i32): i32`, actual return type id {}, parameter types {:?}",
                function.return_type, function.params
            ));
        }
        Ok(())
    }

    pub fn execute_i32_onearg_by_name(&self, name: &str, frame: i32) -> Result<i32, String> {
        self.validate_i32_onearg_by_name(name)?;
        let function = self.unique_function_by_name(name)?;
        let artifact = self
            .artifact_for_function_id(function.id)
            .ok_or_else(|| format!("compiled artifact missing for before-tick hook '{name}'"))?;
        stasis_dynload::invoke_i32_to_i32(artifact.code_ptr as usize, frame)
    }

    pub fn execute_void_noarg_by_name(&self, name: &str) -> Result<(), String> {
        let function = self.unique_function_by_name(name)?;
        if function.return_type != TYPE_ID_VOID {
            return Err(format!(
                "function '{name}' is not void-returning (type id {})",
                function.return_type
            ));
        }
        if !function.params.is_empty() {
            return Err(format!(
                "function '{name}' is not a no-argument function (param count {})",
                function.params.len()
            ));
        }
        let artifact = self
            .artifact_for_function_id(function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;
        stasis_dynload::invoke_noarg_void(artifact.code_ptr as usize)
    }

    pub fn read_i32_global_path(&self, path: &str) -> i32 {
        stasis_dynload::stasis_jit_global_i32_load(hash_global_path(path))
    }

    pub fn has_global_path(&self, path: &str) -> bool {
        self.program_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.analysis.global_path_types.contains_key(path))
    }

    pub fn global_scalar_type(&self, path: &str) -> Option<&'static str> {
        let type_id = self
            .program_snapshot
            .as_ref()?
            .analysis
            .global_path_types
            .get(path)?;
        scalar_type_name(*type_id)
    }

    pub fn global_binding_type(&self, path: &str) -> Option<&'static str> {
        let type_id = *self
            .program_snapshot
            .as_ref()?
            .analysis
            .global_path_types
            .get(path)?;
        if let Some(type_name) = scalar_type_name(type_id) {
            return Some(type_name);
        }
        let type_table = self.program_snapshot.as_ref()?.types();
        match type_table.type_info(type_id)?.category {
            TypeCategory::AsciiFixed
            | TypeCategory::AsciiView
            | TypeCategory::Utf8Fixed
            | TypeCategory::Utf8View => Some("string"),
            TypeCategory::ArrayFixed | TypeCategory::ArrayView => {
                scalar_type_name(type_table.indexed_element_type_id(type_id)?)
            }
            _ => None,
        }
    }

    pub fn global_binding_capacity(&self, path: &str) -> Option<i32> {
        let type_id = *self
            .program_snapshot
            .as_ref()?
            .analysis
            .global_path_types
            .get(path)?;
        self.program_snapshot
            .as_ref()?
            .types()
            .fixed_collection_len(type_id)
    }

    pub fn global_collection_capacity(&self, path: &str) -> Option<i32> {
        self.program_snapshot
            .as_ref()?
            .analysis
            .collection_infos
            .get(path)
            .map(|info| info.len)
    }

    pub fn global_collection_field_type(&self, path: &str, field: &str) -> Option<&'static str> {
        let type_id = self
            .program_snapshot
            .as_ref()?
            .analysis
            .collection_infos
            .get(path)?
            .field_types
            .get(field)?;
        scalar_type_name(*type_id)
    }

    pub fn global_scalar_paths(&self) -> Vec<(String, &'static str)> {
        let Some(snapshot) = self.program_snapshot.as_ref() else {
            return Vec::new();
        };
        let analysis = &snapshot.analysis;
        analysis
            .global_path_types
            .iter()
            .filter_map(|(path, type_id)| {
                scalar_type_name(*type_id).map(|type_name| (path.clone(), type_name))
            })
            .collect()
    }

    pub fn state_layout(&self) -> JitStateLayout {
        self.program_snapshot
            .as_ref()
            .map_or_else(JitStateLayout::default, |snapshot| {
                snapshot.state_layout().clone()
            })
    }

    pub fn state_memory_report(
        &self,
        capacity_overrides: &BTreeMap<String, u64>,
        mobile_budget_bytes: u64,
    ) -> Result<JitStateMemoryReport, String> {
        let layout = self.state_layout();
        let active_counts = layout
            .collections
            .iter()
            .map(|collection| {
                let capacity = u64::try_from(collection.capacity).unwrap_or(0);
                let active = if self.global_fixed_text_encoding(&collection.path).is_some() {
                    self.read_global_scalar(&format!("{}.byte_length", collection.path))
                        .ok()
                        .and_then(jit_value_as_nonnegative_u64)
                        .unwrap_or(0)
                        .min(capacity)
                } else {
                    capacity
                };
                (collection.path.clone(), active)
            })
            .collect::<BTreeMap<_, _>>();
        build_state_memory_report(
            &layout,
            &active_counts,
            capacity_overrides,
            mobile_budget_bytes,
        )
    }

    pub fn performance_cost_report(
        &self,
        memory: &JitStateMemoryReport,
        aot_object_code_bytes: u64,
        literal_data_bytes: u64,
    ) -> Result<crate::performance::PerformanceCostReport, String> {
        crate::performance::build_performance_cost_report(
            self.compiler.files(),
            self.compiler.function_data_flow_summaries(),
            &self.state_layout(),
            memory,
            aot_object_code_bytes,
            literal_data_bytes,
        )
    }

    pub fn inspect_state_query(&self, query: &str) -> Result<JsonValue, String> {
        self.inspect_state_query_with_scan_limit(query, MAX_STATE_QUERY_SCAN)
    }

    pub fn inspect_state_query_with_scan_limit(
        &self,
        query: &str,
        max_predicate_scan: usize,
    ) -> Result<JsonValue, String> {
        match parse_state_query(query)? {
            StateQuery::Scalar(expression) => {
                let value = self.evaluate_state_expression(&expression)?;
                Ok(json!({
                    "query": query,
                    "path": query,
                    "kind": "scalar",
                    "static_type": value.type_name(),
                    "value": value,
                }))
            }
            StateQuery::Predicate(predicate) => {
                if max_predicate_scan == 0 {
                    return Err("state predicate query scan budget is exhausted".to_string());
                }
                let (_, capacity) =
                    self.global_collection_value_type(&predicate.path, &predicate.field)?;
                let capacity = usize::try_from(capacity).map_err(|_| {
                    format!(
                        "collection '{}' has invalid capacity {capacity}",
                        predicate.path
                    )
                })?;
                let scan_count = capacity.min(MAX_STATE_QUERY_SCAN).min(max_predicate_scan);
                let right = self.evaluate_state_expression(&predicate.right)?;
                let mut total_matches = 0usize;
                let mut matches = Vec::new();
                for index in 0..scan_count {
                    let value = self.read_global_collection_scalar(
                        &predicate.path,
                        &predicate.field,
                        i32::try_from(index)
                            .map_err(|_| "state query index overflow".to_string())?,
                    )?;
                    let matched = apply_state_binary(value, predicate.operator, right)?;
                    if matched != JitScalarValue::Bool(true) {
                        continue;
                    }
                    total_matches += 1;
                    if matches.len() < MAX_STATE_QUERY_MATCHES {
                        matches.push(json!({"index": index, "value": value}));
                    }
                }
                Ok(json!({
                    "query": query,
                    "path": query,
                    "kind": "predicate",
                    "path": predicate.path,
                    "field": predicate.field,
                    "operator": predicate.operator.text(),
                    "capacity": capacity,
                    "scanned": scan_count,
                    "scan_truncated": scan_count < capacity,
                    "total_matches": total_matches,
                    "matches_truncated": total_matches > matches.len(),
                    "matches": matches,
                }))
            }
        }
    }

    fn evaluate_state_expression(
        &self,
        expression: &ScalarExpression,
    ) -> Result<JitScalarValue, String> {
        match expression {
            ScalarExpression::Value(reference) => self.evaluate_state_reference(reference),
            ScalarExpression::Negate(expression) => {
                negate_state_value(self.evaluate_state_expression(expression)?)
            }
            ScalarExpression::Binary {
                left,
                operator,
                right,
            } => apply_state_binary(
                self.evaluate_state_expression(left)?,
                *operator,
                self.evaluate_state_expression(right)?,
            ),
        }
    }

    fn evaluate_state_reference(
        &self,
        reference: &StateValueReference,
    ) -> Result<JitScalarValue, String> {
        match reference {
            StateValueReference::Path(path) => self.read_global_scalar(path),
            StateValueReference::CollectionItem { path, index, field } => {
                self.read_global_collection_scalar(path, field, *index)
            }
            StateValueReference::I32(value) => Ok(JitScalarValue::I32(*value)),
            StateValueReference::F64(value) => Ok(JitScalarValue::F64(*value)),
            StateValueReference::Bool(value) => Ok(JitScalarValue::Bool(*value)),
        }
    }

    pub fn read_global_collection_scalar(
        &self,
        path: &str,
        field: &str,
        index: i32,
    ) -> Result<JitScalarValue, String> {
        let (type_id, capacity) = self.global_collection_value_type(path, field)?;
        if index < 0 || index >= capacity {
            return Err(format!(
                "global collection path '{path}' index {index} is outside capacity {capacity}"
            ));
        }
        let collection_hash = hash_global_path(path);
        let field_hash = hash_foreach_field_suffix(field);
        if field.is_empty() && self.global_fixed_text_capacity(path).is_some() {
            return Ok(JitScalarValue::U8(
                stasis_dynload::stasis_jit_global_i32_array_load(collection_hash, field_hash, index)
                    as u8,
            ));
        }
        match type_id {
            TYPE_ID_I32 => Ok(JitScalarValue::I32(
                stasis_dynload::stasis_jit_global_i32_array_load(
                    collection_hash,
                    field_hash,
                    index,
                ),
            )),
            TYPE_ID_F32 => Ok(JitScalarValue::F32(
                stasis_dynload::stasis_jit_global_f32_array_load(
                    collection_hash,
                    field_hash,
                    index,
                ),
            )),
            TYPE_ID_F64 => Ok(JitScalarValue::F64(
                stasis_dynload::stasis_jit_global_f64_array_load(
                    collection_hash,
                    field_hash,
                    index,
                ),
            )),
            TYPE_ID_BOOL => Ok(JitScalarValue::Bool(
                stasis_dynload::stasis_jit_global_i32_array_load(
                    collection_hash,
                    field_hash,
                    index,
                ) != 0,
            )),
            type_id if is_u8_type(self.compiler.types(), type_id) => Ok(JitScalarValue::U8(
                stasis_dynload::stasis_jit_global_i32_array_load(collection_hash, field_hash, index)
                    as u8,
            )),
            TYPE_ID_U16 => Ok(JitScalarValue::U16(
                stasis_dynload::stasis_jit_global_i32_array_load(collection_hash, field_hash, index)
                    as u16,
            )),
            TYPE_ID_U32 => Ok(JitScalarValue::U32(
                stasis_dynload::stasis_jit_global_i32_array_load(collection_hash, field_hash, index)
                    as u32,
            )),
            _ => Err(format!(
                "global collection path '{path}' field '{field}' is not a supported scalar"
            )),
        }
    }

    pub fn write_global_collection_scalar(
        &self,
        path: &str,
        field: &str,
        index: i32,
        value: JitScalarValue,
    ) -> Result<(), String> {
        let (type_id, capacity) = self.global_collection_value_type(path, field)?;
        if index < 0 || index >= capacity {
            return Err(format!(
                "global collection path '{path}' index {index} is outside capacity {capacity}"
            ));
        }
        let collection_hash = hash_global_path(path);
        let field_hash = hash_foreach_field_suffix(field);
        if field.is_empty() && self.global_fixed_text_capacity(path).is_some() {
            let JitScalarValue::U8(value) = value else {
                return Err(format!(
                    "global text path '{path}' does not accept {}",
                    value.type_name()
                ));
            };
            stasis_dynload::stasis_jit_global_i32_array_store(
                collection_hash,
                field_hash,
                index,
                i32::from(value),
            );
            return Ok(());
        }
        match (type_id, value) {
            (TYPE_ID_I32, JitScalarValue::I32(value)) => {
                stasis_dynload::stasis_jit_global_i32_array_store(
                    collection_hash,
                    field_hash,
                    index,
                    value,
                )
            }
            (TYPE_ID_F32, JitScalarValue::F32(value)) => {
                stasis_dynload::stasis_jit_global_f32_array_store(
                    collection_hash,
                    field_hash,
                    index,
                    value,
                )
            }
            (TYPE_ID_F64, JitScalarValue::F64(value)) => {
                stasis_dynload::stasis_jit_global_f64_array_store(
                    collection_hash,
                    field_hash,
                    index,
                    value,
                )
            }
            (TYPE_ID_BOOL, JitScalarValue::Bool(value)) => {
                stasis_dynload::stasis_jit_global_i32_array_store(
                    collection_hash,
                    field_hash,
                    index,
                    i32::from(value),
                )
            }
            (type_id, JitScalarValue::U8(value)) if is_u8_type(self.compiler.types(), type_id) => {
                stasis_dynload::stasis_jit_global_i32_array_store(
                    collection_hash,
                    field_hash,
                    index,
                    i32::from(value),
                )
            }
            (TYPE_ID_U16, JitScalarValue::U16(value)) => {
                stasis_dynload::stasis_jit_global_i32_array_store(
                    collection_hash,
                    field_hash,
                    index,
                    i32::from(value),
                )
            }
            (TYPE_ID_U32, JitScalarValue::U32(value)) => {
                stasis_dynload::stasis_jit_global_i32_array_store(
                    collection_hash,
                    field_hash,
                    index,
                    value as i32,
                )
            }
            (_, value) => {
                return Err(format!(
                    "global collection path '{path}' field '{field}' does not accept {}",
                    value.type_name()
                ))
            }
        }
        Ok(())
    }

    pub fn read_global_scalar(&self, path: &str) -> Result<JitScalarValue, String> {
        let type_id = self.global_path_type(path)?;
        let path_hash = hash_global_path(path);
        match type_id {
            TYPE_ID_I32 => Ok(JitScalarValue::I32(
                stasis_dynload::stasis_jit_global_i32_load(path_hash),
            )),
            TYPE_ID_F32 => Ok(JitScalarValue::F32(
                stasis_dynload::stasis_jit_global_f32_load(path_hash),
            )),
            TYPE_ID_F64 => Ok(JitScalarValue::F64(
                stasis_dynload::stasis_jit_global_f64_load(path_hash),
            )),
            TYPE_ID_BOOL => Ok(JitScalarValue::Bool(
                stasis_dynload::stasis_jit_global_i32_load(path_hash) != 0,
            )),
            TYPE_ID_U8 => Ok(JitScalarValue::U8(
                stasis_dynload::stasis_jit_global_i32_array_load(path_hash, 0, 0) as u8,
            )),
            TYPE_ID_U16 => Ok(JitScalarValue::U16(
                stasis_dynload::stasis_jit_global_i32_array_load(path_hash, 0, 0) as u16,
            )),
            TYPE_ID_U32 => Ok(JitScalarValue::U32(
                stasis_dynload::stasis_jit_global_i32_load(path_hash) as u32,
            )),
            type_id if self.is_named_i32_state_scalar(path, type_id) => Ok(JitScalarValue::I32(
                stasis_dynload::stasis_jit_global_i32_load(path_hash),
            )),
            _ => Err(format!("global path '{path}' is not a supported scalar")),
        }
    }

    pub fn write_global_scalar(&self, path: &str, value: JitScalarValue) -> Result<(), String> {
        let type_id = self.global_path_type(path)?;
        let path_hash = hash_global_path(path);
        match (type_id, value) {
            (TYPE_ID_I32, JitScalarValue::I32(value)) => {
                stasis_dynload::stasis_jit_global_i32_store(path_hash, value)
            }
            (TYPE_ID_F32, JitScalarValue::F32(value)) => {
                stasis_dynload::stasis_jit_global_f32_store(path_hash, value)
            }
            (TYPE_ID_F64, JitScalarValue::F64(value)) => {
                stasis_dynload::stasis_jit_global_f64_store(path_hash, value)
            }
            (TYPE_ID_BOOL, JitScalarValue::Bool(value)) => {
                stasis_dynload::stasis_jit_global_i32_store(path_hash, i32::from(value))
            }
            (TYPE_ID_U8, JitScalarValue::U8(value)) => {
                stasis_dynload::stasis_jit_global_i32_array_store(path_hash, 0, 0, i32::from(value))
            }
            (TYPE_ID_U16, JitScalarValue::U16(value)) => {
                stasis_dynload::stasis_jit_global_i32_array_store(path_hash, 0, 0, i32::from(value))
            }
            (TYPE_ID_U32, JitScalarValue::U32(value)) => {
                stasis_dynload::stasis_jit_global_i32_store(path_hash, value as i32)
            }
            (type_id, JitScalarValue::I32(value))
                if self.is_named_i32_state_scalar(path, type_id) =>
            {
                stasis_dynload::stasis_jit_global_i32_store(path_hash, value)
            }
            (_, value) => {
                return Err(format!(
                    "global path '{path}' does not accept {}",
                    value.type_name()
                ))
            }
        }
        Ok(())
    }

    pub fn snapshot_global_scalars(&self) -> Vec<(String, JitScalarValue)> {
        self.global_scalar_paths()
            .into_iter()
            .filter_map(|(path, _)| {
                self.read_global_scalar(&path)
                    .ok()
                    .map(|value| (path, value))
            })
            .collect()
    }

    pub fn restore_global_scalars(
        &self,
        snapshot: &[(String, JitScalarValue)],
    ) -> Result<(), String> {
        for (path, value) in snapshot {
            self.write_global_scalar(path, *value)?;
        }
        Ok(())
    }

    fn global_path_type(&self, path: &str) -> Result<u16, String> {
        self.program_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.analysis.global_path_types.get(path).copied())
            .ok_or_else(|| format!("global path '{path}' was not found in compiler metadata"))
    }

    fn is_named_i32_state_scalar(&self, path: &str, type_id: u16) -> bool {
        self.program_snapshot.as_ref().is_some_and(|snapshot| {
            is_named_scalar_state_path(
                path,
                type_id,
                &snapshot.analysis.global_path_types,
                snapshot.types(),
            )
        })
    }

    fn global_collection_value_type(&self, path: &str, field: &str) -> Result<(u16, i32), String> {
        let analysis = &self
            .program_snapshot
            .as_ref()
            .ok_or_else(|| "JIT collection metadata is unavailable".to_string())?
            .analysis;
        if let Some(info) = analysis.collection_infos.get(path) {
            let type_id = if field.is_empty() {
                info.element_type
            } else {
                info.field_types.get(field).copied()
            }
            .ok_or_else(|| {
                format!("global collection path '{path}' field '{field}' was not found")
            })?;
            return Ok((type_id, info.len));
        }
        if field.is_empty() {
            if let Some(capacity) = self.global_fixed_text_capacity(path) {
                let type_id = analysis
                    .global_path_types
                    .get(path)
                    .and_then(|type_id| {
                        self.program_snapshot
                            .as_ref()?
                            .types()
                            .indexed_element_type_id(*type_id)
                    })
                    .ok_or_else(|| format!("global text path '{path}' has no payload type"))?;
                return Ok((type_id, capacity));
            }
        }
        Err(format!("global collection path '{path}' was not found"))
    }

    fn global_fixed_text_capacity(&self, path: &str) -> Option<i32> {
        let type_id = self
            .program_snapshot
            .as_ref()?
            .analysis
            .global_path_types
            .get(path)?;
        let category = self
            .program_snapshot
            .as_ref()?
            .types()
            .type_info(*type_id)?
            .category;
        matches!(category, TypeCategory::AsciiFixed | TypeCategory::Utf8Fixed)
            .then(|| {
                self.program_snapshot
                    .as_ref()?
                    .types()
                    .fixed_collection_len(*type_id)
            })
            .flatten()
    }

    pub fn global_fixed_text_encoding(&self, path: &str) -> Option<&'static str> {
        let type_id = self
            .program_snapshot
            .as_ref()?
            .analysis
            .global_path_types
            .get(path)?;
        match self
            .program_snapshot
            .as_ref()?
            .types()
            .type_info(*type_id)?
            .category
        {
            TypeCategory::AsciiFixed => Some("ascii"),
            TypeCategory::Utf8Fixed => Some("utf8"),
            _ => None,
        }
    }

    pub fn preflight_global_collection_capacity(
        &self,
        path: &str,
        field: &str,
        capacity: u32,
    ) -> Result<(), String> {
        self.collection_capacity_operation(path, field, capacity, false)
    }

    pub fn ensure_global_collection_capacity(
        &self,
        path: &str,
        field: &str,
        capacity: u32,
    ) -> Result<(), String> {
        self.collection_capacity_operation(path, field, capacity, true)
    }

    fn collection_capacity_operation(
        &self,
        path: &str,
        field: &str,
        capacity: u32,
        grow: bool,
    ) -> Result<(), String> {
        let (type_id, _) = self.global_collection_value_type(path, field)?;
        let capacity = capacity as usize;
        let collection_hash = hash_global_path(path);
        let field_hash = hash_foreach_field_suffix(field);
        let i32_capacity = if grow {
            stasis_dynload::ensure_jit_i32_array_capacity
        } else {
            stasis_dynload::preflight_jit_i32_array_capacity
        };
        let f32_capacity = if grow {
            stasis_dynload::ensure_jit_f32_array_capacity
        } else {
            stasis_dynload::preflight_jit_f32_array_capacity
        };
        let f64_capacity = if grow {
            stasis_dynload::ensure_jit_f64_array_capacity
        } else {
            stasis_dynload::preflight_jit_f64_array_capacity
        };
        let u16_capacity = if grow {
            stasis_dynload::ensure_jit_u16_array_capacity
        } else {
            stasis_dynload::preflight_jit_u16_array_capacity
        };
        let u8_capacity = if grow {
            stasis_dynload::ensure_jit_u8_array_capacity
        } else {
            stasis_dynload::preflight_jit_u8_array_capacity
        };
        if field.is_empty() && self.global_fixed_text_capacity(path).is_some() {
            return i32_capacity(collection_hash, field_hash, capacity);
        }
        match type_id {
            TYPE_ID_I32 | TYPE_ID_BOOL | TYPE_ID_U32 => {
                i32_capacity(collection_hash, field_hash, capacity)
            }
            TYPE_ID_F32 => f32_capacity(collection_hash, field_hash, capacity),
            TYPE_ID_F64 => f64_capacity(collection_hash, field_hash, capacity),
            type_id if is_u8_type(self.compiler.types(), type_id) => {
                u8_capacity(collection_hash, field_hash, capacity)
            }
            TYPE_ID_U16 => u16_capacity(collection_hash, field_hash, capacity),
            _ => Err(format!(
                "global collection path '{path}' field '{field}' is not resizable"
            )),
        }
    }

    pub fn write_i32_global_path(&self, path: &str, value: i32) {
        stasis_dynload::stasis_jit_global_i32_store(hash_global_path(path), value);
    }

    pub fn execute_bool_noarg_by_name(&self, name: &str) -> Result<bool, String> {
        let function = self
            .compiler
            .functions()
            .iter()
            .find(|function| function.name == name)
            .ok_or_else(|| format!("function '{name}' not found"))?;
        if function.return_type != TYPE_ID_BOOL {
            return Err(format!(
                "function '{name}' is not bool-returning (type id {})",
                function.return_type
            ));
        }
        if !function.params.is_empty() {
            return Err(format!(
                "function '{name}' is not a no-argument function (param count {})",
                function.params.len()
            ));
        }
        let artifact = self
            .artifact_for_function_id(function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;
        let raw = stasis_dynload::invoke_noarg_u64(artifact.code_ptr as usize)?;
        Ok((raw as u32) != 0)
    }

    pub fn execute_i32_twoarg_by_name(
        &self,
        name: &str,
        left: i32,
        right: i32,
    ) -> Result<i32, String> {
        let function = self
            .compiler
            .functions()
            .iter()
            .find(|function| function.name == name)
            .ok_or_else(|| format!("function '{name}' not found"))?;
        if function.return_type != TYPE_ID_I32 {
            return Err(format!(
                "function '{name}' is not i32-returning (type id {})",
                function.return_type
            ));
        }
        if function.params.len() != 2 {
            return Err(format!(
                "function '{name}' is not a two-argument function (param count {})",
                function.params.len()
            ));
        }
        let artifact = self
            .artifact_for_function_id(function.id)
            .ok_or_else(|| format!("compiled artifact missing for function '{name}'"))?;
        stasis_dynload::invoke_i32_i32_to_i32(artifact.code_ptr as usize, left, right)
    }

    pub fn symbol_code_ptrs(&self) -> BTreeMap<String, u64> {
        self.generation_metadata
            .as_ref()
            .map(|metadata| metadata.host_export_code_ptrs.clone())
            .unwrap_or_default()
    }

    fn validate_host_aliases(&self) -> Result<(), String> {
        let mut counts = BTreeMap::new();
        for function in self
            .compiler
            .functions()
            .iter()
            .filter(|function| self.is_host_export_name(&function.name))
        {
            *counts.entry(function.name.as_str()).or_insert(0usize) += 1;
        }
        if let Some((name, count)) = counts.into_iter().find(|(_, count)| *count != 1) {
            return Err(format!(
                "host ABI alias '{name}' requires exactly one canonical identity (found {count})"
            ));
        }
        Ok(())
    }

    fn unique_function_by_name(&self, name: &str) -> Result<&ProgramFunction, String> {
        let mut matches = self
            .program_snapshot
            .as_ref()
            .ok_or_else(|| "program has not compiled successfully".to_string())?
            .functions()
            .iter()
            .filter(|function| function.name == name);
        let function = matches
            .next()
            .ok_or_else(|| format!("function '{name}' not found"))?;
        if matches.next().is_some() {
            return Err(format!(
                "function alias '{name}' is ambiguous across canonical identities"
            ));
        }
        Ok(function)
    }

    pub fn set_project_root(&mut self, root: impl Into<String>) -> Result<(), String> {
        self.compiler.set_project_root(root)
    }

    /// Canonical artifact map. Host-export names are aliases only and must not be
    /// used as the authoritative key for runtime patch identity.
    pub fn function_code_ptrs(&self) -> BTreeMap<FunctionId, u64> {
        self.artifacts
            .iter()
            .map(|artifact| (artifact.function_id, artifact.code_ptr))
            .collect()
    }

    pub fn validate_on_code_swap_signature(&self) -> Result<(), String> {
        let matches = self
            .program_snapshot
            .as_ref()
            .map(|snapshot| snapshot.functions())
            .unwrap_or_default()
            .iter()
            .filter(|function| function.name == "on_code_swap")
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return Ok(());
        }
        if matches.len() != 1 {
            return Err(format!(
                "host ABI alias 'on_code_swap' requires exactly one canonical identity (found {})",
                matches.len()
            ));
        }
        let function = matches[0];
        if function.return_type != TYPE_ID_VOID || !function.params.is_empty() {
            return Err(
                "invalid on_code_swap signature; expected function on_code_swap(): void"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub fn execute_optional_on_code_swap(&self) -> Result<(), String> {
        self.validate_on_code_swap_signature()?;
        if !self.has_on_code_swap() {
            return Ok(());
        }
        let function = self.unique_function_by_name("on_code_swap")?;
        let artifact = self
            .artifact_for_function_id(function.id)
            .ok_or_else(|| "compiled artifact missing for function 'on_code_swap'".to_string())?;
        stasis_dynload::invoke_code_swap_hook(artifact.code_ptr as usize)
    }

    pub fn has_on_code_swap(&self) -> bool {
        self.program_snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .functions()
                .iter()
                .any(|function| function.name == "on_code_swap")
        })
    }

    pub fn build_engine_package(
        &self,
        entrypoints: &EngineEntrypoints,
    ) -> Result<JitEnginePackage, String> {
        let tick_code_ptr = self.code_ptr_for_i32_noarg_entrypoint(&entrypoints.tick)?;
        let render_code_ptr = self.code_ptr_for_i32_noarg_entrypoint(&entrypoints.render)?;
        let on_code_swap_code_ptr = if let Some(name) = entrypoints.on_code_swap.as_ref() {
            self.validate_on_code_swap_signature()?;
            Some(self.code_ptr_for_function_name(name)?)
        } else {
            None
        };

        Ok(JitEnginePackage {
            tick_code_ptr,
            render_code_ptr,
            on_code_swap_code_ptr,
            symbol_code_ptrs: self.symbol_code_ptrs(),
            function_code_ptrs: self.function_code_ptrs(),
        })
    }

    fn code_ptr_for_function_name(&self, name: &str) -> Result<u64, String> {
        let function = self.unique_host_alias(name)?;
        let artifact = self
            .artifact_for_function_id(function.id)
            .ok_or_else(|| format!("compiled artifact missing for required entrypoint '{name}'"))?;
        Ok(artifact.code_ptr)
    }

    fn code_ptr_for_i32_noarg_entrypoint(&self, name: &str) -> Result<u64, String> {
        let function = self.unique_host_alias(name)?;
        if function.return_type != TYPE_ID_I32 || !function.params.is_empty() {
            return Err(format!(
                "engine entrypoint signature mismatch for '{name}': expected `function {name}(): i32`; actual return type id {}, parameter count {}",
                function.return_type,
                function.params.len()
            ));
        }
        self.artifact_for_function_id(function.id)
            .map(|artifact| artifact.code_ptr)
            .ok_or_else(|| format!("compiled artifact missing for required entrypoint '{name}'"))
    }

    fn unique_host_alias(&self, name: &str) -> Result<&FunctionMeta, String> {
        let mut matches = self
            .compiler
            .functions()
            .iter()
            .filter(|function| function.name == name);
        let first = matches
            .next()
            .ok_or_else(|| format!("required engine entrypoint '{name}' not found"))?;
        if matches.next().is_some() {
            return Err(format!(
                "host ABI alias '{name}' is ambiguous across canonical function identities"
            ));
        }
        Ok(first)
    }

    fn rebuild_artifact_index(&mut self) {
        self.artifact_index.clear();
        for (index, artifact) in self.artifacts.iter().enumerate() {
            self.artifact_index.insert(artifact.function_id, index);
        }
    }

    fn artifact_for_function_id(&self, function_id: FunctionId) -> Option<&JitArtifact> {
        let index = self.artifact_index.get(&function_id).copied()?;
        self.artifacts.get(index)
    }

    fn resolve_extern_call_signatures(
        &mut self,
        extern_signatures: &[ExternCallSignature],
    ) -> Result<(Vec<ResolvedExternCallSignature>, ExternSymbolAddressMap), String> {
        resolve_extern_call_signatures_with(extern_signatures, |_signature, candidate| {
            self.resolve_host_symbol_address(candidate)
        })
    }

    fn resolve_host_symbol_address(&mut self, symbol: &str) -> Option<usize> {
        if let Some(existing) = self.runtime_symbol_cache.get(symbol).copied() {
            return Some(existing);
        }
        if let Some(address) = builtin_host_symbol_address(symbol) {
            self.runtime_symbol_cache
                .insert(symbol.to_string(), address);
            return Some(address);
        }
        self.ensure_runtime_libraries_loaded();
        for library in &self.runtime_libraries {
            if let Ok(address) = library.symbol_address(symbol) {
                self.runtime_symbol_cache
                    .insert(symbol.to_string(), address);
                return Some(address);
            }
        }
        None
    }

    fn ensure_runtime_libraries_loaded(&mut self) {
        if !self.runtime_libraries.is_empty() {
            return;
        }
        for path in stasis_dynload::runtime_library_candidate_paths() {
            if !path.exists() {
                continue;
            }
            if let Ok(library) = stasis_dynload::Library::load(&path) {
                self.runtime_libraries.push(library);
            }
        }
    }
}

#[cfg(test)]
impl Drop for JitProcess {
    fn drop(&mut self) {
        if self._test_guard.is_some() {
            // Clear registrations before modules and the serialization guard are dropped. This
            // prevents the next test from observing raw pointers into an already-freed JIT/host
            // allocation, even when that test snapshots runtime state before creating a process.
            clear_jit_process_test_runtime();
        }
    }
}

#[cfg(test)]
fn clear_jit_process_test_runtime() {
    // Many unit tests manipulate process-global JIT tables. Keep them isolated and
    // deterministic by clearing under the test guard.
    stasis_dynload::clear_jit_i32_global_table();
    stasis_dynload::clear_jit_f32_global_table();
    stasis_dynload::clear_jit_f64_global_table();
    stasis_dynload::clear_jit_i32_array_global_table();
    stasis_dynload::clear_jit_f32_array_global_table();
    stasis_dynload::clear_jit_f64_array_global_table();
    stasis_dynload::clear_jit_string_literal_table();
    stasis_dynload::clear_registered_global_memory();
}

fn scalar_type_name(type_id: u16) -> Option<&'static str> {
    match type_id {
        TYPE_ID_I32 => Some("i32"),
        TYPE_ID_F32 => Some("f32"),
        TYPE_ID_F64 => Some("f64"),
        TYPE_ID_BOOL => Some("bool"),
        TYPE_ID_U8 => Some("u8"),
        TYPE_ID_U16 => Some("u16"),
        TYPE_ID_U32 => Some("u32"),
        _ => None,
    }
}

fn jit_value_as_nonnegative_u64(value: JitScalarValue) -> Option<u64> {
    match value {
        JitScalarValue::I32(value) if value >= 0 => Some(value as u64),
        JitScalarValue::U8(value) => Some(u64::from(value)),
        JitScalarValue::U16(value) => Some(u64::from(value)),
        JitScalarValue::U32(value) => Some(u64::from(value)),
        _ => None,
    }
}

fn negate_state_value(value: JitScalarValue) -> Result<JitScalarValue, String> {
    match value {
        JitScalarValue::I32(value) => value
            .checked_neg()
            .map(JitScalarValue::I32)
            .ok_or_else(|| "state expression i32 negation overflow".to_string()),
        JitScalarValue::U8(value) => Ok(JitScalarValue::I32(-i32::from(value))),
        JitScalarValue::U16(value) => Ok(JitScalarValue::I32(-i32::from(value))),
        JitScalarValue::U32(_) => {
            Err("state expression cannot negate an unsigned u32 value".to_string())
        }
        JitScalarValue::F32(value) => Ok(JitScalarValue::F32(-value)),
        JitScalarValue::F64(value) => Ok(JitScalarValue::F64(-value)),
        JitScalarValue::Bool(_) => Err("state expression cannot negate a bool value".to_string()),
    }
}

fn apply_state_binary(
    left: JitScalarValue,
    operator: BinaryOperator,
    right: JitScalarValue,
) -> Result<JitScalarValue, String> {
    let integer_pair = state_integer_pair(left, right);
    if matches!(operator, BinaryOperator::Equal | BinaryOperator::NotEqual) {
        let equal = if let Some((left, right, unsigned)) = integer_pair {
            if unsigned {
                left as u32 == right as u32
            } else {
                left == right
            }
        } else {
            match (left, right) {
                (JitScalarValue::Bool(left), JitScalarValue::Bool(right)) => left == right,
                (left, right) => {
                    let (left, right) = state_numeric_pair(left, right).ok_or_else(|| {
                    format!(
                        "state expression operator '{}' requires two numeric operands or two bool operands",
                        operator.text()
                    )
                })?;
                    left == right
                }
            }
        };
        return Ok(JitScalarValue::Bool(if operator == BinaryOperator::Equal {
            equal
        } else {
            !equal
        }));
    }
    if matches!(
        operator,
        BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual
    ) {
        if let Some((left, right, unsigned)) = integer_pair {
            let result = if unsigned {
                let (left, right) = (left as u32, right as u32);
                match operator {
                    BinaryOperator::Less => left < right,
                    BinaryOperator::LessEqual => left <= right,
                    BinaryOperator::Greater => left > right,
                    BinaryOperator::GreaterEqual => left >= right,
                    _ => unreachable!("comparison operators are matched above"),
                }
            } else {
                match operator {
                    BinaryOperator::Less => left < right,
                    BinaryOperator::LessEqual => left <= right,
                    BinaryOperator::Greater => left > right,
                    BinaryOperator::GreaterEqual => left >= right,
                    _ => unreachable!("comparison operators are matched above"),
                }
            };
            return Ok(JitScalarValue::Bool(result));
        }
        let (left, right) = state_numeric_pair(left, right).ok_or_else(|| {
            format!(
                "state expression operator '{}' requires numeric operands",
                operator.text()
            )
        })?;
        let result = match operator {
            BinaryOperator::Less => left < right,
            BinaryOperator::LessEqual => left <= right,
            BinaryOperator::Greater => left > right,
            BinaryOperator::GreaterEqual => left >= right,
            _ => unreachable!("comparison operators are matched above"),
        };
        return Ok(JitScalarValue::Bool(result));
    }
    if matches!(left, JitScalarValue::Bool(_)) || matches!(right, JitScalarValue::Bool(_)) {
        return Err(format!(
            "state expression operator '{}' does not accept bool operands",
            operator.text()
        ));
    }
    if matches!(left, JitScalarValue::F64(_)) || matches!(right, JitScalarValue::F64(_)) {
        let (left, right) = state_numeric_pair(left, right).expect("numeric operands checked");
        return apply_state_f64(left, operator, right).map(JitScalarValue::F64);
    }
    if matches!(left, JitScalarValue::F32(_)) || matches!(right, JitScalarValue::F32(_)) {
        let left = state_value_as_f32(left).expect("numeric operands checked");
        let right = state_value_as_f32(right).expect("numeric operands checked");
        return apply_state_f32(left, operator, right).map(JitScalarValue::F32);
    }
    if let Some(bits) = state_unsigned_bits(left)
        .into_iter()
        .chain(state_unsigned_bits(right))
        .max()
    {
        return apply_state_unsigned(left, operator, right, bits);
    }
    let left = state_value_as_i32(left).expect("integer operands checked");
    let right = state_value_as_i32(right).expect("integer operands checked");
    apply_state_i32(left, operator, right).map(JitScalarValue::I32)
}

fn state_integer_pair(left: JitScalarValue, right: JitScalarValue) -> Option<(i32, i32, bool)> {
    let unsigned = state_unsigned_bits(left).is_some() || state_unsigned_bits(right).is_some();
    Some((
        state_value_as_i32(left)?,
        state_value_as_i32(right)?,
        unsigned,
    ))
}

fn state_unsigned_bits(value: JitScalarValue) -> Option<u8> {
    match value {
        JitScalarValue::U8(_) => Some(8),
        JitScalarValue::U16(_) => Some(16),
        JitScalarValue::U32(_) => Some(32),
        _ => None,
    }
}

fn apply_state_unsigned(
    left: JitScalarValue,
    operator: BinaryOperator,
    right: JitScalarValue,
    bits: u8,
) -> Result<JitScalarValue, String> {
    let left = state_value_as_i32(left)
        .map(|value| value as u32)
        .ok_or_else(|| "state expression requires integer operands".to_string())?;
    let right = state_value_as_i32(right)
        .map(|value| value as u32)
        .ok_or_else(|| "state expression requires integer operands".to_string())?;
    let value = match operator {
        BinaryOperator::Add => left.wrapping_add(right),
        BinaryOperator::Subtract => left.wrapping_sub(right),
        BinaryOperator::Multiply => left.wrapping_mul(right),
        BinaryOperator::Divide if right != 0 => left / right,
        BinaryOperator::Remainder if right != 0 => left % right,
        BinaryOperator::Divide | BinaryOperator::Remainder => {
            return Err("state expression division by zero".to_string())
        }
        _ => {
            return Err(format!(
                "unsupported unsigned operator '{}'",
                operator.text()
            ))
        }
    };
    Ok(match bits {
        8 => JitScalarValue::U8(value as u8),
        16 => JitScalarValue::U16(value as u16),
        _ => JitScalarValue::U32(value),
    })
}

fn state_numeric_pair(left: JitScalarValue, right: JitScalarValue) -> Option<(f64, f64)> {
    Some((state_value_as_f64(left)?, state_value_as_f64(right)?))
}

fn state_value_as_f64(value: JitScalarValue) -> Option<f64> {
    match value {
        JitScalarValue::I32(value) => Some(f64::from(value)),
        JitScalarValue::U8(value) => Some(f64::from(value)),
        JitScalarValue::U16(value) => Some(f64::from(value)),
        JitScalarValue::U32(value) => Some(f64::from(value)),
        JitScalarValue::F32(value) => Some(f64::from(value)),
        JitScalarValue::F64(value) => Some(value),
        JitScalarValue::Bool(_) => None,
    }
}

fn state_value_as_f32(value: JitScalarValue) -> Option<f32> {
    match value {
        JitScalarValue::I32(value) => Some(value as f32),
        JitScalarValue::U8(value) => Some(f32::from(value)),
        JitScalarValue::U16(value) => Some(f32::from(value)),
        JitScalarValue::U32(value) => Some(value as f32),
        JitScalarValue::F32(value) => Some(value),
        JitScalarValue::F64(_) | JitScalarValue::Bool(_) => None,
    }
}

fn state_value_as_i32(value: JitScalarValue) -> Option<i32> {
    match value {
        JitScalarValue::I32(value) => Some(value),
        JitScalarValue::U8(value) => Some(i32::from(value)),
        JitScalarValue::U16(value) => Some(i32::from(value)),
        JitScalarValue::U32(value) => Some(value as i32),
        JitScalarValue::F32(_) | JitScalarValue::F64(_) | JitScalarValue::Bool(_) => None,
    }
}

fn apply_state_i32(left: i32, operator: BinaryOperator, right: i32) -> Result<i32, String> {
    match operator {
        BinaryOperator::Add => left.checked_add(right),
        BinaryOperator::Subtract => left.checked_sub(right),
        BinaryOperator::Multiply => left.checked_mul(right),
        BinaryOperator::Divide if right != 0 => left.checked_div(right),
        BinaryOperator::Remainder if right != 0 => left.checked_rem(right),
        BinaryOperator::Divide | BinaryOperator::Remainder => {
            return Err("state expression division by zero".to_string())
        }
        _ => return Err(format!("unsupported i32 operator '{}'", operator.text())),
    }
    .ok_or_else(|| format!("state expression i32 '{}' overflow", operator.text()))
}

fn apply_state_f32(left: f32, operator: BinaryOperator, right: f32) -> Result<f32, String> {
    if matches!(operator, BinaryOperator::Divide | BinaryOperator::Remainder) && right == 0.0 {
        return Err("state expression division by zero".to_string());
    }
    match operator {
        BinaryOperator::Add => Ok(left + right),
        BinaryOperator::Subtract => Ok(left - right),
        BinaryOperator::Multiply => Ok(left * right),
        BinaryOperator::Divide => Ok(left / right),
        BinaryOperator::Remainder => Ok(left % right),
        _ => Err(format!("unsupported f32 operator '{}'", operator.text())),
    }
}

fn apply_state_f64(left: f64, operator: BinaryOperator, right: f64) -> Result<f64, String> {
    if matches!(operator, BinaryOperator::Divide | BinaryOperator::Remainder) && right == 0.0 {
        return Err("state expression division by zero".to_string());
    }
    match operator {
        BinaryOperator::Add => Ok(left + right),
        BinaryOperator::Subtract => Ok(left - right),
        BinaryOperator::Multiply => Ok(left * right),
        BinaryOperator::Divide => Ok(left / right),
        BinaryOperator::Remainder => Ok(left % right),
        _ => Err(format!("unsupported f64 operator '{}'", operator.text())),
    }
}

fn is_u8_type(type_table: &TypeTable, type_id: u16) -> bool {
    let _ = type_table;
    type_id == TYPE_ID_U8
}

fn scalar_storage_kind(
    _type_table: &TypeTable,
    type_id: u16,
) -> Option<stasis_dynload::JitStorageKind> {
    match type_id {
        TYPE_ID_I32 | TYPE_ID_BOOL => Some(stasis_dynload::JitStorageKind::I32),
        TYPE_ID_F32 => Some(stasis_dynload::JitStorageKind::F32),
        TYPE_ID_F64 => Some(stasis_dynload::JitStorageKind::F64),
        TYPE_ID_U8 => Some(stasis_dynload::JitStorageKind::U8),
        TYPE_ID_U16 => Some(stasis_dynload::JitStorageKind::U16),
        TYPE_ID_U32 => Some(stasis_dynload::JitStorageKind::I32),
        _ => None,
    }
}

fn array_storage_kind(
    type_table: &TypeTable,
    type_id: u16,
) -> Option<stasis_dynload::JitStorageKind> {
    if is_u8_type(type_table, type_id) {
        return Some(stasis_dynload::JitStorageKind::U8);
    }
    if type_id == TYPE_ID_U16 {
        return Some(stasis_dynload::JitStorageKind::U16);
    }
    if crate::backend::emit::is_i32_abi_compatible_type(type_id, type_table) {
        return Some(stasis_dynload::JitStorageKind::I32);
    }
    scalar_storage_kind(type_table, type_id)
}

fn build_direct_storage_bindings(
    global_path_types: &crate::backend::emit::GlobalPathTypeMap,
    collection_infos: &crate::backend::emit::CollectionInfoMap,
    type_table: &TypeTable,
    provision: bool,
) -> Result<DirectStorageBindings, String> {
    let mut bindings = DirectStorageBindings::default();
    for (path, type_id) in global_path_types {
        if collection_infos.contains_key(path) {
            continue;
        }
        let kind = scalar_storage_kind(type_table, *type_id).or_else(|| {
            is_named_scalar_state_path(path, *type_id, global_path_types, type_table)
                .then_some(stasis_dynload::JitStorageKind::I32)
        });
        if let Some(kind) = kind {
            let path_hash = crate::backend::emit::hash_global_path(path);
            let address = stasis_dynload::direct_scalar_storage_slot_address(kind, path_hash)?;
            if provision {
                stasis_dynload::provision_direct_scalar_storage(kind, path_hash)?;
            }
            bindings
                .scalars
                .insert(path.clone(), DirectStorageBinding::Absolute(address));
        }
    }
    for (path, info) in collection_infos {
        let path_hash = crate::backend::emit::hash_global_path(path);
        if let Some(type_id) = info.element_type {
            let text_storage = global_path_types
                .get(path)
                .and_then(|global_type| type_table.type_info(*global_type))
                .is_some_and(|type_info| {
                    matches!(
                        type_info.category,
                        TypeCategory::AsciiFixed | TypeCategory::Utf8Fixed
                    )
                });
            let kind = if text_storage {
                Some(stasis_dynload::JitStorageKind::U8)
            } else {
                array_storage_kind(type_table, type_id)
            }
            .ok_or_else(|| {
                format!("unsupported direct storage element type {type_id} for '{path}'")
            })?;
            let address = stasis_dynload::direct_array_storage_slot_address(kind, path_hash, 0)?;
            if provision {
                stasis_dynload::provision_direct_array_storage(
                    kind,
                    path_hash,
                    0,
                    info.len as usize,
                )?;
            }
            bindings.arrays.insert(
                (path.clone(), String::new()),
                crate::backend::emit::DirectArrayStorageBinding {
                    slot: DirectStorageBinding::Absolute(address),
                    storage_bytes: storage_kind_bytes(kind),
                    static_len: Some(info.len as usize),
                },
            );
        }
        for (field, type_id) in &info.field_types {
            let kind = array_storage_kind(type_table, *type_id).ok_or_else(|| {
                format!("unsupported direct storage field type {type_id} for '{path}.{field}'")
            })?;
            let field_hash = crate::backend::emit::hash_foreach_field_suffix(field);
            let address =
                stasis_dynload::direct_array_storage_slot_address(kind, path_hash, field_hash)?;
            if provision {
                stasis_dynload::provision_direct_array_storage(
                    kind,
                    path_hash,
                    field_hash,
                    info.len as usize,
                )?;
            }
            bindings.arrays.insert(
                (path.clone(), field.clone()),
                crate::backend::emit::DirectArrayStorageBinding {
                    slot: DirectStorageBinding::Absolute(address),
                    storage_bytes: storage_kind_bytes(kind),
                    static_len: Some(info.len as usize),
                },
            );
        }
    }
    Ok(bindings)
}

fn storage_kind_bytes(kind: stasis_dynload::JitStorageKind) -> u8 {
    match kind {
        stasis_dynload::JitStorageKind::U8 => 1,
        stasis_dynload::JitStorageKind::U16 => 2,
        stasis_dynload::JitStorageKind::I32 | stasis_dynload::JitStorageKind::F32 => 4,
        stasis_dynload::JitStorageKind::F64 => 8,
    }
}

impl Default for JitProcess {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
fn acquire_jit_process_test_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

use super::emit::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceDiskProbe {
    len: u64,
    modified: Option<SystemTime>,
}

fn probe_disk_source(path: &Path) -> Option<SourceDiskProbe> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok();
    Some(SourceDiskProbe {
        len: metadata.len(),
        modified,
    })
}

fn function_address(function: *const ()) -> usize {
    function as usize
}

fn builtin_host_symbol_address(symbol: &str) -> Option<usize> {
    let address = match symbol {
        "print_i32" | "stasis_jit_print_i32" => {
            function_address(stasis_dynload::stasis_jit_print_i32 as *const ())
        }
        "print_string" | "stasis_jit_print_string" => {
            function_address(stasis_dynload::stasis_jit_print_string as *const ())
        }
        "gfx_load_sprite" | "stasis_gfx_load_sprite" => {
            function_address(stasis_dynload::stasis_jit_gfx_load_sprite as *const ())
        }
        "stasis_jit_asset_request_sprite" | "asset_request_sprite" => {
            function_address(stasis_dynload::stasis_jit_asset_request_sprite as *const ())
        }
        "stasis_jit_asset_request_audio" | "asset_request_audio" => {
            function_address(stasis_dynload::stasis_jit_asset_request_audio as *const ())
        }
        "stasis_jit_asset_task_poll" | "asset_task_poll" => {
            function_address(stasis_dynload::stasis_jit_asset_task_poll as *const ())
        }
        "stasis_jit_asset_task_take_handle" | "asset_task_take_handle" => {
            function_address(stasis_dynload::stasis_jit_asset_task_take_handle as *const ())
        }
        "stasis_jit_asset_task_cancel" | "asset_task_cancel" => {
            function_address(stasis_dynload::stasis_jit_asset_task_cancel as *const ())
        }
        "gfx_release_sprite" | "stasis_gfx_release_sprite" | "stasis_jit_gfx_release_sprite" => {
            function_address(stasis_dynload::stasis_jit_gfx_release_sprite as *const ())
        }
        "gfx_dump_bmp" | "stasis_gfx_dump_bmp" => {
            function_address(stasis_dynload::stasis_jit_gfx_dump_bmp as *const ())
        }
        "gfx_dump_png" | "stasis_gfx_dump_png" => {
            function_address(stasis_dynload::stasis_jit_gfx_dump_png as *const ())
        }
        "gfx_poll_reload" | "stasis_gfx_poll_reload" | "stasis_jit_gfx_poll_reload" => {
            function_address(stasis_dynload::stasis_jit_gfx_poll_reload as *const ())
        }
        "load_font" | "stasis_load_font" => {
            function_address(stasis_dynload::stasis_jit_load_font as *const ())
        }
        "measure_text" | "stasis_measure_text" => {
            function_address(stasis_dynload::stasis_jit_measure_text as *const ())
        }
        "gfx_cache_text" | "stasis_gfx_cache_text" | "stasis_jit_gfx_cache_text" => {
            function_address(stasis_dynload::stasis_jit_gfx_cache_text as *const ())
        }
        "gfx_measure_text_cached"
        | "stasis_gfx_measure_text_cached"
        | "stasis_jit_gfx_measure_text_cached" => {
            function_address(stasis_dynload::stasis_jit_gfx_measure_text_cached as *const ())
        }
        "gfx_measure_text_cached_height"
        | "stasis_gfx_measure_text_cached_height"
        | "stasis_jit_gfx_measure_text_cached_height" => {
            function_address(stasis_dynload::stasis_jit_gfx_measure_text_cached_height as *const ())
        }
        "stasis_jit_sprite_load_from" => {
            function_address(stasis_dynload::stasis_jit_sprite_load_from as *const ())
        }
        "stasis_jit_text_run_load_from" => {
            function_address(stasis_dynload::stasis_jit_text_run_load_from as *const ())
        }
        "time" | "stasis_time" | "stasis_jit_time" | "stasis_get_time_ms" => {
            function_address(stasis_dynload::stasis_get_time_ms as *const ())
        }
        "time_us" | "stasis_time_us" | "stasis_jit_time_us" | "stasis_get_time_us" => {
            function_address(stasis_dynload::stasis_get_time_us as *const ())
        }
        "sleep_ms" | "stasis_sleep_ms" | "stasis_jit_sleep_ms" => {
            function_address(stasis_dynload::stasis_jit_sleep_ms as *const ())
        }
        "clipboard_load_ascii"
        | "stasis_clipboard_load_ascii"
        | "stasis_jit_clipboard_load_ascii" => {
            function_address(stasis_dynload::stasis_jit_clipboard_load_ascii as *const ())
        }
        "clipboard_save_ascii"
        | "stasis_clipboard_save_ascii"
        | "stasis_jit_clipboard_save_ascii" => {
            function_address(stasis_dynload::stasis_jit_clipboard_save_ascii as *const ())
        }
        "storage_load_ascii" | "stasis_storage_load_ascii" | "stasis_jit_storage_load_ascii" => {
            function_address(stasis_dynload::stasis_jit_storage_load_ascii as *const ())
        }
        "storage_load_i32" | "stasis_storage_load_i32" | "stasis_jit_storage_load_i32" => {
            function_address(stasis_dynload::stasis_jit_storage_load_i32 as *const ())
        }
        "storage_save_ascii" | "stasis_storage_save_ascii" | "stasis_jit_storage_save_ascii" => {
            function_address(stasis_dynload::stasis_jit_storage_save_ascii as *const ())
        }
        "storage_save_i32" | "stasis_storage_save_i32" | "stasis_jit_storage_save_i32" => {
            function_address(stasis_dynload::stasis_jit_storage_save_i32 as *const ())
        }
        "audio_init" | "stasis_audio_init" => {
            function_address(stasis_dynload::stasis_jit_audio_init as *const ())
        }
        "audio_shutdown" | "stasis_audio_shutdown" => {
            function_address(stasis_dynload::stasis_jit_audio_shutdown as *const ())
        }
        "audio_is_available" | "stasis_audio_is_available" => {
            function_address(stasis_dynload::stasis_jit_audio_is_available as *const ())
        }
        "audio_get_sample_rate" | "stasis_audio_get_sample_rate" => {
            function_address(stasis_dynload::stasis_jit_audio_get_sample_rate as *const ())
        }
        "audio_get_channels" | "stasis_audio_get_channels" => {
            function_address(stasis_dynload::stasis_jit_audio_get_channels as *const ())
        }
        "audio_get_queued_frames" | "stasis_audio_get_queued_frames" => {
            function_address(stasis_dynload::stasis_jit_audio_get_queued_frames as *const ())
        }
        "audio_get_underruns" | "stasis_audio_get_underruns" => {
            function_address(stasis_dynload::stasis_jit_audio_get_underruns as *const ())
        }
        "audio_push_f32_interleaved" | "stasis_audio_push_f32_interleaved" => {
            function_address(stasis_dynload::stasis_jit_audio_push_f32_interleaved as *const ())
        }
        "audio_load_wav" | "stasis_audio_load_wav" => {
            function_address(stasis_dynload::stasis_jit_audio_load_wav as *const ())
        }
        "audio_release" | "stasis_audio_release" => {
            function_address(stasis_dynload::stasis_jit_audio_release as *const ())
        }
        "audio_play" | "stasis_audio_play" => {
            function_address(stasis_dynload::stasis_jit_audio_play as *const ())
        }
        "audio_stop" | "stasis_audio_stop" => {
            function_address(stasis_dynload::stasis_jit_audio_stop as *const ())
        }
        "audio_voice_is_playing" | "stasis_audio_voice_is_playing" => {
            function_address(stasis_dynload::stasis_jit_audio_voice_is_playing as *const ())
        }
        "audio_voice_set_paused" | "stasis_audio_voice_set_paused" => {
            function_address(stasis_dynload::stasis_jit_audio_voice_set_paused as *const ())
        }
        "audio_voice_set_volume_pan" | "stasis_audio_voice_set_volume_pan" => {
            function_address(stasis_dynload::stasis_jit_audio_voice_set_volume_pan as *const ())
        }
        "stasis_jit_audio_load_music" | "audio_load_music" | "stasis_audio_load_music" => {
            function_address(stasis_dynload::stasis_jit_audio_load_music as *const ())
        }
        "stasis_jit_audio_load_effect" | "audio_load_effect" | "stasis_audio_load_effect" => {
            function_address(stasis_dynload::stasis_jit_audio_load_effect as *const ())
        }
        "stasis_jit_audio_play_music" | "audio_play_music" | "stasis_audio_play_music" => {
            function_address(stasis_dynload::stasis_jit_audio_play_music as *const ())
        }
        "stasis_jit_audio_stop_music" | "audio_stop_music" | "stasis_audio_stop_music" => {
            function_address(stasis_dynload::stasis_jit_audio_stop_music as *const ())
        }
        "stasis_jit_audio_pause_music" | "audio_pause_music" | "stasis_audio_pause_music" => {
            function_address(stasis_dynload::stasis_jit_audio_pause_music as *const ())
        }
        "stasis_jit_audio_set_music_volume"
        | "audio_set_music_volume"
        | "stasis_audio_set_music_volume" => {
            function_address(stasis_dynload::stasis_jit_audio_set_music_volume as *const ())
        }
        "stasis_jit_audio_play_effect" | "audio_play_effect" | "stasis_audio_play_effect" => {
            function_address(stasis_dynload::stasis_jit_audio_play_effect as *const ())
        }
        "sin_fast" | "stasis_jit_sin_fast" => {
            function_address(stasis_dynload::stasis_jit_sin_fast as *const ())
        }
        "cos_fast" | "stasis_jit_cos_fast" => {
            function_address(stasis_dynload::stasis_jit_cos_fast as *const ())
        }
        "stasis_jit_global_i32_load" => {
            function_address(stasis_dynload::stasis_jit_global_i32_load as *const ())
        }
        "stasis_jit_global_i32_store" => {
            function_address(stasis_dynload::stasis_jit_global_i32_store as *const ())
        }
        "stasis_jit_global_f32_load" => {
            function_address(stasis_dynload::stasis_jit_global_f32_load as *const ())
        }
        "stasis_jit_global_f32_store" => {
            function_address(stasis_dynload::stasis_jit_global_f32_store as *const ())
        }
        "stasis_jit_global_f64_load" => {
            function_address(stasis_dynload::stasis_jit_global_f64_load as *const ())
        }
        "stasis_jit_global_f64_store" => {
            function_address(stasis_dynload::stasis_jit_global_f64_store as *const ())
        }
        "stasis_jit_collection_i32_load" => {
            function_address(stasis_dynload::stasis_jit_collection_i32_load as *const ())
        }
        "stasis_jit_collection_i32_store" => {
            function_address(stasis_dynload::stasis_jit_collection_i32_store as *const ())
        }
        "sys_memcpy_u8" | "stasis_jit_sys_memcpy_u8" => {
            function_address(stasis_dynload::stasis_jit_sys_memcpy_u8 as *const ())
        }
        "sys_memcpy_i32" | "stasis_jit_sys_memcpy_i32" => {
            function_address(stasis_dynload::stasis_jit_sys_memcpy_i32 as *const ())
        }
        "sys_memcpy_f32" | "stasis_jit_sys_memcpy_f32" => {
            function_address(stasis_dynload::stasis_jit_sys_memcpy_f32 as *const ())
        }
        "sys_memmove_u8" | "stasis_jit_sys_memmove_u8" => {
            function_address(stasis_dynload::stasis_jit_sys_memmove_u8 as *const ())
        }
        "sys_memmove_i32" | "stasis_jit_sys_memmove_i32" => {
            function_address(stasis_dynload::stasis_jit_sys_memmove_i32 as *const ())
        }
        "sys_memmove_f32" | "stasis_jit_sys_memmove_f32" => {
            function_address(stasis_dynload::stasis_jit_sys_memmove_f32 as *const ())
        }
        "reject_code_swap" | "stasis_jit_reject_code_swap" => {
            function_address(stasis_dynload::stasis_jit_reject_code_swap as *const ())
        }
        "stasis_jit_render_v2_trace" => {
            function_address(stasis_dynload::stasis_jit_render_v2_trace as *const ())
        }
        _ => return None,
    };
    Some(address)
}

fn seed_fixed_collection_max_length_headers(
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> Result<(), String> {
    let mut headers = Vec::new();
    for (path, type_id) in global_path_types {
        let Some(type_info) = type_table.type_info(*type_id) else {
            continue;
        };
        match type_info.category {
            TypeCategory::AsciiFixed => {
                let Some(payload_bytes) = type_info.layout.payload_size_bytes else {
                    continue;
                };
                let max_length = i32::try_from(payload_bytes).map_err(|_| {
                    format!(
                        "ascii max_length overflow for '{}' (payload bytes {})",
                        path, payload_bytes
                    )
                })?;
                headers.push((path, max_length));
            }
            TypeCategory::Utf8Fixed => {
                let Some(payload_bytes) = type_info.layout.payload_size_bytes else {
                    continue;
                };
                let max_length = i32::try_from(payload_bytes).map_err(|_| {
                    format!(
                        "utf8 max_length overflow for '{}' (payload bytes {})",
                        path, payload_bytes
                    )
                })?;
                headers.push((path, max_length));
            }
            TypeCategory::ArrayFixed => {
                let Some(max_length) = type_table.fixed_collection_len(*type_id) else {
                    continue;
                };
                headers.push((path, max_length));
            }
            _ => {}
        }
    }
    for (path, max_length) in headers {
        seed_collection_max_length(path, max_length);
    }
    Ok(())
}

fn seed_collection_max_length(path: &str, max_length: i32) {
    let max_length_path = format!("{path}.max_length");
    stasis_dynload::stasis_jit_global_i32_store(hash_global_path(&max_length_path), max_length);
}

fn new_stasis_jit_builder() -> Result<JITBuilder, String> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("is_pic", "false")
        .map_err(|error| format!("failed to configure JIT relocation model: {error}"))?;
    let isa_builder = cranelift_native::builder()
        .map_err(|message| format!("host machine is not supported by Cranelift: {message}"))?;
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|error| format!("failed to construct native JIT ISA: {error}"))?;
    Ok(JITBuilder::with_isa(isa, default_libcall_names()))
}
fn runtime_helper_addresses() -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    macro_rules! insert_helpers {
        ($($name:ident),+ $(,)?) => {
            $(
                out.insert(
                    stringify!($name).to_string(),
                    function_address(stasis_dynload::$name as *const ()),
                );
            )+
        };
    }
    insert_helpers!(
        stasis_jit_print_i32,
        stasis_jit_print_string,
        stasis_jit_sin_fast,
        stasis_jit_cos_fast,
        stasis_jit_global_i32_load,
        stasis_jit_global_i32_store,
        stasis_jit_global_f32_load,
        stasis_jit_global_f32_store,
        stasis_jit_global_f64_load,
        stasis_jit_global_f64_store,
        stasis_jit_collection_i32_load,
        stasis_jit_collection_i32_store,
        stasis_jit_global_i32_array_load,
        stasis_jit_global_i32_array_store,
        stasis_jit_global_i32_array_ptr,
        stasis_jit_global_f32_array_load,
        stasis_jit_global_f32_array_store,
        stasis_jit_global_f32_array_ptr,
        stasis_jit_global_f64_array_load,
        stasis_jit_global_f64_array_store,
        stasis_jit_global_f64_array_ptr,
        stasis_jit_reject_code_swap,
        stasis_jit_debug_frame_enter,
        stasis_jit_debug_frame_leave,
        stasis_jit_debug_statement,
        stasis_jit_debug_values_begin,
        stasis_jit_debug_value_i64,
        stasis_jit_debug_value_f64,
        stasis_jit_profile_frame_enter,
        stasis_jit_profile_frame_leave,
    );
    out
}

fn new_generation_jit_module(
    extern_symbol_addresses: &ExternSymbolAddressMap,
    retained_function_addresses: &BTreeMap<String, usize>,
) -> Result<JITModule, String> {
    let mut builder = new_stasis_jit_builder()?;
    for (symbol, address) in runtime_helper_addresses() {
        builder.symbol(&symbol, address as *const u8);
    }
    for (symbol, address) in extern_symbol_addresses {
        if *address != 0 {
            builder.symbol(symbol, *address as *const u8);
        }
    }
    for (symbol, address) in retained_function_addresses {
        if *address != 0 {
            builder.symbol(symbol, *address as *const u8);
        }
    }
    Ok(JITModule::new(builder))
}

#[allow(clippy::too_many_arguments)]
fn compile_function_into_jit_module(
    module: JITModule,
    meta: &FunctionMeta,
    hir: &FunctionHIR,
    symbol: &str,
    call_signatures: &CallSignatureMap,
    type_table: &mut TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    data_flow_summary: Option<&crate::data_flow::FunctionDataFlowSummary>,
    extern_symbol_addresses: &ExternSymbolAddressMap,
    direct_storage: &DirectStorageBindings,
    local_runtime_helper_trampolines: bool,
    debug_instrumentation: bool,
    profile_instrumentation: bool,
    defined_runtime_helper_trampolines: &mut BTreeSet<String>,
) -> Result<(JITModule, cranelift_module::FuncId, String, usize), String> {
    let runtime_helper_addresses = local_runtime_helper_trampolines.then(|| {
        let mut addresses = runtime_helper_addresses();
        addresses.extend(
            extern_symbol_addresses
                .iter()
                .map(|(symbol, address)| (symbol.clone(), *address)),
        );
        addresses
    });
    let runtime_helper_linkage = runtime_helper_addresses
        .as_ref()
        .map_or(RuntimeHelperLinkage::Imported, |addresses| {
            RuntimeHelperLinkage::LocalTrampolines(addresses)
        });
    let clif = Rc::new(RefCell::new(String::new()));
    let clif_capture = Rc::clone(&clif);
    let (module, function_id, executable_bytes) = compile_function_with_module(
        module,
        meta,
        hir,
        symbol,
        runtime_helper_linkage,
        SharedCompileBackendMode::JitDirect,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        data_flow_summary,
        Some(direct_storage),
        Some(defined_runtime_helper_trampolines),
        debug_instrumentation,
        profile_instrumentation,
        |_| Ok(()),
        move |_, function| {
            *clif_capture.borrow_mut() = function.display().to_string();
        },
        |mut module, function_id, mut context| {
            module
                .define_function(function_id, &mut context)
                .map_err(|error| format!("failed to define JIT function {symbol}: {error:?}"))?;
            let executable_bytes = context
                .compiled_code()
                .map(|code| code.code_buffer().len())
                .unwrap_or(0);
            module.clear_context(&mut context);
            Ok((module, function_id, executable_bytes))
        },
    )?;
    let clif = clif.borrow().clone();
    Ok((module, function_id, clif, executable_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_asset_task_externs_resolve_to_builtin_bridges() {
        assert!(builtin_host_symbol_address("stasis_jit_asset_request_sprite").is_some());
        assert!(builtin_host_symbol_address("stasis_jit_asset_request_audio").is_some());
        assert!(builtin_host_symbol_address("stasis_jit_asset_task_poll").is_some());
        assert!(builtin_host_symbol_address("stasis_jit_asset_task_take_handle").is_some());
        assert!(builtin_host_symbol_address("stasis_jit_asset_task_cancel").is_some());
    }
    use crate::backend::EngineEntrypoints;

    #[test]
    fn before_tick_hook_requires_unique_i32_to_i32_signature() {
        let mut process = JitProcess::new();
        process.set_required_emit_roots(&["before".to_string()]);
        process.upsert_file(
            "hook.stasis",
            "function before(frame: i32): i32 { return frame + 1; }\nfunction tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\nfunction main(): i32 { return 0; }\n",
        );
        process.compile().expect("compile valid hook");
        process
            .validate_i32_onearg_by_name("before")
            .expect("valid hook signature");
        drop(process);

        let mut missing = JitProcess::new();
        missing.upsert_file(
            "missing.stasis",
            "function tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\nfunction main(): i32 { return 0; }\n",
        );
        missing.compile().expect("compile missing-hook program");
        assert!(missing
            .validate_i32_onearg_by_name("before")
            .expect_err("missing hook")
            .contains("function 'before' not found"));
        drop(missing);

        for source in [
            "function before(frame: f32): i32 { return 1; }\nfunction tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\nfunction main(): i32 { return 0; }\n",
            "function before(frame: i32): void { }\nfunction tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\nfunction main(): i32 { return 0; }\n",
        ] {
            let mut invalid = JitProcess::new();
            invalid.set_required_emit_roots(&["before".to_string()]);
            invalid.upsert_file("invalid_hook.stasis", source);
            invalid.compile().expect("compile invalid-hook program");
            assert!(invalid
                .validate_i32_onearg_by_name("before")
                .is_err());
        }

        let mut ambiguous = JitProcess::new();
        ambiguous.upsert_file(
            "main.stasis",
            "import \"helper.stasis\";\nfunction before(frame: i32): i32 { return frame; }\nfunction tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\nfunction main(): i32 { return 0; }\n",
        );
        ambiguous.upsert_file(
            "helper.stasis",
            "function before(frame: i32): i32 { return frame + 1; }\n",
        );
        ambiguous.compile().expect("compile ambiguous-hook program");
        assert!(ambiguous
            .validate_i32_onearg_by_name("before")
            .expect_err("ambiguous hook")
            .contains("ambiguous"));
    }

    #[cfg(windows)]
    #[test]
    fn before_tick_hook_executes_i32_frame_argument() {
        let mut process = JitProcess::new();
        process.set_required_emit_roots(&["before".to_string()]);
        process.upsert_file(
            "hook.stasis",
            "function before(frame: i32): i32 { return frame + 1; }\n",
        );
        process.compile().expect("compile valid hook");
        assert_eq!(process.execute_i32_onearg_by_name("before", 41), Ok(42));
    }

    #[test]
    fn collection_layout_edit_rejits_every_reachable_function() {
        fn source(capacity: usize) -> String {
            format!(
                "global values: i32[{capacity}];\nfunction read_value(): i32 {{ return values.max_length; }}\nfunction untouched(): i32 {{ return 7; }}\nfunction tick(): i32 {{ return read_value(); }}\nfunction render(): i32 {{ return untouched(); }}\nfunction main(): i32 {{ return tick() + render(); }}\n"
            )
        }

        let mut process = JitProcess::new();
        process.upsert_file("layout.stasis", source(2));
        process.compile().expect("compile initial layout");
        let old_pointers: BTreeMap<_, _> = process
            .artifacts()
            .iter()
            .map(|artifact| (artifact.function_key.name.clone(), artifact.code_ptr))
            .collect();
        process.upsert_file("layout.stasis", source(3));
        let report = process.compile().expect("compile changed layout");

        let metadata = process.generation_metadata().expect("generation metadata");
        let emitted_names: BTreeSet<_> = metadata
            .emitted_function_ids
            .iter()
            .map(|id| {
                process
                    .compiler
                    .functions()
                    .iter()
                    .find(|function| function.id == *id)
                    .expect("emitted identity")
                    .name
                    .as_str()
            })
            .collect();
        assert_eq!(
            emitted_names,
            BTreeSet::from(["main", "read_value", "render", "tick", "untouched"])
        );
        assert!(metadata
            .patch_reasons
            .iter()
            .all(|reason| reason.reason == PatchReason::CompilerLayoutChanged));
        let reused_names: BTreeSet<_> = metadata
            .reused_function_ids
            .iter()
            .map(|id| {
                process
                    .compiler
                    .functions()
                    .iter()
                    .find(|function| function.id == *id)
                    .expect("reused identity")
                    .name
                    .as_str()
            })
            .collect();
        assert!(reused_names.is_empty());
        for artifact in process.artifacts() {
            if reused_names.contains(artifact.function_key.name.as_str()) {
                assert_eq!(
                    old_pointers.get(&artifact.function_key.name),
                    Some(&artifact.code_ptr),
                    "{} pointer",
                    artifact.function_key.name
                );
            }
        }
        assert_eq!(report.emit.emitted_functions, 5);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute changed layout"),
            10
        );
    }

    #[test]
    fn non_state_struct_layout_edit_rejits_every_reachable_function() {
        fn source(extra_field: bool) -> String {
            let fields = if extra_field {
                "value: i32; extra: f32;"
            } else {
                "value: i32;"
            };
            format!(
                "struct LocalOnly {{ {fields} }}\nfunction tick(): i32 {{ return 1; }}\nfunction render(): i32 {{ return 2; }}\nfunction main(): i32 {{ return tick() + render(); }}\n"
            )
        }

        let mut process = JitProcess::new();
        process.upsert_file("local_layout.stasis", source(false));
        process.compile().expect("compile initial local layout");
        process.upsert_file("local_layout.stasis", source(true));
        let report = process.compile().expect("compile changed local layout");

        let metadata = process.generation_metadata().expect("generation metadata");
        let emitted_names: BTreeSet<_> = metadata
            .emitted_function_ids
            .iter()
            .map(|id| {
                process
                    .compiler
                    .functions()
                    .iter()
                    .find(|function| function.id == *id)
                    .expect("emitted identity")
                    .name
                    .as_str()
            })
            .collect();
        assert_eq!(emitted_names, BTreeSet::from(["main", "render", "tick"]));
        assert!(metadata
            .patch_reasons
            .iter()
            .all(|reason| reason.reason == PatchReason::CompilerLayoutChanged));
        assert!(metadata.reused_function_ids.is_empty());
        assert_eq!(report.emit.emitted_functions, 3);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute changed local layout"),
            3
        );
    }

    #[test]
    fn lowering_contract_cache_respects_scopes_and_failed_retries() {
        fn source(value: i32, valid: bool) -> String {
            let main = if valid {
                "function main(): i32 { return read_value(); }"
            } else {
                "function main(): i32 { return missing(); }"
            };
            format!(
                "const value: i32 = {value};\nfunction read_value(): i32 {{ let before: i32 = value; if (true) {{ let value: i32 = 9; }} return before; }}\n{main}\n"
            )
        }

        let mut process = JitProcess::new();
        process.upsert_file("scope.stasis", source(1, true));
        process.compile().expect("compile accepted contract");
        process.upsert_file("scope.stasis", source(2, false));
        assert!(process.compile().is_err(), "reject invalid candidate");
        process.upsert_file("scope.stasis", source(2, true));
        process.compile().expect("retry changed contract");

        let emitted_names: BTreeSet<_> = process
            .generation_metadata()
            .expect("generation metadata")
            .emitted_function_ids
            .iter()
            .map(|id| {
                process
                    .compiler
                    .functions()
                    .iter()
                    .find(|function| function.id == *id)
                    .expect("emitted identity")
                    .name
                    .as_str()
            })
            .collect();
        assert_eq!(emitted_names, BTreeSet::from(["main", "read_value"]));
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute retried contract"),
            2
        );
    }

    #[test]
    fn global_struct_layout_change_rejits_local_and_global_consumers() {
        fn source(extra_field: bool) -> String {
            let global_fields = if extra_field {
                "flag: i32; extra: i32;"
            } else {
                "flag: i32;"
            };
            format!(
                "struct Local {{ value: i32; }}\nstruct Global {{ {global_fields} }}\nglobal item: Global;\nglobal local_item: Local;\nfunction local_user(item: Local): i32 {{ item.value = 3; return item.value; }}\nfunction tick(): i32 {{ return local_user(local_item); }}\nfunction render(): i32 {{ return item.flag; }}\nfunction main(): i32 {{ return tick() + render(); }}\n"
            )
        }

        let mut process = JitProcess::new();
        process.upsert_file("local_global.stasis", source(false));
        process.compile().expect("compile initial global layout");
        process.upsert_file("local_global.stasis", source(true));
        process.compile().expect("compile changed global layout");

        let metadata = process.generation_metadata().expect("generation metadata");
        let emitted_names: BTreeSet<_> = metadata
            .emitted_function_ids
            .iter()
            .map(|id| {
                process
                    .compiler
                    .functions()
                    .iter()
                    .find(|function| function.id == *id)
                    .expect("emitted identity")
                    .name
                    .as_str()
            })
            .collect();
        assert_eq!(
            emitted_names,
            BTreeSet::from(["local_user", "main", "render", "tick"])
        );
        assert!(metadata.reused_function_ids.is_empty());
    }

    #[test]
    fn selective_patch_uses_direct_calls_and_retains_unaffected_pointers() {
        let mut process = JitProcess::new();
        let fixture_root = Path::new("direct_call_generation_fixture");
        process.upsert_file(
            fixture_root.join("main.stasis").to_string_lossy(),
            include_str!("../../../../samples/direct_call_generation/main.stasis"),
        );
        process.upsert_file(
            fixture_root.join("math.stasis").to_string_lossy(),
            include_str!("../../../../samples/direct_call_generation/math.stasis"),
        );

        let report = process.compile().expect("complete generation compile");
        assert_eq!(report.emit.emitted_functions, 9);
        assert_eq!(process.artifacts().len(), 9);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute direct-call sample"),
            17
        );

        let metadata = process
            .generation_metadata()
            .expect("complete generation metadata");
        assert_eq!(metadata.module_count, 1);
        assert_eq!(metadata.emitted_function_ids.len(), 9);
        assert!(metadata.executable_bytes > 0);
        for name in ["main", "tick", "render", "on_code_swap"] {
            assert!(metadata.host_export_signatures.contains_key(name));
        }

        let host_exports = process.symbol_code_ptrs();
        for name in ["main", "tick", "render", "on_code_swap"] {
            assert!(host_exports.contains_key(name));
        }
        for internal in ["leaf", "shared", "mid", "even", "odd"] {
            assert!(!host_exports.contains_key(internal));
            let clif = process
                .clif_for_function_name(internal)
                .unwrap_or_else(|| panic!("missing CLIF for {internal}"));
            assert!(
                !clif.contains("stasis_jit_call_") && !clif.contains("stasis_jit_lookup_code_ptr"),
                "internal dispatch leaked into {internal}:\n{clif}"
            );
        }
        for caller in ["main", "tick", "mid", "shared", "even", "odd"] {
            let clif = process
                .clif_for_function_name(caller)
                .unwrap_or_else(|| panic!("missing CLIF for {caller}"));
            assert!(
                clif.contains("call fn"),
                "expected direct call in {caller}:\n{clif}"
            );
        }
        assert!(process.clif_for_function_name("unreachable").is_none());

        let cold_ptrs: BTreeMap<String, u64> = process
            .artifacts()
            .iter()
            .map(|artifact| (artifact.function_key.name.clone(), artifact.code_ptr))
            .collect();
        let edited_math = include_str!("../../../../samples/direct_call_generation/math.stasis")
            .replace(
                "return shared(value) + leaf(value);",
                "return shared(value) + leaf(value) + 1;",
            );
        process.upsert_file(
            fixture_root.join("math.stasis").to_string_lossy(),
            edited_math,
        );
        let patch = process.compile().expect("selective direct-call patch");
        assert_eq!(patch.emit.emitted_functions, 3);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute patched direct-call sample"),
            18
        );
        let patched_ptrs: BTreeMap<String, u64> = process
            .artifacts()
            .iter()
            .map(|artifact| (artifact.function_key.name.clone(), artifact.code_ptr))
            .collect();
        for retained in ["leaf", "shared", "even", "odd", "render", "on_code_swap"] {
            assert_eq!(cold_ptrs[retained], patched_ptrs[retained], "{retained}");
        }
        for rebuilt in ["mid", "tick", "main"] {
            assert_ne!(cold_ptrs[rebuilt], patched_ptrs[rebuilt], "{rebuilt}");
        }
        let mid_clif = process
            .clif_for_function_name("mid")
            .expect("patched mid CLIF");
        assert!(mid_clif.matches("call fn").count() >= 2, "{mid_clif}");
        assert!(!mid_clif.contains("stasis_jit_call_") && !mid_clif.contains("lookup_code_ptr"));
    }

    #[test]
    fn jit_process_runs_full_compile_and_records_slots() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return 2; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 2);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process
            .artifacts()
            .iter()
            .all(|artifact| artifact.code_ptr != 0));
    }

    #[test]
    fn jit_process_executes_representative_immediate_axis_layout() {
        let mut process = JitProcess::new();
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        process
            .set_project_root(repository.to_string_lossy())
            .expect("set repository root");
        process.upsert_file(
            repository
                .join("samples/immediate_axis_layout/verify_jit.stasis")
                .to_string_lossy()
                .into_owned(),
            include_str!("../../../../samples/immediate_axis_layout/verify_jit.stasis"),
        );
        process
            .compile()
            .expect("compile immediate axis layout sample");
        assert!(
            !process.state_layout().scalars.is_empty(),
            "JIT composition fixture must cover shared scalar menu bounds"
        );
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute immediate axis layout sample"),
            0
        );
    }

    #[test]
    fn jit_process_supports_local_annotation_only_type_during_emit() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let token: local_only = 7; return token; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
    }

    #[test]
    fn direct_global_storage_emits_no_local_runtime_helper_trampolines() {
        crate::backend::emit::reset_runtime_helper_trampoline_count_for_test();
        let mut process = JitProcess::new();
        process.set_local_runtime_helper_trampolines(true);
        process.upsert_file(
            "sample.stasis",
            "global State { value: i32; }\nglobal bytes: u8[3];\nfunction main(): i32 { State.value = 7; bytes[0] = 1; bytes[1] = 2; bytes[2] = 3; let total: i32 = 0; foreach (let byte in bytes) { total += byte; byte += 1; } return State.value + total + bytes[0]; }\n",
        );
        process.compile().expect("jit compile");
        assert_eq!(process.execute_i32_noarg_by_name("main").unwrap(), 15);
        assert_eq!(
            crate::backend::emit::runtime_helper_trampoline_count_for_test(),
            0
        );
    }

    #[test]
    fn direct_foreach_rejects_backing_shorter_than_fixed_capacity() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "rebound_foreach.stasis",
            "struct ReboundItem172 { score: i32; weight: f32; }\nglobal rebound_nums_172: i32[4];\nglobal rebound_bytes_172: u8[4];\nglobal rebound_items_172: ReboundItem172[4];\nfunction main(): i32 { let total: i32 = 0; foreach (let value in rebound_nums_172) { total += value; } foreach (let byte in rebound_bytes_172) { total += byte; } foreach (let item in rebound_items_172) { total += item.score; if (item.weight > 0.0) { total += 1; } } return total; }\n",
        );
        process.compile().expect("jit compile");

        let nums = Box::leak(Box::new([2, 3]));
        let bytes = Box::leak(Box::new([4]));
        let scores = Box::leak(Box::new([5, 6]));
        let weights = Box::leak(Box::new([1.0]));
        stasis_dynload::register_global_i32_array(
            hash_global_path("rebound_nums_172"),
            0,
            nums.as_mut_ptr(),
            nums.len(),
        );
        stasis_dynload::register_global_u8_array(
            hash_global_path("rebound_bytes_172"),
            0,
            bytes.as_mut_ptr(),
            bytes.len(),
        );
        stasis_dynload::register_global_i32_array(
            hash_global_path("rebound_items_172"),
            crate::backend::emit::hash_foreach_field_suffix("score"),
            scores.as_mut_ptr(),
            scores.len(),
        );
        stasis_dynload::register_global_f32_array(
            hash_global_path("rebound_items_172"),
            crate::backend::emit::hash_foreach_field_suffix("weight"),
            weights.as_mut_ptr(),
            weights.len(),
        );

        assert_eq!(
            process.execute_i32_noarg_by_name("main").unwrap(),
            0,
            "short host arrays must not replace fixed-capacity backing"
        );
    }

    #[test]
    fn local_runtime_resolves_production_preview_externs() {
        crate::backend::emit::reset_runtime_helper_trampoline_count_for_test();
        let mut process = JitProcess::new();
        process.set_local_runtime_helper_trampolines(true);
        process.upsert_file(
            "sample.stasis",
            "extern function time(): i32;\nextern function time_us(): i32;\nextern function gfx_poll_reload(handle: i32): bool;\nextern function gfx_measure_text_cached(handle: i32): f32;\nextern function audio_is_available(): bool;\nfunction main(): i32 { let ms: i32 = time(); let us: i32 = time_us(); let width: f32 = gfx_measure_text_cached(0); if (gfx_poll_reload(0) || audio_is_available() || width != 0.0) { return 2; } if (ms == 0 && us == 0) { return 0; } return 1; }\n",
        );
        process
            .compile()
            .expect("compile production preview externs");
        assert_eq!(process.execute_i32_noarg_by_name("main").unwrap(), 1);
        assert!(crate::backend::emit::runtime_helper_trampoline_count_for_test() >= 5);
    }

    #[test]
    fn jit_process_links_brickout_compatible_audio_asset_api() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "audio_asset_api.stasis",
            "function @extern(\"stasis_jit_audio_load_music\") audio_load_music(path: string): i32;\nfunction @extern(\"stasis_jit_audio_load_effect\") audio_load_effect(path: string): i32;\nfunction @extern(\"stasis_jit_audio_play_music\") audio_play_music(handle: i32, loop: bool, volume: f32): bool;\nfunction @extern(\"stasis_jit_audio_pause_music\") audio_pause_music(handle: i32, paused: bool): void;\nfunction @extern(\"stasis_jit_audio_set_music_volume\") audio_set_music_volume(handle: i32, volume: f32): void;\nfunction @extern(\"stasis_jit_audio_stop_music\") audio_stop_music(handle: i32): void;\nfunction @extern(\"stasis_jit_audio_play_effect\") audio_play_effect(handle: i32, volume: f32): bool;\nfunction main(): i32 { let music: i32 = audio_load_music(\"missing.wav\"); let effect: i32 = audio_load_effect(\"missing.wav\"); audio_play_music(music, true, 0.4); audio_pause_music(music, true); audio_set_music_volume(music, 0.2); audio_stop_music(music); audio_play_effect(effect, 0.5); return music + effect; }\n",
        );
        process.compile().expect("jit compile audio asset API");
        assert_eq!(process.execute_i32_noarg_by_name("main").unwrap(), 0);
    }

    #[test]
    fn local_runtime_defines_shared_helper_trampoline_once_per_module() {
        crate::backend::emit::reset_runtime_helper_trampoline_count_for_test();
        let mut process = JitProcess::new();
        process.set_local_runtime_helper_trampolines(true);
        process.upsert_file(
            "sample.stasis",
            "extern function time(): i32;\nfunction helper(): i32 { return time(); }\nfunction main(): i32 { return time() + helper(); }\n",
        );

        let report = process.compile().expect("compile shared runtime helper");

        assert_eq!(report.emit.emitted_functions, 2);
        process.execute_i32_noarg_by_name("main").unwrap();
        assert_eq!(
            crate::backend::emit::runtime_helper_trampoline_count_for_test(),
            1
        );
    }

    #[test]
    fn jit_process_rejects_non_literal_i32_return() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { return helper(1, 2, 3); }\n",
        );
        let error = process.compile().expect_err("expected compile error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("unknown call target 'helper'")
                        || message.contains("unsupported call arity 3"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_removed_reachable_callee_and_keeps_active_code() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function leaf(): i32 { return 1; } function main(): i32 { return leaf(); }",
        );
        process.compile().expect("compile active source");
        assert_eq!(process.execute_i32_noarg_by_name("main"), Ok(1));

        process.upsert_file("sample.stasis", "function main(): i32 { return leaf(); }");
        let error = process
            .compile()
            .expect_err("removed reachable callee must reject the candidate");
        assert!(
            matches!(
                error,
                crate::compiler::CompileError::Backend(ref message)
                    if message.contains("unknown call target 'leaf'")
            ),
            "unexpected error: {error:?}"
        );
        assert_eq!(
            process.execute_i32_noarg_by_name("main"),
            Ok(1),
            "failed candidate must retain the active complete generation"
        );
    }

    #[test]
    fn jit_snapshot_shares_compiler_data_flow_owner() {
        let mut process = JitProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return 1; }");
        process.compile().expect("compile fixture");
        let snapshot = process.program_snapshot().expect("snapshot");
        assert!(Arc::ptr_eq(
            &snapshot.data_flow_summaries_shared(),
            &process.compiler.data_flow_summaries_shared(),
        ));
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_two_arg_i32_function_in_memory() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function add_pair(left: i32, right: i32): i32 { return left + right; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_twoarg_by_name("add_pair", 4, 6)
            .expect("execute two-arg function");
        assert_eq!(value, 10);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_global_path_set_and_read() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct State { rng_state: i32; }\nglobal state: State;\nfunction set_seed(): i32 { state.rng_state = 7; return state.rng_state; }\nfunction main(): i32 { return set_seed(); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_exposes_typed_scalar_state_for_live_transactions() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global score: i32;\nglobal ratio: f32;\nglobal precise: f64;\nglobal ready: bool;\nfunction main(): i32 { score = 4; ratio = 1.5; precise = 2.25; ready = true; return 0; }\n",
        );
        process.compile().expect("compile");
        process.execute_i32_noarg_by_name("main").expect("main");

        let paths = process.global_scalar_paths();
        assert!(paths.contains(&("score".to_string(), "i32")));
        assert!(paths.contains(&("ratio".to_string(), "f32")));
        assert!(paths.contains(&("precise".to_string(), "f64")));
        assert!(paths.contains(&("ready".to_string(), "bool")));
        assert_eq!(
            process.read_global_scalar("score"),
            Ok(JitScalarValue::I32(4))
        );

        let snapshot = process.snapshot_global_scalars();
        process
            .write_global_scalar("score", JitScalarValue::I32(99))
            .expect("write score");
        assert_eq!(
            process.read_global_scalar("score"),
            Ok(JitScalarValue::I32(99))
        );
        process.restore_global_scalars(&snapshot).expect("restore");
        assert_eq!(
            process.read_global_scalar("score"),
            Ok(JitScalarValue::I32(4))
        );
        assert!(process
            .write_global_scalar("score", JitScalarValue::Bool(true))
            .expect_err("type mismatch")
            .contains("does not accept bool"));
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_call_expression_statement() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global State { value: i32; }\nfunction set_value(): i32 { State.value = 9; return 0; }\nfunction main(): i32 { set_value(); return State.value; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_void_function_body_and_call_statement() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global State { value: i32; }\nfunction set_value(): void { State.value = 12; return; }\nfunction main(): i32 { set_value(); return State.value; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 12);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_void_call_with_mixed_f32_i32_arguments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global State { value: i32; }\nfunction bump(dt: f32, delta: i32): void { if (dt > 0.0) { State.value += delta; } return; }\nfunction main(): i32 { bump(1.0, 5); return State.value; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 5);
    }

    #[cfg(windows)]
    #[test]
    fn jit_inlines_eligible_expression_and_keeps_real_function() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function @inline helper(value: i32): i32 { return value + 1; }\nfunction main(): i32 { return helper(9); }\n",
        );
        process.compile().expect("compile inline fixture");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute inline fixture"),
            10
        );
        let main = process.clif_for_function_name("main").expect("main CLIF");
        assert!(
            !main.lines().any(|line| line.contains("call fn")),
            "eligible @inline call remained in JIT CLIF:\n{main}"
        );
        assert!(
            process.clif_for_function_name("helper").is_some(),
            "the real inline function must remain independently callable"
        );

        process.upsert_file(
            "sample.stasis",
            "function @inline helper(value: i32): i32 { return value + 2; }\nfunction main(): i32 { return helper(9); }\n",
        );
        process.compile().expect("hot patch inline callee");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute patched inline fixture"),
            11,
            "changing an inline callee must regenerate callers that embed its body"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_inline_preserves_overloads_and_indexed_struct_fields() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Enemy { hp: i32; }\nstruct Player { score: i32; }\nglobal enemies: Enemy[2];\nglobal player: Player;\nfunction @inline hp(enemy: Enemy): i32 { return enemy.hp; }\nfunction @inline value(item: Enemy): i32 { return item.hp + 100; }\nfunction value(item: Player): i32 { return item.score; }\nfunction main(): i32 { enemies[0].hp = 7; player.score = 5; return hp(enemies[0]) + value(player); }\n",
        );
        process.compile().expect("compile inline review fixture");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute inline review fixture"),
            12
        );
        let main = process.clif_for_function_name("main").expect("main CLIF");
        assert!(
            main.lines().any(|line| line.contains("call fn")),
            "same-arity overload call must remain for typed backend selection:\n{main}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_print_string_literal_statement() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { print_string(\"hello\\n\"); print_i32(7); return 1; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[test]
    fn jit_process_stages_string_literals_until_runtime_activation() {
        let mut process = JitProcess::new();
        let live_id = crate::backend::emit::hash_string_literal("live");
        let candidate_id = crate::backend::emit::hash_string_literal("candidate");
        stasis_dynload::upsert_jit_string_literal(live_id, "live");
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { print_string(\"candidate\"); return 0; }\n",
        );

        process.compile_staged().expect("staged compile");
        assert_eq!(
            stasis_dynload::jit_string_literal_value(live_id).as_deref(),
            Some("live")
        );
        assert_eq!(stasis_dynload::jit_string_literal_value(candidate_id), None);

        process
            .activate_staged_runtime()
            .expect("runtime activation");
        assert_eq!(stasis_dynload::jit_string_literal_value(live_id), None);
        assert_eq!(
            stasis_dynload::jit_string_literal_value(candidate_id).as_deref(),
            Some("candidate")
        );

        process.upsert_file("sample.stasis", "function main(): i32 { return 1; }\n");
        process.compile_staged().expect("replacement compile");
        process
            .activate_staged_runtime()
            .expect("replacement activation");
        assert_eq!(stasis_dynload::jit_string_literal_value(candidate_id), None);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_string_constant_identifier_argument() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const UI_FONT_PATH: string = \"../../docs/assets/fonts/dejavu-sans-mono.ttf\";\nfunction consume(path: string): i32 { return 5; }\nfunction main(): i32 { return consume(UI_FONT_PATH); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 5);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_ascii_constant_identifier_argument() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const TITLE: ascii[] = \"play\";\nfunction consume(path: ascii[]): i32 { return 6; }\nfunction main(): i32 { return consume(TITLE); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 6);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_utf8_literal_for_ascii_parameter_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function take_ascii(value: ascii[]): i32 { return 7; }\nfunction main(): i32 { return take_ascii(\"hi\"); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_non_ascii_utf8_string_literal_argument() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function take_text(value: utf8[]): i32 { return 9; }\nfunction main(): i32 { return take_text(\"cafÃƒÂ© Ã¢Ëœâ€¢\"); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_block_comments_between_statements() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n    let value: i32 = 2;\n    /* increment path */\n    value += 5;\n    return value;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_string_literal_with_semicolon_in_call_statement() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n    print_string(\"alpha; beta {x}\");\n    return 1;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_control_flow_comments_near_delimiters() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n\
                let sum: i32 = 0;\n\
                for /* before header */ (let i: i32 = 0; i < 3; i += 1) /* before body */ {\n\
                    sum += i;\n\
                }\n\
                if /* before condition */ (sum == 3) /* before then */ {\n\
                    return 1;\n\
                } else /* between else and body */ {\n\
                    return 0;\n\
                }\n\
            }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_comments_inside_expression_arithmetic() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n\
                let value: i32 = 1 /* plus */ + 2;\n\
                let value2: i32 = value + // trailing comment\n\
                    1;\n\
                return value2;\n\
            }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 4);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_comments_inside_condition_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n\
                let value: i32 = 4;\n\
                if (value /* lhs */ == /* rhs */ 4) { return 1; }\n\
                return 0;\n\
            }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_for_header_comment_with_semicolon() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n\
                let sum: i32 = 0;\n\
                for (let i: i32 = 0 /* ; in comment */; i < 4; i += 1) {\n\
                    sum += i;\n\
                }\n\
                return sum;\n\
            }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 6);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_ignores_logical_operator_text_inside_comments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 {\n\
                let value: i32 = 2;\n\
                if ((value > 0 /* || hidden */) && (value < 3 /* && hidden */)) {\n\
                    return 1;\n\
                }\n\
                return 0;\n\
            }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_print_int_alias_statement() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { print_int(3); return 1; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_print_char_alias_statement() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { print_char(10); return 1; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_global_path_compound_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct State { counter: i32; }\nglobal state: State;\nfunction main(): i32 { state.counter = 10; state.counter -= 3; return state.counter; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_typed_f32_global_path_set_and_read() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Layout { width: f32; }\nglobal state: Layout;\nfunction main(): i32 { state.width = 3.5; let w: f32 = state.width; if (w > 3.0) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_typed_f64_global_path_set_and_read() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Layout { width: f64; }\nglobal state: Layout;\nfunction main(): i32 { state.width = 3.5; let w: f64 = state.width; if (w > 3.0) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_f64_array_set_and_read() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global values: f64[4];\nfunction main(): i32 { values[0] = 3.5; let w: f64 = values[0]; if (w > 3.0) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_f64_return_call_and_conversions() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(value: f64): f64 { return value + 1.0; }\nfunction main(): i32 {\n    let src: i32 = 8;\n    let x: f64 = 0.0;\n    x.from_i32(src);\n    let v: f64 = helper(x);\n    let out: i32 = 0;\n    out.from_f64(v);\n    return out;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_global_path_from_i32_conversion_target() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct State { src: i32; value: f32; }\nglobal state: State;\nfunction main(): i32 { state.src = 8; state.value.from_i32(state.src); let out: i32 = 0; out.from_f32(state.value); return out; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 8);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_collection_handle_rebind_with_set_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global left: i32[4];\nglobal right: i32[4];\nfunction main(): i32 {\n    let view: i32[] = left;\n    view[0] = 65;\n    view = right;\n    view[0] = 66;\n    return left[0] * 100 + right[0];\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 6566);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_typed_ascii_view_let_binding() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global text: ascii[4];\nfunction main(): i32 {\n    let view: ascii[] = text;\n    view[0] = 90;\n    return text[0];\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 90);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_collection_handle_compound_assignment_for_local() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global left: i32[4];\nglobal right: i32[4];\nfunction main(): i32 {\n    let view: i32[] = left;\n    view += right;\n    return 0;\n}\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("collection handle assignment only supports '='"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_indexed_path_from_i32_conversion_target() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Node { value: f32; }\nglobal nodes: Node[COUNT];\nfunction main(): i32 { nodes[1].value.from_i32(7); let out: i32 = 0; out.from_f32(nodes[1].value); return out; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_string_header_length_path() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global hud_line: utf8[64];\nfunction main(): i32 { hud_line.length = 7; return hud_line.length; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_initializes_string_max_length_headers() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global a: ascii[32];\nglobal u: utf8[64];\nfunction main(): i32 {\n    return a.max_length + u.max_length;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 96);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_initializes_fixed_array_max_length_header_and_path() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global values: i32[12];\nfunction main(): i32 {\n    return values.max_length;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 12);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_ascii_copy_truncates_to_destination_capacity() {
        let mut process = JitProcess::new();
        process
            .set_project_root(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .to_string_lossy(),
            )
            .expect("set repository root");
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("jit_stdlib_ascii_copy_sample.stasis");
        process.upsert_file(
            sample_path.to_string_lossy().to_string(),
            "import \"src/stdlib/stdlib.stasis\";\nglobal src_text: ascii[8];\nglobal dst_text: ascii[4];\nfunction main(): i32 {\n    src_text[0] = 65;\n    src_text[1] = 66;\n    src_text[2] = 67;\n    src_text[3] = 68;\n    src_text[4] = 69;\n    src_text[5] = 0;\n    ascii_set_len(src_text, 5);\n    ascii_copy(dst_text, src_text);\n    return length(dst_text) * 10 + dst_text[3];\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 30);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_ascii_recount_is_bounded_by_capacity() {
        let mut process = JitProcess::new();
        process
            .set_project_root(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .to_string_lossy(),
            )
            .expect("set repository root");
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("jit_stdlib_ascii_recount_sample.stasis");
        process.upsert_file(
            sample_path.to_string_lossy().to_string(),
            "import \"src/stdlib/stdlib.stasis\";\nglobal text: ascii[4];\nfunction main(): i32 {\n    text[0] = 65;\n    text[1] = 66;\n    text[2] = 67;\n    text[3] = 68;\n    ascii_set_len(text, 0);\n    return ascii_recount(text);\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 4);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_utf8_from_ascii_clamps_to_header_capacity() {
        let mut process = JitProcess::new();
        process
            .set_project_root(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .to_string_lossy(),
            )
            .expect("set repository root");
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("jit_stdlib_utf8_from_ascii_capacity_sample.stasis");
        process.upsert_file(
            sample_path.to_string_lossy().to_string(),
            "import \"src/stdlib/stdlib.stasis\";\nglobal src_text: ascii[8];\nglobal dst_text: utf8[4];\nfunction main(): i32 {\n    src_text[0] = 65;\n    src_text[1] = 66;\n    src_text[2] = 67;\n    src_text[3] = 68;\n    src_text[4] = 69;\n    src_text[5] = 0;\n    ascii_set_len(src_text, 5);\n    let written: i32 = utf8_from_ascii(dst_text, src_text, 99);\n    return written * 1000 + length_bytes(dst_text) * 100 + length_chars(dst_text) * 10 + dst_text[3];\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 3330);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_ascii_set_len_clamps_to_max_length() {
        let mut process = JitProcess::new();
        process
            .set_project_root(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .to_string_lossy(),
            )
            .expect("set repository root");
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("jit_stdlib_ascii_set_len_clamp_sample.stasis");
        process.upsert_file(
            sample_path.to_string_lossy().to_string(),
            "import \"src/stdlib/stdlib.stasis\";\nglobal text: ascii[4];\nfunction main(): i32 {\n    ascii_set_len(text, 99);\n    return length(text);\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 4);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_utf8_set_len_ascii_clamps_to_max_length() {
        let mut process = JitProcess::new();
        process
            .set_project_root(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .to_string_lossy(),
            )
            .expect("set repository root");
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("jit_stdlib_utf8_set_len_clamp_sample.stasis");
        process.upsert_file(
            sample_path.to_string_lossy().to_string(),
            "import \"src/stdlib/stdlib.stasis\";\nglobal text: utf8[4];\nfunction main(): i32 {\n    utf8_set_len_ascii(text, 99);\n    return length_bytes(text) * 10 + length_chars(text);\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 44);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_stdlib_utf8_from_ascii_respects_source_capacity_without_terminator() {
        let mut process = JitProcess::new();
        process
            .set_project_root(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .to_string_lossy(),
            )
            .expect("set repository root");
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("jit_stdlib_utf8_from_ascii_src_cap_sample.stasis");
        process.upsert_file(
            sample_path.to_string_lossy().to_string(),
            "import \"src/stdlib/stdlib.stasis\";\nglobal src_text: ascii[4];\nglobal dst_text: utf8[8];\nfunction main(): i32 {\n    src_text[0] = 65;\n    src_text[1] = 66;\n    src_text[2] = 67;\n    src_text[3] = 68;\n    ascii_set_len(src_text, 0);\n    let written: i32 = utf8_from_ascii(dst_text, src_text, 8);\n    return written * 100 + length_bytes(dst_text) * 10 + dst_text[4];\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 440);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_foreach_struct_array_with_index_alias() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal state: Enemy[COUNT];\nfunction main(): i32 {\n    foreach (let enemy, i in state) { enemy.hp = i + 1; }\n    let sum: i32 = 0;\n    foreach (let enemy in state) { sum += enemy.hp; }\n    return sum;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 6);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_foreach_struct_array_without_let_header_style() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal state: Enemy[COUNT];\nfunction main(): i32 {\n    foreach (i, enemy in state) { enemy.hp = i + 1; }\n    let sum: i32 = 0;\n    foreach (enemy in state) { sum += enemy.hp; }\n    return sum;\n}\n",
        );
        let error = process
            .compile()
            .expect_err("compile should reject non-let foreach");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("foreach header must start with 'let'"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_foreach_over_local_fixed_array_parameter() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 4;\nglobal nums: i32[COUNT];\nfunction init(arr: i32[4]): i32 {\n    foreach (let v, i in arr) { arr[i] = i + 1; }\n    let sum: i32 = 0;\n    foreach (let v in arr) { sum += v; }\n    return sum;\n}\nfunction main(): i32 { return init(nums); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 10);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_foreach_over_local_fixed_array_without_let_header_style() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 4;\nglobal nums: i32[COUNT];\nfunction init(arr: i32[4]): i32 {\n    foreach (i, v in arr) { arr[i] = i + 1; }\n    let sum: i32 = 0;\n    foreach (v in arr) { sum += v; }\n    return sum;\n}\nfunction main(): i32 { return init(nums); }\n",
        );
        let error = process
            .compile()
            .expect_err("compile should reject non-let foreach");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("foreach header must start with 'let'"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_foreach_struct_array_f32_fields() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Node { x: f32; }\nglobal nodes: Node[COUNT];\nfunction main(): i32 {\n    foreach (let node, i in nodes) { let fi: f32 = 0.0; fi.from_i32(i); node.x = fi + 1.5; }\n    let total: f32 = 0.0;\n    foreach (let node in nodes) { total = total + node.x; }\n    if (total > 3.9) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_tick_from_stasis_fixture_with_input_snapshot() {
        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("stasis")
            .join("rust_native_tick_input_snapshot.stasis");
        let source = std::fs::read_to_string(&fixture_path)
            .expect("read rust_native_tick_input_snapshot.stasis fixture");

        let mut process = JitProcess::new();
        process
            .set_project_root(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .to_string_lossy(),
            )
            .expect("set repository root");
        process.upsert_file(fixture_path.to_string_lossy().to_string(), source);
        process.compile().expect("compile");

        let snapshot_mode_hash = hash_global_path("snapshot_mode");
        stasis_dynload::stasis_jit_global_i32_store(snapshot_mode_hash, 1);

        let tick_value = process
            .execute_i32_noarg_by_name("tick")
            .expect("execute tick");
        assert_eq!(tick_value, 1);

        let pointer_count =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("model_pointer_count"));
        let escape_down =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("model_escape_down"));
        let first_went_down =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("model_first_went_down"));
        let first_x = stasis_dynload::stasis_jit_global_f32_load(hash_global_path("model_first_x"));
        let latched =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("last_tick_code"));
        assert_eq!(pointer_count, 1);
        assert_eq!(escape_down, 1);
        assert_eq!(first_went_down, 1);
        assert!(first_x > 12.0);
        assert_eq!(latched, 1);

        stasis_dynload::stasis_jit_global_i32_store(snapshot_mode_hash, 2);

        let quit_tick = process
            .execute_i32_noarg_by_name("tick")
            .expect("execute quit tick");
        assert_eq!(quit_tick, -1);
        let quit_latched =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("last_tick_code"));
        assert_eq!(quit_latched, -1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_indexed_i32_array_access() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 4;\nglobal nums: i32[COUNT];\nfunction main(): i32 {\n    nums[0] = 2;\n    nums[1] = 5;\n    let i: i32 = 1;\n    nums[i] += 3;\n    return nums[0] + nums[1];\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 10);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_indexed_i32_array_parameter_access() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nglobal nums: i32[COUNT];\nfunction read_at(arr: i32[], idx: i32): i32 { return arr[idx]; }\nfunction write_at(arr: i32[], idx: i32, value: i32): void { arr[idx] = value; return; }\nfunction main(): i32 {\n    nums[1] = 9;\n    write_at(nums, 2, 11);\n    return read_at(nums, 1) + read_at(nums, 2);\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 20);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_indexed_struct_array_parameter_field_access() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal enemies: Enemy[COUNT];\nfunction mutate(arr: Enemy[3], idx: i32): i32 {\n    arr[idx].hp = 10;\n    arr[idx + 1].hp = arr[idx].hp + 4;\n    return arr[idx + 1].hp;\n}\nfunction main(): i32 { return mutate(enemies, 0); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 14);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_indexed_struct_array_view_parameter_field_access() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal enemies: Enemy[COUNT];\nfunction mutate(arr: Enemy[], idx: i32): i32 {\n    arr[idx].hp = 10;\n    arr[idx + 1].hp = arr[idx].hp + 4;\n    return arr[idx + 1].hp;\n}\nfunction main(): i32 { return mutate(enemies, 0); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 14);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_struct_parameter_field_access() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Pipe { active: bool; }\nglobal pipe: Pipe;\nfunction read_active(p: Pipe): i32 { if (p.active) { return 1; } return 0; }\nfunction main(): i32 { pipe.active = true; return read_active(pipe); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_i32_to_f32_intrinsic_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let x: f32 = i32_to_f32(7); if (x > 6.9) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_continue_in_for_and_foreach_loops() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            include_str!("../../../../tests/stasis/seams/continue_loop_parity.stasis"),
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 434);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_indexed_struct_element_alias_binding() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Enemy { hp: i32; }\nglobal enemies: Enemy[2];\nfunction bad(arr: Enemy[2]): i32 {\n    let enemy = arr[0];\n    enemy.hp = 10;\n    return arr[0].hp;\n}\nfunction main(): i32 { return bad(enemies); }\n",
        );
        process.compile().expect("compile");
        let clif = process
            .clif_for_function_name("bad")
            .expect("bad function CLIF");
        assert!(
            !clif.contains("brif"),
            "known SoA alias retained storage dispatch:\n{clif}"
        );
        assert!(
            !clif.contains("icmp"),
            "known SoA alias retained storage test:\n{clif}"
        );
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 10);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_specializes_nested_struct_array_alias_as_soa() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Enemy { hp: i32; }\nstruct Game { enemies: Enemy[2]; }\nglobal State { game: Game; }\nfunction main(): i32 {\n    let enemy: Enemy = State.game.enemies[1];\n    enemy.hp = 17;\n    return enemy.hp;\n}\n",
        );
        process.compile().expect("compile");
        let clif = process.clif_for_function_name("main").expect("main CLIF");
        let has_call_instruction = clif.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("call ") || line.contains(" = call ")
        });
        assert!(
            !has_call_instruction,
            "known nested SoA alias lost direct storage:\n{clif}"
        );
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 17);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_specializes_global_struct_alias_as_aos() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Pipe { active: bool; }\nstruct StateData { pipe: Pipe; }\nglobal State: StateData;\nfunction main(): i32 {\n    let alias: Pipe = State.pipe;\n    alias.active = true;\n    if (alias.active) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let clif = process.clif_for_function_name("main").expect("main CLIF");
        assert_eq!(
            clif.matches("brif").count(),
            1,
            "known AoS alias added storage dispatch:\n{clif}"
        );
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_retains_dynamic_dispatch_for_struct_parameter() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Pipe { active: bool; }\nglobal pipe: Pipe;\nglobal pipes: Pipe[1];\nfunction read_active(value: Pipe): i32 { if (value.active) { return 1; } return 0; }\nfunction main(): i32 {\n    pipe.active = true;\n    pipes[0].active = true;\n    return read_active(pipe) + read_active(pipes[0]);\n}\n",
        );
        process.compile().expect("compile");
        let clif = process
            .clif_for_function_name("read_active")
            .expect("read_active CLIF");
        assert!(
            clif.matches("brif").count() >= 2,
            "mixed struct parameter lost storage dispatch:\n{clif}"
        );
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 2);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_propagates_soa_struct_parameter_through_helper_chain() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Sample { value: i32; }\nglobal items: Sample[1];\nfunction leaf(value: Sample): i32 { return value.value; }\nfunction middle(value: Sample): i32 { return leaf(value); }\nfunction main(): i32 { items[0].value = 7; return middle(items[0]); }\n",
        );
        process.compile().expect("compile");
        let summaries = process.compiler.data_flow_summaries_shared();
        for function in ["leaf", "middle"] {
            let summary = summaries
                .iter()
                .find(|summary| summary.function == function)
                .expect("helper summary");
            assert_eq!(
                summary.parameter_storage_kinds,
                vec![crate::data_flow::ParameterStorageKind::Soa],
                "unexpected {function} provenance; summaries={summaries:#?}"
            );
        }
        for function in ["leaf", "middle"] {
            let clif = process
                .clif_for_function_name(function)
                .expect("helper CLIF");
            assert!(
                !clif.contains("brif"),
                "known SoA provenance retained storage dispatch in {function}:\n{clif}"
            );
        }
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute main"),
            7
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_propagates_aos_struct_parameter_through_helper_chain() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Sample { value: i32; }\nglobal item: Sample;\nfunction leaf(value: Sample): i32 { return value.value; }\nfunction middle(value: Sample): i32 { return leaf(value); }\nfunction main(): i32 { item.value = 8; return middle(item); }\n",
        );
        process.compile().expect("compile");
        let summaries = process.compiler.data_flow_summaries_shared();
        for function in ["leaf", "middle"] {
            let summary = summaries
                .iter()
                .find(|summary| summary.function == function)
                .expect("helper summary");
            assert_eq!(
                summary.parameter_storage_kinds,
                vec![crate::data_flow::ParameterStorageKind::Aos],
                "unexpected {function} provenance; summaries={summaries:#?}"
            );
            let clif = process
                .clif_for_function_name(function)
                .expect("helper CLIF");
            assert!(
                !clif.contains("brif"),
                "known AoS provenance retained storage dispatch in {function}:\n{clif}"
            );
        }
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute main"),
            8
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_propagates_soa_struct_parameter_through_recursive_cycle() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Sample { value: i32; }\nglobal items: Sample[1];\nfunction even(value: Sample, depth: i32): i32 { if (depth <= 0) { return value.value; } return odd(value, depth - 1); }\nfunction odd(value: Sample, depth: i32): i32 { if (depth <= 0) { return value.value; } return even(value, depth - 1); }\nfunction main(): i32 { items[0].value = 9; return even(items[0], 3); }\n",
        );
        process.compile().expect("compile");
        let summaries = process.compiler.data_flow_summaries_shared();
        for function in ["even", "odd"] {
            let summary = summaries
                .iter()
                .find(|summary| summary.function == function)
                .expect("recursive helper summary");
            assert_eq!(
                summary.parameter_storage_kinds[0],
                crate::data_flow::ParameterStorageKind::Soa,
                "unexpected {function} provenance; summaries={summaries:#?}"
            );
        }
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute main"),
            9
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_keeps_mixed_struct_parameter_provenance_dynamic() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Sample { value: i32; }\nglobal item: Sample;\nglobal items: Sample[1];\nfunction leaf(value: Sample): i32 { return value.value; }\nfunction middle(value: Sample): i32 { return leaf(value); }\nfunction main(): i32 { item.value = 3; items[0].value = 4; return middle(item) + middle(items[0]); }\n",
        );
        process.compile().expect("compile");
        let summaries = process.compiler.data_flow_summaries_shared();
        for function in ["leaf", "middle"] {
            let summary = summaries
                .iter()
                .find(|summary| summary.function == function)
                .expect("helper summary");
            assert_eq!(
                summary.parameter_storage_kinds,
                vec![crate::data_flow::ParameterStorageKind::Dynamic],
                "unexpected {function} provenance; summaries={summaries:#?}"
            );
        }
        let leaf_clif = process.clif_for_function_name("leaf").expect("leaf CLIF");
        assert!(
            leaf_clif.contains("brif"),
            "mixed provenance incorrectly specialized leaf:\n{leaf_clif}"
        );
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute main"),
            7
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_storage_specialization_preserves_live_aliases() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Sample { value: i32; }\nglobal item: Sample;\nfunction observe(first: Sample, second: Sample): i32 { let before: i32 = first.value; second.value = 9; return before + first.value; }\nfunction main(): i32 { item.value = 2; return observe(item, item); }\n",
        );
        process.compile().expect("compile");
        let summary = process
            .compiler
            .data_flow_summaries_shared()
            .iter()
            .find(|summary| summary.function == "observe")
            .cloned()
            .expect("observe summary");
        assert_eq!(
            summary.parameter_storage_kinds,
            vec![
                crate::data_flow::ParameterStorageKind::Aos,
                crate::data_flow::ParameterStorageKind::Aos,
            ]
        );
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute main"),
            11
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_recompiles_helper_when_call_storage_provenance_changes() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Sample { value: i32; }\nglobal item: Sample;\nglobal items: Sample[1];\nfunction leaf(value: Sample): i32 { return value.value; }\nfunction main(): i32 { items[0].value = 7; return leaf(items[0]); }\n",
        );
        process.compile().expect("initial compile");
        let initial_leaf_ptr = process
            .artifacts()
            .iter()
            .find(|artifact| artifact.function_key.name == "leaf")
            .expect("initial leaf artifact")
            .code_ptr;
        assert!(!process
            .clif_for_function_name("leaf")
            .expect("initial leaf CLIF")
            .contains("brif"));

        process.upsert_file(
            "sample.stasis",
            "struct Sample { value: i32; }\nglobal item: Sample;\nglobal items: Sample[1];\nfunction leaf(value: Sample): i32 { return value.value; }\nfunction main(): i32 { item.value = 3; items[0].value = 4; return leaf(item) + leaf(items[0]); }\n",
        );
        let report = process.compile().expect("mixed-provenance patch");
        assert_eq!(report.emit.emitted_functions, 2);
        let patched_leaf_ptr = process
            .artifacts()
            .iter()
            .find(|artifact| artifact.function_key.name == "leaf")
            .expect("patched leaf artifact")
            .code_ptr;
        assert_ne!(initial_leaf_ptr, patched_leaf_ptr);
        assert!(process
            .clif_for_function_name("leaf")
            .expect("patched leaf CLIF")
            .contains("brif"));
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute patched main"),
            7
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_local_indexed_struct_element_in_scalar_context() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Enemy { hp: i32; }\nglobal enemies: Enemy[2];\nfunction bad(arr: Enemy[2]): i32 { return arr[0]; }\nfunction main(): i32 { return bad(enemies); }\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains(
                        "local indexed collection access requires field path for struct elements"
                    ),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_foreach_over_local_struct_array_parameter() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal enemies: Enemy[COUNT];\nfunction sum_fill(arr: Enemy[3]): i32 {\n    foreach (let enemy, i in arr) { enemy.hp = i + 2; }\n    let total: i32 = 0;\n    foreach (let enemy in arr) { total += enemy.hp; }\n    return total;\n}\nfunction main(): i32 { return sum_fill(enemies); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_indexed_struct_field_access() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 3;\nstruct Enemy { hp: i32; }\nglobal state: Enemy[COUNT];\nfunction main(): i32 {\n    let i: i32 = 1;\n    state[i].hp = 7;\n    return state[i].hp;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_indexed_struct_value_copy_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Enemy { hp: i32; speed: f32; }\nglobal enemies: Enemy[COUNT];\nfunction main(): i32 {\n    enemies[0].hp = 11;\n    enemies[0].speed = 2.5;\n    enemies[1] = enemies[0];\n    if (enemies[1].speed > 2.4) { return enemies[1].hp; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 11);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_indexed_struct_value_copy_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Enemy { hp: i32; speed: f32; }\nglobal enemies: Enemy[COUNT];\nfunction copy_local(arr: Enemy[2]): i32 {\n    arr[0].hp = 11;\n    arr[0].speed = 2.5;\n    arr[1] = arr[0];\n    if (arr[1].speed > 2.4) { return arr[1].hp; }\n    return 0;\n}\nfunction main(): i32 { return copy_local(enemies); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 11);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_local_indexed_struct_value_copy_assignment_for_view_param() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Enemy { hp: i32; speed: f32; }\nglobal enemies: Enemy[COUNT];\nfunction copy_local(arr: Enemy[]): i32 {\n    arr[0].hp = 11;\n    arr[0].speed = 2.5;\n    arr[1] = arr[0];\n    if (arr[1].speed > 2.4) { return arr[1].hp; }\n    return 0;\n}\nfunction main(): i32 { return copy_local(enemies); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 11);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_local_indexed_struct_copy_assignment_for_mismatched_layouts() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct A { hp: i32; }\nstruct B { hp: i32; speed: f32; }\nglobal lhs: A[2];\nglobal rhs: B[2];\nfunction copy(arr_lhs: A[2], arr_rhs: B[2]): i32 { arr_rhs[0] = arr_lhs[0]; return 0; }\nfunction main(): i32 { return copy(lhs, rhs); }\n",
        );
        let error = process
            .compile()
            .expect_err("compile should reject local mismatched indexed struct copy assignment");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains(
                        "struct indexed copy assignment requires matching field layout for 'arr_rhs[...]' and 'arr_lhs[...]'"
                    ),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_local_indexed_struct_copy_compound_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Enemy { hp: i32; }\nglobal enemies: Enemy[2];\nfunction bad(arr: Enemy[2]): i32 { arr[1] += arr[0]; return 0; }\nfunction main(): i32 { return bad(enemies); }\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains(
                        "struct indexed copy assignment only supports '=' for 'arr[...]'"
                    ),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_global_struct_path_value_copy_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Pos { x: f32; y: f32; }\nstruct Enemy { hp: i32; pos: Pos; }\nglobal src: Enemy;\nglobal dst: Enemy;\nfunction main(): i32 {\n    src.hp = 13;\n    src.pos.x = 4.5;\n    src.pos.y = 2.0;\n    dst = src;\n    if (dst.pos.x > 4.4) { return dst.hp; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 13);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_global_block_nested_struct_path_copy_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Enemy { hp: i32; speed: f32; }\nglobal state { src: Enemy; dst: Enemy; }\nfunction main(): i32 {\n    state.src.hp = 9;\n    state.src.speed = 3.25;\n    state.dst = state.src;\n    if (state.dst.speed > 3.2) { return state.dst.hp; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_struct_copy_from_indexed_to_global_path() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Enemy { hp: i32; speed: f32; }\nglobal source: Enemy[COUNT];\nglobal target: Enemy;\nfunction main(): i32 {\n    source[1].hp = 6;\n    source[1].speed = 2.75;\n    target = source[1];\n    if (target.speed > 2.7) { return target.hp; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 6);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_struct_copy_from_global_to_indexed_path() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Enemy { hp: i32; speed: f32; }\nglobal source: Enemy;\nglobal target: Enemy[COUNT];\nfunction main(): i32 {\n    source.hp = 8;\n    source.speed = 4.25;\n    target[1] = source;\n    if (target[1].speed > 4.2) { return target[1].hp; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 8);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_struct_copy_from_indexed_to_global_on_layout_mismatch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 1;\nstruct Src { hp: i32; armor: i32; }\nstruct Dst { hp: i32; }\nglobal source: Src[COUNT];\nglobal target: Dst;\nfunction main(): i32 {\n    target = source[0];\n    return 0;\n}\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains(
                        "struct copy assignment from indexed source requires matching field layout"
                    ),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_struct_copy_from_global_to_indexed_on_layout_mismatch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 1;\nstruct Src { hp: i32; }\nstruct Dst { hp: i32; armor: i32; }\nglobal source: Src;\nglobal target: Dst[COUNT];\nfunction main(): i32 {\n    target[0] = source;\n    return 0;\n}\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains(
                        "struct copy assignment to indexed target requires matching field layout"
                    ),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_evaluates_struct_copy_indices_once_each() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Enemy { hp: i32; speed: f32; }\nglobal enemies: Enemy[COUNT];\nglobal target_calls: i32;\nglobal source_calls: i32;\nfunction next_target(): i32 { target_calls += 1; return 1; }\nfunction next_source(): i32 { source_calls += 1; return 0; }\nfunction main(): i32 {\n    enemies[0].hp = 9;\n    enemies[0].speed = 3.5;\n    enemies[next_target()] = enemies[next_source()];\n    if (enemies[1].speed > 3.4) { return target_calls * 100 + source_calls * 10 + enemies[1].hp; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 119);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_indexed_struct_copy_assignment_for_mismatched_layouts() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 1;\nstruct One { hp: i32; }\nstruct Two { hp: i32; armor: i32; }\nglobal a: One[COUNT];\nglobal b: Two[COUNT];\nfunction main(): i32 {\n    a[0] = b[0];\n    return 0;\n}\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message
                        .contains("struct indexed copy assignment requires matching field layout"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_global_struct_copy_assignment_with_collection_fields() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Player { hp: i32; name: ascii[8]; }\nglobal left: Player;\nglobal right: Player;\nfunction main(): i32 {\n    left = right;\n    return 0;\n}\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains(
                        "struct path copy assignment currently supports scalar fields only"
                    ),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_global_struct_path_copy_compound_assignment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Enemy { hp: i32; }\nglobal a: Enemy;\nglobal b: Enemy;\nfunction main(): i32 {\n    b += a;\n    return 0;\n}\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("struct path copy assignment only supports '='"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_indexed_named_field_assignment_from_enum_variant() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "enum BrickType { Basic, Armored, Reflector, }\nconst COUNT: i32 = 2;\nstruct Brick { brick_type: BrickType; hp: i32; }\nglobal bricks: Brick[COUNT];\nfunction place(kind: BrickType): i32 { bricks[0].brick_type = kind; return 0; }\nfunction main(): i32 {\n    let ignored: i32 = place(BrickType.Reflector);\n    if (bricks[0].brick_type == BrickType.Reflector) { return 1; }\n    return ignored;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_named_type_let_binding() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "enum BrickType { Basic, Armored, }\nfunction cost(kind: BrickType): i32 { if (kind == BrickType.Armored) { return 2; } return 1; }\nfunction main(): i32 { let t: BrickType = BrickType.Armored; return cost(t); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 2);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_swapped_ui_axis_enum_argument() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "enum UiHorizontal { Left, Center, Right, }\nenum UiVertical { Top, Center, Bottom, }\nfunction ui_place_x(parent_x: f32, horizontal: UiHorizontal): f32 { return parent_x; }\nfunction main(): i32 { let x: f32 = ui_place_x(0.0, UiVertical.Top); return 0; }\n",
        );
        process
            .compile()
            .expect_err("swapped UI axis enum argument must fail");
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_i32_ui_axis_argument() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "enum UiHorizontal { Left, Center, Right, }\nfunction ui_place_x(parent_x: f32, horizontal: UiHorizontal): f32 { return parent_x; }\nfunction main(): i32 { let x: f32 = ui_place_x(0.0, 1); return 0; }\n",
        );
        process
            .compile()
            .expect_err("i32 UI axis argument must fail");
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_allows_i32_return_from_named_i32_abi_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function as_i32(v: u8): i32 { return v; }\nfunction main(): i32 { let b: u8 = 7; return as_i32(b); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_accepts_i32_literal_for_named_i32_abi_parameter() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function as_i32(v: u8): i32 { return v; }\nfunction main(): i32 { return as_i32(7); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_named_i32_abi_binary_arithmetic() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function seed(): u8 { return 60; }\nfunction main(): i32 { let b: u8 = seed(); return b - 48; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 12);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_named_i32_abi_to_f32_coercion_in_binary_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function seed(): u8 { return 2; }\nfunction main(): i32 {\n    let b: u8 = seed();\n    let value: f32 = b + 0.5;\n    if (value > 2.4) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_unsigned_wrap_and_deterministic_fixed32_intrinsics() {
        let source = include_str!("../../../../samples/deterministic_numerics/main.stasis");
        let mut process = JitProcess::new();
        process.upsert_file("main.stasis", source);
        process
            .compile()
            .expect("compile deterministic numeric sample");
        let result = process
            .execute_i32_noarg_by_name("main")
            .expect("execute deterministic numeric sample");
        assert_eq!(
            process
                .read_global_collection_scalar("byte_values", "", 1)
                .expect("read byte value"),
            JitScalarValue::U8(255)
        );
        assert_eq!(
            process
                .read_global_collection_scalar("word_values", "", 1)
                .expect("read word value"),
            JitScalarValue::U16(65_535)
        );
        assert_eq!(
            process
                .read_global_collection_scalar("wide_values", "", 1)
                .expect("read wide value"),
            JitScalarValue::U32(u32::MAX)
        );
        assert_eq!(
            process
                .read_global_collection_scalar("foreach_words", "", 1)
                .expect("read foreach word"),
            JitScalarValue::U16(0)
        );
        assert_eq!(
            process.read_global_scalar("byte_value"),
            Ok(JitScalarValue::U8(0))
        );
        assert_eq!(
            process.read_global_scalar("word_value"),
            Ok(JitScalarValue::U16(0))
        );
        assert_eq!(
            process.read_global_scalar("wide_value"),
            Ok(JitScalarValue::U32(u32::MAX))
        );
        assert_eq!(result, 0);
    }

    #[test]
    fn state_expression_arithmetic_and_comparisons_preserve_unsigned_semantics() {
        assert_eq!(
            apply_state_binary(
                JitScalarValue::U32(u32::MAX),
                BinaryOperator::Divide,
                JitScalarValue::I32(2),
            ),
            Ok(JitScalarValue::U32(i32::MAX as u32))
        );
        assert_eq!(
            apply_state_binary(
                JitScalarValue::U8(u8::MAX),
                BinaryOperator::Add,
                JitScalarValue::I32(1),
            ),
            Ok(JitScalarValue::U8(0))
        );
        assert_eq!(
            apply_state_binary(
                JitScalarValue::U32(u32::MAX),
                BinaryOperator::Equal,
                JitScalarValue::I32(-1),
            ),
            Ok(JitScalarValue::Bool(true))
        );
        assert_eq!(
            apply_state_binary(
                JitScalarValue::U8(u8::MAX),
                BinaryOperator::Greater,
                JitScalarValue::I32(-1),
            ),
            Ok(JitScalarValue::Bool(false))
        );
    }

    #[cfg(windows)]
    #[test]
    fn signed_numeric_intrinsics_reject_unsigned_arguments() {
        for (intrinsic, expected_message) in [
            (
                "let result: i32 = fixed32_from_i32(wide)",
                "requires exact i32 arguments",
            ),
            (
                "let result: f32 = i32_to_f32(wide)",
                "requires exact i32 argument",
            ),
        ] {
            let mut process = JitProcess::new();
            process.upsert_file(
                "main.stasis",
                format!(
                    "function main(): i32 {{ let wide: u32 = 4294967295; {intrinsic}; return 0; }}\n"
                ),
            );
            let error = process.compile().expect_err("reject unsigned argument");
            let diagnostic = format!("{error:?}");
            assert!(
                diagnostic.contains(expected_message),
                "unexpected diagnostic for {intrinsic}: {diagnostic}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_i32_call_with_five_arguments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function sum5(a: i32, b: i32, c: i32, d: i32, e: i32): i32 { return a + b + c + d + e; }\nfunction main(): i32 { return sum5(1, 2, 3, 4, 5); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 15);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_bool_return_call_with_f32_arguments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function point_in_rect(px: f32, py: f32, rx: f32, ry: f32, rw: f32, rh: f32): bool {\n    if (px < rx) { return false; }\n    if (py < ry) { return false; }\n    if (px > (rx + rw)) { return false; }\n    if (py > (ry + rh)) { return false; }\n    return true;\n}\nfunction main(): i32 {\n    if (point_in_rect(5.0, 6.0, 0.0, 0.0, 10.0, 10.0)) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_f32_return_call_with_four_arguments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function dist2(ax: f32, ay: f32, bx: f32, by: f32): f32 {\n    let dx: f32 = ax - bx;\n    let dy: f32 = ay - by;\n    return (dx * dx) + (dy * dy);\n}\nfunction main(): i32 {\n    let value: f32 = dist2(0.0, 0.0, 3.0, 4.0);\n    if (value > 24.9) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_f32_return_call_with_mixed_arguments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function mix(a: f32, b: i32, c: f32, d: f32): f32 {\n    let bf: f32 = 0.0;\n    bf.from_i32(b);\n    return a + bf + c + d;\n}\nfunction main(): i32 {\n    let value: f32 = mix(1.0, 2, 3.0, 4.0);\n    if (value > 9.9) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_i32_return_call_with_mixed_arguments() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function score(base: i32, scale: f32): i32 {\n    let x: i32 = 0;\n    x.from_f32(scale);\n    return base + x;\n}\nfunction main(): i32 { return score(2, 3.0); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 5);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_bool_return_condition_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function fits(size_w: i32, size_h: i32, max_w: i32, max_h: i32): bool {\n    return size_w <= max_w && size_h <= max_h;\n}\nfunction main(): i32 {\n    if (fits(10, 20, 10, 21)) { return 1; }\n    return 0;\n}\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_sin_fast_intrinsic_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let x: f32 = sin_fast(0.0); if (x < 0.001) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_extern_keyword_function_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "extern function global_i32_load(path_hash: i32): i32;\nfunction main(): i32 { return global_i32_load(123) + 1; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_annotated_extern_function_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function @extern(\"stasis_jit_global_i32_load\") host_load(path_hash: i32): i32;\nfunction main(): i32 { return host_load(456) + 2; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 2);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_sys_memcpy_i32_extern_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "extern function sys_memcpy_i32(dst: i32[], dst_index: i32, src: i32[], src_index: i32, count: i32): void;\nconst COUNT: i32 = 4;\nglobal src: i32[COUNT];\nglobal dst: i32[COUNT];\nfunction main(): i32 { src[0] = 3; src[1] = 5; sys_memcpy_i32(dst, 0, src, 0, 2); return dst[0] + dst[1]; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 8);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_supports_global_block_style_path_set_and_read() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global State { score: i32; }\nfunction main(): i32 { State.score = 7; return State.score; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute main");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_lowers_core_global_storage_without_runtime_calls() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "direct_storage.stasis",
            "struct Enemy { hp: i32; speed: f32; }\nglobal count: i32;\nglobal ratio: f32;\nglobal precise: f64;\nglobal ints: i32[2];\nglobal floats: f32[2];\nglobal doubles: f64[2];\nglobal bytes: u8[3];\nglobal enemies: Enemy[1];\nglobal label: ascii[4];\nfunction main(): i32 {\n    count = 7;\n    ratio = 1.5;\n    precise = 2.5;\n    ints[0] = 11;\n    floats[1] = 3.5;\n    doubles[0] = 4.5;\n    bytes[2] = 250;\n    enemies[0].hp = 13;\n    enemies[0].speed = 6.5;\n    label[0] = 65;\n    let result: i32 = count + ints[0] + bytes[2] + enemies[0].hp + label[0] + label.max_length;\n    if (ratio < 1.4) { return 3; }\n    if (precise < 2.4) { return 4; }\n    if (floats[1] < 3.4) { return 5; }\n    if (doubles[0] < 4.4) { return 6; }\n    if (enemies[0].speed < 6.4) { return 7; }\n    return result;\n}\n",
        );
        process.compile().expect("compile direct storage fixture");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute direct storage fixture"),
            350
        );
        let clif = process.clif_for_function_name("main").expect("main CLIF");
        assert!(clif.contains("load.i32"), "expected direct loads:\n{clif}");
        assert!(clif.contains("store"), "expected direct stores:\n{clif}");
        let has_call_instruction = clif.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("call ") || line.contains(" = call ")
        });
        assert!(
            !has_call_instruction,
            "core global storage emitted a runtime call:\n{clif}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_elides_static_array_checks_for_canonical_max_length_loop() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "bounds.stasis",
            "global values: i32[4];\nfunction main(): i32 {\n    let total: i32 = 0;\n    let i: i32 = 0;\n    for (i = 0; i < values.max_length; i = i + 1) {\n        total += values[i];\n    }\n    return total;\n}\n",
        );
        process.compile().expect("compile proven bounds fixture");
        let clif = process.clif_for_function_name("main").expect("main CLIF");
        assert!(clif.contains("load.i32"), "expected direct load:\n{clif}");
        assert!(
            !clif.contains("trapz"),
            "proven loop retained array bounds trap:\n{clif}"
        );
        let loop_bound_loads = clif
            .lines()
            .filter(|line| line.trim_start().contains("load.i32"))
            .count();
        assert_eq!(
            loop_bound_loads, 1,
            "fixed max_length should be an immediate, leaving only the element load:\n{clif}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_keeps_view_max_length_dynamic() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "view_capacity.stasis",
            "global storage: i32[4];\nfunction capacity(values: i32[]): i32 { return values.max_length; }\nfunction main(): i32 { return capacity(storage); }\n",
        );
        process.compile().expect("compile view capacity fixture");
        let clif = process
            .clif_for_function_name("capacity")
            .expect("capacity CLIF");
        assert!(
            clif.lines().any(|line| {
                let line = line.trim_start();
                line.starts_with("call ") || line.contains(" = call ")
            }),
            "view max_length incorrectly became a static constant:\n{clif}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_elides_static_array_store_checks_for_canonical_max_length_loop() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "bounds.stasis",
            "global values: i32[4];\nfunction main(): i32 {\n    let i: i32 = 0;\n    for (i = 0; i < values.max_length; i = i + 1) {\n        values[i] = i;\n    }\n    return 0;\n}\n",
        );
        process
            .compile()
            .expect("compile proven store bounds fixture");
        let clif = process.clif_for_function_name("main").expect("main CLIF");
        assert!(clif.contains("store"), "expected direct store:\n{clif}");
        assert!(
            !clif.contains("trapz"),
            "proven loop retained array store bounds trap:\n{clif}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_preserves_proven_bounds_through_struct_element_alias() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "bounds.stasis",
            "struct Enemy { hp: i32; }\nglobal enemies: Enemy[4];\nfunction main(): i32 {\n    let total: i32 = 0;\n    let i: i32 = 0;\n    for (i = 0; i < enemies.max_length; i = i + 1) {\n        let enemy: Enemy = enemies[i];\n        total += enemy.hp;\n    }\n    return total;\n}\n",
        );
        process.compile().expect("compile alias bounds fixture");
        let clif = process.clif_for_function_name("main").expect("main CLIF");
        assert!(
            clif.contains("load.i32"),
            "expected direct alias load:\n{clif}"
        );
        assert!(
            !clif.contains("trapz"),
            "proven struct alias retained array bounds trap:\n{clif}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_retains_fatal_check_for_unproven_static_array_index() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "bounds.stasis",
            "global values: i32[4];\nfunction read(index: i32): i32 { return values[index]; }\nfunction main(): i32 { return read(0); }\n",
        );
        process.compile().expect("compile checked bounds fixture");
        let clif = process.clif_for_function_name("read").expect("read CLIF");
        assert!(
            clif.contains("trapz"),
            "unproven static array index omitted fatal check:\n{clif}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_retains_fatal_check_for_unproven_static_array_store() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "bounds.stasis",
            "global values: i32[4];\nfunction write(index: i32): i32 { values[index] = 7; return 0; }\nfunction main(): i32 { return write(0); }\n",
        );
        process
            .compile()
            .expect("compile checked store bounds fixture");
        let clif = process.clif_for_function_name("write").expect("write CLIF");
        assert!(
            clif.contains("trapz"),
            "unproven static array store omitted fatal check:\n{clif}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_checks_unproven_static_struct_view_once() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "bounds.stasis",
            "struct Draw { x: f32; y: f32; opacity: i32; }\nglobal draws: Draw[4];\nfunction write(index: i32): i32 {\n    let draw: Draw = draws[index];\n    draw.x = 1.0;\n    draw.y = 2.0;\n    draw.opacity = 255;\n    return draw.opacity;\n}\nfunction main(): i32 { return write(0); }\n",
        );
        process
            .compile()
            .expect("compile checked struct view fixture");
        let clif = process.clif_for_function_name("write").expect("write CLIF");
        assert_eq!(
            clif.matches("trapz").count(),
            1,
            "static struct view did not consolidate bounds checks at alias creation:\n{clif}"
        );
        assert!(
            clif.contains("store"),
            "expected direct field stores:\n{clif}"
        );
        assert!(
            clif.contains("load.i32"),
            "expected direct field load:\n{clif}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_storage_rebinding_is_rejected_during_execution_windows() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "rebind.stasis",
            "global value: i32;\nfunction main(): i32 { value += 1; return value; }\n",
        );
        process.compile().expect("compile rebind fixture");
        let path_hash = hash_global_path("value");
        let first = Box::leak(Box::new(40));
        let second = Box::leak(Box::new(90));
        stasis_dynload::register_global_i32_ptr(path_hash, first);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("first run"),
            41
        );
        {
            let _execution = stasis_dynload::JitExecutionGuard::enter();
            stasis_dynload::register_global_i32_ptr(path_hash, second);
        }
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("run after rejected in-window rebind"),
            42
        );
        assert_eq!(*second, 90);
        stasis_dynload::register_global_i32_ptr(path_hash, second);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("run after boundary rebind"),
            91
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_elides_static_array_checks_for_in_bounds_literal_indexes() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "literal_bounds.stasis",
            "global values: i32[4];\nfunction main(): i32 { values[0] = 3; values[1 + 2] = 4; return values[0] + values[6 / 2]; }\n",
        );
        process.compile().expect("compile literal bounds fixture");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute literal bounds fixture"),
            7
        );
        let clif = process.clif_for_function_name("main").expect("main CLIF");
        assert!(clif.contains("load.i32"), "expected direct loads:\n{clif}");
        assert!(clif.contains("store"), "expected direct stores:\n{clif}");
        assert!(
            !clif.contains("trapz"),
            "in-bounds literal indexes retained array bounds traps:\n{clif}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_retains_fatal_check_for_out_of_bounds_literal_index() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "literal_bounds.stasis",
            "global values: i32[4];\nfunction read(): i32 { return values[4]; }\nfunction main(): i32 { if (values[0] < 0) { return read(); } return 0; }\n",
        );
        process
            .compile()
            .expect("compile invalid literal bounds fixture");
        let clif = process.clif_for_function_name("read").expect("read CLIF");
        assert!(
            clif.contains("trapz"),
            "out-of-bounds literal index omitted fatal check:\n{clif}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_does_not_prove_unevaluable_constant_array_index() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "constant_bounds.stasis",
            "global values: i32[4];\nfunction divide_by_zero(): i32 { return values[1 / 0]; }\nfunction main(): i32 { if (values[0] < 0) { return divide_by_zero(); } return 0; }\n",
        );
        process
            .compile()
            .expect("compile invalid constant bounds fixture");
        let clif = process
            .clif_for_function_name("divide_by_zero")
            .expect("unevaluable constant function CLIF");
        assert!(
            clif.contains("trap"),
            "unevaluable constant index was incorrectly proven safe:\n{clif}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_rejects_negative_constant_array_index() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "constant_bounds.stasis",
            "global values: i32[4];\nfunction main(): i32 { return values[0 - 1]; }\n",
        );
        let error = process
            .compile()
            .expect_err("reject negative constant index");
        match error {
            crate::compiler::CompileError::Backend(message) => assert!(
                message.contains("negative collection indices"),
                "unexpected negative-index diagnostic: {message}"
            ),
            other => panic!("unexpected negative-index error: {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_rejects_backing_shorter_than_compiled_fixed_array() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "resize.stasis",
            "global values: i32[2];\nfunction main(): i32 { return values[3]; }\n",
        );
        process.compile().expect("compile resize fixture");
        let hash = hash_global_path("values");
        let expanded = Box::leak(Box::new([1, 2, 3, 44]));
        stasis_dynload::register_global_i32_array(hash, 0, expanded.as_mut_ptr(), expanded.len());
        assert_eq!(
            stasis_dynload::direct_array_storage_slot_len_for_test(
                stasis_dynload::JitStorageKind::I32,
                hash,
                0,
            ),
            Some(expanded.len())
        );

        let contracted = Box::leak(Box::new([9]));
        stasis_dynload::register_global_i32_array(
            hash,
            0,
            contracted.as_mut_ptr(),
            contracted.len(),
        );
        assert_eq!(
            stasis_dynload::direct_array_storage_slot_len_for_test(
                stasis_dynload::JitStorageKind::I32,
                hash,
                0,
            ),
            Some(expanded.len()),
            "short host backing replaced compiled fixed-array storage"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_emits_selective_patches_for_reachable_edit_shapes() {
        fn source(
            leaf: &str,
            shared: &str,
            mid: &str,
            tick: &str,
            render: &str,
            swap_delta: i32,
            extra: &str,
        ) -> String {
            format!(
                "global swap_count: i32;\nfunction leaf(): i32 {{ {leaf} }}\nfunction shared(): i32 {{ {shared} }}\nfunction mid(): i32 {{ {mid} }}\nfunction tick(): i32 {{ {tick} }}\nfunction render(): i32 {{ {render} }}\nfunction on_code_swap(): void {{ swap_count += {swap_delta}; return; }}\nfunction main(): i32 {{ return tick() + render(); }}\n{extra}\n"
            )
        }

        let mut process = JitProcess::new();
        let cases = [
            (
                "baseline",
                source(
                    "return 1;",
                    "return leaf() + 1;",
                    "return shared() + 1;",
                    "return mid() + 10;",
                    "return shared() + 20;",
                    1,
                    "",
                ),
                35,
                7,
            ),
            (
                "tick body",
                source(
                    "return 1;",
                    "return leaf() + 1;",
                    "return shared() + 1;",
                    "return mid() + 11;",
                    "return shared() + 20;",
                    1,
                    "",
                ),
                36,
                2,
            ),
            (
                "render body",
                source(
                    "return 1;",
                    "return leaf() + 1;",
                    "return shared() + 1;",
                    "return mid() + 11;",
                    "return shared() + 21;",
                    1,
                    "",
                ),
                37,
                2,
            ),
            (
                "on_code_swap body",
                source(
                    "return 1;",
                    "return leaf() + 1;",
                    "return shared() + 1;",
                    "return mid() + 11;",
                    "return shared() + 21;",
                    3,
                    "",
                ),
                37,
                1,
            ),
            (
                "leaf body",
                source(
                    "return 2;",
                    "return leaf() + 1;",
                    "return shared() + 1;",
                    "return mid() + 11;",
                    "return shared() + 21;",
                    3,
                    "",
                ),
                39,
                6,
            ),
            (
                "mid body",
                source(
                    "return 2;",
                    "return leaf() + 1;",
                    "return shared() + 2;",
                    "return mid() + 11;",
                    "return shared() + 21;",
                    3,
                    "",
                ),
                40,
                3,
            ),
            (
                "shared body",
                source(
                    "return 2;",
                    "return leaf() + 2;",
                    "return shared() + 2;",
                    "return mid() + 11;",
                    "return shared() + 21;",
                    3,
                    "",
                ),
                42,
                5,
            ),
            (
                "multiple bodies",
                source(
                    "return 2;",
                    "return leaf() + 2;",
                    "return shared() + 2;",
                    "return mid() + 12;",
                    "return shared() + 22;",
                    3,
                    "",
                ),
                44,
                3,
            ),
            (
                "added reachable function",
                source(
                    "return 2;",
                    "return leaf() + 2;",
                    "return shared() + added();",
                    "return mid() + 12;",
                    "return shared() + 22;",
                    3,
                    "function added(): i32 { return 4; }",
                ),
                46,
                4,
            ),
            (
                "deleted and renamed function",
                source(
                    "return 2;",
                    "return leaf() + 2;",
                    "return shared() + renamed();",
                    "return mid() + 12;",
                    "return shared() + 22;",
                    3,
                    "function renamed(): i32 { return 5; }",
                ),
                47,
                4,
            ),
            (
                "unreachable function",
                source(
                    "return 2;",
                    "return leaf() + 2;",
                    "return shared() + renamed();",
                    "return mid() + 12;",
                    "return shared() + 22;",
                    3,
                    "function renamed(): i32 { return 5; }\nfunction unreachable(): i32 { return missing(); }",
                ),
                47,
                0,
            ),
        ];

        let mut previous_revision = None;
        let mut expected_module_count = 0;
        for (label, source, expected, emitted) in cases {
            process.upsert_file("sample.stasis", source);
            let report = process
                .compile()
                .unwrap_or_else(|error| panic!("compile {label}: {error:?}"));
            assert_eq!(report.emit.emitted_functions, emitted, "{label}");
            let reachable_count = if matches!(
                label,
                "added reachable function"
                    | "deleted and renamed function"
                    | "unreachable function"
            ) {
                8
            } else {
                7
            };
            assert_eq!(process.artifacts().len(), reachable_count, "{label}");
            assert_eq!(
                process
                    .execute_i32_noarg_by_name("main")
                    .unwrap_or_else(|error| panic!("execute {label}: {error}")),
                expected,
                "{label}"
            );
            let metadata = process
                .generation_metadata()
                .unwrap_or_else(|| panic!("metadata {label}"));
            if emitted > 0 {
                expected_module_count += 1;
            }
            assert_eq!(metadata.emitted_function_ids.len(), emitted, "{label}");
            assert_eq!(
                metadata.reused_function_ids.len(),
                reachable_count - emitted,
                "{label}"
            );
            assert_eq!(metadata.module_count, expected_module_count, "{label}");
            if let Some(previous) = previous_revision {
                assert_ne!(metadata.source_revision, previous, "{label}");
            }
            previous_revision = Some(metadata.source_revision);
        }

        process
            .execute_void_noarg_by_name("on_code_swap")
            .expect("execute edited on_code_swap");
        assert_eq!(process.read_i32_global_path("swap_count"), 3);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_skips_unreachable_invalid_function_body() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function bad(): i32 { return missing(); }\nfunction tick(): i32 { return 1; }\n",
        );
        let report = process.compile().expect("compile");
        assert_eq!(report.emit.emitted_functions, 1);
        let value = process
            .execute_i32_noarg_by_name("tick")
            .expect("execute tick");
        assert_eq!(value, 1);
    }

    #[test]
    fn jit_process_supports_binary_literal_return_expression() {
        let mut process = JitProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return 4 + 5; }\n");
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn selectively_profiled_jit_reports_only_named_functions() {
        let mut process = JitProcess::new();
        process
            .set_profile_functions(vec!["helper".to_string()])
            .expect("configure profiler");
        process.upsert_file(
            "src/main.stasis",
            "function helper(value: i32): i32 { return value * 2; }\nfunction main(): i32 { return helper(3) + helper(4); }\n",
        );
        process.compile().expect("profiled compile");
        let helper_id = process
            .program_snapshot()
            .expect("program snapshot")
            .functions()
            .iter()
            .find(|function| function.name == "helper")
            .map(|function| function.id)
            .expect("helper id");

        stasis_dynload::enable_jit_profiler();
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute profiled main"),
            14
        );
        stasis_dynload::disable_jit_profiler();
        let samples = stasis_dynload::jit_profile_snapshot();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].function_id, helper_id);
        assert_eq!(samples[0].calls, 2);
        assert!(samples[0].inclusive_ns >= samples[0].exclusive_ns);
        stasis_dynload::reset_jit_profile();
    }

    #[test]
    fn instrumented_jit_stops_with_nested_frames_and_current_lexical_values() {
        let mut process = JitProcess::new();
        process
            .set_debug_instrumentation(true)
            .expect("enable instrumentation");
        process.upsert_file(
            "src/main.stasis",
            "function helper(value: i32): i32 {\n    let doubled: i32 = value * 2;\n    return doubled;\n}\nfunction main(): i32 {\n    let base: i32 = 3;\n    return helper(base);\n}\n",
        );
        process.compile().expect("instrumented compile");
        let helper = process
            .debug_metadata()
            .values()
            .find(|metadata| metadata.name == "helper")
            .cloned()
            .expect("helper debug metadata");
        let main = process
            .debug_metadata()
            .values()
            .find(|metadata| metadata.name == "main")
            .cloned()
            .expect("main debug metadata");
        assert_eq!(helper.sites.len(), 2);
        let first_site = helper.sites[0].site_id;
        let second_site = helper.sites[1].site_id;
        stasis_dynload::enable_jit_debugger([(helper.function_id, first_site)]);

        let controller = std::thread::spawn(move || {
            let first =
                stasis_dynload::wait_for_jit_debug_stop(0, std::time::Duration::from_secs(2))
                    .expect("first JIT stop");
            assert_eq!(
                (first.function_id, first.site_id),
                (helper.function_id, first_site)
            );
            assert_eq!(
                first
                    .frames
                    .iter()
                    .map(|frame| frame.function_id)
                    .collect::<Vec<_>>(),
                vec![main.function_id, helper.function_id]
            );
            assert_eq!(
                first.frames[0].values.get(&debug_variable_slot("base")),
                Some(&stasis_dynload::JitDebugValue::I64 {
                    type_tag: i32::from(TYPE_ID_I32),
                    value: 3,
                })
            );
            assert_eq!(
                first.frames[1].values.get(&debug_variable_slot("value")),
                Some(&stasis_dynload::JitDebugValue::I64 {
                    type_tag: i32::from(TYPE_ID_I32),
                    value: 3,
                })
            );

            stasis_dynload::resume_jit_debugger(stasis_dynload::JitDebugResume::StepOver)
                .expect("step helper");
            let second = stasis_dynload::wait_for_jit_debug_stop(
                first.sequence,
                std::time::Duration::from_secs(2),
            )
            .expect("second JIT stop");
            assert_eq!(
                (second.function_id, second.site_id),
                (helper.function_id, second_site)
            );
            assert_eq!(
                second.frames[1].values.get(&debug_variable_slot("doubled")),
                Some(&stasis_dynload::JitDebugValue::I64 {
                    type_tag: i32::from(TYPE_ID_I32),
                    value: 6,
                })
            );
            stasis_dynload::resume_jit_debugger(stasis_dynload::JitDebugResume::Continue)
                .expect("continue JIT");
        });

        let result = process
            .execute_i32_noarg_by_name("main")
            .expect("execute instrumented main");
        assert_eq!(result, 6);
        controller.join().expect("debug controller");
        stasis_dynload::disable_jit_debugger();
    }

    #[test]
    fn instrumented_jit_exposes_scalar_foreach_item_and_index() {
        let source = "global values: i32[1];\nfunction main(): i32 {\n    values[0] = 7;\n    foreach (let value, index in values) {\n        let copied: i32 = value;\n    }\n    return 0;\n}\n";
        let mut process = JitProcess::new();
        process
            .set_debug_instrumentation(true)
            .expect("enable instrumentation");
        process.upsert_file("src/main.stasis", source);
        process.compile().expect("instrumented compile");
        let main = process
            .debug_metadata()
            .values()
            .find(|metadata| metadata.name == "main")
            .cloned()
            .expect("main debug metadata");
        assert_eq!(
            main.variables.get(&debug_variable_slot("value")),
            Some(&"value".to_string())
        );
        assert_eq!(
            main.variables.get(&debug_variable_slot("index")),
            Some(&"index".to_string())
        );
        let copied_offset = u32::try_from(source.find("let copied").expect("copied statement"))
            .expect("source offset fits u32");
        let body_site = main
            .sites
            .iter()
            .find(|site| site.source_offset == copied_offset)
            .expect("foreach body debug site")
            .site_id;
        stasis_dynload::enable_jit_debugger([(main.function_id, body_site)]);

        let expected_function_id = main.function_id;
        let controller = std::thread::spawn(move || {
            let stop =
                stasis_dynload::wait_for_jit_debug_stop(0, std::time::Duration::from_secs(2))
                    .expect("foreach body stop");
            assert_eq!(
                (stop.function_id, stop.site_id),
                (expected_function_id, body_site)
            );
            let frame = stop.frames.last().expect("foreach frame");
            assert_eq!(
                frame.values.get(&debug_variable_slot("value")),
                Some(&stasis_dynload::JitDebugValue::I64 {
                    type_tag: i32::from(TYPE_ID_I32),
                    value: 7,
                })
            );
            assert_eq!(
                frame.values.get(&debug_variable_slot("index")),
                Some(&stasis_dynload::JitDebugValue::I64 {
                    type_tag: i32::from(TYPE_ID_I32),
                    value: 0,
                })
            );
            stasis_dynload::resume_jit_debugger(stasis_dynload::JitDebugResume::Continue)
                .expect("continue JIT");
        });

        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute instrumented main"),
            0
        );
        controller.join().expect("debug controller");
        stasis_dynload::disable_jit_debugger();
    }

    #[test]
    fn jit_process_supports_i32_let_and_if_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let total: i32 = 5; if (total > 4) { return total + 2; } return 0; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_for_loop_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 0; i < 4; i += 1) { sum += i; } return sum; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_for_loop_with_empty_init_segment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let i: i32 = 0; let sum: i32 = 0; for (; i < 4; i += 1) { sum += i; } return sum; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected missing init segment to be rejected");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("for header must include init, condition, and step"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_supports_for_loop_with_empty_step_segment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let i: i32 = 0; let sum: i32 = 0; for (; i < 4; ) { sum += i; i += 1; } return sum; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected missing step segment to be rejected");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("for header must include init, condition, and step"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_supports_for_loop_call_init_and_conversion_step_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global State { init_calls: i32; }\nfunction mark_init(): void { State.init_calls += 1; return; }\nfunction main(): i32 { let i: f32 = 0.0; let sum: i32 = 0; for (mark_init(); i < 3.0; i.from_i32(sum)) { sum += 1; } return sum + State.init_calls; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 2);
        assert_eq!(report.emit.emitted_functions, 2);
        assert_eq!(process.artifacts().len(), 2);
        assert!(process.artifacts()[0].code_ptr != 0);
        assert!(process.artifacts()[1].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_for_loop_global_init_and_indexed_conversion_step_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Node { value: f32; }\nglobal nodes: Node[COUNT];\nglobal State { snap: f32; }\nfunction main(): i32 { let sum: i32 = 0; for (State.snap.from_i32(0); sum < 2; nodes[1].value.from_i32(sum)) { sum += 1; } let out: i32 = 0; out.from_f32(nodes[1].value); return out; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_inferred_let_and_for_init_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum = 0; for (let i = 0; i < 4; i += 1) { sum += i; } return sum; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_inferred_float_let_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let alpha = 0.5; if (alpha > 0.4) { return 1; } return 0; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_bool_condition_expression_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let ready = true; if (ready) { return 1; } return 0; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_rejects_i32_condition_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value = 1; if (value) { return 1; } return 0; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected condition type error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("condition expression must be bool"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_i32_return_from_bool_function() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function enabled(): bool { return 1; }\nfunction main(): i32 { if (enabled()) { return 1; } return 0; }\n",
        );
        let error = process.compile().expect_err("expected return type error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("return expression expected bool but found i32"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_i32_initializer_for_bool_binding() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let ready: bool = 1; if (ready) { return 1; } return 0; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected assignment type error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("let binding 'ready' expected bool expression but found i32"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_f32_condition_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let alpha = 1.0; if (alpha) { return 1; } return 0; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected condition type error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("condition expression must be bool"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_i32_for_condition_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let i = 2; for (i -= 0; i; i -= 1) { return 1; } return 0; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected condition type error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("condition expression must be bool"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_let_shadowing_parameter() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(value: i32): i32 { let value: i32 = 1; return value; }\n",
        );
        let error = process.compile().expect_err("expected shadowing error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("let binding 'value' shadows existing variable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_for_init_shadowing_outer_local() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let i: i32 = 4; for (let i: i32 = 0; i < 1; i += 1) { } return i; }\n",
        );
        let error = process.compile().expect_err("expected shadowing error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("let binding 'i' shadows existing variable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_foreach_item_shadowing_outer_local() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nglobal values: i32[COUNT];\nfunction main(): i32 { let value: i32 = 0; foreach (let value in values) { value += 1; } return value; }\n",
        );
        let error = process.compile().expect_err("expected shadowing error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("foreach item binding 'value' shadows existing variable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_rejects_foreach_item_and_index_name_collision() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nglobal values: i32[COUNT];\nfunction main(): i32 { foreach (let v, v in values) { } return 0; }\n",
        );
        let error = process.compile().expect_err("expected shadowing error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("foreach index binding 'v' shadows existing variable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_for_loop_missing_init_segment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (; sum < 3; sum += 1) { } return sum; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected for header segment error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("for header must include init, condition, and step"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_for_loop_missing_condition_segment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (sum = 0; ; sum += 1) { } return sum; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected for header segment error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("for header must include init, condition, and step"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_for_loop_missing_step_segment() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (sum = 0; sum < 3; ) { } return sum; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected for header segment error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("for header must include init, condition, and step"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_rejects_duplicate_parameter_names() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function add(v: i32, v: i32): i32 { return v; }\n",
        );
        let error = process.compile().expect_err("expected shadowing error");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("parameter 'v' shadows existing variable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_supports_void_return_functions() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function on_code_swap(): void { return; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_spec_compound_assignment_operators() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 20; value -= 3; value *= 2; value /= 7; value %= 4; return value; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_for_loop_logical_condition_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 0; (i < 5) && !(i == 3); i += 1) { sum += i; } return sum; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_if_else_if_else_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 2; if (value == 0) { return 1; } else if (value == 2) { return 5; } else { return 9; } }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_supports_logical_if_condition_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 2; if ((value > 1 && value < 4) || !(value == 2)) { return 11; } return 0; }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert!(process.artifacts()[0].code_ptr != 0);
    }

    #[test]
    fn jit_process_rejects_while_loop_keyword() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let i: i32 = 0; while (i < 3) { i += 1; } return i; }\n",
        );
        let error = process
            .compile()
            .expect_err("expected unsupported statement");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("unsupported statement in function body near 'while"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn jit_process_exposes_symbol_code_ptr_map() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return 2; }\n",
        );
        process.compile().expect("compile");
        let map = process.symbol_code_ptrs();
        assert!(map.contains_key("main"));
        assert!(map.get("main").copied().unwrap_or(0) != 0);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_i32_in_memory_for_verification() {
        let mut process = JitProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return -7; }\n");
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, -7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_i32_let_and_if_true_branch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let total: i32 = 5; if (total > 4) { return total + 2; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_i32_let_and_if_false_branch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let total: i32 = 3; if (total > 4) { return total + 2; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 0);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_for_loop_accumulation() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 0; i < 4; i += 1) { sum += i; } return sum; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 6);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_compound_assignment_operators() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 20; value -= 3; value *= 2; value /= 7; value %= 4; return value; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 0);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_if_else_if_branch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 2; if (value == 0) { return 1; } else if (value == 2) { return 5; } else { return 9; } }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 5);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_if_else_fallback_branch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 3; if (value == 0) { return 1; } else if (value == 2) { return 5; } else { return 9; } }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_logical_and_or_not_condition_true() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 2; if ((value > 1 && value < 4) || !(value == 2)) { return 11; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 11);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_logical_and_or_not_condition_false() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let value: i32 = 5; if ((value > 1 && value < 4) || !(value == 5)) { return 11; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 0);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_for_loop_with_decrement_step() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 5; i > 0; i -= 2) { sum += i; } return sum; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_for_loop_with_logical_condition() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 0; (i < 5) && !(i == 3); i += 1) { sum += i; } return sum; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 3);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_for_loop_call_init_and_conversion_step() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global State { init_calls: i32; }\nfunction mark_init(): void { State.init_calls += 1; return; }\nfunction main(): i32 { let i: f32 = 0.0; let sum: i32 = 0; for (mark_init(); i < 3.0; i.from_i32(sum)) { sum += 1; } return sum + State.init_calls; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 4);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_for_loop_global_init_and_indexed_conversion_step() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const COUNT: i32 = 2;\nstruct Node { value: f32; }\nglobal nodes: Node[COUNT];\nglobal State { snap: f32; }\nfunction main(): i32 { let sum: i32 = 0; for (State.snap.from_i32(0); sum < 2; nodes[1].value.from_i32(sum)) { sum += 1; } let out: i32 = 0; out.from_f32(nodes[1].value); return out; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 2);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_inferred_let_and_for_init() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let sum = 0; for (let i = 0; i < 4; i += 1) { sum += i; } return sum; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 6);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_inferred_float_let() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let alpha = 0.5; if (alpha > 0.4) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_bool_condition_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function main(): i32 { let ready = true; if (ready) { return 1; } return 0; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 1);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_noarg_call_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 7; }\nfunction main(): i32 { return helper() + 2; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_two_arg_call_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function add_pair(left: i32, right: i32): i32 { return left + right; }\nfunction main(): i32 { return add_pair(2, 3); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 5);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_receiver_style_call_expression() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function damage(enemy: i32, amount: i32): i32 { return enemy - amount; }\nfunction main(): i32 { let enemy: i32 = 10; return enemy.damage(3); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 7);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_executes_nested_struct_receiver_call() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Sprite { handle: i32; }\nstruct GameState { aura: Sprite; sprites: Sprite[2]; }\nglobal state: GameState;\nfunction set_handle(self: Sprite, value: i32): void { self.handle = value; }\nfunction main(): i32 { state.aura.set_handle(37); state.sprites[1].set_handle(5); return state.aura.handle + state.sprites[1].handle; }\n",
        );
        process.compile().expect("compile nested receiver");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute nested receiver");
        assert_eq!(value, 42);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_resolves_same_method_name_by_receiver_type() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function damage(enemy: Enemy, amount: i32): i32 { return amount + 1; }\nfunction damage(player: Player, amount: i32): i32 { return amount + 2; }\nfunction main(enemy: Enemy, player: Player): i32 { return enemy.damage(3) + player.damage(3); }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_twoarg_by_name("main", 10, 20)
            .expect("execute in memory");
        assert_eq!(value, 9);
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_resolves_receiver_methods_with_different_arities() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "struct Sprite { handle: i32; }\nstruct TextRun { handle: i32; }\nglobal sprite: Sprite;\nglobal text: TextRun;\nfunction draw(self: Sprite, x: f32, alpha: i32): void { self.handle = alpha; }\nfunction draw(self: TextRun, x: f32, r: f32, g: f32, b: f32, a: f32): void { self.handle = 7; }\nfunction main(): i32 { sprite.draw(1.0, 255); text.draw(2.0, 1.0, 1.0, 1.0, 1.0); return sprite.handle + text.handle; }\n",
        );
        process.compile().expect("compile");
        let value = process
            .execute_i32_noarg_by_name("main")
            .expect("execute in memory");
        assert_eq!(value, 262);
    }

    #[test]
    fn jit_process_rejects_ambiguous_overload_for_same_receiver_shape() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function damage(enemy: Enemy, amount: i32): i32 { return amount + 1; }\nfunction damage(other_enemy: Enemy, amount: i32): i32 { return amount + 2; }\nfunction main(enemy: Enemy): i32 { return enemy.damage(3); }\n",
        );
        let error = process.compile().expect_err("expected ambiguous overload");
        match error {
            crate::compiler::CompileError::Frontend(message) => {
                assert!(
                    message.contains("duplicate declaration identity"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected frontend identity error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn duplicate_host_alias_rejects_candidate_and_preserves_active_jit_identity() {
        let mut process = JitProcess::new();
        process.upsert_file("src/main.stasis", "function main(): i32 { return 17; }\n");
        process.compile().expect("compile accepted generation");
        let accepted_snapshot = process.program_snapshot().expect("snapshot").clone();
        let accepted_ptrs = process.function_code_ptrs();

        process.upsert_file(
            "src/duplicate.stasis",
            "function main(): i32 { return 99; }\n",
        );
        let error = process
            .compile()
            .expect_err("duplicate host alias must fail");

        assert!(matches!(
            error,
            crate::compiler::CompileError::Backend(message)
                if message.contains("host ABI alias 'main' requires exactly one canonical identity")
        ));
        assert_eq!(
            process
                .program_snapshot()
                .expect("active snapshot")
                .functions(),
            accepted_snapshot.functions()
        );
        assert_eq!(process.function_code_ptrs(), accepted_ptrs);
        assert_eq!(process.execute_i32_noarg_by_name("main"), Ok(17));
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_execution_reflects_incremental_recompile() {
        let mut process = JitProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return 1; }\n");
        process.compile().expect("first compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute first"),
            1
        );

        process.upsert_file("sample.stasis", "function main(): i32 { return 3; }\n");
        process.compile().expect("second compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute second"),
            3
        );
    }

    #[cfg(windows)]
    #[test]
    fn staged_candidate_reuses_active_arenas_and_keeps_active_code_running() {
        let mut active = JitProcess::new();
        active.upsert_file(
            "sample.stasis",
            "function leaf(): i32 { return 1; } function render(): i32 { return 9; } function main(): i32 { return leaf(); }",
        );
        active.compile().expect("cold compile");
        let render_ptr = active
            .artifacts()
            .iter()
            .find(|artifact| artifact.function_key.name == "render")
            .expect("render artifact")
            .code_ptr;

        let mut candidate = active.staged_candidate();
        candidate.upsert_file(
            "sample.stasis",
            "function leaf(): i32 { return 2; } function render(): i32 { return 9; } function main(): i32 { return leaf(); }",
        );
        let report = candidate.compile().expect("selective candidate compile");
        assert_eq!(report.emit.emitted_functions, 2);
        assert_eq!(active.execute_i32_noarg_by_name("main").unwrap(), 1);
        assert_eq!(candidate.execute_i32_noarg_by_name("main").unwrap(), 2);
        assert_eq!(
            candidate
                .artifacts()
                .iter()
                .find(|artifact| artifact.function_key.name == "render")
                .expect("retained render artifact")
                .code_ptr,
            render_ptr
        );

        active.accept_staged_candidate(candidate);
        assert_eq!(active.execute_i32_noarg_by_name("main").unwrap(), 2);
    }

    #[cfg(windows)]
    #[test]
    fn effect_contract_rejects_staged_candidate_and_preserves_active_code() {
        let mut active = JitProcess::new();
        active.upsert_file(
            "effects.stasis",
            "struct State { value: i32; } global state: State; function leaf(): void { return; } function @effects() render(): i32 { leaf(); return 7; } function main(): i32 { return render(); }",
        );
        active.compile().expect("compile compliant generation");
        let revision = active
            .generation_metadata()
            .expect("active metadata")
            .source_revision;
        assert_eq!(active.execute_i32_noarg_by_name("main").unwrap(), 7);

        let mut candidate = active.staged_candidate();
        candidate.upsert_file(
            "effects.stasis",
            "struct State { value: i32; } global state: State; function leaf(): void { state.value += 1; } function @effects() render(): i32 { leaf(); return 9; } function main(): i32 { return render(); }",
        );
        let error = candidate
            .compile()
            .expect_err("candidate violates pure render boundary");
        assert!(format!("{error:?}").contains("render -> leaf"));
        assert_eq!(active.execute_i32_noarg_by_name("main").unwrap(), 7);
        assert_eq!(
            active.generation_metadata().unwrap().source_revision,
            revision
        );
    }

    #[test]
    fn effect_contract_only_edit_reuses_emitted_machine_code() {
        let mut active = JitProcess::new();
        active.upsert_file(
            "effects.stasis",
            "struct State { value: i32; } global state: State; function @effects(state) tick(): i32 { state.value += 1; return 0; }",
        );
        active.compile().expect("compile broad contract");
        let pointer = active
            .artifacts()
            .iter()
            .find(|artifact| artifact.function_key.name == "tick")
            .expect("tick artifact")
            .code_ptr;

        let mut candidate = active.staged_candidate();
        candidate.upsert_file(
            "effects.stasis",
            "struct State { value: i32; } global state: State; function @effects(state.value) tick(): i32 { state.value += 1; return 0; }",
        );
        let report = candidate.compile().expect("validate narrower contract");
        assert_eq!(report.emit.emitted_functions, 0);
        assert_eq!(
            candidate
                .artifacts()
                .iter()
                .find(|artifact| artifact.function_key.name == "tick")
                .unwrap()
                .code_ptr,
            pointer
        );
    }

    #[test]
    fn staged_candidate_applies_custom_roots_before_indexing_source_edits() {
        let mut active = JitProcess::new();
        active.set_required_emit_roots(&["extra".to_string()]);
        active.upsert_file(
            "sample.stasis",
            "function main(): i32 { return 1; } function extra(): i32 { return 2; }",
        );
        active.compile().expect("baseline compile");

        let mut candidate = active.staged_candidate();
        candidate.upsert_file(
            "sample.stasis",
            "function main(): i32 { return 1; } function extra(): i32 { return 3; }",
        );
        candidate.compile().expect("custom-root candidate compile");
        let snapshot = candidate.program_snapshot().expect("candidate snapshot");
        assert!(snapshot
            .data_flow_summaries()
            .iter()
            .any(|summary| summary.function == "extra"));
        assert!(Arc::ptr_eq(
            &snapshot.data_flow_summaries_shared(),
            &candidate.compiler.data_flow_summaries_shared(),
        ));

        active.accept_staged_candidate(candidate);
        let snapshot = active.program_snapshot().expect("accepted snapshot");
        assert!(Arc::ptr_eq(
            &snapshot.data_flow_summaries_shared(),
            &active.compiler.data_flow_summaries_shared(),
        ));
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_reuses_stable_functions_when_ordinal_ids_shift() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function f0(): i32 { return 1; }\nfunction f1(): i32 { return 2; }\nfunction main(): i32 { return f1() + 100; }\n",
        );
        let first = process.compile().expect("first compile");
        assert_eq!(first.emit.emitted_functions, 2);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute first"),
            102
        );
        let first_ptrs = process.symbol_code_ptrs();

        process.upsert_file(
            "sample.stasis",
            "function inserted(): i32 { return 9; }\nfunction f0(): i32 { return 1; }\nfunction f1(): i32 { return 2; }\nfunction main(): i32 { return f1() + 100; }\n",
        );
        let second = process.compile().expect("second compile");
        assert_eq!(second.emit.emitted_functions, 0);
        assert_eq!(process.symbol_code_ptrs(), first_ptrs);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute second"),
            102
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_keeps_previous_artifacts_on_partial_emit_failure() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function stable_helper(): i32 { return 1; }\nfunction main(): i32 { return stable_helper(); }\n",
        );
        process.compile().expect("initial compile");
        let first_metadata = process
            .generation_metadata()
            .expect("initial generation metadata")
            .clone();
        let first_snapshot = process
            .program_snapshot()
            .expect("initial program snapshot")
            .clone();
        let first_main_ptr = process
            .symbol_code_ptrs()
            .get("main")
            .copied()
            .expect("main ptr after initial compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute initial main"),
            1
        );

        process.upsert_file(
            "sample.stasis",
            "function inserted(): i32 { return 9; }\nfunction stable_helper(): i32 { return 1; }\nfunction main(): i32 { return missing(); }\n",
        );
        let error = process.compile().expect_err("expected compile failure");
        match error {
            crate::compiler::CompileError::Backend(message) => {
                assert!(
                    message.contains("unknown call target 'missing'"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected backend error, got {other:?}"),
        }

        let second_main_ptr = process
            .symbol_code_ptrs()
            .get("main")
            .copied()
            .expect("main ptr after failed compile");
        assert_eq!(second_main_ptr, first_main_ptr);
        assert_eq!(
            process.generation_metadata(),
            Some(&first_metadata),
            "a failed staged generation must not publish metadata"
        );
        assert_eq!(
            process
                .program_snapshot()
                .expect("preserved program snapshot")
                .layout_digest(),
            first_snapshot.layout_digest(),
            "a failed candidate must not publish state metadata"
        );
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute preserved main"),
            1
        );

        process.upsert_file(
            "sample.stasis",
            "function inserted(): i32 { return 9; }\nfunction stable_helper(): i32 { return 5; }\nfunction main(): i32 { return stable_helper(); }\n",
        );
        process.compile().expect("recovery compile");
        assert_ne!(
            process.generation_metadata(),
            Some(&first_metadata),
            "the repaired source should publish a new complete generation"
        );
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute recovered main"),
            5
        );

        let recovered_metadata = process
            .generation_metadata()
            .expect("recovered generation metadata")
            .clone();
        process.upsert_file(
            "sample.stasis",
            "function another_inserted(): i32 { return 10; }\nfunction stable_helper(): i32 { return 5; }\nfunction main(): i32 { let i: i32 = 0; while (i < 2) { i += 1; } return stable_helper(); }\n",
        );
        process.compile().expect_err("expected frontend failure");
        let diagnostic = process
            .last_source_diagnostic()
            .expect("failed source diagnostic should survive active snapshot restore");
        assert_eq!(diagnostic.path, "sample.stasis");
        assert!(!diagnostic.message.is_empty());
        assert!(diagnostic.end > diagnostic.start);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute generation preserved after frontend failure"),
            5
        );
        assert_eq!(
            process.generation_metadata(),
            Some(&recovered_metadata),
            "frontend failure must not publish a generation"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jit_process_keeps_previous_module_when_runtime_activation_fails() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "global task175_activation_values: i32[1];\nfunction main(): i32 { return 1; }\n",
        );
        process.compile().expect("initial compile");
        let first_metadata = process
            .generation_metadata()
            .expect("initial metadata")
            .clone();
        let first_main_ptr = process
            .symbol_code_ptrs()
            .get("main")
            .copied()
            .expect("initial main pointer");

        let host_values = Box::leak(Box::new([0_i32]));
        stasis_dynload::register_global_i32_array(
            hash_global_path("task175_activation_values"),
            0,
            host_values.as_mut_ptr(),
            host_values.len(),
        );
        process.upsert_file(
            "sample.stasis",
            "global task175_activation_values: i32[2];\nfunction main(): i32 { return 2; }\n",
        );
        let error = process
            .compile()
            .expect_err("activation should reject growth");
        assert!(
            format!("{error:?}").contains("cannot grow host-owned collection storage"),
            "unexpected activation error: {error:?}"
        );
        assert_eq!(
            process.symbol_code_ptrs().get("main").copied(),
            Some(first_main_ptr)
        );
        assert_eq!(process.generation_metadata(), Some(&first_metadata));
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("old main remains executable"),
            1
        );
    }

    #[test]
    fn jit_process_keeps_program_snapshot_for_unchanged_sources() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "const VALUE: i32 = 5;\nfunction main(): i32 { return VALUE; }\n",
        );

        process.compile().expect("first compile");
        let first_fingerprint = process
            .program_snapshot
            .as_ref()
            .expect("snapshot after first compile")
            .source_revision();

        process.compile().expect("second compile");
        let second_fingerprint = process
            .program_snapshot
            .as_ref()
            .expect("snapshot after second compile")
            .source_revision();

        assert_eq!(
            first_fingerprint, second_fingerprint,
            "unchanged sources should keep the same snapshot revision"
        );
    }

    #[test]
    fn jit_process_rebuilds_program_snapshot_when_dependency_changes() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "import \"stdlib.stasis\";\nfunction main(): i32 { return helper(); }\n",
        );
        process.upsert_file("stdlib.stasis", "function helper(): i32 { return 11; }\n");

        process.compile().expect("first compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute first"),
            11
        );
        let first_fingerprint = process
            .program_snapshot
            .as_ref()
            .expect("snapshot after first compile")
            .source_revision();

        process.upsert_file("stdlib.stasis", "function helper(): i32 { return 27; }\n");
        process.compile().expect("second compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute second"),
            27
        );
        let second_fingerprint = process
            .program_snapshot
            .as_ref()
            .expect("snapshot after second compile")
            .source_revision();

        assert_ne!(
            first_fingerprint, second_fingerprint,
            "dependency source change should invalidate compile-analysis cache key"
        );
    }

    #[test]
    fn jit_process_refreshes_compiler_owned_module_graph_when_imports_change() {
        let mut process = JitProcess::new();
        process.upsert_file("dep.stasis", "function dep(): i32 { return 1; }\n");
        process.upsert_file(
            "main.stasis",
            "import \"dep.stasis\";\nfunction main(): i32 { return dep(); }\n",
        );
        process.compile().expect("first compile");
        assert_eq!(
            process
                .program_snapshot()
                .unwrap()
                .module_graph()
                .direct_dependencies("main.stasis"),
            vec!["dep.stasis"],
        );

        process.upsert_file("dep2.stasis", "function dep2(): i32 { return 2; }\n");
        process.upsert_file(
            "main.stasis",
            "import \"dep.stasis\";\nimport \"dep2.stasis\";\nfunction main(): i32 { return dep() + dep2(); }\n",
        );
        process.compile().expect("second compile");
        assert_eq!(
            process
                .program_snapshot()
                .unwrap()
                .module_graph()
                .direct_dependencies("main.stasis"),
            vec!["dep.stasis", "dep2.stasis"],
        );
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute second"),
            3
        );
    }

    #[test]
    fn failed_module_graph_refresh_preserves_accepted_graph_and_code() {
        let mut process = JitProcess::new();
        process.upsert_file("dep.stasis", "function dep(): i32 { return 7; }\n");
        process.upsert_file(
            "main.stasis",
            "import \"dep.stasis\"; function main(): i32 { return dep(); }\n",
        );
        process.compile().unwrap();
        assert_eq!(process.execute_i32_noarg_by_name("main"), Ok(7));

        process.upsert_file(
            "main.stasis",
            "import \"missing.stasis\"; function main(): i32 { return dep(); }\n",
        );
        process.compile().unwrap_err();
        assert_eq!(process.execute_i32_noarg_by_name("main"), Ok(7));
        assert_eq!(
            process
                .program_snapshot()
                .unwrap()
                .module_graph()
                .direct_dependencies("main.stasis"),
            vec!["dep.stasis"]
        );
        let diagnostic = process.last_source_diagnostic().unwrap();
        assert_eq!(diagnostic.path, "main.stasis");
        assert_eq!(diagnostic.symbol, "missing");
    }

    #[test]
    fn jit_process_reemits_reachable_functions_when_imported_constant_changes() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "import \"constants.stasis\";\nfunction main(): i32 { return VALUE; }\n",
        );
        process.upsert_file("constants.stasis", "const VALUE: i32 = 11;\n");

        let first = process.compile().expect("first compile");
        assert_eq!(first.emit.emitted_functions, 1);
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute first"),
            11
        );

        process.upsert_file("constants.stasis", "const VALUE: i32 = 27;\n");
        let second = process.compile().expect("second compile");
        assert_eq!(
            second.emit.emitted_functions, 1,
            "main should be re-emitted when imported constants change"
        );
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("execute second"),
            27
        );
    }

    #[test]
    fn jit_engine_package_exposes_required_entrypoints() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        );
        process.compile().expect("compile");
        let package = process
            .build_engine_package(&EngineEntrypoints::runtime_default())
            .expect("engine package");
        assert_ne!(package.tick_code_ptr, 0);
        assert_ne!(package.render_code_ptr, 0);
        assert_eq!(
            package.on_code_swap_code_ptr.is_some(),
            true,
            "expected on_code_swap pointer"
        );
        assert_eq!(
            package.symbol_code_ptrs.contains_key("tick"),
            true,
            "expected tick in package symbol map"
        );
        assert_eq!(
            package.symbol_code_ptrs.contains_key("render"),
            true,
            "expected render in package symbol map"
        );
    }

    #[test]
    fn jit_engine_package_errors_when_required_entrypoint_missing() {
        let mut process = JitProcess::new();
        process.upsert_file("sample.stasis", "function tick(): i32 { return 0; }\n");
        process.compile().expect("compile");
        let error = process
            .build_engine_package(&EngineEntrypoints::runtime_default())
            .expect_err("missing render should fail");
        assert!(
            error.contains("required engine entrypoint 'render' not found"),
            "unexpected message: {error}"
        );
    }

    #[test]
    fn jit_engine_package_reports_signature_mismatch() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function tick(): void { return; }\nfunction render(): i32 { return 0; }\n",
        );
        process.compile().expect("compile");
        let error = process
            .build_engine_package(&EngineEntrypoints::runtime_default())
            .expect_err("void tick should fail");
        assert!(error.contains("expected `function tick(): i32`"));
        assert!(error.contains("actual return type id"));
    }

    #[test]
    fn compiler_indexed_state_queries_cover_indexes_predicates_and_expressions() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "state_queries.stasis",
            "struct Enemy { hp: i32; alive: bool; }\n\
             global enemies: Enemy[4];\n\
             global score: i32;\n\
             function main(): i32 {\n\
                 score = 5;\n\
                 enemies[0].hp = 2; enemies[0].alive = true;\n\
                 enemies[1].hp = 7; enemies[1].alive = true;\n\
                 enemies[2].hp = 9; enemies[2].alive = false;\n\
                 return 0;\n\
             }\n",
        );
        process.compile().expect("compile query fixture");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("initialize query fixture"),
            0
        );

        let indexed = process
            .inspect_state_query("enemies[1].hp + score * 2")
            .expect("indexed expression");
        assert_eq!(indexed["kind"], "scalar");
        assert_eq!(indexed["static_type"], "i32");
        assert_eq!(indexed["value"]["value"], 17);

        let predicate = process
            .inspect_state_query("enemies[?hp >= score]")
            .expect("predicate query");
        assert_eq!(predicate["kind"], "predicate");
        assert_eq!(predicate["capacity"], 4);
        assert_eq!(predicate["total_matches"], 2);
        assert_eq!(predicate["matches"][0]["index"], 1);
        assert_eq!(predicate["matches"][1]["index"], 2);
        let bounded = process
            .inspect_state_query_with_scan_limit("enemies[?hp >= score]", 2)
            .expect("bounded predicate query");
        assert_eq!(bounded["scanned"], 2);
        assert_eq!(bounded["total_matches"], 1);
        assert!(bounded["scan_truncated"].as_bool().unwrap_or(false));

        assert!(process
            .inspect_state_query("enemies[9].hp")
            .expect_err("out-of-range index")
            .contains("outside capacity 4"));
        assert!(process
            .inspect_state_query("enemies[?missing > 0]")
            .expect_err("unknown field")
            .contains("field 'missing' was not found"));
        assert!(process
            .inspect_state_query("score / 0")
            .expect_err("division by zero")
            .contains("division by zero"));
        assert!(process
            .inspect_state_query("enemies[0].alive == score")
            .expect_err("mixed equality operands")
            .contains("two numeric operands or two bool operands"));
        assert!(process
            .inspect_state_query("enemies[?alive == score]")
            .expect_err("mixed predicate operands")
            .contains("two numeric operands or two bool operands"));
    }
}
